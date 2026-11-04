use crate::{Tool, ToolContext};
use async_trait::async_trait;
use recode_schema::permission::Decision;
use recode_schema::question::Question;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct QuestionTool;

#[derive(Deserialize)]
struct Input {
    questions: Vec<Question>,
}

#[async_trait]
impl Tool for QuestionTool {
    fn name(&self) -> &str {
        "question"
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for an answer. Use this when a decision is genuinely \
         ambiguous and guessing would be costly: offer 2-5 concrete options with short labels \
         and descriptions, and set `multiple` only when more than one answer applies. The user \
         can also type a custom answer. Do not use it for confirmation of routine actions; \
         permissions already cover those."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "Questions to ask, in order. At most 8.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The complete question, ending in a question mark."
                            },
                            "header": {
                                "type": "string",
                                "description": "Short label (3-5 words) for the prompt header."
                            },
                            "options": {
                                "type": "array",
                                "description": "Concrete answers to choose from.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Short answer shown in the list."
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "One sentence explaining the trade-off."
                                        }
                                    },
                                    "required": ["label"],
                                    "additionalProperties": false
                                }
                            },
                            "multiple": {
                                "type": "boolean",
                                "description": "Allow selecting more than one option. Defaults to false."
                            }
                        },
                        "required": ["question", "options"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["questions"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> &str {
        "question"
    }

    fn default_decision(&self) -> Decision {
        Decision::Allow
    }

    fn title(&self, input: &Value) -> String {
        let header = input
            .get("questions")
            .and_then(Value::as_array)
            .and_then(|questions| questions.first())
            .and_then(|question| question.get("header"))
            .and_then(Value::as_str)
            .filter(|header| !header.is_empty());
        match header {
            Some(header) => format!("Ask: {header}"),
            None => "Ask the user".to_owned(),
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid question arguments: {error}")))?;
        let asker = context
            .question
            .as_ref()
            .ok_or_else(|| ToolFailure::new("questions are not available in this session"))?;
        let answers = asker.ask(input.questions.clone()).await?;
        if answers.len() != input.questions.len() {
            return Err(ToolFailure::new(
                "the answer did not cover every question; ask again",
            ));
        }
        let mut output = String::new();
        for (question, answer) in input.questions.iter().zip(&answers) {
            output.push_str("Q: ");
            output.push_str(question.question.trim());
            output.push('\n');
            output.push_str("A: ");
            let answer = answer
                .iter()
                .map(|entry| entry.trim())
                .filter(|entry| !entry.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(if answer.is_empty() {
                "(no answer)"
            } else {
                &answer
            });
            output.push('\n');
        }
        Ok(ToolOutcome {
            title: Some(format!("Asked {} question(s)", input.questions.len())),
            output,
            metadata: Some(json!({"answers": answers})),
            attachments: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QuestionAsker;
    use crate::ToolRegistry;
    use recode_schema::id::{MessageId, SessionId};
    use recode_schema::question::QuestionOption;
    use std::sync::Arc;

    struct Fixed;

    #[async_trait]
    impl QuestionAsker for Fixed {
        async fn ask(&self, questions: Vec<Question>) -> Result<Vec<Vec<String>>, ToolFailure> {
            Ok(questions
                .iter()
                .map(|question| {
                    vec![question
                        .options
                        .first()
                        .map(|option: &QuestionOption| option.label.clone())
                        .unwrap_or_else(|| "custom".into())]
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn returns_the_selected_answers() {
        let dir = tempfile::tempdir().unwrap();
        let context = ToolContext::new(
            SessionId::new(),
            MessageId::new(),
            "call",
            dir.path().to_path_buf(),
        )
        .with_question(Arc::new(Fixed));
        let registry = ToolRegistry::builtin();
        let outcome = registry
            .execute(
                "question",
                json!({"questions": [{
                    "question": "Which database?",
                    "header": "Database",
                    "options": [{"label": "SQLite"}, {"label": "Postgres"}]
                }]}),
                &context,
            )
            .await
            .unwrap();
        assert!(outcome.output.contains("Q: Which database?"));
        assert!(outcome.output.contains("A: SQLite"));
    }
}
