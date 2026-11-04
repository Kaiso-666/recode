//! Minimal Language Server Protocol client.
//!
//! Servers are started on demand for the languages listed in configuration
//! (with built-in defaults for Rust, TypeScript, Python, Go and C/C++). The
//! client speaks JSON-RPC over stdio, opens/changes documents and waits for
//! `textDocument/publishDiagnostics` so edits can carry compiler feedback.
//!
//! The manager is deliberately small: it does not implement every LSP
//! capability, only initialize, document sync and diagnostics.

use crate::config::ConfigManager;
use recode_schema::config::LspServerConfig;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, oneshot, Mutex};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// 1-based line for display.
    pub line: u32,
    /// 1-based column for display.
    pub character: u32,
    /// LSP severity: 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u8,
    pub message: String,
    pub source: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FileDiagnostics {
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone)]
struct ServerSpec {
    key: String,
    config: LspServerConfig,
}

struct ClientSlot {
    client: Option<Arc<LspClient>>,
    failed: bool,
}

pub struct LspManager {
    root: PathBuf,
    servers: Vec<ServerSpec>,
    state: Mutex<HashMap<String, ClientSlot>>,
}

impl LspManager {
    pub fn new(config: &ConfigManager, root: PathBuf) -> Self {
        let mut servers: HashMap<String, ServerSpec> = builtin_servers()
            .into_iter()
            .map(|spec| (spec.key.clone(), spec))
            .collect();
        for (key, user) in &config.merged.lsp {
            match servers.get_mut(key) {
                Some(existing) => {
                    if !user.command.is_empty() {
                        existing.config.command = user.command.clone();
                    }
                    if !user.extensions.is_empty() {
                        existing.config.extensions = user.extensions.clone();
                    }
                    if user.language_id.is_some() {
                        existing.config.language_id = user.language_id.clone();
                    }
                    if !user.root_markers.is_empty() {
                        existing.config.root_markers = user.root_markers.clone();
                    }
                    existing.config.enabled = user.enabled;
                    if !user.initialization.is_null() {
                        existing.config.initialization = user.initialization.clone();
                    }
                }
                None => {
                    if user.command.is_empty() {
                        continue;
                    }
                    servers.insert(
                        key.clone(),
                        ServerSpec {
                            key: key.clone(),
                            config: user.clone(),
                        },
                    );
                }
            }
        }
        let mut servers = servers.into_values().collect::<Vec<_>>();
        servers.sort_by(|left, right| left.key.cmp(&right.key));
        Self {
            root,
            servers,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Diagnostics for edited files. Files without a configured server, or
    /// whose server cannot start, are skipped silently.
    pub async fn diagnostics(&self, paths: &[PathBuf]) -> Vec<FileDiagnostics> {
        let mut waits = Vec::new();
        for path in paths {
            let Some(spec) = self.spec_for(path) else {
                continue;
            };
            let Some(client) = self.ensure_client(&spec).await else {
                continue;
            };
            let language_id = language_id(&spec, path);
            if let Some((uri, receiver)) = client.touch(path, &language_id).await {
                waits.push((path.clone(), client, uri, receiver));
            }
        }
        if waits.is_empty() {
            return Vec::new();
        }
        let deadline = tokio::time::Instant::now() + DIAGNOSTICS_TIMEOUT;
        let mut results = Vec::new();
        for (path, client, uri, receiver) in waits {
            let diagnostics = client.wait(&uri, receiver, deadline).await;
            if !diagnostics.is_empty() {
                results.push(FileDiagnostics { path, diagnostics });
            }
        }
        results
    }

    fn spec_for(&self, path: &Path) -> Option<ServerSpec> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        let extension = format!(".{extension}");
        self.servers
            .iter()
            .find(|spec| {
                spec.config.enabled
                    && spec
                        .config
                        .extensions
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(&extension))
            })
            .cloned()
    }

    async fn ensure_client(&self, spec: &ServerSpec) -> Option<Arc<LspClient>> {
        let mut state = self.state.lock().await;
        if let Some(slot) = state.get(&spec.key) {
            if let Some(client) = &slot.client {
                return Some(client.clone());
            }
            if slot.failed {
                return None;
            }
        }
        if spec.config.command.is_empty() {
            state.insert(
                spec.key.clone(),
                ClientSlot {
                    client: None,
                    failed: true,
                },
            );
            return None;
        }
        match LspClient::start(spec, &self.root).await {
            Ok(client) => {
                state.insert(
                    spec.key.clone(),
                    ClientSlot {
                        client: Some(client.clone()),
                        failed: false,
                    },
                );
                Some(client)
            }
            Err(error) => {
                tracing::debug!(server = %spec.key, %error, "language server unavailable");
                state.insert(
                    spec.key.clone(),
                    ClientSlot {
                        client: None,
                        failed: true,
                    },
                );
                None
            }
        }
    }
}

fn language_id(spec: &ServerSpec, path: &Path) -> String {
    if let Some(language) = &spec.config.language_id {
        return language.clone();
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
    {
        "tsx" => "typescriptreact".to_owned(),
        "jsx" => "javascriptreact".to_owned(),
        other => other.to_owned(),
    }
}

fn builtin_servers() -> Vec<ServerSpec> {
    fn spec(key: &str, command: &[&str], extensions: &[&str], markers: &[&str]) -> ServerSpec {
        ServerSpec {
            key: key.to_owned(),
            config: LspServerConfig {
                command: command.iter().map(|part| (*part).to_owned()).collect(),
                extensions: extensions.iter().map(|part| (*part).to_owned()).collect(),
                language_id: None,
                root_markers: markers.iter().map(|part| (*part).to_owned()).collect(),
                enabled: true,
                initialization: Value::Null,
            },
        }
    }
    vec![
        spec("rust", &["rust-analyzer"], &[".rs"], &["Cargo.toml"]),
        spec(
            "typescript",
            &["typescript-language-server", "--stdio"],
            &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"],
            &["package.json", "tsconfig.json"],
        ),
        spec(
            "python",
            &["pyright-langserver", "--stdio"],
            &[".py", ".pyi"],
            &["pyproject.toml", "setup.py"],
        ),
        spec("go", &["gopls"], &[".go"], &["go.mod"]),
        spec(
            "clangd",
            &["clangd"],
            &[".c", ".h", ".cc", ".cpp", ".cxx", ".hpp"],
            &["compile_commands.json", "CMakeLists.txt"],
        ),
    ]
}

/// Render diagnostics as a compact block appended to a tool result.
pub fn format_diagnostics(results: &[FileDiagnostics]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let mut output = String::from("--- diagnostics ---\n");
    let mut shown = 0;
    'files: for file in results {
        for diagnostic in &file.diagnostics {
            if shown >= 20 {
                output.push_str("… more diagnostics omitted\n");
                break 'files;
            }
            output.push_str(severity_label(diagnostic.severity));
            output.push(' ');
            output.push_str(&file.path.display().to_string());
            output.push(':');
            output.push_str(&diagnostic.line.to_string());
            output.push(':');
            output.push_str(&diagnostic.character.to_string());
            output.push_str(": ");
            output.push_str(diagnostic.message.trim());
            if let Some(source) = &diagnostic.source {
                if !source.is_empty() {
                    output.push_str(&format!(" [{source}]"));
                }
            }
            output.push('\n');
            shown += 1;
        }
    }
    if shown == 0 {
        return String::new();
    }
    truncate(&output, 4000)
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(limit).collect();
    truncated.push_str("… truncated");
    truncated
}

fn severity_label(severity: u8) -> &'static str {
    match severity {
        1 => "error",
        2 => "warning",
        3 => "info",
        _ => "hint",
    }
}

struct LspClient {
    name: String,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingMap,
    next_id: AtomicI64,
    diagnostics: broadcast::Sender<(String, Vec<Diagnostic>)>,
    opened: Mutex<HashMap<String, i64>>,
    _child: Mutex<Child>,
}

impl LspClient {
    async fn start(spec: &ServerSpec, root: &Path) -> Result<Arc<Self>, String> {
        let executable = spec
            .config
            .command
            .first()
            .ok_or_else(|| "language server command is empty".to_owned())?;
        let mut command = Command::new(executable);
        command
            .args(&spec.config.command[1..])
            .current_dir(root)
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
            .ok_or_else(|| "language server has no stdin".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "language server has no stdout".to_owned())?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "recode::lsp", "{line}");
                }
            });
        }
        let (diagnostics, _) = broadcast::channel(64);
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let client = Arc::new(Self {
            name: spec.key.clone(),
            stdin: stdin.clone(),
            pending: pending.clone(),
            next_id: AtomicI64::new(1),
            diagnostics: diagnostics.clone(),
            opened: Mutex::new(HashMap::new()),
            _child: Mutex::new(child),
        });
        tokio::spawn(read_loop(stdout, stdin, pending, diagnostics));
        client.initialize(root, spec).await?;
        Ok(client)
    }

    async fn initialize(&self, root: &Path, spec: &ServerSpec) -> Result<(), String> {
        let root_uri = path_to_uri(root);
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_owned());
        self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": {"name": "recode", "version": env!("CARGO_PKG_VERSION")},
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": name}],
                "capabilities": {
                    "workspace": {"workspaceFolders": true, "configuration": true},
                    "textDocument": {
                        "synchronization": {"didSave": true},
                        "publishDiagnostics": {"relatedInformation": false}
                    },
                    "window": {"workDoneProgress": false}
                },
                "initializationOptions": spec.config.initialization
            }),
        )
        .await?;
        self.notify("initialized", json!({})).await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        if let Err(error) = write_message(&self.stdin, &message).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!("{} exited during {method}", self.name)),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!("{method} timed out after {REQUEST_TIMEOUT:?}"))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        write_message(
            &self.stdin,
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
        .await
    }

    async fn touch(
        &self,
        path: &Path,
        language_id: &str,
    ) -> Option<(String, broadcast::Receiver<(String, Vec<Diagnostic>)>)> {
        let text = tokio::fs::read_to_string(path).await.ok()?;
        if text.len() > 2_000_000 {
            return None;
        }
        let uri = path_to_uri(path);
        let receiver = self.diagnostics.subscribe();
        let version = {
            let mut opened = self.opened.lock().await;
            let version = opened.get(&uri).copied().unwrap_or(0) + 1;
            opened.insert(uri.clone(), version);
            version
        };
        let sent = if version == 1 {
            self.notify(
                "textDocument/didOpen",
                json!({"textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text
                }}),
            )
            .await
        } else {
            self.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": {"uri": uri, "version": version},
                    "contentChanges": [{"text": text}]
                }),
            )
            .await
        };
        if sent.is_err() {
            return None;
        }
        let _ = self
            .notify(
                "textDocument/didSave",
                json!({"textDocument": {"uri": uri}}),
            )
            .await;
        Some((uri, receiver))
    }

    async fn wait(
        &self,
        uri: &str,
        mut receiver: broadcast::Receiver<(String, Vec<Diagnostic>)>,
        deadline: tokio::time::Instant,
    ) -> Vec<Diagnostic> {
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Vec::new();
            }
            match tokio::time::timeout(remaining, receiver.recv()).await {
                Ok(Ok((event_uri, diagnostics))) if event_uri == uri => return diagnostics,
                Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return Vec::new(),
            }
        }
    }
}

async fn read_loop(
    stdout: tokio::process::ChildStdout,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingMap,
    diagnostics: broadcast::Sender<(String, Vec<Diagnostic>)>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => {
                    fail_pending(&pending).await;
                    return;
                }
                Ok(_) => {}
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line
                .split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim())
            {
                length = value.parse::<usize>().ok();
            }
        }
        let Some(length) = length.filter(|length| *length <= MAX_MESSAGE_BYTES) else {
            continue;
        };
        let mut body = vec![0u8; length];
        if reader.read_exact(&mut body).await.is_err() {
            fail_pending(&pending).await;
            return;
        }
        let Ok(message) = serde_json::from_slice::<Value>(&body) else {
            continue;
        };
        let Some(id) = message.get("id").and_then(Value::as_i64) else {
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
            {
                if let Some(params) = message.get("params") {
                    if let Some(uri) = params.get("uri").and_then(Value::as_str) {
                        let list = parse_diagnostics(params.get("diagnostics"));
                        let _ = diagnostics.send((uri.to_owned(), list));
                    }
                }
            }
            continue;
        };
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            // Server-to-client request. Answer with the closest harmless value.
            let result = if method == "workspace/configuration" {
                let count = message
                    .get("params")
                    .and_then(|params| params.get("items"))
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                Value::Array(vec![Value::Null; count])
            } else {
                Value::Null
            };
            let _ = write_message(
                &stdin,
                &json!({"jsonrpc": "2.0", "id": id, "result": result}),
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
}

async fn fail_pending(pending: &PendingMap) {
    for (_, sender) in pending.lock().await.drain() {
        let _ = sender.send(Err("language server exited".to_owned()));
    }
}

async fn write_message(stdin: &Mutex<ChildStdin>, message: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    let mut stdin = stdin.lock().await;
    stdin
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .map_err(|error| format!("cannot write to language server: {error}"))?;
    stdin
        .write_all(&body)
        .await
        .map_err(|error| format!("cannot write to language server: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("cannot flush language server: {error}"))
}

fn parse_diagnostics(value: Option<&Value>) -> Vec<Diagnostic> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let start = item.get("range")?.get("start")?;
            let line = start.get("line").and_then(Value::as_u64).unwrap_or(0) + 1;
            let character = start.get("character").and_then(Value::as_u64).unwrap_or(0) + 1;
            let message = item
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if message.is_empty() {
                return None;
            }
            Some(Diagnostic {
                line: line.min(u32::MAX as u64) as u32,
                character: character.min(u32::MAX as u64) as u32,
                severity: item
                    .get("severity")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 4) as u8,
                message: message.chars().take(500).collect(),
                source: item
                    .get("source")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// `file:///C:/path` on Windows and `file:///path` elsewhere.
pub fn path_to_uri(path: &Path) -> String {
    let path = recode_tool::canonicalish(path);
    let mut text = path.to_string_lossy().replace('\\', "/");
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    let mut uri = String::from("file://");
    for character in text.chars() {
        match character {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' | ':' => {
                uri.push(character)
            }
            ' ' => uri.push_str("%20"),
            '#' => uri.push_str("%23"),
            '?' => uri.push_str("%3F"),
            '%' => uri.push_str("%25"),
            other => {
                let mut buffer = [0u8; 4];
                for byte in other.encode_utf8(&mut buffer).bytes() {
                    uri.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diagnostics_from_lsp() {
        let value = json!([{
            "range": {"start": {"line": 4, "character": 2}, "end": {"line": 4, "character": 8}},
            "severity": 2,
            "source": "rust-analyzer",
            "message": "unused variable"
        }, {
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
            "message": "  "
        }]);
        let diagnostics = parse_diagnostics(Some(&value));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 5);
        assert_eq!(diagnostics[0].character, 3);
        assert_eq!(diagnostics[0].severity, 2);
        assert_eq!(diagnostics[0].source.as_deref(), Some("rust-analyzer"));
    }

    #[test]
    fn formats_diagnostics_with_a_cap() {
        let results = vec![FileDiagnostics {
            path: PathBuf::from("src/main.rs"),
            diagnostics: vec![Diagnostic {
                line: 3,
                character: 1,
                severity: 1,
                message: "mismatched types".into(),
                source: Some("rustc".into()),
            }],
        }];
        let text = format_diagnostics(&results);
        assert!(text.starts_with("--- diagnostics ---"));
        assert!(text.contains("error src/main.rs:3:1: mismatched types [rustc]"));
    }

    #[test]
    fn windows_paths_become_file_uris() {
        let uri = path_to_uri(Path::new(r"C:\Users\meow\my project\a.rs"));
        assert!(uri.starts_with("file:///"));
        assert!(uri.ends_with("/a.rs"));
        assert!(uri.contains("my%20project"));
    }
}
