use serde::{Deserialize, Serialize};

/// A file or image produced by a tool and attached to the transcript.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAttachment {
    pub mime: String,
    pub filename: String,
    /// `data:` URL.
    pub url: String,
}

/// Successful tool execution, before it is persisted as a `tool` part.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ToolAttachment>,
}

impl ToolOutcome {
    pub fn text(output: impl Into<String>) -> Self {
        Self {
            title: None,
            output: output.into(),
            metadata: None,
            attachments: Vec::new(),
        }
    }

    pub fn titled(title: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..Self::text(output)
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// A tool failed in a way the model should see and can recover from.
#[derive(Clone, Debug, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct ToolFailure {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl ToolFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            metadata: None,
        }
    }
}

impl From<std::io::Error> for ToolFailure {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub type ToolResult = Result<ToolOutcome, ToolFailure>;
