use crate::cli::{AuthArgs, AuthCommand};
use recode_core::auth::AuthStore;

pub async fn run(args: AuthArgs) -> anyhow::Result<()> {
    let paths = crate::cmd::paths();
    paths.ensure()?;
    let mut store = AuthStore::load(&paths.auth_path);
    match args.command {
        AuthCommand::Login { provider, key } => {
            let key = match key {
                Some(key) => key,
                None => {
                    use std::io::Write;
                    eprint!("API key for {provider}: ");
                    std::io::stderr().flush()?;
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line.trim().to_owned()
                }
            };
            if key.is_empty() {
                anyhow::bail!("no key provided");
            }
            store.set(&provider, &key).map_err(anyhow::Error::msg)?;
            println!(
                "Stored credential for {provider} in {}",
                store.path().display()
            );
        }
        AuthCommand::Logout { provider } => {
            if store.remove(&provider).map_err(anyhow::Error::msg)? {
                println!("Removed credential for {provider}");
            } else {
                println!("No stored credential for {provider}");
            }
        }
        AuthCommand::List => {
            let entries: Vec<_> = store.list().collect();
            if entries.is_empty() {
                println!("No stored credentials. Run `recode auth login <provider>`.");
                return Ok(());
            }
            for (provider, present) in entries {
                println!(
                    "{provider}\t{}",
                    if present { "configured" } else { "empty" }
                );
            }
        }
    }
    Ok(())
}
