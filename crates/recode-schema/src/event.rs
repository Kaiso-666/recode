use crate::id::{MessageId, PartId, ProjectId, SessionId};
use crate::message::{MessageError, MessageInfo};
use crate::part::Part;
use crate::permission::{PermissionReply, PermissionRequest};
use crate::Timestamp;
use serde::{Deserialize, Serialize};

/// Session header projection.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub directory: std::path::PathBuf,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<SessionId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::model::ModelRef>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

impl SessionInfo {
    pub fn new(project_id: ProjectId, directory: impl Into<std::path::PathBuf>) -> Self {
        let now = crate::now_ms();
        Self {
            id: SessionId::new(),
            project_id,
            directory: directory.into(),
            title: String::new(),
            parent_id: None,
            created_at: now,
            updated_at: now,
            archived: false,
            agent: None,
            model: None,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn display_title(&self) -> String {
        if self.title.is_empty() {
            "Untitled session".to_owned()
        } else {
            self.title.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    Text,
    Reasoning,
}

/// Durable events. Every projection table is a fold over this stream.
///
/// Events are appended one at a time and broadcast immediately; the largest
/// variant (a full session header) is rare and boxing it would complicate
/// every producer for no measurable gain.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    SessionCreated {
        session: SessionInfo,
    },
    SessionUpdated {
        session: SessionInfo,
    },
    SessionDeleted {
        session_id: SessionId,
    },
    MessageAdded {
        message: MessageInfo,
    },
    MessageRemoved {
        session_id: SessionId,
        message_id: MessageId,
    },
    PartAdded {
        session_id: SessionId,
        message_id: MessageId,
        part: Part,
    },
    PartUpdated {
        session_id: SessionId,
        message_id: MessageId,
        part: Part,
    },
    PartDelta {
        session_id: SessionId,
        message_id: MessageId,
        part_id: PartId,
        kind: DeltaKind,
        delta: String,
    },
    PartRemoved {
        session_id: SessionId,
        message_id: MessageId,
        part_id: PartId,
    },
    SessionIdle {
        session_id: SessionId,
    },
    SessionError {
        session_id: SessionId,
        error: MessageError,
    },
    PermissionAsked {
        request: PermissionRequest,
    },
    PermissionReplied {
        session_id: SessionId,
        request_id: String,
        reply: PermissionReply,
    },
}

impl Event {
    pub fn session_id(&self) -> &SessionId {
        match self {
            Event::SessionCreated { session } | Event::SessionUpdated { session } => &session.id,
            Event::SessionDeleted { session_id }
            | Event::MessageRemoved { session_id, .. }
            | Event::PartAdded { session_id, .. }
            | Event::PartUpdated { session_id, .. }
            | Event::PartDelta { session_id, .. }
            | Event::PartRemoved { session_id, .. }
            | Event::SessionIdle { session_id }
            | Event::SessionError { session_id, .. } => session_id,
            Event::MessageAdded { message } => &message.session_id,
            Event::PermissionAsked { request } => &request.session_id,
            Event::PermissionReplied { session_id, .. } => session_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub id: crate::id::EventId,
    pub session_id: SessionId,
    pub seq: u64,
    pub created_at: Timestamp,
    pub event: Event,
}

/// Events broadcast to live subscribers (TUI, SSE clients). This is a
/// superset of durable events with volatile signals such as streaming deltas
/// and permission requests kept out of the store where noted.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize)]
pub enum LiveEvent {
    Durable(EventEnvelope),
    /// The session runner changed state without a durable event.
    SessionStatus {
        session_id: SessionId,
        status: SessionStatus,
    },
    /// A provider stream produced a token delta. Also persisted via
    /// `PartDelta`; this copy is delivery-only.
    Delta {
        session_id: SessionId,
        message_id: MessageId,
        part_id: PartId,
        kind: DeltaKind,
        delta: String,
    },
    /// A permission request is waiting for a client reply.
    PermissionAsked(PermissionRequest),
    PermissionReplied {
        session_id: SessionId,
        request_id: String,
        reply: PermissionReply,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Idle,
    Busy,
    Retry,
}
