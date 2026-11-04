use crate::id::SessionId;
use crate::Timestamp;
use serde::{Deserialize, Serialize};

/// One selectable answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A single question the model wants the user to answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    /// The full question text.
    pub question: String,
    /// Short label for tab/header rendering (3-5 words).
    #[serde(default)]
    pub header: String,
    /// Offered answers. The user may always type a custom answer instead.
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    /// Whether more than one option may be selected.
    #[serde(default)]
    pub multiple: bool,
}

/// A pending request for user input.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionRequest {
    pub id: String,
    pub session_id: SessionId,
    pub questions: Vec<Question>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub created_at: Timestamp,
}

/// The user's answer. `answers[i]` holds the selected labels for
/// `questions[i]`; a free-form answer is stored as a single entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuestionReply {
    Answer { answers: Vec<Vec<String>> },
    Reject,
}
