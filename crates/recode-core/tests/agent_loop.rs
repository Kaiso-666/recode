use recode_core::engine::Engine;
use recode_core::permission::AutoApprove;
use recode_core::EngineOptions;
use recode_schema::part::{Part, ToolStatus};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A tiny mock OpenAI-compatible server. Each accepted connection gets the
/// next canned SSE response.
async fn mock_server(responses: Vec<String>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for body in responses {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
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

fn sse(choices: &str) -> String {
    format!("data: {choices}\n\ndata: [DONE]\n\n")
}

#[tokio::test]
async fn agent_runs_a_tool_loop_and_persists_the_transcript() {
    let first = sse(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"file_path\":\"a.txt\"}"}}]}}]}"#,
    );
    let second =
        sse(r#"{"choices":[{"delta":{"content":"The file says hello."},"finish_reason":"stop"}]}"#);
    let base_url = mock_server(vec![first, second]).await;

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("a.txt"), "hello from disk").unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "model": "mock/test-model",
        "provider": {
            "mock": {
                "name": "Mock",
                "api": "openai",
                "options": {"baseUrl": base_url, "apiKey": "test"},
                "models": {"test-model": {"name": "Test Model"}}
            }
        },
        "permission": {"read": "allow", "edit": "allow", "bash": "allow"}
    });
    std::fs::write(
        config_dir.join("recode.json"),
        serde_json::to_string_pretty(&config).unwrap(),
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
        question_responder: None,
    })
    .unwrap();
    let session = engine.create_session(None, None, None).unwrap();
    engine
        .prompt(&session.id, "read a.txt".into(), Vec::new())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.wait_idle(&session.id),
    )
    .await
    .expect("run timed out");

    let messages = engine.messages(&session.id).unwrap();
    assert_eq!(messages.len(), 3, "user + two assistant steps");
    let tool_part = messages[1]
        .parts
        .iter()
        .find_map(|part| match part {
            Part::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("tool part");
    assert_eq!(tool_part.state.status, ToolStatus::Completed);
    assert!(tool_part
        .state
        .output
        .as_deref()
        .unwrap_or("")
        .contains("hello from disk"));
    let final_text = messages[2].text();
    assert!(final_text.contains("The file says hello."));
}

#[tokio::test]
async fn permission_denial_is_returned_to_the_model() {
    let first = sse(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"bash","arguments":"{\"command\":\"echo nope\"}"}}]}}]}"#,
    );
    let second =
        sse(r#"{"choices":[{"delta":{"content":"I could not run it."},"finish_reason":"stop"}]}"#);
    let base_url = mock_server(vec![first, second]).await;

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "model": "mock/test-model",
        "provider": {"mock": {"api": "openai", "options": {"baseUrl": base_url, "apiKey": "test"}, "models": {"test-model": {}}}},
        "permission": {"bash": "deny", "read": "allow"}
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
        question_responder: None,
    })
    .unwrap();
    let session = engine.create_session(None, None, None).unwrap();
    engine
        .prompt(&session.id, "run echo".into(), Vec::new())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.wait_idle(&session.id),
    )
    .await
    .expect("run timed out");
    let messages = engine.messages(&session.id).unwrap();
    let tool_part = messages[1]
        .parts
        .iter()
        .find_map(|part| match part {
            Part::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("tool part");
    assert_eq!(tool_part.state.status, ToolStatus::Error);
    assert!(tool_part
        .state
        .error
        .as_deref()
        .unwrap_or("")
        .contains("permission denied"));
}
