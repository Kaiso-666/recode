use crate::id::{MessageId, PartId, ProjectId, SessionId};
use crate::message::{MessageError, MessageInfo};
use crate::part::Part;
use crate::permission::{PermissionReply, PermissionRequest};
use crate::question::{QuestionReply, QuestionRequest};
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
    /// Older history was summarized. `through` is the last message included in
    /// the summary; only messages after it are sent to the model verbatim.
    SessionCompacted {
        session_id: SessionId,
        summary: String,
        through: MessageId,
        tokens_before: u64,
        tokens_after: u64,
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
            | Event::SessionError { session_id, .. }
            | Event::SessionCompacted { session_id, .. } => session_id,
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// The model asked the user a question and is waiting for an answer.
    QuestionAsked(QuestionRequest),
    QuestionReplied {
        session_id: SessionId,
        request_id: String,
        reply: QuestionReply,
    },
}

impl LiveEvent {
    /// Apply this event to a transcript in place.
    ///
    /// Returns `false` when the event cannot be applied — a delta for a part
    /// the transcript does not have yet, for example — so the caller can
    /// reload the transcript from the store instead of guessing.
    pub fn apply(&self, messages: &mut Vec<crate::message::Message>) -> bool {
        match self {
            LiveEvent::Delta {
                message_id,
                part_id,
                kind,
                delta,
                ..
            } => append_delta(messages, message_id, part_id, kind, delta),
            LiveEvent::Durable(envelope) => match &envelope.event {
                Event::PartDelta {
                    message_id,
                    part_id,
                    kind,
                    delta,
                    ..
                } => append_delta(messages, message_id, part_id, kind, delta),
                Event::MessageAdded { message } => {
                    if messages.iter().any(|item| item.info.id == message.id) {
                        return true;
                    }
                    messages.push(crate::message::Message::new(message.clone()));
                    true
                }
                Event::MessageRemoved { message_id, .. } => {
                    messages.retain(|item| item.info.id != *message_id);
                    true
                }
                Event::PartAdded {
                    message_id, part, ..
                } => {
                    let Some(message) =
                        messages.iter_mut().find(|item| item.info.id == *message_id)
                    else {
                        return false;
                    };
                    if message
                        .parts
                        .iter()
                        .any(|existing| existing.id() == part.id())
                    {
                        return true;
                    }
                    message.parts.push(part.clone());
                    true
                }
                Event::PartUpdated {
                    message_id, part, ..
                } => {
                    let Some(message) =
                        messages.iter_mut().find(|item| item.info.id == *message_id)
                    else {
                        return false;
                    };
                    match message
                        .parts
                        .iter_mut()
                        .find(|existing| existing.id() == part.id())
                    {
                        Some(existing) => *existing = part.clone(),
                        None => message.parts.push(part.clone()),
                    }
                    true
                }
                Event::PartRemoved {
                    message_id,
                    part_id,
                    ..
                } => {
                    let Some(message) =
                        messages.iter_mut().find(|item| item.info.id == *message_id)
                    else {
                        return false;
                    };
                    message.parts.retain(|existing| existing.id() != part_id);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// Append streamed text to the matching part of a transcript.
fn append_delta(
    messages: &mut [crate::message::Message],
    message_id: &MessageId,
    part_id: &PartId,
    kind: &DeltaKind,
    delta: &str,
) -> bool {
    let Some(message) = messages
        .iter_mut()
        .find(|message| message.info.id == *message_id)
    else {
        return false;
    };
    let Some(part) = message.parts.iter_mut().find(|part| part.id() == part_id) else {
        return false;
    };
    match kind {
        DeltaKind::Text => match part.text_mut() {
            Some(text) => {
                text.text.push_str(delta);
                true
            }
            None => false,
        },
        DeltaKind::Reasoning => match part.reasoning_mut() {
            Some(reasoning) => {
                reasoning.text.push_str(delta);
                true
            }
            None => false,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Idle,
    Busy,
    Retry,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::EventId;
    use crate::message::MessageInfo;
    use crate::part::TextPart;
    use crate::Role;

    fn transcript() -> (Vec<crate::message::Message>, MessageId, PartId, SessionId) {
        let session_id = SessionId::new();
        let message_id = MessageId::new();
        let part_id = PartId::new();
        let mut info = MessageInfo::user(session_id.clone());
        info.id = message_id.clone();
        info.role = Role::Assistant;
        let message = crate::message::Message {
            info,
            parts: vec![Part::Text(TextPart {
                id: part_id.clone(),
                text: "Hello".into(),
                synthetic: false,
                time: None,
            })],
        };
        (vec![message], message_id, part_id, session_id)
    }

    fn text_of(messages: &[crate::message::Message]) -> String {
        messages[0]
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn applies_live_and_durable_deltas() {
        let (mut messages, message_id, part_id, session_id) = transcript();

        let live = LiveEvent::Delta {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            part_id: part_id.clone(),
            kind: DeltaKind::Text,
            delta: " world".into(),
        };
        assert!(live.apply(&mut messages));
        assert_eq!(text_of(&messages), "Hello world");

        let durable = LiveEvent::Durable(EventEnvelope {
            id: EventId::new(),
            session_id: session_id.clone(),
            seq: 1,
            created_at: 0,
            event: Event::PartDelta {
                session_id,
                message_id: message_id.clone(),
                part_id: part_id.clone(),
                kind: DeltaKind::Text,
                delta: "!".into(),
            },
        });
        assert!(durable.apply(&mut messages));
        assert_eq!(text_of(&messages), "Hello world!");
    }

    #[test]
    fn applies_durable_parts() {
        let (mut messages, message_id, _part_id, session_id) = transcript();
        let part_id = PartId::new();
        let durable = LiveEvent::Durable(EventEnvelope {
            id: EventId::new(),
            session_id: session_id.clone(),
            seq: 2,
            created_at: 0,
            event: Event::PartAdded {
                session_id,
                message_id,
                part: Part::Text(TextPart {
                    id: part_id,
                    text: "second".into(),
                    synthetic: false,
                    time: None,
                }),
            },
        });
        assert!(durable.apply(&mut messages));
        assert_eq!(messages[0].parts.len(), 2);
    }

    #[test]
    fn refuses_events_for_unknown_messages() {
        let (mut messages, _message_id, _part_id, _session_id) = transcript();
        let durable = LiveEvent::Durable(EventEnvelope {
            id: EventId::new(),
            session_id: SessionId::new(),
            seq: 3,
            created_at: 0,
            event: Event::PartDelta {
                session_id: SessionId::new(),
                message_id: MessageId::new(),
                part_id: PartId::new(),
                kind: DeltaKind::Text,
                delta: "ignored".into(),
            },
        });
        assert!(!durable.apply(&mut messages));
        assert_eq!(text_of(&messages), "Hello");
    }
}
