use crate::cli::ModelsArgs;
use recode_core::{ConfigManager, Project};

pub async fn run(args: ModelsArgs) -> anyhow::Result<()> {
    let paths = crate::cmd::paths();
    paths.ensure()?;
    let directory = crate::cmd::directory(None)?;
    let project = Project::discover(&directory).map_err(anyhow::Error::msg)?;
    let mut config = ConfigManager::load(paths.clone(), &project.root);
    if args.refresh {
        let catalog = recode_provider::catalog::fetch()
            .await
            .map_err(anyhow::Error::msg)?;
        let providers = catalog.providers.len();
        let models: usize = catalog
            .providers
            .values()
            .map(|provider| provider.models.len())
            .sum();
        let path = paths.data_dir.join("models.json");
        catalog.save(&path).map_err(anyhow::Error::msg)?;
        config.reload_catalog();
        eprintln!(
            "Refreshed models.dev catalog: {providers} providers, {models} models ({})",
            path.display()
        );
    }
    let current = config.merged.model.clone();
    let mut any = false;
    for provider in config.providers() {
        if args
            .provider
            .as_deref()
            .is_some_and(|filter| filter != provider.id)
        {
            continue;
        }
        if !args.all && args.provider.is_none() && !provider.configured {
            continue;
        }
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
                if selected { "● current" } else { "" }
            );
        }
    }
    if !any {
        if args.all {
            println!("No models available. Try `recode models --refresh`.");
        } else {
            println!(
                "No configured models. Run `recode auth login <provider>`, \
                 `recode models --all`, or `recode models --refresh`."
            );
        }
    }
    Ok(())
}
