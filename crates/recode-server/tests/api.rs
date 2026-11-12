//! End-to-end checks for the HTTP API through the typed client.

use recode_client::Client;
use recode_core::engine::Engine;
use recode_core::permission::AutoApprove;
use recode_core::EngineOptions;
use recode_server::Server;
use std::path::Path;
use std::sync::Arc;

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
async fn sessions_can_be_created_listed_and_deleted_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let client = start(engine(dir.path(), &config_dir)).await;
    client.health().await.expect("health");

    let session = client
        .create_session(Some("build".into()), None, Some("remote".into()))
        .await
        .expect("create session");
    assert_eq!(session.title, "remote");

    let sessions = client.sessions(false, false).await.expect("list");
    assert!(sessions.iter().any(|item| item.id == session.id));

    client.delete_session(&session.id.0).await.expect("delete");
    let sessions = client.sessions(false, false).await.expect("list");
    assert!(!sessions.iter().any(|item| item.id == session.id));
}

#[tokio::test]
async fn restoring_without_a_snapshot_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let client = start(engine(dir.path(), &config_dir)).await;

    let session = client
        .create_session(None, None, None)
        .await
        .expect("create session");
    let error = client
        .restore(&session.id.0, None)
        .await
        .expect_err("restore must fail without a snapshot");
    assert!(!error.is_empty());
}

#[tokio::test]
async fn commands_and_agents_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(config_dir.join("commands")).unwrap();
    std::fs::write(
        config_dir.join("commands/review.md"),
        "---\ndescription: Review a change\n---\nReview $1.\n",
    )
    .unwrap();
    let client = start(engine(dir.path(), &config_dir)).await;

    let commands = client.commands().await.expect("commands");
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "review");
    let agents = client.agents().await.expect("agents");
    assert!(agents.iter().any(|agent| agent.id == "build"));
}
