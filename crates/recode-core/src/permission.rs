use async_trait::async_trait;
use parking_lot::Mutex;
use recode_schema::id::SessionId;
use recode_schema::permission::{Decision, PermissionReply, PermissionRequest, Rule, Ruleset};
use recode_schema::tool::ToolFailure;
use std::sync::Arc;
use std::time::Duration;

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

/// Deny/allow/ask evaluation with session-scoped "always" grants.
pub struct PermissionService {
    rules: Mutex<Ruleset>,
    responder: Arc<dyn PermissionResponder>,
    timeout: Duration,
}

impl PermissionService {
    pub fn new(rules: Ruleset, responder: Arc<dyn PermissionResponder>, timeout: Duration) -> Self {
        Self {
            rules: Mutex::new(rules),
            responder,
            timeout,
        }
    }

    pub fn add_rule(&self, rule: Rule) {
        self.rules.lock().push(rule);
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
            session_id,
            permission: permission.to_owned(),
            patterns: patterns.clone(),
            tool: Some(tool.to_owned()),
            metadata,
            created_at: recode_schema::now_ms(),
        };
        let reply = tokio::time::timeout(self.timeout, self.responder.ask(request))
            .await
            .unwrap_or(PermissionReply::Reject);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn denies_star_deny_rules() {
        let rules = Ruleset::new(vec![Rule::deny("edit", "*")]);
        let service = PermissionService::new(rules, Arc::new(AutoReject), Duration::from_secs(1));
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
        let service =
            PermissionService::new(Ruleset::default(), Arc::new(Always), Duration::from_secs(1));
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
}
