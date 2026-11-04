use crate::id::PartId;
use crate::llm::{FinishReason, Usage};
use crate::model::ModelRef;
use crate::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartTime {
    pub start: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Timestamp>,
}

impl PartTime {
    pub fn started() -> Self {
        Self {
            start: crate::now_ms(),
            end: None,
        }
    }

    pub fn finish(&mut self) {
        self.end = Some(crate::now_ms());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPart {
    pub id: PartId,
    #[serde(default)]
    pub text: String,
    /// Synthetic text is injected by the system (summaries, reminders) and is
    /// rendered dimmed; it is still sent to the model.
    #[serde(default)]
    pub synthetic: bool,
    #[serde(default)]
    pub time: Option<PartTime>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningPart {
    pub id: PartId,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default)]
    pub time: Option<PartTime>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Error,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolState {
    #[serde(default)]
    pub status: ToolStatus,
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub time: Option<PartTime>,
    /// Raw accumulated argument JSON, useful for diagnostics while streaming.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw: String,
}

impl ToolState {
    pub fn pending(input: serde_json::Value) -> Self {
        Self {
            status: ToolStatus::Pending,
            input,
            ..Default::default()
        }
    }

    pub fn running(mut self, title: Option<String>) -> Self {
        self.status = ToolStatus::Running;
        self.title = title;
        self.time = Some(PartTime::started());
        self
    }

    pub fn completed(mut self, output: String, title: Option<String>) -> Self {
        self.status = ToolStatus::Completed;
        self.output = Some(output);
        if title.is_some() {
            self.title = title;
        }
        match &mut self.time {
            Some(time) => time.finish(),
            None => {
                self.time = Some(PartTime {
                    start: crate::now_ms(),
                    end: Some(crate::now_ms()),
                })
            }
        }
        self
    }

    pub fn failed(mut self, error: String) -> Self {
        self.status = ToolStatus::Error;
        self.error = Some(error);
        match &mut self.time {
            Some(time) => time.finish(),
            None => {
                self.time = Some(PartTime {
                    start: crate::now_ms(),
                    end: Some(crate::now_ms()),
                })
            }
        }
        self
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.status, ToolStatus::Completed | ToolStatus::Error)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPart {
    pub id: PartId,
    pub call_id: String,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default)]
    pub state: ToolState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepStartPart {
    pub id: PartId,
    #[serde(default)]
    pub time: Option<PartTime>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepFinishPart {
    pub id: PartId,
    pub reason: FinishReason,
    #[serde(default)]
    pub tokens: Usage,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub time: Option<PartTime>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePart {
    pub id: PartId,
    pub mime: String,
    pub filename: String,
    /// `data:` URL or `file://` URL.
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPart {
    pub id: PartId,
    pub hash: String,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPart {
    pub id: PartId,
    pub snapshot: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPart {
    pub id: PartId,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPart {
    pub id: PartId,
    pub model: ModelRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionPart {
    pub id: PartId,
    #[serde(default)]
    pub auto: bool,
    #[serde(default)]
    pub overflow: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPart {
    pub id: PartId,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One block of a message. Parts are the unit of persistence and streaming;
/// the TUI renders them in order and the provider adapters translate them into
/// wire messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text(TextPart),
    Reasoning(ReasoningPart),
    Tool(ToolPart),
    StepStart(StepStartPart),
    StepFinish(StepFinishPart),
    File(FilePart),
    Patch(PatchPart),
    Snapshot(SnapshotPart),
    Agent(AgentPart),
    Model(ModelPart),
    Compaction(CompactionPart),
    Error(ErrorPart),
}

impl Part {
    pub fn id(&self) -> &PartId {
        match self {
            Part::Text(part) => &part.id,
            Part::Reasoning(part) => &part.id,
            Part::Tool(part) => &part.id,
            Part::StepStart(part) => &part.id,
            Part::StepFinish(part) => &part.id,
            Part::File(part) => &part.id,
            Part::Patch(part) => &part.id,
            Part::Snapshot(part) => &part.id,
            Part::Agent(part) => &part.id,
            Part::Model(part) => &part.id,
            Part::Compaction(part) => &part.id,
            Part::Error(part) => &part.id,
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Part::Text(TextPart {
            id: PartId::new(),
            text: text.into(),
            synthetic: false,
            time: None,
        })
    }

    pub fn synthetic(text: impl Into<String>) -> Self {
        Part::Text(TextPart {
            id: PartId::new(),
            text: text.into(),
            synthetic: true,
            time: None,
        })
    }

    pub fn reasoning(text: impl Into<String>) -> Self {
        Part::Reasoning(ReasoningPart {
            id: PartId::new(),
            text: text.into(),
            signature: None,
            time: None,
        })
    }

    pub fn plain_text(&self) -> Option<&str> {
        match self {
            Part::Text(text) => Some(text.text.as_str()),
            _ => None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut TextPart> {
        match self {
            Part::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn reasoning_mut(&mut self) -> Option<&mut ReasoningPart> {
        match self {
            Part::Reasoning(reasoning) => Some(reasoning),
            _ => None,
        }
    }

    pub fn tool_mut(&mut self) -> Option<&mut ToolPart> {
        match self {
            Part::Tool(tool) => Some(tool),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_round_trip_with_type_tag() {
        let part = Part::text("hello");
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");
        let back: Part = serde_json::from_value(json).unwrap();
        assert_eq!(back.plain_text(), Some("hello"));
    }

    #[test]
    fn tool_state_transitions() {
        let state = ToolState::pending(serde_json::json!({"path": "a.txt"}))
            .running(Some("Reading a.txt".into()))
            .completed("contents".into(), None);
        assert_eq!(state.status, ToolStatus::Completed);
        assert!(state.is_terminal());
        assert!(state.time.unwrap().end.is_some());
    }
}
