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
        SessionsCommand::Export { id, output } => {
            let session_id = SessionId::from_existing(id.clone());
            let session = engine
                .session(&session_id)
                .map_err(anyhow::Error::msg)?
                .ok_or_else(|| anyhow::anyhow!("session {id} not found"))?;
            let messages = engine.messages(&session_id).map_err(anyhow::Error::msg)?;
            let value = serde_json::json!({"session": session, "messages": messages});
            match output {
                Some(path) => {
                    std::fs::write(&path, serde_json::to_string_pretty(&value)?)?;
                    println!("Wrote {}", path.display());
                }
                None => println!("{}", serde_json::to_string_pretty(&value)?),
            }
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
