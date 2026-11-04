//! Shell completion generation.

use clap::CommandFactory;
use clap_complete::Shell;
use std::io::Write;

pub fn run(shell: Shell) {
    let mut command = crate::cli::Cli::command();
    let mut stdout = std::io::stdout();
    clap_complete::generate(shell, &mut command, "recode", &mut stdout);
    let _ = stdout.flush();
}
