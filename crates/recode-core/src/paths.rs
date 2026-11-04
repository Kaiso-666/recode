use std::path::PathBuf;

/// Filesystem locations used by Re:Code. Overridable for tests and portable
/// installs via `RECODE_CONFIG` and `RECODE_DATA`.
#[derive(Clone, Debug)]
pub struct RecodePaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub auth_path: PathBuf,
    pub log_dir: PathBuf,
    pub global_config: PathBuf,
}

impl RecodePaths {
    pub fn discover() -> Self {
        let config_dir = std::env::var_os("RECODE_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(default_config_dir);
        let data_dir = std::env::var_os("RECODE_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);
        let db_path = std::env::var_os("RECODE_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("recode.db"));
        let auth_path = config_dir.join("auth.json");
        let log_dir = data_dir.join("log");
        let global_config = config_dir.join("recode.json");
        Self {
            config_dir,
            data_dir,
            db_path,
            auth_path,
            log_dir,
            global_config,
        }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.log_dir)?;
        Ok(())
    }
}

fn default_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("recode")
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("recode")
}
