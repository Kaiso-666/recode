use crate::cli::{SessionsArgs, SessionsCommand};
use recode_core::engine::Engine;
use recode_core::permission::AutoReject;
use recode_core::EngineOptions;
use recode_schema::part::Part;
use recode_schema::SessionId;
use std::sync::Arc;

pub async fn run(args: SessionsArgs) -> anyhow::Result<()> {
    let directory = crate::cmd::directory(None)?;
    let engine = Engine::new(EngineOptions {
        paths: crate::cmd::paths(),
        directory,
        responder: Some(Arc::new(AutoReject)),
        question_responder: Some(Arc::new(recode_core::question::AutoRejectQuestions)),
    })
    .map_err(anyhow::Error::msg)?;
    match args.command {
        SessionsCommand::List { all, archived } => {
            let sessions = if all {
                engine.all_sessions(archived)
            } else {
                engine.sessions(archived)
            }
            .map_err(anyhow::Error::msg)?;
            if sessions.is_empty() {
                println!("No sessions.");
                return Ok(());
            }
            for session in sessions {
                println!(
                    "{}\t{}\t{}\t{}",
                    session.id,
                    format_time(session.updated_at),
                    session.directory.display(),
                    if session.title.is_empty() {
                        "(untitled)"
                    } else {
                        &session.title
                    }
                );
            }
        }
        SessionsCommand::Show { id } => {
            let session_id = SessionId::from_existing(id);
            let messages = engine.messages(&session_id).map_err(anyhow::Error::msg)?;
            for message in messages {
                println!("\n=== {} {} ===", message.info.role, message.info.id);
                for part in message.parts {
                    match part {
                        Part::Text(text) => println!("{}", text.text),
                        Part::Reasoning(reasoning) => println!("[thinking] {}", reasoning.text),
                        Part::Tool(tool) => {
                            println!(
                                "[tool] {} ({:?})\n{}",
                                tool.tool,
                                tool.state.status,
                                tool.state.output.or(tool.state.error).unwrap_or_default()
                            );
                        }
                        Part::StepFinish(finish) => println!(
                            "[step] {:?} · {} tokens",
                            finish.reason, finish.tokens.total_tokens
                        ),
                        Part::Error(error) => println!("[error] {}", error.message),
                        _ => {}
                    }
                }
            }
        }
        SessionsCommand::Delete { id } => {
            engine
                .delete_session(&SessionId::from_existing(id.clone()))
                .map_err(anyhow::Error::msg)?;
            println!("Deleted {id}");
        }
        SessionsCommand::Export { id, output, format } => {
            let session_id = SessionId::from_existing(id.clone());
            let session = engine
                .session(&session_id)
                .map_err(anyhow::Error::msg)?
                .ok_or_else(|| anyhow::anyhow!("session {id} not found"))?;
            let messages = engine.messages(&session_id).map_err(anyhow::Error::msg)?;
            let text = if format == "json" {
                serde_json::to_string_pretty(&serde_json::json!({
                    "session": session,
                    "messages": messages,
                }))?
            } else {
                to_markdown(&session, &messages)
            };
            match output {
                Some(path) => {
                    std::fs::write(&path, text)?;
                    println!("Wrote {}", path.display());
                }
                None => println!("{text}"),
            }
        }
        SessionsCommand::Fork { id, title } => {
            let session_id = SessionId::from_existing(id.clone());
            let forked = engine
                .fork_session(&session_id, title)
                .map_err(anyhow::Error::msg)?;
            let count = engine
                .messages(&forked.id)
                .map_err(anyhow::Error::msg)?
                .len();
            println!("Forked {id} -> {} ({count} messages)", forked.id);
        }
        SessionsCommand::Revert { id, message } => {
            let session_id = SessionId::from_existing(id.clone());
            let removed = engine
                .revert(
                    &session_id,
                    &recode_schema::MessageId::from_existing(message),
                )
                .map_err(anyhow::Error::msg)?;
            println!("Reverted {id}: removed {removed} message(s)");
        }
        SessionsCommand::Restore { id, message } => {
            let session_id = SessionId::from_existing(id.clone());
            let message = message.map(recode_schema::MessageId::from_existing);
            let snapshot = engine
                .restore_files(&session_id, message.as_ref())
                .await
                .map_err(anyhow::Error::msg)?;
            println!("Restored {id} to snapshot {snapshot}");
        }
        SessionsCommand::Compact { id } => {
            let session_id = SessionId::from_existing(id.clone());
            match engine
                .compact(&session_id)
                .await
                .map_err(anyhow::Error::msg)?
            {
                Some(record) => println!(
                    "Compacted {id}: {} → {} tokens",
                    record.tokens_before, record.tokens_after
                ),
                None => println!("Nothing to compact in {id}"),
            }
        }
    }
    Ok(())
}

fn format_time(timestamp: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(timestamp)
        .single()
        .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn to_markdown(
    session: &recode_schema::event::SessionInfo,
    messages: &[recode_schema::message::Message],
) -> String {
    use std::fmt::Write as _;
    let mut output = String::new();
    let _ = writeln!(output, "# {}\n", session.display_title());
    let _ = writeln!(output, "- session: `{}`", session.id);
    let _ = writeln!(output, "- directory: `{}`", session.directory.display());
    let _ = writeln!(output, "- created: {}", format_time(session.created_at));
    if let Some(model) = &session.model {
        let _ = writeln!(output, "- model: `{}`", model.qualified());
    }
    output.push('\n');
    for message in messages {
        let heading = match message.info.role {
            recode_schema::Role::User => "## User",
            recode_schema::Role::Assistant => "## Assistant",
        };
        let _ = writeln!(output, "{heading}\n");
        for part in &message.parts {
            match part {
                Part::Text(text) if !text.text.trim().is_empty() => {
                    output.push_str(text.text.trim_end());
                    output.push_str("\n\n");
                }
                Part::Reasoning(reasoning) if !reasoning.text.trim().is_empty() => {
                    let _ = writeln!(
                        output,
                        "<details><summary>Reasoning</summary>\n\n```\n{}\n```\n\n</details>\n",
                        truncate(&reasoning.text, 8_000)
                    );
                }
                Part::Tool(tool) => {
                    let status = match tool.state.status {
                        recode_schema::ToolStatus::Completed => "completed",
                        recode_schema::ToolStatus::Error => "failed",
                        recode_schema::ToolStatus::Running => "running",
                        recode_schema::ToolStatus::Pending => "pending",
                    };
                    let _ = writeln!(
                        output,
                        "**Tool `{}`** — {status}\n",
                        tool.state
                            .title
                            .clone()
                            .unwrap_or_else(|| tool.tool.clone())
                    );
                    if let Some(body) = tool.state.output.as_deref().or(tool.state.error.as_deref())
                    {
                        let _ = writeln!(output, "```\n{}\n```\n", truncate(body, 20_000));
                    }
                }
                Part::Error(error) => {
                    let _ = writeln!(output, "**Error:** {}\n", error.message);
                }
                Part::File(file) => {
                    let _ = writeln!(
                        output,
                        "_Attached file: {} ({})_\n",
                        file.filename, file.mime
                    );
                }
                Part::Patch(patch) if !patch.files.is_empty() => {
                    let files = patch
                        .files
                        .iter()
                        .map(|file| format!("`{file}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let _ = writeln!(output, "_Changed files: {files}_\n");
                }
                _ => {}
            }
        }
    }
    output
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(limit).collect();
    truncated.push_str("\n… [truncated]");
    truncated
}
