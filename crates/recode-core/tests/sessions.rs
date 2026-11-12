use recode_core::engine::Engine;
use recode_core::permission::AutoApprove;
use recode_core::EngineOptions;
use recode_schema::event::Event;
use recode_schema::id::{MessageId, PartId, SessionId};
use recode_schema::message::MessageInfo;
use recode_schema::part::{Part, TextPart};
use recode_schema::{Message, Role};
use std::sync::Arc;

fn engine(directory: &std::path::Path, config_dir: &std::path::Path) -> Arc<Engine> {
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
    .unwrap()
}

fn append_text(engine: &Engine, session_id: &SessionId, role: Role, text: &str) -> MessageId {
    let mut info = MessageInfo::user(session_id.clone());
    info.role = role;
    let message_id = info.id.clone();
    engine
        .store
        .append(Event::MessageAdded { message: info })
        .unwrap();
    engine
        .store
        .append(Event::PartAdded {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            part: Part::Text(TextPart {
                id: PartId::new(),
                text: text.to_owned(),
                synthetic: false,
                time: None,
            }),
        })
        .unwrap();
    message_id
}

#[test]
fn agent_markdown_model_wins_over_the_global_default() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(project.join(".recode/agent")).unwrap();
    std::fs::write(
        project.join(".recode/agent/reviewer.md"),
        "---\nmodel: acme/acme-chat\n---\nReview code.\n",
    )
    .unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("recode.json"),
        r#"{"model":"openai/gpt-5"}"#,
    )
    .unwrap();
    let engine = engine(dir.path(), &config_dir);

    let session = engine
        .create_session(Some("reviewer".into()), None, None)
        .unwrap();
    assert_eq!(
        session.model.as_ref().map(|model| model.qualified()),
        Some("acme/acme-chat".to_owned())
    );

    let explicit = recode_schema::model::ModelRef::new("anthropic", "claude-sonnet-4-5");
    let session = engine
        .create_session(Some("reviewer".into()), Some(explicit.clone()), None)
        .unwrap();
    assert_eq!(session.model, Some(explicit));

    // Switching to the agent later also adopts its model.
    let session = engine
        .create_session(Some("build".into()), None, None)
        .unwrap();
    assert_eq!(
        session.model.as_ref().map(|model| model.qualified()),
        Some("openai/gpt-5".to_owned())
    );
    engine.set_agent(&session.id, "reviewer").unwrap();
    let session = engine.session(&session.id).unwrap().unwrap();
    assert_eq!(
        session.model.as_ref().map(|model| model.qualified()),
        Some("acme/acme-chat".to_owned())
    );
}

#[tokio::test]
async fn a_child_session_keeps_its_parent_and_inherits_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(project.join(".recode/agent")).unwrap();
    std::fs::write(
        project.join(".recode/agent/reviewer.md"),
        "---\nmode: subagent\n---\nReview code.\n",
    )
    .unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("recode.json"),
        r#"{"model":"openai/gpt-5"}"#,
    )
    .unwrap();
    let engine = engine(dir.path(), &config_dir);

    let parent = engine
        .create_session(Some("build".into()), None, None)
        .unwrap();
    let child = engine
        .create_child_session(
            &parent.id,
            Some("reviewer".into()),
            None,
            Some("Review".into()),
        )
        .unwrap();
    assert_eq!(child.parent_id.as_ref(), Some(&parent.id));
    assert_eq!(child.agent.as_deref(), Some("reviewer"));
    assert_eq!(child.title, "Review");
    // The child is a subagent, so it stays out of the session list.
    let sessions = engine.sessions(false).unwrap();
    assert!(!sessions.iter().any(|session| session.id == child.id));
    let children = engine.child_sessions(&parent.id).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].id, child.id);
}

#[test]
fn fork_copies_the_transcript_and_revert_trims_it() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let engine = engine(dir.path(), &config_dir);
    let session = engine.create_session(None, None, None).unwrap();
    let first = append_text(&engine, &session.id, Role::User, "first question");
    append_text(&engine, &session.id, Role::Assistant, "first answer");

    let forked = engine
        .fork_session(&session.id, Some("fork".into()))
        .unwrap();
    assert_eq!(forked.title, "fork");
    assert_eq!(
        forked
            .metadata
            .get("forkedFrom")
            .and_then(|value| value.as_str()),
        Some(session.id.0.as_str())
    );
    let copied: Vec<Message> = engine.messages(&forked.id).unwrap();
    assert_eq!(copied.len(), 2);
    assert_eq!(copied[0].text(), "first question");
    assert_eq!(copied[1].text(), "first answer");
    // Fresh identities so the fork can evolve independently.
    assert_ne!(copied[0].info.id, first);
    assert!(copied
        .iter()
        .all(|message| message.info.session_id == forked.id));

    let removed = engine.revert(&forked.id, &copied[0].info.id).unwrap();
    assert_eq!(removed, 1);
    let left = engine.messages(&forked.id).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].text(), "first question");
    // The source session is untouched.
    assert_eq!(engine.messages(&session.id).unwrap().len(), 2);
}

#[test]
fn revert_rejects_an_unknown_message() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let engine = engine(dir.path(), &config_dir);
    let session = engine.create_session(None, None, None).unwrap();
    append_text(&engine, &session.id, Role::User, "hello");
    let error = engine.revert(&session.id, &MessageId::new()).unwrap_err();
    assert!(error.contains("not found"));
}
