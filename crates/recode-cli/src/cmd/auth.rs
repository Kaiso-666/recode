use crate::cli::{AuthArgs, AuthCommand};
use recode_core::auth::AuthStore;
use recode_core::oauth;

pub async fn run(args: AuthArgs) -> anyhow::Result<()> {
    let paths = crate::cmd::paths();
    paths.ensure()?;
    let mut store = AuthStore::load(&paths.auth_path);
    match args.command {
        AuthCommand::Login {
            provider,
            key,
            server,
        } => {
            if provider == "opencode" && key.is_none() {
                let server = server
                    .or_else(|| std::env::var("OPENCODE_CONSOLE_URL").ok())
                    .unwrap_or_else(|| oauth::DEFAULT_SERVER.to_owned());
                let device = oauth::device_code(&server)
                    .await
                    .map_err(anyhow::Error::msg)?;
                eprintln!(
                    "Open {} and enter code {}",
                    device.verification_uri_complete, device.user_code
                );
                eprintln!("Waiting for approval…");
                let credential = oauth::poll_token(&server, &device)
                    .await
                    .map_err(anyhow::Error::msg)?;
                store
                    .set_oauth(&provider, credential)
                    .map_err(anyhow::Error::msg)?;
                println!(
                    "Signed in to OpenCode Console for {provider} ({})",
                    store.path().display()
                );
                return Ok(());
            }
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
