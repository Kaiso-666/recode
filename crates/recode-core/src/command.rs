//! Custom slash commands from markdown files.
//!
//! opencode stores commands as `command(s)/**/*.md`. The relative path is the
//! command name, the body is the template, and frontmatter can pin an agent or
//! model. Templates support `$ARGUMENTS`, positional `$1`…`$N`, and inline
//! shell substitution with ``!`command` ``.

use crate::config::ConfigManager;
use crate::frontmatter::{discover, markdown_name, Frontmatter};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

const COMMAND_SOURCES: &[(&str, bool, bool)] =
    &[("command", true, false), ("commands", true, false)];
const SHELL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct CustomCommand {
    pub name: String,
    pub description: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subagent: bool,
    /// Template text after frontmatter.
    pub template: String,
}

#[derive(Clone, Debug)]
pub struct Expansion {
    pub text: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub description: String,
    pub subagent: bool,
}

#[derive(Clone, Default)]
pub struct CommandRegistry {
    commands: BTreeMap<String, CustomCommand>,
}

impl CommandRegistry {
    /// Global files first, then `.opencode`, then `.recode`, so a project
    /// command overrides a global one with the same name.
    pub fn load(config: &ConfigManager) -> Self {
        let mut registry = Self::default();
        let roots = [
            config.paths.config_dir.clone(),
            config.project_root.join(".opencode"),
            config.project_root.join(".recode"),
        ];
        for root in roots {
            for file in discover(&root, COMMAND_SOURCES) {
                let Ok(content) = std::fs::read_to_string(&file.path) else {
                    continue;
                };
                let Some(name) = markdown_name(&root, &file.path) else {
                    continue;
                };
                let frontmatter = Frontmatter::parse(&content);
                registry.commands.insert(
                    name.clone(),
                    CustomCommand {
                        name,
                        description: frontmatter
                            .get("description")
                            .unwrap_or_default()
                            .to_owned(),
                        agent: frontmatter.get("agent").map(str::to_owned),
                        model: frontmatter.get("model").map(str::to_owned),
                        subagent: frontmatter.boolean("subagent") || frontmatter.boolean("subtask"),
                        template: frontmatter.body,
                    },
                );
            }
        }
        registry
    }

    pub fn get(&self, name: &str) -> Option<&CustomCommand> {
        self.commands.get(name)
    }

    pub fn list(&self) -> impl Iterator<Item = &CustomCommand> {
        self.commands.values()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl CustomCommand {
    /// Expand the template for `arguments`, running inline shell commands in
    /// `directory`. Failures name the command so the user can fix the file.
    pub async fn expand(&self, arguments: &str, directory: &Path) -> Result<Expansion, String> {
        let text = substitute(&self.template, arguments);
        let text = interpolate(&text, directory)
            .await
            .map_err(|error| format!("command {:?}: {error}", self.name))?;
        Ok(Expansion {
            text,
            agent: self.agent.clone(),
            model: self.model.clone(),
            description: self.description.clone(),
            subagent: self.subagent,
        })
    }
}

/// Replace `$ARGUMENTS` and `$1`…`$N`; append the input when the template has
/// no placeholders, matching opencode.
fn substitute(template: &str, input: &str) -> String {
    let arguments = split_arguments(input);
    let last = last_placeholder(template);
    let mut output = String::with_capacity(template.len() + input.len());
    let mut rest = template;
    while let Some(index) = rest.find('$') {
        output.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        if let Some(remainder) = after.strip_prefix("ARGUMENTS") {
            output.push_str(input);
            rest = remainder;
            continue;
        }
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        let position: usize = digits.parse().unwrap_or(0);
        if position == 0 {
            output.push('$');
            rest = after;
            continue;
        }
        let argument_index = position - 1;
        if argument_index < arguments.len() {
            if Some(position) == last {
                output.push_str(&arguments[argument_index..].join(" "));
            } else {
                output.push_str(&arguments[argument_index]);
            }
        }
        rest = &after[digits.len()..];
    }
    output.push_str(rest);
    let text = if last.is_none() && !template.contains("$ARGUMENTS") && !input.trim().is_empty() {
        format!("{}\n\n{}", output.trim_end(), input.trim())
    } else {
        output.trim().to_owned()
    };
    text
}

fn last_placeholder(template: &str) -> Option<usize> {
    let mut last = None;
    let mut rest = template;
    while let Some(index) = rest.find('$') {
        let after = &rest[index + 1..];
        if let Some(remainder) = after.strip_prefix("ARGUMENTS") {
            rest = remainder;
            continue;
        }
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        let position: usize = digits.parse().unwrap_or(0);
        if position > 0 {
            last = Some(last.map_or(position, |last: usize| last.max(position)));
        }
        rest = &after[digits.len()..];
    }
    last
}

/// Split on whitespace, keeping quoted runs together and dropping the quotes.
fn split_arguments(input: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in input.chars() {
        match quote {
            Some(delimiter) if character == delimiter => quote = None,
            Some(_) => current.push(character),
            None => match character {
                '"' | '\'' => quote = Some(character),
                character if character.is_whitespace() => {
                    if !current.is_empty() {
                        arguments.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(character),
            },
        }
    }
    if !current.is_empty() {
        arguments.push(current);
    }
    arguments
}

/// Replace ``!`command` `` with its combined output.
async fn interpolate(text: &str, directory: &Path) -> Result<String, String> {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("!`") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('`') else {
            output.push_str(&rest[start..]);
            return Ok(output);
        };
        let source = &after[..end];
        output.push_str(run_shell(source, directory).await?.trim_end());
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

async fn run_shell(source: &str, directory: &Path) -> Result<String, String> {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", source]);
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", source]);
        command
    };
    command.current_dir(directory).kill_on_drop(true);
    let output = tokio::time::timeout(SHELL_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            format!(
                "shell interpolation timed out after {}s",
                SHELL_TIMEOUT.as_secs()
            )
        })?
        .map_err(|error| format!("cannot run shell interpolation: {error}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(format!(
            "shell interpolation failed ({}): {}",
            output.status,
            text.trim()
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::RecodePaths;

    fn config(dir: &Path) -> ConfigManager {
        let paths = RecodePaths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            db_path: dir.join("data/recode.db"),
            auth_path: dir.join("config/auth.json"),
            log_dir: dir.join("data/log"),
            global_config: dir.join("config/recode.json"),
        };
        ConfigManager::load(paths, dir)
    }

    #[test]
    fn loads_commands_and_prefers_the_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("config/commands")).unwrap();
        std::fs::write(
            dir.path().join("config/commands/review.md"),
            "---\ndescription: Global review\n---\nGlobal $ARGUMENTS\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".opencode/command")).unwrap();
        std::fs::write(
            dir.path().join(".opencode/command/test.md"),
            "---\ndescription: Run tests\nagent: plan\n---\nRun $1\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".recode/command")).unwrap();
        std::fs::write(
            dir.path().join(".recode/command/review.md"),
            "---\ndescription: Project review\nmodel: acme/reviewer\n---\nReview $ARGUMENTS\n",
        )
        .unwrap();

        let registry = CommandRegistry::load(&config(dir.path()));
        assert_eq!(
            registry.get("review").unwrap().description,
            "Project review"
        );
        assert_eq!(
            registry.get("review").unwrap().model.as_deref(),
            Some("acme/reviewer")
        );
        assert_eq!(registry.get("test").unwrap().agent.as_deref(), Some("plan"));
    }

    #[test]
    fn substitutes_named_and_positional_arguments() {
        assert_eq!(
            substitute("Review $1 and $2 — $ARGUMENTS", "\"src/main.rs\" tests"),
            "Review src/main.rs and tests — \"src/main.rs\" tests"
        );
        // The last placeholder consumes the remaining arguments.
        assert_eq!(substitute("Fix $1 in $2", "bug a b"), "Fix bug in a b");
        assert_eq!(substitute("Missing $3 here", "a b"), "Missing  here");
    }

    #[test]
    fn appends_input_when_the_template_has_no_placeholders() {
        assert_eq!(
            substitute("Review the diff.", "focus on errors"),
            "Review the diff.\n\nfocus on errors"
        );
        assert_eq!(substitute("Review the diff.", ""), "Review the diff.");
    }

    #[tokio::test]
    async fn interpolates_shell_output() {
        let dir = tempfile::tempdir().unwrap();
        let text = interpolate("version !`echo 1.2.3` done", dir.path())
            .await
            .unwrap();
        assert_eq!(text.trim(), "version 1.2.3 done");
    }

    #[tokio::test]
    async fn expansion_carries_agent_and_model() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".recode/command")).unwrap();
        std::fs::write(
            dir.path().join(".recode/command/ship.md"),
            "---\ndescription: Ship it\nagent: plan\nmodel: acme/reviewer\n---\nShip $ARGUMENTS\n",
        )
        .unwrap();
        let registry = CommandRegistry::load(&config(dir.path()));
        let expansion = registry
            .get("ship")
            .unwrap()
            .expand("now", dir.path())
            .await
            .unwrap();
        assert_eq!(expansion.text, "Ship now");
        assert_eq!(expansion.agent.as_deref(), Some("plan"));
        assert_eq!(expansion.model.as_deref(), Some("acme/reviewer"));
        assert_eq!(expansion.description, "Ship it");
        assert!(!expansion.subagent);
    }
}
