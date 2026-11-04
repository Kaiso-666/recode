use recode_core::{ConfigManager, Project};

pub async fn run() -> anyhow::Result<()> {
    let paths = crate::cmd::paths();
    let directory = crate::cmd::directory(None)?;
    let project = Project::discover(&directory).map_err(anyhow::Error::msg)?;
    let config = ConfigManager::load(paths, &project.root);
    let current = config.merged.model.clone();
    let mut any = false;
    for provider in config.providers() {
        for model in &provider.models {
            any = true;
            let qualified = format!("{}/{}", provider.id, model);
            let selected = current.as_deref() == Some(qualified.as_str())
                || current.as_deref() == Some(model.as_str());
            println!(
                "{:<14} {:<42} {:<12} {}",
                provider.id,
                model,
                if provider.configured {
                    "configured"
                } else {
                    "no key"
                },
                if selected { "← current" } else { "" }
            );
        }
    }
    if !any {
        println!("No models configured. Add providers to recode.json or run `recode auth login <provider>`.");
    }
    Ok(())
}
