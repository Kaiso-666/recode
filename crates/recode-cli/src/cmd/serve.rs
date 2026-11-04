use crate::cli::ServeArgs;

pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    let directory = crate::cmd::directory(args.directory.clone())?;
    let responder: Option<std::sync::Arc<dyn recode_core::PermissionResponder>> = if args.auto {
        Some(std::sync::Arc::new(recode_core::permission::AutoApprove))
    } else {
        // Server clients approve through POST /api/permissions/{id}.
        None
    };
    let engine = recode_core::Engine::new(recode_core::EngineOptions {
        paths: crate::cmd::paths(),
        directory,
        responder,
    })
    .map_err(anyhow::Error::msg)?;
    let token = args
        .token
        .unwrap_or_else(|| format!("rct_{}", uuid_simple()));
    let app = recode_server::Server::new(engine, token.clone());
    let listener = tokio::net::TcpListener::bind((args.hostname.as_str(), args.port)).await?;
    let address = listener.local_addr()?;
    eprintln!("Re:Code server listening on http://{address}");
    eprintln!("Token: {token}");
    recode_server::serve(app, listener).await?;
    Ok(())
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{now:x}")
}
