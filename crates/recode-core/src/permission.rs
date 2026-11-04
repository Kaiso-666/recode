use async_trait::async_trait;
use parking_lot::Mutex;
use recode_schema::event::LiveEvent;
use recode_schema::id::SessionId;
use recode_schema::permission::{Decision, PermissionReply, PermissionRequest, Rule, Ruleset};
use recode_schema::tool::ToolFailure;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};

/// Asks the user (or an automated policy) to resolve an `ask` rule.
#[async_trait]
pub trait PermissionResponder: Send + Sync {
    async fn ask(&self, request: PermissionRequest) -> PermissionReply;
}

/// Approves everything. Used by `recode run --auto` and tests.
pub struct AutoApprove;

#[async_trait]
impl PermissionResponder for AutoApprove {
    async fn ask(&self, _request: PermissionRequest) -> PermissionReply {
        PermissionReply::Once
    }
}

/// Rejects everything that requires approval. Safe default for headless runs.
pub struct AutoReject;

#[async_trait]
impl PermissionResponder for AutoReject {
    async fn ask(&self, _request: PermissionRequest) -> PermissionReply {
        PermissionReply::Reject
    }
}

struct Pending {
    session_id: SessionId,
    request: PermissionRequest,
    sender: oneshot::Sender<PermissionReply>,
}

/// Deny/allow/ask evaluation with session-scoped "always" grants.
///
/// When no in-process [`PermissionResponder`] is configured (server mode), an
/// `ask` publishes [`LiveEvent::PermissionAsked`] on the engine bus and waits
/// for a client to call [`PermissionService::reply`]. This is the same broker
/// the TUI uses through its responder, just with the approval transported over
/// HTTP/SSE instead of a modal.
pub struct PermissionService {
    rules: Mutex<Ruleset>,
    responder: Option<Arc<dyn PermissionResponder>>,
    bus: Option<broadcast::Sender<LiveEvent>>,
    pending: Mutex<HashMap<String, Pending>>,
    timeout: Duration,
}

impl PermissionService {
    pub fn new(
        rules: Ruleset,
        responder: Option<Arc<dyn PermissionResponder>>,
        bus: Option<broadcast::Sender<LiveEvent>>,
        timeout: Duration,
    ) -> Self {
        Self {
            rules: Mutex::new(rules),
            responder,
            bus,
            pending: Mutex::new(HashMap::new()),
            timeout,
        }
    }

    pub fn add_rule(&self, rule: Rule) {
        self.rules.lock().push(rule);
    }

    /// Requests waiting for a reply, optionally filtered by session.
    pub fn pending_requests(&self, session_id: Option<&SessionId>) -> Vec<PermissionRequest> {
        self.pending
            .lock()
            .values()
            .filter(|pending| session_id.is_none_or(|id| &pending.session_id == id))
            .map(|pending| pending.request.clone())
            .collect()
    }

    /// Resolve a pending request. Returns false when the id is unknown or the
    /// request already timed out.
    pub fn reply(&self, request_id: &str, reply: PermissionReply) -> bool {
        let Some(pending) = self.pending.lock().remove(request_id) else {
            return false;
        };
        let session_id = pending.session_id.clone();
        if pending.sender.send(reply).is_err() {
            return false;
        }
        if let Some(bus) = &self.bus {
            let _ = bus.send(LiveEvent::PermissionReplied {
                session_id,
                request_id: request_id.to_owned(),
                reply,
            });
        }
        true
    }

    /// Authorize a tool invocation. `patterns` are the concrete resources the
    /// tool will touch; `default` applies when no rule matches.
    #[allow(clippy::too_many_arguments)]
    pub async fn authorize(
        &self,
        session_id: SessionId,
        permission: &str,
        tool: &str,
        patterns: Vec<String>,
        default: Decision,
        metadata: serde_json::Value,
    ) -> Result<(), ToolFailure> {
        let patterns = if patterns.is_empty() {
            vec!["*".to_owned()]
        } else {
            patterns
        };
        let mut must_ask = false;
        for pattern in &patterns {
            match self.rules.lock().evaluate(permission, pattern) {
                Some(Decision::Deny) => {
                    return Err(ToolFailure::new(format!(
                        "permission denied: {permission} {pattern}\n\
                         A deny rule matched. Change `permission` in recode.json to allow it."
                    )));
                }
                Some(Decision::Ask) => must_ask = true,
                Some(Decision::Allow) => {}
                None => match default {
                    Decision::Deny => {
                        return Err(ToolFailure::new(format!(
                            "permission denied by default: {permission} {pattern}"
                        )));
                    }
                    Decision::Ask => must_ask = true,
                    Decision::Allow => {}
                },
            }
        }
        if !must_ask {
            return Ok(());
        }
        let request = PermissionRequest {
            id: format!("per_{}", uuid::Uuid::new_v4().simple()),
            session_id: session_id.clone(),
            permission: permission.to_owned(),
            patterns: patterns.clone(),
            tool: Some(tool.to_owned()),
            metadata,
            created_at: recode_schema::now_ms(),
        };
        let reply = match &self.responder {
            Some(responder) => tokio::time::timeout(self.timeout, responder.ask(request))
                .await
                .unwrap_or(PermissionReply::Reject),
            None => self.ask_broker(request).await,
        };
        match reply {
            PermissionReply::Once => Ok(()),
            PermissionReply::Always => {
                let mut rules = self.rules.lock();
                for pattern in patterns {
                    rules.push(Rule::allow(permission, pattern));
                }
                Ok(())
            }
            PermissionReply::Reject => Err(ToolFailure::new(format!(
                "permission denied by the user: {permission} {}",
                patterns.join(", ")
            ))),
        }
    }

    async fn ask_broker(&self, request: PermissionRequest) -> PermissionReply {
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
            let _ = bus.send(LiveEvent::PermissionAsked(request.clone()));
        }
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(reply)) => reply,
            _ => {
                self.pending.lock().remove(&request.id);
                PermissionReply::Reject
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn denies_star_deny_rules() {
        let rules = Ruleset::new(vec![Rule::deny("edit", "*")]);
        let service = PermissionService::new(
            rules,
            Some(Arc::new(AutoReject)),
            None,
            Duration::from_secs(1),
        );
        let error = service
            .authorize(
                SessionId::new(),
                "edit",
                "write",
                vec!["a.txt".into()],
                Decision::Allow,
                serde_json::Value::Null,
            )
            .await
            .unwrap_err();
        assert!(error.message.contains("permission denied"));
    }

    #[tokio::test]
    async fn always_adds_a_session_rule() {
        struct Always;
        #[async_trait]
        impl PermissionResponder for Always {
            async fn ask(&self, _request: PermissionRequest) -> PermissionReply {
                PermissionReply::Always
            }
        }
        let service = PermissionService::new(
            Ruleset::default(),
            Some(Arc::new(Always)),
            None,
            Duration::from_secs(1),
        );
        service
            .authorize(
                SessionId::new(),
                "bash",
                "bash",
                vec!["git status".into()],
                Decision::Ask,
                serde_json::Value::Null,
            )
            .await
            .unwrap();
        assert_eq!(
            service.rules.lock().evaluate("bash", "git status"),
            Some(Decision::Allow)
        );
    }

    #[tokio::test]
    async fn broker_reply_resolves_the_request() {
        let session = SessionId::new();
        let (bus, mut receiver) = broadcast::channel(8);
        let service = Arc::new(PermissionService::new(
            Ruleset::default(),
            None,
            Some(bus),
            Duration::from_secs(5),
        ));
        let waiting = {
            let service = service.clone();
            let session = session.clone();
            tokio::spawn(async move {
                service
                    .authorize(
                        session,
                        "bash",
                        "bash",
                        vec!["git push".into()],
                        Decision::Ask,
                        serde_json::Value::Null,
                    )
                    .await
            })
        };
        let event = receiver.recv().await.unwrap();
        let LiveEvent::PermissionAsked(request) = event else {
            panic!("expected a permission request");
        };
        assert_eq!(request.patterns, vec!["git push".to_owned()]);
        assert!(service.reply(&request.id, PermissionReply::Once));
        assert!(waiting.await.unwrap().is_ok());
    }
}
