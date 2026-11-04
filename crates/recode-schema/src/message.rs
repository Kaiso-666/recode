use crate::id::{MessageId, SessionId};
use crate::model::ModelRef;
use crate::part::Part;
use crate::Timestamp;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failure attached to an assistant message, surfaced in the transcript and
/// retried by the runner when appropriate.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageError {
    pub name: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl MessageError {
    pub fn new(name: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            name: name.into(),
            message: message.into(),
            retryable,
            metadata: None,
        }
    }
}

/// Message header persisted in the `message` projection table.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageInfo {
    pub id: MessageId,
    pub session_id: SessionId,
    pub role: Role,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<MessageId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<MessageError>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

impl MessageInfo {
    pub fn user(session_id: SessionId) -> Self {
        Self {
            id: MessageId::new(),
            session_id,
            role: Role::User,
            created_at: crate::now_ms(),
            agent: None,
            model: None,
            parent_id: None,
            error: None,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn assistant(session_id: SessionId, agent: impl Into<String>, model: ModelRef) -> Self {
        Self {
            id: MessageId::new(),
            session_id,
            role: Role::Assistant,
            created_at: crate::now_ms(),
            agent: Some(agent.into()),
            model: Some(model),
            parent_id: None,
            error: None,
            metadata: serde_json::Value::Null,
        }
    }
}

/// A complete message: header plus ordered parts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    #[serde(flatten)]
    pub info: MessageInfo,
    #[serde(default)]
    pub parts: Vec<Part>,
}

impl Message {
    pub fn new(info: MessageInfo) -> Self {
        Self {
            info,
            parts: Vec::new(),
        }
    }

    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}
