use crate::cli::TuiArgs;
use anyhow::Context;
use recode_core::engine::Engine;
use recode_core::EngineOptions;
use recode_schema::SessionId;
use std::sync::Arc;

pub async fn run(args: TuiArgs) -> anyhow::Result<()> {
    crate::cmd::init_logging(true);
    let paths = crate::cmd::paths();
    let directory = crate::cmd::directory(args.directory.clone())?;
    let responder = Arc::new(recode_tui::TuiResponder::new());
    let engine = Engine::new(EngineOptions {
        paths,
        directory,
        responder: Some(responder.clone()),
        question_responder: None,
    })
    .map_err(anyhow::Error::msg)?;
    let session = match &args.session {
        Some(id) => engine
            .session(&SessionId::from_existing(id.clone()))
            .map_err(anyhow::Error::msg)?
            .context("session not found")?,
        None => engine
            .create_session(args.agent.clone(), None, None)
            .map_err(anyhow::Error::msg)?,
    };
    if let Some(model) = &args.model {
        let model = engine
            .config
            .resolve_model(Some(model))
            .map_err(anyhow::Error::msg)?;
        engine
            .set_model(&session.id, model)
            .map_err(anyhow::Error::msg)?;
    }
    recode_tui::run(engine, responder, session.id).await?;
    Ok(())
}
