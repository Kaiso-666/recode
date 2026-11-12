//! HTTP client for a running `recode serve` instance.
//!
//! Every method maps to one route of [`recode_server`]'s API. The client is
//! deliberately synchronous in shape (async functions, no event loop of its
//! own) so a TUI can poll a cached snapshot at its own pace and headless
//! commands can make one call and exit.

use recode_schema::event::SessionInfo;
use recode_schema::message::Message;
use recode_schema::permission::{PermissionReply, PermissionRequest};
use recode_schema::question::{QuestionReply, QuestionRequest};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Debug, Deserialize)]
pub struct ProviderSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CommandSummary {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub subagent: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CommandExpansion {
    pub text: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub subagent: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SessionSnapshot {
    pub session: SessionInfo,
    #[serde(default)]
    pub busy: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompactSummary {
    pub compacted: bool,
    #[serde(default, rename = "tokensBefore")]
    pub tokens_before: Option<u64>,
    #[serde(default, rename = "tokensAfter")]
    pub tokens_after: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RevertResponse {
    pub removed: usize,
    #[serde(default)]
    pub snapshot: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct RestoreResponse {
    snapshot: String,
}

#[derive(Clone, Debug, Deserialize)]
struct SessionsResponse {
    sessions: Vec<SessionInfo>,
}

#[derive(Clone, Debug, Deserialize)]
struct MessagesResponse {
    messages: Vec<Message>,
}

#[derive(Clone, Debug, Deserialize)]
struct AgentsResponse {
    agents: Vec<AgentSummary>,
}

#[derive(Clone, Debug, Deserialize)]
struct CommandsResponse {
    #[serde(default)]
    commands: Vec<CommandSummary>,
}

#[derive(Clone, Debug, Deserialize)]
struct ProvidersResponse {
    #[serde(default)]
    providers: Vec<ProviderSummary>,
}

#[derive(Clone, Debug, Deserialize)]
struct FilesResponse {
    files: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct FileResponse {
    content: String,
}

#[derive(Clone, Debug, Deserialize)]
struct PermissionsResponse {
    permissions: Vec<PermissionRequest>,
}

#[derive(Clone, Debug, Deserialize)]
struct QuestionsResponse {
    questions: Vec<QuestionRequest>,
}

#[derive(Deserialize)]
struct SessionResponse {
    session: SessionInfo,
}

#[derive(Deserialize)]
struct AbortResponse {
    aborted: bool,
}

#[derive(Clone)]
pub struct Client {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base: impl Into<String>, token: Option<String>) -> Self {
        let mut base = base.into();
        while base.ends_with('/') {
            base.pop();
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self { base, token, http }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// `GET /health` without credentials; useful for a quick connectivity
    /// check before the TUI takes over the terminal.
    pub async fn health(&self) -> Result<(), String> {
        let url = format!("{}/health", self.base);
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| format!("cannot reach {}: {error}", self.base))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("server returned HTTP {}", response.status()))
        }
    }

    pub async fn providers(&self) -> Result<Vec<ProviderSummary>, String> {
        let response: ProvidersResponse = self.get("/api/providers").await?;
        Ok(response.providers)
    }

    pub async fn agents(&self) -> Result<Vec<AgentSummary>, String> {
        let response: AgentsResponse = self.get("/api/agents").await?;
        Ok(response.agents)
    }

    pub async fn commands(&self) -> Result<Vec<CommandSummary>, String> {
        let response: CommandsResponse = self.get("/api/commands").await?;
        Ok(response.commands)
    }

    /// Expand a custom command on the server, where inline shell interpolation
    /// runs in the session directory.
    pub async fn expand_command(
        &self,
        session: &str,
        name: &str,
        arguments: &str,
    ) -> Result<CommandExpansion, String> {
        let body = serde_json::json!({ "sessionId": session, "arguments": arguments });
        self.post(&format!("/api/commands/{}", encode_segment(name)), &body)
            .await
    }

    pub async fn files(&self) -> Result<Vec<String>, String> {
        let response: FilesResponse = self.get("/api/files").await?;
        Ok(response.files)
    }

    pub async fn file_content(&self, path: &str) -> Result<String, String> {
        let response = self
            .http
            .get(format!("{}/api/files/content", self.base))
            .query(&[("path", path)])
            .headers(self.headers()?)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let response: FileResponse = self.decode(response).await?;
        Ok(response.content)
    }

    pub async fn sessions(&self, all: bool, archived: bool) -> Result<Vec<SessionInfo>, String> {
        let response = self
            .http
            .get(format!("{}/api/sessions", self.base))
            .query(&[("all", all.to_string()), ("archived", archived.to_string())])
            .headers(self.headers()?)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let response: SessionsResponse = self.decode(response).await?;
        Ok(response.sessions)
    }

    pub async fn delete_session(&self, id: &str) -> Result<(), String> {
        let response = self
            .http
            .delete(format!("{}/api/sessions/{id}", self.base))
            .headers(self.headers()?)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let _: serde_json::Value = self.decode(response).await?;
        Ok(())
    }

    pub async fn session(&self, id: &str) -> Result<SessionSnapshot, String> {
        self.get(&format!("/api/sessions/{id}")).await
    }

    pub async fn create_session(
        &self,
        agent: Option<String>,
        model: Option<String>,
        title: Option<String>,
    ) -> Result<SessionInfo, String> {
        let body = serde_json::json!({
            "agent": agent,
            "model": model,
            "title": title,
        });
        let response: SessionResponse = self.post("/api/sessions", &body).await?;
        Ok(response.session)
    }

    pub async fn messages(&self, id: &str) -> Result<Vec<Message>, String> {
        let response: MessagesResponse = self.get(&format!("/api/sessions/{id}/messages")).await?;
        Ok(response.messages)
    }

    pub async fn prompt(&self, id: &str, text: &str) -> Result<(), String> {
        let body = serde_json::json!({ "text": text });
        let _: serde_json::Value = self
            .post(&format!("/api/sessions/{id}/prompt"), &body)
            .await?;
        Ok(())
    }

    pub async fn abort(&self, id: &str) -> Result<bool, String> {
        let response: AbortResponse = self
            .post(&format!("/api/sessions/{id}/abort"), &serde_json::json!({}))
            .await?;
        Ok(response.aborted)
    }

    pub async fn fork(&self, id: &str, title: Option<String>) -> Result<SessionInfo, String> {
        let body = serde_json::json!({ "title": title });
        let response: SessionResponse = self
            .post(&format!("/api/sessions/{id}/fork"), &body)
            .await?;
        Ok(response.session)
    }

    /// Remove messages after `message`, optionally restoring the worktree.
    pub async fn revert(
        &self,
        id: &str,
        message: &str,
        files: bool,
    ) -> Result<RevertResponse, String> {
        let body = serde_json::json!({ "messageId": message, "files": files });
        self.post(&format!("/api/sessions/{id}/revert"), &body)
            .await
    }

    pub async fn compact(&self, id: &str) -> Result<CompactSummary, String> {
        self.post(
            &format!("/api/sessions/{id}/compact"),
            &serde_json::json!({}),
        )
        .await
    }

    /// Restore the server's worktree to the snapshot attached to `message`
    /// (or the last user message when omitted).
    pub async fn restore(&self, id: &str, message: Option<&str>) -> Result<String, String> {
        let body = serde_json::json!({ "messageId": message });
        let response: RestoreResponse = self
            .post(&format!("/api/sessions/{id}/restore"), &body)
            .await?;
        Ok(response.snapshot)
    }

    pub async fn set_model(&self, id: &str, model: &str) -> Result<(), String> {
        let body = serde_json::json!({ "model": model });
        let _: serde_json::Value = self
            .post(&format!("/api/sessions/{id}/model"), &body)
            .await?;
        Ok(())
    }

    pub async fn set_agent(&self, id: &str, agent: &str) -> Result<(), String> {
        let body = serde_json::json!({ "agent": agent });
        let _: serde_json::Value = self
            .post(&format!("/api/sessions/{id}/agent"), &body)
            .await?;
        Ok(())
    }

    pub async fn pending_permissions(
        &self,
        session: &str,
    ) -> Result<Vec<PermissionRequest>, String> {
        let response = self
            .http
            .get(format!("{}/api/permissions", self.base))
            .query(&[("session", session)])
            .headers(self.headers()?)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let response: PermissionsResponse = self.decode(response).await?;
        Ok(response.permissions)
    }

    pub async fn reply_permission(&self, id: &str, reply: PermissionReply) -> Result<(), String> {
        let body = serde_json::json!({ "reply": reply });
        let _: serde_json::Value = self.post(&format!("/api/permissions/{id}"), &body).await?;
        Ok(())
    }

    pub async fn pending_questions(&self, session: &str) -> Result<Vec<QuestionRequest>, String> {
        let response = self
            .http
            .get(format!("{}/api/questions", self.base))
            .query(&[("session", session)])
            .headers(self.headers()?)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let response: QuestionsResponse = self.decode(response).await?;
        Ok(response.questions)
    }

    pub async fn reply_question(&self, id: &str, reply: QuestionReply) -> Result<(), String> {
        let body = serde_json::json!({ "reply": reply });
        let _: serde_json::Value = self.post(&format!("/api/questions/{id}"), &body).await?;
        Ok(())
    }

    fn headers(&self) -> Result<reqwest::header::HeaderMap, String> {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = &self.token {
            let value = format!("Bearer {token}")
                .parse()
                .map_err(|error| format!("invalid token: {error}"))?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        Ok(headers)
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let response = self
            .http
            .get(format!("{}{path}", self.base))
            .headers(self.headers()?)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        self.decode(response).await
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, String> {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .headers(self.headers()?)
            .json(body)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        self.decode(response).await
    }

    async fn decode<T: DeserializeOwned>(&self, response: reqwest::Response) -> Result<T, String> {
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| format!("cannot read response: {error}"))?;
        if !status.is_success() {
            let message = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(|error| error.as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| text.trim().chars().take(300).collect());
            return Err(if message.is_empty() {
                format!("server returned HTTP {}", status.as_u16())
            } else {
                message
            });
        }
        serde_json::from_str(&text).map_err(|error| format!("invalid server response: {error}"))
    }
}

/// Percent-encode one path segment so nested command names (`review/security`)
/// survive `matchit` routing as a single `{name}` parameter.
fn encode_segment(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_nested_command_names() {
        assert_eq!(encode_segment("review"), "review");
        assert_eq!(encode_segment("review/security"), "review%2Fsecurity");
        assert_eq!(encode_segment("a b"), "a%20b");
    }
}
