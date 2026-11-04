use recode_core::config::ConfigManager;
use recode_core::lsp::LspManager;
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

fn paths(root: &std::path::Path) -> recode_core::RecodePaths {
    let config_dir = root.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    recode_core::RecodePaths {
        config_dir: config_dir.clone(),
        data_dir: root.join("data"),
        db_path: root.join("data/recode.db"),
        auth_path: config_dir.join("auth.json"),
        log_dir: root.join("data/log"),
        global_config: config_dir.join("recode.json"),
    }
}

#[tokio::test]
async fn receives_diagnostics_from_an_lsp_server() {
    if !python_available() {
        eprintln!("python is not installed; skipping LSP test");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let source = project.join("a.rs");
    std::fs::write(&source, "fn main() { bad }\n").unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../script/mock_lsp.py")
        .canonicalize()
        .unwrap();
    let paths = paths(root.path());
    let config = serde_json::json!({
        "lsp": {
            "mock": {
                "command": ["python", script.to_string_lossy()],
                "extensions": [".rs"],
                "enabled": true
            },
            "rust": {"enabled": false}
        }
    });
    std::fs::write(
        &paths.global_config,
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    let config = ConfigManager::load(paths, &project);
    let manager = LspManager::new(&config, project.clone());
    let results = manager.diagnostics(std::slice::from_ref(&source)).await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].path, source);
    assert!(results[0].diagnostics[0].message.contains("bad token"));
    assert_eq!(results[0].diagnostics[0].severity, 1);

    std::fs::write(&source, "fn main() { good }\n").unwrap();
    let results = manager.diagnostics(&[source]).await;
    assert!(results.is_empty());
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

#[tokio::test]
async fn edited_files_get_diagnostics_in_the_tool_result() {
    if !python_available() {
        eprintln!("python is not installed; skipping LSP test");
        return;
    }
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = repo
        .join("../../script/mock_lsp.py")
        .canonicalize()
        .unwrap();
    let provider = mock_provider(vec![
        sse(serde_json::json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call_1",
                "function": {
                    "name": "write",
                    "arguments": "{\"file_path\":\"a.rs\",\"content\":\"fn main() { bad }\\n\"}"
                }
            }]}}]
        })),
        sse(serde_json::json!({
            "choices": [{"delta": {"content": "Wrote it."}, "finish_reason": "stop"}]
        })),
    ])
    .await;

    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("a.rs"), "fn main() { good }\n").unwrap();
    let paths = paths(root.path());
    let config = serde_json::json!({
        "model": "mock/test-model",
        "provider": {
            "mock": {
                "api": "openai",
                "options": {"baseUrl": provider, "apiKey": "test"},
                "models": {"test-model": {}}
            }
        },
        "permission": {"read": "allow", "edit": "allow", "bash": "allow"},
        "lsp": {
            "mock": {
                "command": ["python", script.to_string_lossy()],
                "extensions": [".rs"],
                "enabled": true
            },
            "rust": {"enabled": false}
        }
    });
    std::fs::write(
        &paths.global_config,
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    let engine = recode_core::Engine::new(recode_core::EngineOptions {
        paths,
        directory: project,
        responder: Some(Arc::new(recode_core::permission::AutoApprove)),
        question_responder: Some(Arc::new(recode_core::question::AutoRejectQuestions)),
    })
    .unwrap();
    let session = engine.create_session(None, None, None).unwrap();
    engine
        .prompt(&session.id, "make a.rs bad".into(), Vec::new())
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
            recode_schema::part::Part::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("write tool part");
    let output = tool.state.output.as_deref().unwrap_or_default();
    assert!(
        output.contains("--- diagnostics ---"),
        "missing diagnostics header in {output:?}"
    );
    assert!(output.contains("mock: found bad token"), "output: {output}");
}
