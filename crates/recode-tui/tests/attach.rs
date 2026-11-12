//! Attach mode: the backend folds the server's live events into its copy of
//! the transcript.

use recode_client::Client;
use recode_core::engine::Engine;
use recode_core::permission::AutoApprove;
use recode_core::EngineOptions;
use recode_server::Server;
use recode_tui::Backend;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn engine(directory: &Path, config_dir: &Path) -> Arc<Engine> {
    std::fs::create_dir_all(directory.join("project")).unwrap();
    Engine::new(EngineOptions {
        paths: recode_core::RecodePaths {
            config_dir: config_dir.to_path_buf(),
            data_dir: directory.join("data"),
            db_path: directory.join("data/recode.db"),
            auth_path: config_dir.join("auth.json"),
            log_dir: directory.join("data/log"),
            global_config: config_dir.join("recode.json"),
        },
        directory: directory.join("project"),
        responder: Some(Arc::new(AutoApprove)),
        question_responder: Some(Arc::new(recode_core::question::AutoRejectQuestions)),
    })
    .expect("engine")
}

async fn start(engine: Arc<Engine>) -> Client {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr");
    let server = Server::new(engine, "test-token".into());
    tokio::spawn(async move {
        let _ = recode_server::serve(server, listener).await;
    });
    Client::new(format!("http://{address}"), Some("test-token".into()))
}

#[tokio::test]
async fn live_events_reach_the_attached_backend() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("recode.json"),
        r#"{"model":"acme/acme-chat"}"#,
    )
    .unwrap();
    let engine = engine(dir.path(), &config_dir);
    let client = start(engine.clone()).await;

    let session = client
        .create_session(None, None, None)
        .await
        .expect("create");
    let backend = Backend::remote(client, &session.id.0)
        .await
        .expect("attach");
    assert!(backend.is_remote());

    engine
        .prompt(&session.id, "hello".into(), Vec::new())
        .await
        .expect("prompt");

    // The event stream folds the message into the backend's transcript.
    let mut messages = Vec::new();
    for _ in 0..50 {
        messages = backend.messages(&session.id).unwrap_or_default();
        if !messages.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(messages.len(), 1, "transcript: {messages:?}");
    assert_eq!(messages[0].info.role, recode_schema::Role::User);
    assert!(backend.transcript_seq() >= 1);
}
