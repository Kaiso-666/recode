//! Re:Code domain schema.
//!
//! This crate is deliberately dependency-light and I/O-free: it defines the
//! vocabulary shared by storage, providers, tools, the server and the TUI.

pub mod config;
pub mod event;
pub mod id;
pub mod llm;
pub mod message;
pub mod model;
pub mod part;
pub mod permission;
pub mod project;
pub mod tool;

pub use id::{EventId, MessageId, PartId, ProjectId, SessionId, ToolCallId};
pub use message::{Message, MessageInfo, Role};
pub use model::ModelRef;
pub use part::{Part, ToolState, ToolStatus};
pub use project::ProjectInfo;

/// Milliseconds since the Unix epoch, matching the JavaScript ecosystem and
/// opencode's on-disk format.
pub type Timestamp = i64;

/// Current Unix time in milliseconds.
pub fn now_ms() -> Timestamp {
    chrono::Utc::now().timestamp_millis()
}

/// A small helper for serde defaults on booleans that should default to true.
pub(crate) fn default_true() -> bool {
    true
}
