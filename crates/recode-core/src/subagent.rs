//! Child-session execution for the `task` tool.

use crate::engine::Engine;
use async_trait::async_trait;
use recode_schema::event::SessionInfo;
use recode_schema::id::SessionId;
use recode_schema::model::ModelRef;
use recode_schema::tool::ToolFailure;
use recode_tool::SubagentRunner;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// How many levels deep a subagent may delegate. Two levels keeps token spend
/// bounded while still allowing explore → general compositions.
pub const MAX_SUBAGENT_DEPTH: u64 = 2;

pub struct EngineSubagent {
    engine: Arc<Engine>,
    parent: SessionId,
    model: ModelRef,
    depth: u64,
    cancel: CancellationToken,
}

impl EngineSubagent {
    pub fn new(
        engine: Arc<Engine>,
        parent: SessionId,
        model: ModelRef,
        depth: u64,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            engine,
            parent,
            model,
            depth,
            cancel,
        }
    }
}

#[async_trait]
impl SubagentRunner for EngineSubagent {
    async fn run(
        &self,
        description: String,
        prompt: String,
        agent: Option<String>,
    ) -> Result<String, ToolFailure> {
        let agent_id = agent.unwrap_or_else(|| "general".to_owned());
        if self.engine.agents.get(&agent_id).is_none() {
            return Err(ToolFailure::new(format!(
                "unknown subagent type {agent_id:?}; available agents: {}",
                self.engine
                    .agents
                    .all()
                    .map(|agent| agent.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let mut session = SessionInfo::new(
            self.engine.project.info.id.clone(),
            self.engine.project.root.clone(),
        );
        session.parent_id = Some(self.parent.clone());
        session.agent = Some(agent_id);
        session.model = Some(self.model.clone());
        session.title = description.clone();
        session.metadata = serde_json::json!({
            "subagentDepth": self.depth + 1,
            "parentSession": self.parent.0,
            "description": description,
        });
        let envelope = self.engine.store.create_session(session).map_err(|error| {
            ToolFailure::new(format!("cannot create subagent session: {error}"))
        })?;
        let session_id = envelope.session_id;
        self.engine
            .prompt(&session_id, prompt, Vec::new())
            .await
            .map_err(ToolFailure::new)?;
        let idle = self.engine.wait_idle(&session_id);
        tokio::select! {
            _ = self.cancel.cancelled() => {
                self.engine.abort(&session_id);
                return Err(ToolFailure::new("subagent aborted with its parent"));
            }
            _ = idle => {}
        }
        let messages = self.engine.messages(&session_id).map_err(|error| {
            ToolFailure::new(format!("cannot read subagent transcript: {error}"))
        })?;
        let last_assistant = messages
            .iter()
            .rev()
            .find(|message| message.info.role == recode_schema::Role::Assistant);
        let Some(message) = last_assistant else {
            return Err(ToolFailure::new("subagent produced no assistant message"));
        };
        let mut text = String::new();
        for part in &message.parts {
            match part {
                recode_schema::part::Part::Text(part) => text.push_str(&part.text),
                recode_schema::part::Part::Error(part) => {
                    return Err(ToolFailure::new(format!(
                        "subagent failed: {}",
                        part.message
                    )));
                }
                _ => {}
            }
        }
        if text.trim().is_empty() {
            return Err(ToolFailure::new(format!(
                "subagent {} finished without a final answer",
                session_id
            )));
        }
        Ok(text)
    }
}
