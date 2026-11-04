use recode_core::engine::Engine;
use recode_core::permission::AutoApprove;
use recode_core::EngineOptions;
use recode_schema::part::Part;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn mock_server(responses: Vec<String>) -> String {
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
                if let Some(position) = find_headers_end(&buffer) {
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

fn find_headers_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn sse(payload: serde_json::Value) -> String {
    format!("data: {payload}\n\ndata: [DONE]\n\n")
}

fn text_response(text: &str) -> String {
    sse(serde_json::json!({
        "choices": [{"delta": {"content": text}, "finish_reason": "stop"}]
    }))
}

fn tool_response(name: &str, arguments: &str) -> String {
    sse(serde_json::json!({
        "choices": [{"delta": {"tool_calls": [{
            "index": 0,
            "id": "call_1",
            "function": {"name": name, "arguments": arguments}
        }]}}]
    }))
}

#[tokio::test]
async fn task_tool_runs_a_child_session() {
    let mock = mock_server(vec![
        tool_response(
            "task",
            r#"{"description":"explore the project","prompt":"report the file list","subagent_type":"general"}"#,
        ),
        text_response("child answer"),
        text_response("parent done"),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "model": "mock/test-model",
        "provider": {"mock": {"api": "openai", "options": {"baseUrl": mock, "apiKey": "test"}, "models": {"test-model": {}}}},
        "permission": {"read": "allow", "edit": "allow", "bash": "allow", "task": "allow"}
    });
    std::fs::write(
        config_dir.join("recode.json"),
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    let paths = recode_core::RecodePaths {
        config_dir: config_dir.clone(),
        data_dir: dir.path().join("data"),
        db_path: dir.path().join("data/recode.db"),
        auth_path: config_dir.join("auth.json"),
        log_dir: dir.path().join("data/log"),
        global_config: config_dir.join("recode.json"),
    };
    let engine = Engine::new(EngineOptions {
        paths,
        directory: project,
        responder: Some(Arc::new(AutoApprove)),
    })
    .unwrap();
    let session = engine.create_session(None, None, None).unwrap();
    engine
        .prompt(&session.id, "delegate this".into(), Vec::new())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        engine.wait_idle(&session.id),
    )
    .await
    .expect("run timed out");

    let messages = engine.messages(&session.id).unwrap();
    let tool = messages
        .iter()
        .flat_map(|message| &message.parts)
        .find_map(|part| match part {
            Part::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("task tool part");
    assert_eq!(tool.tool, "task");
    assert_eq!(
        tool.state.output.as_deref().map(str::trim),
        Some("child answer")
    );

    let sessions = engine.sessions(false).unwrap();
    let child = sessions
        .iter()
        .find(|candidate| candidate.parent_id.as_ref() == Some(&session.id))
        .expect("child session");
    assert_eq!(child.agent.as_deref(), Some("general"));
    assert!(child
        .metadata
        .get("subagentDepth")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|depth| depth == 1));
}
