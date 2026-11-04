use async_trait::async_trait;
use parking_lot::Mutex;
use recode_schema::event::LiveEvent;
use recode_schema::id::SessionId;
use recode_schema::question::{Question, QuestionReply, QuestionRequest};
use recode_schema::tool::ToolFailure;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};

/// Answers a question on an interactive surface (terminal, TUI, UI).
#[async_trait]
pub trait QuestionResponder: Send + Sync {
    async fn ask(&self, request: QuestionRequest) -> QuestionReply;
}

/// Picks the first option for every question. Used by `--auto`, where no human
/// is present and a question must never stall the run.
pub struct AutoAnswerFirst;

#[async_trait]
impl QuestionResponder for AutoAnswerFirst {
    async fn ask(&self, request: QuestionRequest) -> QuestionReply {
        let mut answers = Vec::with_capacity(request.questions.len());
        for question in &request.questions {
            match question.options.first() {
                Some(option) => answers.push(vec![option.label.clone()]),
                None => return QuestionReply::Reject,
            }
        }
        QuestionReply::Answer { answers }
    }
}

/// Rejects every question. Safe default for headless commands that never ask.
pub struct AutoRejectQuestions;

#[async_trait]
impl QuestionResponder for AutoRejectQuestions {
    async fn ask(&self, _request: QuestionRequest) -> QuestionReply {
        QuestionReply::Reject
    }
}

struct Pending {
    session_id: SessionId,
    request: QuestionRequest,
    sender: oneshot::Sender<QuestionReply>,
}

/// Broker for model-initiated questions.
///
/// With an in-process [`QuestionResponder`] the question is delivered directly
/// to that surface. Without one (server mode) it is published as
/// [`LiveEvent::QuestionAsked`] and resolved by [`QuestionService::reply`] from
/// an HTTP client.
pub struct QuestionService {
    responder: Option<Arc<dyn QuestionResponder>>,
    bus: Option<broadcast::Sender<LiveEvent>>,
    pending: Mutex<HashMap<String, Pending>>,
    timeout: Duration,
}

impl QuestionService {
    pub fn new(
        responder: Option<Arc<dyn QuestionResponder>>,
        bus: Option<broadcast::Sender<LiveEvent>>,
        timeout: Duration,
    ) -> Self {
        Self {
            responder,
            bus,
            pending: Mutex::new(HashMap::new()),
            timeout,
        }
    }

    /// Requests waiting for a reply, optionally filtered by session.
    pub fn pending_requests(&self, session_id: Option<&SessionId>) -> Vec<QuestionRequest> {
        self.pending
            .lock()
            .values()
            .filter(|pending| session_id.is_none_or(|id| &pending.session_id == id))
            .map(|pending| pending.request.clone())
            .collect()
    }

    /// Resolve a pending request. Returns false when the id is unknown or the
    /// request already timed out.
    pub fn reply(&self, request_id: &str, reply: QuestionReply) -> bool {
        let Some(pending) = self.pending.lock().remove(request_id) else {
            return false;
        };
        let session_id = pending.session_id.clone();
        if pending.sender.send(reply.clone()).is_err() {
            return false;
        }
        if let Some(bus) = &self.bus {
            let _ = bus.send(LiveEvent::QuestionReplied {
                session_id,
                request_id: request_id.to_owned(),
                reply,
            });
        }
        true
    }

    /// Ask the user. Rejection and timeout both fail the tool with a message
    /// the model can react to.
    pub async fn ask(
        &self,
        session_id: SessionId,
        tool: Option<String>,
        questions: Vec<Question>,
    ) -> Result<Vec<Vec<String>>, ToolFailure> {
        if questions.is_empty() {
            return Err(ToolFailure::new("at least one question is required"));
        }
        if questions.len() > 8 {
            return Err(ToolFailure::new("at most 8 questions may be asked at once"));
        }
        for question in &questions {
            if question.question.trim().is_empty() {
                return Err(ToolFailure::new("question text must not be empty"));
            }
            if question.options.len() > 12 {
                return Err(ToolFailure::new(
                    "a question may offer at most 12 options; use a free-form answer instead",
                ));
            }
            if question
                .options
                .iter()
                .any(|option| option.label.trim().is_empty())
            {
                return Err(ToolFailure::new("option labels must not be empty"));
            }
        }
        let request = QuestionRequest {
            id: format!("que_{}", uuid::Uuid::new_v4().simple()),
            session_id,
            questions,
            tool,
            created_at: recode_schema::now_ms(),
        };
        let reply = match &self.responder {
            Some(responder) => tokio::time::timeout(self.timeout, responder.ask(request))
                .await
                .unwrap_or(QuestionReply::Reject),
            None => self.ask_broker(request).await,
        };
        match reply {
            QuestionReply::Answer { answers } => Ok(answers),
            QuestionReply::Reject => Err(ToolFailure::new(
                "the user declined to answer; continue without their input or ask a different way",
            )),
        }
    }

    async fn ask_broker(&self, request: QuestionRequest) -> QuestionReply {
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().insert(
            request.id.clone(),
            Pending {
                session_id: request.session_id.clone(),
                request: request.clone(),
                sender,
            },
        );
        if let Some(bus) = &self.bus {
            let _ = bus.send(LiveEvent::QuestionAsked(request.clone()));
        }
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(reply)) => reply,
            _ => {
                self.pending.lock().remove(&request.id);
                QuestionReply::Reject
            }
        }
    }
}

/// Adapter handed to the `question` tool through [`recode_tool::ToolContext`].
pub struct EngineQuestions {
    engine: Arc<crate::engine::Engine>,
    session_id: SessionId,
}

impl EngineQuestions {
    pub fn new(engine: Arc<crate::engine::Engine>, session_id: SessionId) -> Self {
        Self { engine, session_id }
    }
}

#[async_trait]
impl recode_tool::QuestionAsker for EngineQuestions {
    async fn ask(&self, questions: Vec<Question>) -> Result<Vec<Vec<String>>, ToolFailure> {
        self.engine
            .questions
            .ask(self.session_id.clone(), Some("question".into()), questions)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recode_schema::question::QuestionOption;
    fn question() -> Question {
        Question {
            question: "Which database?".into(),
            header: "Database".into(),
            options: vec![
                QuestionOption {
                    label: "SQLite".into(),
                    description: None,
                },
                QuestionOption {
                    label: "Postgres".into(),
                    description: None,
                },
            ],
            multiple: false,
        }
    }

    #[tokio::test]
    async fn broker_reply_resolves_the_request() {
        let (bus, mut receiver) = broadcast::channel(8);
        let service = Arc::new(QuestionService::new(
            None,
            Some(bus),
            Duration::from_secs(5),
        ));
        let waiting = {
            let service = service.clone();
            tokio::spawn(async move {
                service
                    .ask(SessionId::new(), Some("question".into()), vec![question()])
                    .await
            })
        };
        let event = receiver.recv().await.unwrap();
        let LiveEvent::QuestionAsked(request) = event else {
            panic!("expected a question request");
        };
        assert!(service.reply(
            &request.id,
            QuestionReply::Answer {
                answers: vec![vec!["Postgres".into()]]
            }
        ));
        let answers = waiting.await.unwrap().unwrap();
        assert_eq!(answers, vec![vec!["Postgres".to_owned()]]);
    }

    #[tokio::test]
    async fn auto_answer_picks_the_first_option() {
        let service = QuestionService::new(
            Some(Arc::new(AutoAnswerFirst)),
            None,
            Duration::from_secs(1),
        );
        let answers = service
            .ask(SessionId::new(), None, vec![question()])
            .await
            .unwrap();
        assert_eq!(answers, vec![vec!["SQLite".to_owned()]]);
    }

    #[tokio::test]
    async fn rejection_becomes_a_tool_failure() {
        let service = QuestionService::new(
            Some(Arc::new(AutoRejectQuestions)),
            None,
            Duration::from_secs(1),
        );
        let error = service
            .ask(SessionId::new(), None, vec![question()])
            .await
            .unwrap_err();
        assert!(error.message.contains("declined"));
    }
}
