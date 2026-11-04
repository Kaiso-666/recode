use crate::cli::{ConfigArgs, ConfigCommand};
use recode_core::{ConfigManager, Project};

pub async fn run(args: ConfigArgs) -> anyhow::Result<()> {
    let paths = crate::cmd::paths();
    let directory = crate::cmd::directory(None)?;
    let project = Project::discover(&directory).map_err(anyhow::Error::msg)?;
    match args.command {
        ConfigCommand::Path => {
            println!("config\t{}", paths.global_config.display());
            println!("data\t{}", paths.data_dir.display());
            println!("db\t{}", paths.db_path.display());
            println!("auth\t{}", paths.auth_path.display());
            println!("project\t{}", project.root.display());
        }
        ConfigCommand::Show => {
            let config = ConfigManager::load(paths, &project.root);
            let mut value = serde_json::to_value(&config.merged)?;
            redact(&mut value);
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        ConfigCommand::Rebuild => {
            let store = recode_storage::Store::open(&paths.db_path)?;
            store.rebuild()?;
            println!("Rebuilt projections from {} events.", store.event_count()?);
        }
    }
    Ok(())
}

fn redact(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if matches!(key.as_str(), "apiKey" | "key" | "token" | "password") {
                    *value = serde_json::Value::String("***".into());
                } else {
                    redact(value);
                }
            }
        }
        serde_json::Value::Array(array) => {
            for value in array {
                redact(value);
            }
        }
        _ => {}
    }
}
