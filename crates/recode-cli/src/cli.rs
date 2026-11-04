use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "recode",
    version,
    about = "Re:Code — a Rust-native AI coding agent",
    long_about = "Re:Code is a provider-neutral terminal coding agent. Run `recode` for the TUI, \
                  `recode run \"prompt\"` for headless use, or `recode serve` for the HTTP API."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run one prompt headlessly and print the result.
    Run(RunArgs),
    /// Open the interactive terminal UI (default).
    Tui(TuiArgs),
    /// Serve the local HTTP + SSE API.
    Serve(ServeArgs),
    /// Manage provider credentials.
    Auth(AuthArgs),
    /// List providers and models.
    Models,
    /// Inspect and manage sessions.
    Sessions(SessionsArgs),
    /// Inspect configuration.
    Config(ConfigArgs),
}

#[derive(Args, Default)]
pub struct TuiArgs {
    /// Resume a session by id.
    #[arg(long)]
    pub session: Option<String>,
    /// Directory to open (defaults to the current directory).
    #[arg(long)]
    pub directory: Option<PathBuf>,
    /// Model to select (provider/model).
    #[arg(long)]
    pub model: Option<String>,
    /// Agent to select.
    #[arg(long)]
    pub agent: Option<String>,
    /// Approve all permission requests without prompting.
    #[arg(long)]
    pub auto: bool,
    /// Attach to a running `recode serve` instead of opening the local engine.
    #[arg(long)]
    pub server: Option<String>,
    /// Bearer token for --server (or RECODE_SERVER_TOKEN).
    #[arg(long, env = "RECODE_SERVER_TOKEN")]
    pub token: Option<String>,
}

#[derive(Args, Default)]
pub struct RunArgs {
    /// Prompt text. Reads stdin when omitted and stdin is not a TTY.
    #[arg(
        value_name = "MESSAGE",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub message: Vec<String>,
    /// Model to use (provider/model).
    #[arg(long)]
    pub model: Option<String>,
    /// Agent to use (build, plan, general, explore, or a configured agent).
    #[arg(long)]
    pub agent: Option<String>,
    /// Resume a session by id.
    #[arg(long)]
    pub session: Option<String>,
    /// Continue the most recent session.
    #[arg(long = "continue")]
    pub continue_session: bool,
    /// Approve all permission requests without prompting.
    #[arg(long)]
    pub auto: bool,
    /// Send the prompt to a running `recode serve` instead of a local engine.
    #[arg(long)]
    pub server: Option<String>,
    /// Bearer token for --server (or RECODE_SERVER_TOKEN).
    #[arg(long, env = "RECODE_SERVER_TOKEN")]
    pub token: Option<String>,
    /// Attach a file (repeatable). Images are sent as attachments; text files are inlined.
    #[arg(long, value_name = "FILE")]
    pub file: Vec<PathBuf>,
    /// Output format: text (human) or json (one JSON object per line, then a result).
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
    /// Title for a newly created session.
    #[arg(long)]
    pub title: Option<String>,
    /// Directory to open (defaults to the current directory).
    #[arg(long)]
    pub directory: Option<PathBuf>,
    /// Abort the run after this many seconds.
    #[arg(long, default_value = "0")]
    pub timeout: u64,
}

#[derive(Args)]
pub struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1")]
    pub hostname: String,
    #[arg(long, default_value = "4319")]
    pub port: u16,
    #[arg(long)]
    pub directory: Option<PathBuf>,
    /// Bearer token required by API clients. Generated when omitted.
    #[arg(long)]
    pub token: Option<String>,
    /// Approve all permission requests.
    #[arg(long)]
    pub auto: bool,
}

#[derive(Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommand,
}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Store an API key for a provider.
    Login {
        provider: String,
        #[arg(long)]
        key: Option<String>,
    },
    /// Remove a stored credential.
    Logout { provider: String },
    /// List stored credentials.
    List,
}

#[derive(Args)]
pub struct SessionsArgs {
    #[command(subcommand)]
    pub command: SessionsCommand,
}

#[derive(Subcommand)]
pub enum SessionsCommand {
    /// List sessions for this project.
    List {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        archived: bool,
    },
    /// Print a session transcript.
    Show { id: String },
    /// Delete a session and its events.
    Delete { id: String },
    /// Export a session as JSON or Markdown.
    Export {
        id: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, value_parser = ["json", "md", "markdown"], default_value = "json")]
        format: String,
    },
    /// Copy a session's transcript into a new session.
    Fork {
        id: String,
        #[arg(long)]
        title: Option<String>,
    },
    /// Remove every message after the given message.
    Revert { id: String, message: String },
    /// Restore the worktree to the snapshot taken when a message was sent
    /// (defaults to the last user message).
    Restore {
        id: String,
        #[arg(long)]
        message: Option<String>,
    },
    /// Summarize older history so the session fits the model context.
    Compact { id: String },
}

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Print the merged configuration with secrets redacted.
    Show,
    /// Print the configuration and database paths.
    Path,
    /// Rebuild projection tables from the event log.
    Rebuild,
}
