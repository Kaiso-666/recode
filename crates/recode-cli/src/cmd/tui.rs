use crate::cli::TuiArgs;
use anyhow::Context;
use recode_core::engine::Engine;
use recode_core::EngineOptions;
use recode_schema::SessionId;
use recode_tui::Backend;
use recode_tui::Theme;
use std::sync::Arc;

pub async fn run(args: TuiArgs) -> anyhow::Result<()> {
    crate::cmd::init_logging(true);
    if let Some(server) = &args.server {
        return attach(server.clone(), args).await;
    }
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
    let theme = resolve_theme(args.theme.as_deref(), engine.config.theme_name());
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
    let session_id = session.id.clone();
    recode_tui::run(Backend::local(engine), Some(responder), session_id, theme).await?;
    Ok(())
}

/// Attach to an already-running server. The session defaults to a new one on
/// that server; `--session` opens an existing transcript.
async fn attach(server: String, args: TuiArgs) -> anyhow::Result<()> {
    let token = args.token.clone();
    let client = recode_client::Client::new(server, token);
    let session_id = match &args.session {
        Some(id) => SessionId::from_existing(id.clone()),
        None => {
            let session = client
                .create_session(args.agent.clone(), args.model.clone(), None)
                .await
                .map_err(anyhow::Error::msg)?;
            session.id
        }
    };
    // The theme is a client-side setting, so it comes from the local config.
    let directory = crate::cmd::directory(args.directory.clone())?;
    let configured = recode_core::config::ConfigManager::load(crate::cmd::paths(), &directory)
        .theme_name()
        .map(str::to_owned);
    let theme = resolve_theme(args.theme.as_deref().or(configured.as_deref()), None);
    let backend = Backend::remote(client, &session_id.0)
        .await
        .map_err(anyhow::Error::msg)?;
    recode_tui::run(backend, None, session_id, theme).await?;
    Ok(())
}

/// The theme to draw with. The command line wins over the configuration; an
/// unknown name falls back to the default and says so.
fn resolve_theme(override_name: Option<&str>, configured: Option<&str>) -> Theme {
    let name = override_name.or(configured);
    let Some(name) = name else {
        return Theme::default();
    };
    match Theme::by_name(name) {
        Some(theme) => theme,
        None => {
            eprintln!(
                "unknown theme {name:?}; using the default. Known themes: {}",
                Theme::NAMES.join(", ")
            );
            Theme::default()
        }
    }
}
