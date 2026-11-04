use crate::agent::Agent;
use crate::instructions::Instructions;
use crate::project::Project;
use recode_schema::llm::{ContentBlock, LlmMessage, LlmRole};
use recode_schema::message::Role;
use recode_schema::model::ModelRef;
use recode_schema::part::{Part, ToolStatus};
use recode_storage::StoredMessage;
use std::path::Path;

/// Build the system prompt: agent instructions + environment + project docs.
pub fn system_prompt(
    agent: &Agent,
    instructions: &Instructions,
    project: &Project,
    model: &ModelRef,
    session_directory: &Path,
) -> String {
    let mut prompt = String::new();
    prompt.push_str(agent.system_prompt.trim());
    prompt.push_str("\n\n# Environment\n\n");
    prompt.push_str(&format!(
        "- Working directory: {}\n- Project root: {}\n- Platform: {} {}\n- Date: {}\n",
        session_directory.display(),
        project.root.display(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        chrono::Local::now().format("%Y-%m-%d"),
    ));
    if let Some(git) = project.git_summary() {
        prompt.push_str(&format!("- Git: {git}\n"));
    }
    prompt.push_str(&format!("- Model: {model}\n"));
    if cfg!(windows) {
        prompt.push_str("- Shell: Windows PowerShell (bash tool uses powershell.exe)\n");
    } else {
        prompt.push_str("- Shell: bash\n");
    }
    if !instructions.is_empty() {
        prompt.push_str("\n# Project instructions\n");
        prompt.push_str(&instructions.render(96 * 1024));
    }
    prompt
}

/// Convert persisted messages into the canonical provider transcript.
///
/// Tool calls and their results live in the same assistant message in storage;
/// providers require a call followed by its result, so the conversion emits an
/// assistant message and a tool message per step.
///
/// When a [`CompactionRecord`] is present, messages through its cut point are
/// replaced by a synthetic summary message.
pub fn to_llm_messages(
    messages: &[StoredMessage],
    compaction: Option<&recode_storage::CompactionRecord>,
) -> Vec<LlmMessage> {
    let mut output = Vec::new();
    let start = compaction
        .and_then(|compaction| {
            messages
                .iter()
                .position(|message| message.info.id == compaction.through)
                .map(|index| index + 1)
        })
        .unwrap_or(0);
    if let Some(compaction) = compaction {
        output.push(LlmMessage::user(format!(
            "Summary of the conversation so far (older messages were compacted):\n\n{}\n\n\
             Continue from the current state. Do not repeat work already completed.",
            compaction.summary.trim()
        )));
    }
    for message in &messages[start..] {
        match message.info.role {
            Role::User => {
                let mut blocks = Vec::new();
                for part in &message.parts {
                    match part {
                        Part::Text(text) if !text.text.is_empty() => {
                            blocks.push(ContentBlock::Text {
                                text: text.text.clone(),
                            });
                        }
                        Part::File(file) => blocks.push(ContentBlock::File {
                            mime: file.mime.clone(),
                            filename: file.filename.clone(),
                            url: file.url.clone(),
                        }),
                        _ => {}
                    }
                }
                if !blocks.is_empty() {
                    output.push(LlmMessage {
                        role: LlmRole::User,
                        content: blocks,
                    });
                }
            }
            Role::Assistant => {
                let mut blocks = Vec::new();
                let mut results = Vec::new();
                for part in &message.parts {
                    match part {
                        Part::Text(text) if !text.text.is_empty() => {
                            blocks.push(ContentBlock::Text {
                                text: text.text.clone(),
                            });
                        }
                        Part::Reasoning(reasoning) if !reasoning.text.is_empty() => {
                            blocks.push(ContentBlock::Reasoning {
                                text: reasoning.text.clone(),
                                signature: reasoning.signature.clone(),
                            });
                        }
                        Part::Tool(tool) => match tool.state.status {
                            ToolStatus::Completed => {
                                blocks.push(ContentBlock::ToolCall {
                                    id: tool.call_id.clone(),
                                    name: tool.tool.clone(),
                                    input: tool.state.input.clone(),
                                    namespace: tool.namespace.clone(),
                                });
                                results.push(ContentBlock::ToolResult {
                                    id: tool.call_id.clone(),
                                    name: tool.tool.clone(),
                                    content: tool.state.output.clone().unwrap_or_default(),
                                    is_error: false,
                                });
                            }
                            ToolStatus::Error => {
                                blocks.push(ContentBlock::ToolCall {
                                    id: tool.call_id.clone(),
                                    name: tool.tool.clone(),
                                    input: tool.state.input.clone(),
                                    namespace: tool.namespace.clone(),
                                });
                                results.push(ContentBlock::ToolResult {
                                    id: tool.call_id.clone(),
                                    name: tool.tool.clone(),
                                    content: tool
                                        .state
                                        .error
                                        .clone()
                                        .unwrap_or_else(|| "tool failed".into()),
                                    is_error: true,
                                });
                            }
                            ToolStatus::Pending | ToolStatus::Running => {}
                        },
                        _ => {}
                    }
                }
                if !blocks.is_empty() {
                    output.push(LlmMessage {
                        role: LlmRole::Assistant,
                        content: blocks,
                    });
                }
                if !results.is_empty() {
                    output.push(LlmMessage {
                        role: LlmRole::Tool,
                        content: results,
                    });
                }
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use recode_schema::id::{MessageId, SessionId};
    use recode_schema::message::MessageInfo;
    use recode_schema::part::{ToolPart, ToolState};

    fn message(role: Role, parts: Vec<Part>) -> StoredMessage {
        StoredMessage {
            info: MessageInfo {
                id: MessageId::new(),
                session_id: SessionId::new(),
                role,
                created_at: 0,
                agent: None,
                model: None,
                parent_id: None,
                error: None,
                metadata: serde_json::Value::Null,
            },
            parts,
        }
    }

    #[test]
    fn emits_call_then_result_turns() {
        let mut tool = ToolPart {
            id: recode_schema::id::PartId::new(),
            call_id: "call_1".into(),
            tool: "read".into(),
            namespace: None,
            state: ToolState::pending(serde_json::json!({"file_path":"a.txt"})),
        };
        tool.state = tool.state.completed("hello".into(), None);
        let messages = vec![
            message(Role::User, vec![Part::text("read a.txt")]),
            message(Role::Assistant, vec![Part::Tool(tool)]),
        ];
        let converted = to_llm_messages(&messages, None);
        assert_eq!(converted.len(), 3);
        assert_eq!(converted[1].role, LlmRole::Assistant);
        assert_eq!(converted[2].role, LlmRole::Tool);
    }

    #[test]
    fn skips_empty_assistant_messages() {
        let messages = vec![message(Role::Assistant, vec![])];
        assert!(to_llm_messages(&messages, None).is_empty());
    }
}
