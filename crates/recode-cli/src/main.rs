mod cli;
mod cmd;

use clap::Parser;
use cli::{Cli, Command};

fn main() -> anyhow::Result<()> {
    cmd::upgrade::cleanup_previous();
    // `recode "fix the bug"` is shorthand for `recode run "fix the bug"`.
    let mut args: Vec<String> = std::env::args().collect();
    let known = [
        "run",
        "tui",
        "serve",
        "auth",
        "models",
        "sessions",
        "config",
        "completions",
        "upgrade",
        "help",
    ];
    if args.len() > 1 {
        let first = args[1].as_str();
        if !first.starts_with('-') && !known.contains(&first) {
            args.insert(1, "run".to_owned());
        }
    }
    let cli = Cli::parse_from(args);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        match cli.command.unwrap_or(Command::Tui(cli::TuiArgs::default())) {
            Command::Run(args) => cmd::run::run(args).await,
            Command::Tui(args) => cmd::tui::run(args).await,
            Command::Serve(args) => cmd::serve::run(args).await,
            Command::Auth(args) => cmd::auth::run(args).await,
            Command::Models => cmd::models::run().await,
            Command::Sessions(args) => cmd::sessions::run(args).await,
            Command::Config(args) => cmd::config::run(args).await,
            Command::Completions { shell } => {
                cmd::completions::run(shell);
                Ok(())
            }
            Command::Upgrade(args) => cmd::upgrade::run(args).await,
        }
    })
}
