use recode_core::engine::Engine;
use recode_core::permission::AutoApprove;
use recode_core::EngineOptions;
use recode_schema::part::Part;
use std::process::Command;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn python_available() -> bool {
    Command::new("python")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

async fn mock_provider(responses: Vec<String>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for body in responses {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 8192];
            while let Ok(read) = socket.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&buffer[..position]).to_lowercase();
                    let length = header
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buffer.len() >= position + 4 + length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    format!("http://{address}/v1")
}

fn sse(payload: serde_json::Value) -> String {
    format!("data: {payload}\n\ndata: [DONE]\n\n")
}

async fn run_tool_call(mcp: serde_json::Value) -> (Arc<Engine>, tempfile::TempDir) {
    let provider = mock_provider(vec![
        sse(serde_json::json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call_1",
                "function": {
                    "name": "mcp__mock__echo",
                    "arguments": "{\"text\":\"hello\"}"
                }
            }]}}]
        })),
        sse(serde_json::json!({
            "choices": [{"delta": {"content": "Called echo."}, "finish_reason": "stop"}]
        })),
    ])
    .await;
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let config_dir = root.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "model": "mock/test-model",
        "provider": {
            "mock": {
                "api": "openai",
                "options": {"baseUrl": provider, "apiKey": "test"},
                "models": {"test-model": {}}
            }
        },
        "mcp": {"mock": mcp},
        "permission": {"read": "allow", "edit": "allow", "bash": "allow", "mcp": "allow"}
    });
    std::fs::write(
        config_dir.join("recode.json"),
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    let paths = recode_core::RecodePaths {
        config_dir: config_dir.clone(),
        data_dir: root.path().join("data"),
        db_path: root.path().join("data/recode.db"),
        auth_path: config_dir.join("auth.json"),
        log_dir: root.path().join("data/log"),
        global_config: config_dir.join("recode.json"),
    };
    let engine = Engine::new(EngineOptions {
        paths,
        directory: project,
        responder: Some(Arc::new(AutoApprove)),
        question_responder: Some(Arc::new(recode_core::question::AutoRejectQuestions)),
    })
    .unwrap();
    let session = engine.create_session(None, None, None).unwrap();
    engine
        .prompt(&session.id, "call echo".into(), Vec::new())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        engine.wait_idle(&session.id),
    )
    .await
    .expect("run timed out");
    let messages = engine.messages(&session.id).unwrap();
    let tool = messages
        .iter()
        .flat_map(|message| &message.parts)
        .find_map(|part| match part {
            Part::Tool(tool) if tool.tool == "mcp__mock__echo" => Some(tool),
            _ => None,
        })
        .expect("MCP tool part");
    assert_eq!(
        tool.state.status,
        recode_schema::ToolStatus::Completed,
        "tool failed: {:?}",
        tool.state.error
    );
    let output = tool.state.output.as_deref().unwrap_or_default();
    assert!(output.contains("echo: hello"), "output: {output}");
    assert!(engine.tools.get("mcp__mock__echo").is_some());
    (engine, root)
}

#[tokio::test]
async fn stdio_mcp_tool_is_callable() {
    if !python_available() {
        eprintln!("python is not installed; skipping MCP test");
        return;
    }
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../script/mock_mcp.py")
        .canonicalize()
        .unwrap();
    let (engine, root) = run_tool_call(serde_json::json!({
        "type": "local",
        "command": ["python", script.to_string_lossy()]
    }))
    .await;
    drop(engine);
    drop(root);
}

/// Minimal streamable-HTTP MCP server: JSON responses, a session header on
/// initialize, and a 401 when later requests omit it.
async fn mock_http_mcp() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let read = socket.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buffer[..position]).to_string();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if buffer.len() < position + 4 + length {
                        continue;
                    }
                    let body = &buffer[position + 4..position + 4 + length];
                    let message: serde_json::Value =
                        serde_json::from_slice(body).unwrap_or_default();
                    let method = message
                        .get("method")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    let has_session = head
                        .lines()
                        .any(|line| line.to_ascii_lowercase().starts_with("mcp-session-id:"));
                    let (status, session_header, result) = if method == "initialize" {
                        (
                            "200 OK",
                            "Mcp-Session-Id: mock-session\r\n",
                            serde_json::json!({
                                "protocolVersion": "2024-11-05",
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "mock-http", "version": "0"}
                            }),
                        )
                    } else if !has_session {
                        (
                            "401 Unauthorized",
                            "",
                            serde_json::json!({"error": "missing session"}),
                        )
                    } else if method == "tools/list" {
                        (
                            "200 OK",
                            "",
                            serde_json::json!({"tools": [{
                                "name": "echo",
                                "description": "Echo the provided text.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {"text": {"type": "string"}},
                                    "required": ["text"]
                                }
                            }]}),
                        )
                    } else if method == "tools/call" {
                        let text = message
                            .get("params")
                            .and_then(|params| params.get("arguments"))
                            .and_then(|arguments| arguments.get("text"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        (
                            "200 OK",
                            "",
                            serde_json::json!({"content": [{"type": "text", "text": format!("echo: {text}")}]}),
                        )
                    } else {
                        ("200 OK", "", serde_json::Value::Null)
                    };
                    let identifier = message.get("id").cloned().unwrap_or_default();
                    let payload = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": identifier,
                        "result": result
                    })
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{session_header}\r\n{payload}",
                        payload.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    return;
                }
            });
        }
    });
    format!("http://{address}/mcp")
}

#[tokio::test]
async fn remote_mcp_tool_is_callable() {
    let url = mock_http_mcp().await;
    let (engine, root) = run_tool_call(serde_json::json!({
        "type": "remote",
        "url": url
    }))
    .await;
    drop(engine);
    drop(root);
}
