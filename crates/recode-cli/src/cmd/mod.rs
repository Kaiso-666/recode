pub mod auth;
pub mod config;
pub mod models;
pub mod run;
pub mod serve;
pub mod sessions;
pub mod tui;

use crate::cli::RunArgs;
use recode_core::paths::RecodePaths;
use std::path::PathBuf;

pub fn paths() -> RecodePaths {
    RecodePaths::discover()
}

pub fn directory(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let directory = explicit
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| anyhow::anyhow!("cannot determine the current directory"))?;
    Ok(directory)
}

pub fn init_logging(quiet: bool) {
    use tracing_subscriber::EnvFilter;
    let filter = std::env::var("RECODE_LOG").unwrap_or_else(|_| "warn".to_owned());
    let builder = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_target(false)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()));
    if quiet {
        builder.with_writer(std::io::sink).init();
    } else {
        builder.with_writer(std::io::stderr).init();
    }
}

/// Read a prompt from stdin when it is piped.
pub fn read_stdin_if_piped() -> Option<String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return None;
    }
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).ok()?;
    if input.trim().is_empty() {
        None
    } else {
        Some(input)
    }
}

pub fn validate_run(args: &RunArgs) -> anyhow::Result<()> {
    if !matches!(args.format.as_str(), "text" | "json") {
        anyhow::bail!("invalid --format {:?}: expected text or json", args.format);
    }
    if args.session.is_some() && args.continue_session {
        anyhow::bail!("--session and --continue cannot be combined");
    }
    Ok(())
}
