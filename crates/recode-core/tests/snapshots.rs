use recode_core::snapshot::SnapshotService;
use std::process::Command;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn git_available() -> bool {
    Command::new("git")
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
async fn snapshot_restore_round_trip() {
    if !git_available() {
        eprintln!("git is not installed; skipping snapshot test");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("a.txt"), "one").unwrap();
    std::fs::write(project.join(".gitignore"), "ignored.txt\n").unwrap();
    let service = SnapshotService::new(&paths(root.path()), "prj_test", project.clone(), true);

    let first = service.snapshot("first").await.unwrap().unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "one"
    );

    std::fs::write(project.join("a.txt"), "two").unwrap();
    std::fs::write(project.join("b.txt"), "created later").unwrap();
    // Ignored files are not part of a snapshot, so a restore must not touch
    // them even when they appeared after the snapshot.
    std::fs::write(project.join("ignored.txt"), "keep me").unwrap();
    let second = service.snapshot("second").await.unwrap().unwrap();
    assert_ne!(first, second);
    assert!(service.changed_files(&second).await.unwrap().is_empty());

    service.restore(&first).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "one"
    );
    assert!(!project.join("b.txt").exists());
    assert!(project.join("ignored.txt").exists());
    assert!(service.changed_files(&first).await.unwrap().is_empty());

    let exclude = std::fs::read_to_string(service.git_dir().join("info/exclude")).unwrap();
    assert!(exclude.lines().any(|line| line == ".git"));
}

#[tokio::test]
async fn a_disabled_service_does_nothing() {
    let root = tempfile::tempdir().unwrap();
    let service = SnapshotService::new(
        &paths(root.path()),
        "prj_test",
        root.path().to_path_buf(),
        false,
    );
    assert!(service.snapshot("first").await.unwrap().is_none());
    assert!(service.restore("deadbeef").await.is_err());
    assert!(service.changed_files("deadbeef").await.unwrap().is_empty());
}

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
async fn a_prompt_snapshots_and_restore_undoes_a_tool_edit() {
    if !git_available() {
        eprintln!("git is not installed; skipping snapshot test");
        return;
    }
    let mock = mock_server(vec![
        sse(serde_json::json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call_1",
                "function": {
                    "name": "write",
                    "arguments": "{\"file_path\":\"a.txt\",\"content\":\"changed\"}"
                }
            }]}}]
        })),
        sse(serde_json::json!({
            "choices": [{"delta": {"content": "Wrote the file."}, "finish_reason": "stop"}]
        })),
    ])
    .await;

    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("a.txt"), "one").unwrap();
    let config_dir = root.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "model": "mock/test-model",
        "provider": {
            "mock": {
                "api": "openai",
                "options": {"baseUrl": mock, "apiKey": "test"},
                "models": {"test-model": {}}
            }
        },
        "permission": {"read": "allow", "edit": "allow", "bash": "allow"}
    });
    std::fs::write(
        config_dir.join("recode.json"),
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    let engine = recode_core::Engine::new(recode_core::EngineOptions {
        paths: paths(root.path()),
        directory: project.clone(),
        responder: Some(Arc::new(recode_core::permission::AutoApprove)),
        question_responder: Some(Arc::new(recode_core::question::AutoRejectQuestions)),
    })
    .unwrap();
    let session = engine.create_session(None, None, None).unwrap();
    engine
        .prompt(&session.id, "change a.txt".into(), Vec::new())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        engine.wait_idle(&session.id),
    )
    .await
    .expect("run timed out");
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "changed"
    );

    let messages = engine.messages(&session.id).unwrap();
    let user = messages
        .iter()
        .find(|message| message.info.role == recode_schema::Role::User)
        .unwrap();
    assert!(user
        .parts
        .iter()
        .any(|part| matches!(part, recode_schema::part::Part::Snapshot(_))));
    let assistant = messages
        .iter()
        .rev()
        .find(|message| message.info.role == recode_schema::Role::Assistant)
        .unwrap();
    assert!(assistant.parts.iter().any(|part| matches!(
        part,
        recode_schema::part::Part::Patch(patch) if patch.files.iter().any(|file| file == "a.txt")
    )));

    engine
        .restore_files(&session.id, Some(&user.info.id))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "one"
    );
}
