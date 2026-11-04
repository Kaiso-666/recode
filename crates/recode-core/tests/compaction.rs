use recode_core::engine::Engine;
use recode_core::permission::AutoApprove;
use recode_core::EngineOptions;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Mock {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
}

/// Serve a fixed sequence of streaming responses, recording each request body.
async fn mock_server(responses: Vec<String>) -> Mock {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
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
            if let Some(position) = find_headers_end(&buffer) {
                if let Ok(request) = serde_json::from_slice::<Value>(&buffer[position + 4..]) {
                    recorded.lock().unwrap().push(request);
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
    Mock {
        base_url: format!("http://{address}/v1"),
        requests,
    }
}

fn find_headers_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn sse(choices: &str) -> String {
    format!("data: {choices}\n\ndata: [DONE]\n\n")
}

fn text_response(text: &str) -> String {
    let payload = serde_json::json!({
        "choices": [{"delta": {"content": text}, "finish_reason": "stop"}]
    });
    sse(&payload.to_string())
}

fn tool_response(name: &str, arguments: &str) -> String {
    let payload = serde_json::json!({
        "choices": [{"delta": {"tool_calls": [{
            "index": 0,
            "id": "call_1",
            "function": {"name": name, "arguments": arguments}
        }]}}]
    });
    sse(&payload.to_string())
}

#[tokio::test]
async fn compaction_replaces_old_history_for_the_model() {
    let mock = mock_server(vec![
        tool_response("read", r#"{"file_path":"a.txt"}"#),
        text_response("The file says hello."),
        text_response("SUMMARY: the user asked to read a.txt; it contains hello."),
        text_response("Continuing from the summary."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("a.txt"), "hello").unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "model": "mock/test-model",
        "provider": {"mock": {"api": "openai", "options": {"baseUrl": mock.base_url, "apiKey": "test"}, "models": {"test-model": {}}}},
        "permission": {"read": "allow", "edit": "allow", "bash": "allow"}
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
        .prompt(&session.id, "read a.txt".into(), Vec::new())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.wait_idle(&session.id),
    )
    .await
    .expect("first run timed out");

    let record = engine
        .compact(&session.id)
        .await
        .unwrap()
        .expect("compaction should produce a summary");
    assert!(record.summary.contains("SUMMARY"));

    engine
        .prompt(&session.id, "continue".into(), Vec::new())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.wait_idle(&session.id),
    )
    .await
    .expect("second run timed out");

    let requests = mock.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    let summary_request = &requests[2];
    assert!(summary_request["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .contains("context compactor"));
    let continued = &requests[3];
    let messages = continued["messages"].as_array().unwrap();
    assert!(
        messages.len() <= 3,
        "compacted history should be short, got {messages:?}"
    );
    let summary = messages
        .iter()
        .find(|message| {
            message["content"]
                .as_str()
                .is_some_and(|text| text.contains("Summary of the conversation so far"))
        })
        .expect("summary message should be present");
    assert!(
        summary["content"]
            .as_str()
            .unwrap()
            .contains("SUMMARY: the user asked"),
        "summary text should be forwarded"
    );
    assert!(
        !messages.iter().any(|message| message["role"] == "tool"),
        "old tool output should no longer be sent"
    );
    assert!(messages.iter().any(|message| message["content"]
        .as_str()
        .is_some_and(|text| text.contains("continue"))));
}
