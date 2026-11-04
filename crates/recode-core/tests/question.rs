use async_trait::async_trait;
use recode_core::engine::Engine;
use recode_core::permission::AutoApprove;
use recode_core::question::QuestionResponder;
use recode_core::EngineOptions;
use recode_schema::part::Part;
use recode_schema::question::{QuestionReply, QuestionRequest};
use std::sync::atomic::{AtomicUsize, Ordering};
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

struct FixedResponder {
    requests: AtomicUsize,
}

#[async_trait]
impl QuestionResponder for FixedResponder {
    async fn ask(&self, request: QuestionRequest) -> QuestionReply {
        self.requests.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.questions.len(), 1);
        assert_eq!(request.questions[0].header, "Database");
        QuestionReply::Answer {
            answers: vec![vec!["Postgres".to_owned()]],
        }
    }
}

#[tokio::test]
async fn question_tool_returns_the_selected_answer() {
    let mock = mock_server(vec![
        tool_response(
            "question",
            r#"{"questions":[{"question":"Which database?","header":"Database","options":[{"label":"SQLite"},{"label":"Postgres"}]}]}"#,
        ),
        text_response("Using Postgres."),
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
        "permission": {"read": "allow", "edit": "allow", "bash": "allow", "question": "allow"}
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
    let responder = Arc::new(FixedResponder {
        requests: AtomicUsize::new(0),
    });
    let engine = Engine::new(EngineOptions {
        paths,
        directory: project,
        responder: Some(Arc::new(AutoApprove)),
        question_responder: Some(responder.clone()),
    })
    .unwrap();
    let session = engine.create_session(None, None, None).unwrap();
    engine
        .prompt(&session.id, "pick a database".into(), Vec::new())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.wait_idle(&session.id),
    )
    .await
    .expect("run timed out");

    assert_eq!(responder.requests.load(Ordering::SeqCst), 1);
    let messages = engine.messages(&session.id).unwrap();
    let tool = messages
        .iter()
        .flat_map(|message| &message.parts)
        .find_map(|part| match part {
            Part::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("question tool part");
    assert_eq!(tool.tool, "question");
    assert_eq!(tool.state.status, recode_schema::ToolStatus::Completed);
    let output = tool.state.output.as_deref().unwrap_or_default();
    assert!(output.contains("Q: Which database?"));
    assert!(output.contains("A: Postgres"));
    assert!(messages
        .iter()
        .any(|message| message.text().contains("Using Postgres.")));
}
