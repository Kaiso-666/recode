//! Context compaction.
//!
//! Long sessions eventually exceed the model's context window. Instead of
//! failing or silently dropping messages, Re:Code asks the model for a summary
//! of the older history and records it as a durable `session.compacted` event.
//! [`crate::prompt::to_llm_messages`] then replaces everything through the cut
//! point with the summary; the full transcript remains available to clients.

use crate::engine::Engine;
use crate::prompt;
use futures_util::StreamExt;
use recode_schema::event::Event;
use recode_schema::id::SessionId;
use recode_schema::llm::{LlmEvent, LlmMessage, LlmRequest, LlmRole, ToolChoice};
use recode_schema::model::ModelRef;
use recode_storage::CompactionRecord;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const SUMMARY_SYSTEM_PROMPT: &str = r#"You are the context compactor for a coding agent. Summarize the conversation so work can continue without the original messages.

Preserve, in this order:
1. The user's goals, constraints, and preferences.
2. Decisions made and why, including rejected approaches.
3. Files and symbols touched, with exact paths.
4. Commands run and their outcomes (tests, builds, failures).
5. Errors encountered and how they were fixed.
6. The current state of the work and concrete next steps.

Be specific and terse. Never invent facts. If something is unknown, say so. Output only the summary, with no preamble."#;

/// Rough token estimate from character counts. English and code average
/// roughly four characters per token; this is deliberately conservative.
pub fn estimate_tokens(messages: &[LlmMessage]) -> u64 {
    let chars: usize = messages
        .iter()
        .flat_map(|message| message.content.iter())
        .map(|block| match block {
            recode_schema::llm::ContentBlock::Text { text } => text.chars().count(),
            recode_schema::llm::ContentBlock::Reasoning { text, .. } => text.chars().count(),
            recode_schema::llm::ContentBlock::ToolCall { input, .. } => {
                serde_json::to_string(input)
                    .map(|text| text.len())
                    .unwrap_or(0)
            }
            recode_schema::llm::ContentBlock::ToolResult { content, .. } => content.chars().count(),
            recode_schema::llm::ContentBlock::File { url, .. } => url.chars().count(),
        })
        .sum();
    (chars as u64 / 4).max(1)
}

/// Summarize the current transcript and persist a compaction event.
///
/// Returns `None` when there is nothing worth compacting or the run was
/// cancelled. A summary failure is returned to the caller; automatic callers
/// log it and continue with the uncompacted history.
pub async fn compact_session(
    engine: &Arc<Engine>,
    session_id: &SessionId,
    model: &ModelRef,
    cancel: &CancellationToken,
) -> Result<Option<CompactionRecord>, String> {
    let messages = engine.messages(session_id)?;
    if messages.len() < 3 {
        return Ok(None);
    }
    let stored: Vec<recode_storage::StoredMessage> = messages
        .iter()
        .map(|message| recode_storage::StoredMessage {
            info: message.info.clone(),
            parts: message.parts.clone(),
        })
        .collect();
    let previous = engine
        .store
        .compaction(session_id)
        .map_err(|error| error.to_string())?;
    let history = prompt::to_llm_messages(&stored, previous.as_ref());
    if history.is_empty() {
        return Ok(None);
    }
    let tokens_before = estimate_tokens(&history);
    let model = engine
        .config
        .merged
        .small_model
        .as_deref()
        .and_then(|small| engine.config.resolve_model(Some(small)).ok())
        .unwrap_or_else(|| model.clone());
    let context_limit = engine.config.model_config(&model).limit.context.max(8_000);
    let budget_chars = (context_limit as usize).saturating_mul(4 * 60 / 100);
    let history = trim_history(history, budget_chars);
    let provider = engine.providers.get_for(&model).await?;
    let request = LlmRequest {
        model: model.clone(),
        system: SUMMARY_SYSTEM_PROMPT.to_owned(),
        messages: history,
        tools: Vec::new(),
        tool_choice: ToolChoice::None,
        temperature: Some(0.2),
        max_tokens: Some(4096),
        metadata: Default::default(),
    };
    let mut stream = provider
        .stream(request, cancel.clone())
        .await
        .map_err(|error| error.message)?;
    let mut summary = String::new();
    while let Some(event) = stream.next().await {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        match event.map_err(|error| error.message)? {
            LlmEvent::TextDelta(delta) => summary.push_str(&delta),
            LlmEvent::Error { message, .. } => return Err(message),
            LlmEvent::Finish { .. } => break,
            _ => {}
        }
    }
    if summary.trim().is_empty() {
        return Err("the summarizer returned no text".into());
    }
    let tokens_after = (summary.chars().count() as u64 / 4).max(1);
    let through = messages
        .last()
        .map(|message| message.info.id.clone())
        .ok_or_else(|| "session has no messages".to_owned())?;
    let envelope = engine
        .store
        .append(Event::SessionCompacted {
            session_id: session_id.clone(),
            summary: summary.trim().to_owned(),
            through: through.clone(),
            tokens_before,
            tokens_after,
        })
        .map_err(|error| error.to_string())?;
    let created_at = envelope.created_at;
    let _ = engine
        .bus
        .send(recode_schema::event::LiveEvent::Durable(envelope));
    Ok(Some(CompactionRecord {
        summary: summary.trim().to_owned(),
        through,
        tokens_before,
        tokens_after,
        created_at,
    }))
}

/// Keep the newest messages that fit the summary request budget, trimming
/// oversized tool output first and dropping an orphaned leading tool turn.
fn trim_history(history: Vec<LlmMessage>, budget_chars: usize) -> Vec<LlmMessage> {
    let mut kept = Vec::new();
    let mut used = 0usize;
    for mut message in history.into_iter().rev() {
        for block in &mut message.content {
            match block {
                recode_schema::llm::ContentBlock::Text { text } => {
                    truncate_in_place(text, 8_000);
                }
                recode_schema::llm::ContentBlock::ToolResult { content, .. } => {
                    truncate_in_place(content, 4_000);
                }
                _ => {}
            }
        }
        let size: usize = message
            .content
            .iter()
            .map(|block| match block {
                recode_schema::llm::ContentBlock::Text { text } => text.len(),
                recode_schema::llm::ContentBlock::Reasoning { text, .. } => text.len(),
                recode_schema::llm::ContentBlock::ToolCall { input, .. } => {
                    serde_json::to_string(input)
                        .map(|text| text.len())
                        .unwrap_or(0)
                }
                recode_schema::llm::ContentBlock::ToolResult { content, .. } => content.len(),
                recode_schema::llm::ContentBlock::File { url, .. } => url.len(),
            })
            .sum();
        if !kept.is_empty() && used + size > budget_chars {
            break;
        }
        used += size;
        kept.push(message);
    }
    kept.reverse();
    while kept
        .first()
        .is_some_and(|message| message.role == LlmRole::Tool)
    {
        kept.remove(0);
    }
    kept
}

fn truncate_in_place(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str("\n… [trimmed for compaction]");
}

#[cfg(test)]
mod tests {
    use super::*;
    use recode_schema::llm::ContentBlock;

    #[test]
    fn token_estimate_counts_content() {
        let messages = vec![LlmMessage::user("a".repeat(400))];
        assert_eq!(estimate_tokens(&messages), 100);
    }

    #[test]
    fn trimming_keeps_the_newest_messages() {
        let history = vec![
            LlmMessage::user("old ".repeat(100)),
            LlmMessage::assistant(vec![ContentBlock::text("middle")]),
            LlmMessage::user("newest"),
        ];
        let trimmed = trim_history(history, 40);
        assert!(trimmed
            .iter()
            .any(|message| message.text().contains("newest")));
    }

    #[test]
    fn trimming_drops_orphaned_tool_turns() {
        let history = vec![
            LlmMessage::tool(vec![ContentBlock::ToolResult {
                id: "call_1".into(),
                name: "read".into(),
                content: "result".into(),
                is_error: false,
            }]),
            LlmMessage::user("question"),
        ];
        let trimmed = trim_history(history, 10_000);
        assert_eq!(trimmed[0].role, LlmRole::User);
    }
}
