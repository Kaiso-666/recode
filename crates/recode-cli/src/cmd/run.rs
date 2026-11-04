use crate::cli::RunArgs;
use crate::cmd;
use anyhow::Context;
use async_trait::async_trait;
use base64::Engine as _;
use recode_core::engine::Engine;
use recode_core::permission::{AutoApprove, PermissionResponder};
use recode_core::EngineOptions;
use recode_schema::event::{Event, LiveEvent, SessionStatus};
use recode_schema::llm::ContentBlock;
use recode_schema::part::{Part, ToolStatus};
use recode_schema::permission::{PermissionReply, PermissionRequest};
use recode_schema::Role;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub async fn run(args: RunArgs) -> anyhow::Result<()> {
    cmd::validate_run(&args)?;
    cmd::init_logging(false);
    let paths = cmd::paths();
    let directory = cmd::directory(args.directory.clone())?;
    let responder: std::sync::Arc<dyn PermissionResponder> = if args.auto {
        std::sync::Arc::new(AutoApprove)
    } else {
        std::sync::Arc::new(InteractiveResponder)
    };
    let engine = Engine::new(EngineOptions {
        paths,
        directory,
        responder: Some(responder),
    })
    .map_err(anyhow::Error::msg)?;

    let mut session = if let Some(id) = &args.session {
        engine
            .session(&recode_schema::SessionId::from_existing(id.clone()))
            .map_err(anyhow::Error::msg)?
            .ok_or_else(|| anyhow::anyhow!("session {id} not found"))?
    } else if args.continue_session {
        match engine
            .sessions(false)
            .map_err(anyhow::Error::msg)?
            .into_iter()
            .next()
        {
            Some(session) => session,
            None => engine
                .create_session(args.agent.clone(), None, args.title.clone())
                .map_err(anyhow::Error::msg)?,
        }
    } else {
        let model = match &args.model {
            Some(model) => Some(
                engine
                    .config
                    .resolve_model(Some(model))
                    .map_err(anyhow::Error::msg)?,
            ),
            None => None,
        };
        engine
            .create_session(args.agent.clone(), model, args.title.clone())
            .map_err(anyhow::Error::msg)?
    };

    if let Some(model) = &args.model {
        let model = engine
            .config
            .resolve_model(Some(model))
            .map_err(anyhow::Error::msg)?;
        engine
            .set_model(&session.id, model)
            .map_err(anyhow::Error::msg)?;
        session = engine
            .session(&session.id)
            .map_err(anyhow::Error::msg)?
            .context("session disappeared")?;
    }
    if let Some(agent) = &args.agent {
        engine
            .set_agent(&session.id, agent.clone())
            .map_err(anyhow::Error::msg)?;
        session = engine
            .session(&session.id)
            .map_err(anyhow::Error::msg)?
            .context("session disappeared")?;
    }

    let mut prompt = args.message.join(" ");
    if prompt.trim().is_empty() {
        prompt = cmd::read_stdin_if_piped().unwrap_or_default();
    }
    let files = prepare_files(&args.file, &mut prompt)?;
    if prompt.trim().is_empty() && files.is_empty() {
        anyhow::bail!("you must provide a message");
    }

    let json = args.format == "json";
    let mut events = engine.subscribe();
    engine
        .prompt(&session.id, prompt.clone(), files)
        .await
        .map_err(anyhow::Error::msg)?;

    let timeout = if args.timeout > 0 {
        Some(Duration::from_secs(args.timeout))
    } else {
        None
    };
    let deadline = timeout.map(|timeout| tokio::time::Instant::now() + timeout);
    let mut saw_error = false;
    loop {
        let envelope = if let Some(deadline) = deadline {
            tokio::select! {
                event = events.recv() => event,
                _ = tokio::time::sleep_until(deadline) => {
                    engine.abort(&session.id);
                    eprintln!("\n[aborted after {} seconds]", args.timeout);
                    break;
                }
            }
        } else {
            events.recv().await
        };
        let Ok(event) = envelope else {
            break;
        };
        match event {
            LiveEvent::Delta {
                delta,
                kind,
                session_id,
                ..
            } if session_id == session.id => {
                if !json && kind == recode_schema::event::DeltaKind::Text {
                    print!("{delta}");
                    let _ = std::io::stdout().flush();
                }
            }
            LiveEvent::Durable(envelope) => match &envelope.event {
                Event::PartAdded {
                    session_id,
                    part: Part::Tool(tool),
                    ..
                } if *session_id == session.id => {
                    if !json {
                        eprintln!(
                            "\n[tool] {}",
                            tool.state
                                .title
                                .clone()
                                .unwrap_or_else(|| tool.tool.clone())
                        );
                    }
                }
                Event::PartUpdated {
                    session_id,
                    part: Part::Tool(tool),
                    ..
                } if *session_id == session.id => {
                    if !json {
                        match tool.state.status {
                            ToolStatus::Completed => {
                                eprintln!(
                                    "[tool] {} ✓",
                                    tool.state
                                        .title
                                        .clone()
                                        .unwrap_or_else(|| tool.tool.clone())
                                );
                            }
                            ToolStatus::Error => {
                                eprintln!(
                                    "[tool] {} failed: {}",
                                    tool.state
                                        .title
                                        .clone()
                                        .unwrap_or_else(|| tool.tool.clone()),
                                    tool.state.error.clone().unwrap_or_default()
                                );
                            }
                            _ => {}
                        }
                    }
                }
                Event::SessionError { session_id, error } if *session_id == session.id => {
                    saw_error = true;
                    if !json {
                        eprintln!("\n[error] {}", error.message);
                    }
                }
                _ => {}
            },
            LiveEvent::SessionStatus {
                session_id,
                status: SessionStatus::Idle,
            } if session_id == session.id => {
                if !json {
                    println!();
                }
                break;
            }
            _ => {}
        }
    }
    engine.wait_idle(&session.id).await;

    if json {
        let messages = engine.messages(&session.id).map_err(anyhow::Error::msg)?;
        let usage = total_usage(&messages);
        let text = final_text(&messages);
        let output = serde_json::json!({
            "sessionId": session.id,
            "messages": messages,
            "text": text,
            "usage": usage,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if let Some(usage) = summary_usage(&engine, &session.id) {
        eprintln!(
            "— tokens: {} in / {} out · ${:.4}",
            usage.input_tokens, usage.output_tokens, usage.cost
        );
    }

    let messages = engine.messages(&session.id).map_err(anyhow::Error::msg)?;
    if messages
        .iter()
        .rev()
        .find(|message| message.info.role == Role::Assistant)
        .is_some_and(|message| {
            message
                .parts
                .iter()
                .any(|part| matches!(part, Part::Error(_)))
        })
    {
        saw_error = true;
    }
    if saw_error {
        std::process::exit(1);
    }
    Ok(())
}

struct InteractiveResponder;

#[async_trait]
impl PermissionResponder for InteractiveResponder {
    async fn ask(&self, request: PermissionRequest) -> PermissionReply {
        if !std::io::stdin().is_terminal() {
            return PermissionReply::Reject;
        }
        eprintln!("\nPermission required: {}", request.permission);
        for pattern in &request.patterns {
            eprintln!("  {pattern}");
        }
        eprint!("[y] allow once  [a] always  [N] deny: ");
        let _ = std::io::stderr().flush();
        let answer = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok()?;
            Some(line.trim().to_ascii_lowercase())
        })
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
        match answer.as_str() {
            "y" | "yes" | "" => PermissionReply::Once,
            "a" | "always" => PermissionReply::Always,
            _ => PermissionReply::Reject,
        }
    }
}

fn prepare_files(paths: &[PathBuf], prompt: &mut String) -> anyhow::Result<Vec<ContentBlock>> {
    let mut blocks = Vec::new();
    for path in paths {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("cannot read {}", path.display()))?;
        let metadata = std::fs::metadata(&canonical)?;
        if metadata.len() > 10 * 1024 * 1024 {
            anyhow::bail!(
                "{} is larger than the 10 MB attachment limit",
                canonical.display()
            );
        }
        let bytes = std::fs::read(&canonical)?;
        let mime = mime_for(&canonical);
        let filename = canonical
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_owned());
        if mime.starts_with("image/") || mime == "application/pdf" {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            blocks.push(ContentBlock::File {
                mime: mime.clone(),
                filename,
                url: format!("data:{mime};base64,{encoded}"),
            });
        } else {
            let text = String::from_utf8_lossy(&bytes);
            let text = if text.chars().count() > 100_000 {
                let truncated: String = text.chars().take(100_000).collect();
                format!("{truncated}\n… [truncated]")
            } else {
                text.to_string()
            };
            prompt.push_str(&format!(
                "\n\n<file path=\"{}\">\n{}\n</file>",
                canonical.display(),
                text
            ));
        }
    }
    Ok(blocks)
}

fn mime_for(path: &Path) -> String {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "svg" => "image/svg+xml",
        _ => "text/plain",
    }
    .to_owned()
}

fn total_usage(messages: &[recode_schema::message::Message]) -> recode_schema::llm::Usage {
    let mut usage = recode_schema::llm::Usage::default();
    for message in messages {
        for part in &message.parts {
            if let Part::StepFinish(finish) = part {
                usage.merge(&finish.tokens);
            }
        }
    }
    usage
}

fn summary_usage(
    engine: &Engine,
    session_id: &recode_schema::SessionId,
) -> Option<recode_schema::llm::Usage> {
    let messages = engine.messages(session_id).ok()?;
    let usage = total_usage(&messages);
    if usage.total_tokens == 0 && usage.input_tokens == 0 && usage.output_tokens == 0 {
        None
    } else {
        Some(usage)
    }
}

fn final_text(messages: &[recode_schema::message::Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.info.role == Role::Assistant)
        .map(|message| message.text())
        .unwrap_or_default()
}
