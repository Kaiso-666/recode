//! Optional post-edit formatters.
//!
//! Every built-in formatter is disabled by default; enable one by name in
//! configuration. A formatter is a command that receives the edited file path
//! as its final argument. Missing binaries are remembered so a project using
//! one unavailable formatter does not pay a failed spawn on every edit.

use crate::config::ConfigManager;
use recode_schema::config::FormatterConfig;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

const FORMAT_TIMEOUT: Duration = Duration::from_secs(15);

struct FormatterSpec {
    key: String,
    config: FormatterConfig,
}

pub struct FormatterService {
    root: PathBuf,
    formatters: Vec<FormatterSpec>,
    unavailable: Mutex<HashSet<String>>,
}

impl FormatterService {
    pub fn new(config: &ConfigManager, root: PathBuf) -> Self {
        let mut formatters: Vec<FormatterSpec> = builtin_formatters();
        for (key, user) in &config.merged.formatter {
            match formatters.iter_mut().find(|spec| &spec.key == key) {
                Some(existing) => {
                    if !user.command.is_empty() {
                        existing.config.command = user.command.clone();
                    }
                    if !user.extensions.is_empty() {
                        existing.config.extensions = user.extensions.clone();
                    }
                    existing.config.enabled = user.enabled;
                }
                None => {
                    if user.command.is_empty() {
                        continue;
                    }
                    formatters.push(FormatterSpec {
                        key: key.clone(),
                        config: user.clone(),
                    });
                }
            }
        }
        formatters.retain(|spec| spec.config.enabled && !spec.config.command.is_empty());
        Self {
            root,
            formatters,
            unavailable: Mutex::new(HashSet::new()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.formatters.is_empty()
    }

    /// Format every matching path. Returns how many files were formatted.
    pub async fn format(&self, paths: &[PathBuf]) -> usize {
        let mut formatted = 0;
        for path in paths {
            let Some(spec) = self.spec_for(path) else {
                continue;
            };
            if !path.is_file() {
                continue;
            }
            if self.unavailable.lock().await.contains(&spec.key) {
                continue;
            }
            let mut command = Command::new(&spec.config.command[0]);
            command
                .args(&spec.config.command[1..])
                .arg(path)
                .current_dir(&self.root)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(windows)]
            {
                command.creation_flags(0x0800_0000);
            }
            match tokio::time::timeout(FORMAT_TIMEOUT, command.output()).await {
                Ok(Ok(output)) if output.status.success() => {
                    formatted += 1;
                    tracing::debug!(formatter = %spec.key, file = %path.display(), "formatted");
                }
                Ok(Ok(output)) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    tracing::warn!(
                        formatter = %spec.key,
                        file = %path.display(),
                        "formatter failed: {}",
                        stderr.trim()
                    );
                }
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.unavailable.lock().await.insert(spec.key.clone());
                    tracing::debug!(formatter = %spec.key, "formatter is not installed");
                }
                Ok(Err(error)) => {
                    tracing::warn!(formatter = %spec.key, %error, "cannot run formatter");
                }
                Err(_) => {
                    tracing::warn!(formatter = %spec.key, "formatter timed out");
                }
            }
        }
        formatted
    }

    fn spec_for(&self, path: &Path) -> Option<&FormatterSpec> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        let extension = format!(".{extension}");
        self.formatters.iter().find(|spec| {
            spec.config
                .extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&extension))
        })
    }
}

fn builtin_formatters() -> Vec<FormatterSpec> {
    fn spec(key: &str, command: &[&str], extensions: &[&str]) -> FormatterSpec {
        FormatterSpec {
            key: key.to_owned(),
            config: FormatterConfig {
                command: command.iter().map(|part| (*part).to_owned()).collect(),
                extensions: extensions.iter().map(|part| (*part).to_owned()).collect(),
                enabled: false,
            },
        }
    }
    vec![
        spec("rustfmt", &["rustfmt", "--edition", "2021"], &[".rs"]),
        spec("gofmt", &["gofmt", "-w"], &[".go"]),
        spec("ruff", &["ruff", "format"], &[".py", ".pyi"]),
        spec(
            "prettier",
            &["prettier", "--write"],
            &[
                ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".json", ".css", ".md",
            ],
        ),
        spec(
            "biome",
            &["biome", "format", "--write"],
            &[".ts", ".tsx", ".js", ".jsx", ".json"],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn python_available() -> bool {
        StdCommand::new("python")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn runs_an_enabled_formatter_for_matching_files() {
        if !python_available() {
            eprintln!("python is not installed; skipping formatter test");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let config_dir = root.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = serde_json::json!({
            "formatter": {
                "append": {
                    "command": ["python", "-c", "import sys; open(sys.argv[1], 'ab').write(b'formatted\\n')"],
                    "extensions": [".txt"],
                    "enabled": true
                }
            }
        });
        std::fs::write(
            config_dir.join("recode.json"),
            serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        let paths = crate::RecodePaths {
            config_dir: config_dir.clone(),
            data_dir: root.path().join("data"),
            db_path: root.path().join("data/recode.db"),
            auth_path: config_dir.join("auth.json"),
            log_dir: root.path().join("data/log"),
            global_config: config_dir.join("recode.json"),
        };
        let config = ConfigManager::load(paths, &project);
        let service = FormatterService::new(&config, project.clone());
        let file = project.join("a.txt");
        std::fs::write(&file, "one\n").unwrap();
        let other = project.join("a.rs");
        std::fs::write(&other, "fn main() {}\n").unwrap();
        assert_eq!(service.format(&[file.clone(), other.clone()]).await, 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\nformatted\n");
        // Built-in rustfmt is disabled until requested.
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "fn main() {}\n");
    }
}
