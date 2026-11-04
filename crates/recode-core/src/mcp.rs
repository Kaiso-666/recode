//! Model Context Protocol client.
//!
//! Supports the two transports the protocol defines for tools: line-delimited
//! JSON-RPC over a child process (`local`) and streamable HTTP (`remote`).
//! Servers are started lazily before the first prompt; every exposed tool is
//! registered into the shared [`recode_tool::ToolRegistry`] under
//! `mcp__<server>__<tool>`.

use crate::config::ConfigManager;
use async_trait::async_trait;
use recode_schema::config::{McpLocalConfig, McpRemoteConfig, McpServerConfig};
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use recode_tool::{Tool, ToolContext, ToolRegistry};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{oneshot, Mutex};

const PROTOCOL_VERSION: &str = "2024-11-05";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct McpManager {
    root: PathBuf,
    servers: Vec<(String, McpServerConfig)>,
    started: Mutex<bool>,
}

impl McpManager {
    pub fn new(config: &ConfigManager, root: PathBuf) -> Self {
        Self {
            root,
            servers: config
                .merged
                .mcp
                .iter()
                .map(|(name, config)| (name.clone(), config.clone()))
                .collect(),
            started: Mutex::new(false),
        }
    }

    pub fn server_names(&self) -> Vec<&str> {
        self.servers.iter().map(|(name, _)| name.as_str()).collect()
    }

    /// Connect enabled servers once per process and register their tools.
    /// Failures are logged and do not abort the prompt.
    pub async fn ensure_started(&self, tools: &ToolRegistry) {
        let mut started = self.started.lock().await;
        if *started {
            return;
        }
        *started = true;
        for (server_name, config) in &self.servers {
            if !config.enabled() {
                continue;
            }
            let client = match McpClient::connect(server_name, config, &self.root).await {
                Ok(client) => client,
                Err(error) => {
                    tracing::warn!(server = %server_name, %error, "cannot connect MCP server");
                    continue;
                }
            };
            let listed = match client.list_tools().await {
                Ok(tools) => tools,
                Err(error) => {
                    tracing::warn!(server = %server_name, %error, "cannot list MCP tools");
                    continue;
                }
            };
            let mut count = 0;
            for definition in listed {
                let Some(remote) = definition.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let registration = tool_name(server_name, remote);
                let description = definition
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("MCP tool")
                    .to_owned();
                let schema = definition
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"}));
                tools.register(Arc::new(McpTool {
                    name: registration,
                    server: server_name.clone(),
                    remote: remote.to_owned(),
                    description,
                    schema,
                    client: client.clone(),
                }));
                count += 1;
            }
            tracing::debug!(server = %server_name, tools = count, "MCP server ready");
        }
    }
}

fn tool_name(server: &str, tool: &str) -> String {
    let raw = format!("mcp__{server}__{tool}");
    let mut name = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
            name.push(character);
        } else {
            name.push('_');
        }
        if name.len() >= 64 {
            break;
        }
    }
    name
}

enum McpClient {
    Stdio(Box<StdioClient>),
    Http(HttpClient),
}

impl McpClient {
    async fn connect(
        name: &str,
        config: &McpServerConfig,
        root: &Path,
    ) -> Result<Arc<Self>, String> {
        let client = match config {
            McpServerConfig::Local(local) => Arc::new(Self::Stdio(Box::new(
                StdioClient::start(name, local, root).await?,
            ))),
            McpServerConfig::Remote(remote) => Arc::new(Self::Http(HttpClient::new(name, remote)?)),
        };
        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<(), String> {
        self.request("initialize", initialize_params()).await?;
        self.notify("notifications/initialized", json!({})).await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        match self {
            Self::Stdio(client) => client.request(method, params).await,
            Self::Http(client) => client.request(method, params).await,
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        match self {
            Self::Stdio(client) => client.notify(method, params).await,
            Self::Http(client) => client.notify(method, params).await,
        }
    }

    async fn list_tools(&self) -> Result<Vec<Value>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let result = self.request("tools/list", params).await?;
            if let Some(page) = result.get("tools").and_then(Value::as_array) {
                tools.extend(page.iter().cloned());
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }

    async fn call_tool(&self, tool: &str, arguments: Value) -> Result<ToolOutcome, ToolFailure> {
        let result = self
            .request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await
            .map_err(|error| ToolFailure::new(format!("MCP call failed: {error}")))?;
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let message = render_content(result.get("content"));
            return Err(ToolFailure::new(if message.is_empty() {
                "MCP tool returned an error".to_owned()
            } else {
                message
            }));
        }
        let mut output = render_content(result.get("content"));
        if output.is_empty() {
            if let Some(structured) = result.get("structuredContent") {
                output = serde_json::to_string_pretty(structured).unwrap_or_default();
            }
        }
        Ok(ToolOutcome {
            title: Some(format!("MCP {tool}")),
            output,
            metadata: Some(json!({"mcp": tool})),
            attachments: Vec::new(),
        })
    }
}

fn render_content(content: Option<&Value>) -> String {
    let Some(items) = content.and_then(Value::as_array) else {
        return String::new();
    };
    let mut output = String::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(item.get("text").and_then(Value::as_str).unwrap_or_default());
            }
            Some("resource") => {
                let text = item
                    .get("resource")
                    .and_then(|resource| resource.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !text.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(text);
                }
            }
            Some("image") => {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str("[image content]");
            }
            _ => {
                if let Ok(value) = serde_json::to_string(item) {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&value);
                }
            }
        }
    }
    output
}

struct StdioClient {
    name: String,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingMap,
    next_id: AtomicI64,
    _child: Mutex<Child>,
}

impl StdioClient {
    async fn start(name: &str, config: &McpLocalConfig, root: &Path) -> Result<Self, String> {
        let executable = config
            .command
            .first()
            .ok_or_else(|| "MCP command is empty".to_owned())?;
        let mut command = tokio::process::Command::new(executable);
        command
            .args(&config.command[1..])
            .current_dir(root)
            .envs(&config.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start {executable}: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "MCP server has no stdin".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "MCP server has no stdout".to_owned())?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "recode::mcp", "{line}");
                }
            });
        }
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let client = Self {
            name: name.to_owned(),
            stdin: stdin.clone(),
            pending: pending.clone(),
            next_id: AtomicI64::new(1),
            _child: Mutex::new(child),
        };
        tokio::spawn(read_stdio(stdout, stdin, pending));
        Ok(client)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(error) = write_line(&self.stdin, &message).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!("{} exited during {method}", self.name)),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        write_line(
            &self.stdin,
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
        .await
    }
}

async fn read_stdio(
    stdout: tokio::process::ChildStdout,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingMap,
) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = message.get("id").and_then(Value::as_i64) else {
            continue;
        };
        if message.get("method").is_some() {
            // Server-initiated request: answer null so the server is not stuck.
            let _ = write_line(
                &stdin,
                &json!({"jsonrpc": "2.0", "id": id, "result": Value::Null}),
            )
            .await;
            continue;
        }
        if let Some(sender) = pending.lock().await.remove(&id) {
            let outcome = match message.get("error") {
                Some(error) => Err(error.to_string()),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = sender.send(outcome);
        }
    }
    for (_, sender) in pending.lock().await.drain() {
        let _ = sender.send(Err("MCP server exited".to_owned()));
    }
}

struct HttpClient {
    name: String,
    url: String,
    headers: Vec<(String, String)>,
    session: Mutex<Option<String>>,
    client: reqwest::Client,
    next_id: AtomicI64,
}

impl HttpClient {
    fn new(name: &str, config: &McpRemoteConfig) -> Result<Self, String> {
        let mut client = reqwest::Client::builder();
        if let Some(timeout) = config.timeout_ms {
            client = client.timeout(Duration::from_millis(timeout.max(1_000)));
        } else {
            client = client.timeout(REQUEST_TIMEOUT);
        }
        let client = client
            .build()
            .map_err(|error| format!("cannot build MCP client: {error}"))?;
        Ok(Self {
            name: name.to_owned(),
            url: config.url.clone(),
            headers: config
                .headers
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            session: Mutex::new(None),
            client,
            next_id: AtomicI64::new(1),
        })
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let messages = self.post(&body).await?;
        for message in messages {
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(error.to_string());
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        Err(format!("{} returned no response for {method}", self.name))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.post(&body).await.map(|_| ())
    }

    async fn post(&self, body: &Value) -> Result<Vec<Value>, String> {
        let mut request = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .body(serde_json::to_vec(body).map_err(|error| error.to_string())?);
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        if let Some(session) = self.session.lock().await.clone() {
            request = request.header("mcp-session-id", session);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("MCP HTTP request failed: {error}"))?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            *self.session.lock().await = Some(session.to_owned());
        }
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let text = response
            .text()
            .await
            .map_err(|error| format!("cannot read MCP response: {error}"))?;
        if !status.is_success() {
            return Err(format!(
                "MCP HTTP {}: {}",
                status.as_u16(),
                text.trim().chars().take(500).collect::<String>()
            ));
        }
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        if content_type.contains("text/event-stream") {
            let mut messages = Vec::new();
            for line in text.lines() {
                let Some(data) = line.trim().strip_prefix("data:") else {
                    continue;
                };
                if let Ok(message) = serde_json::from_str::<Value>(data.trim()) {
                    messages.push(message);
                }
            }
            return Ok(messages);
        }
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(messages)) => Ok(messages),
            Ok(message) => Ok(vec![message]),
            Err(error) => Err(format!("invalid MCP response: {error}")),
        }
    }
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "recode", "version": env!("CARGO_PKG_VERSION")}
    })
}

async fn write_line(stdin: &Mutex<ChildStdin>, message: &Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    line.push(b'\n');
    let mut stdin = stdin.lock().await;
    stdin
        .write_all(&line)
        .await
        .map_err(|error| format!("cannot write to MCP server: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("cannot flush MCP server: {error}"))
}

struct McpTool {
    name: String,
    server: String,
    remote: String,
    description: String,
    schema: Value,
    client: Arc<McpClient>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn permission(&self) -> &str {
        "mcp"
    }

    fn default_decision(&self) -> Decision {
        Decision::Ask
    }

    fn permission_patterns(&self, _input: &Value) -> Vec<String> {
        vec![format!("{}:{}", self.server, self.remote)]
    }

    fn title(&self, _input: &Value) -> String {
        format!("MCP {}: {}", self.server, self.remote)
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> ToolResult {
        self.client.call_tool(&self.remote, input).await
    }
}
