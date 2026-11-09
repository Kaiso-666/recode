use crate::config::ConfigManager;
use crate::frontmatter::{discover, markdown_name, Frontmatter};
use indexmap::IndexMap;
use recode_schema::config::{AgentConfig, PermissionConfig, PermissionValue};
use recode_schema::permission::{Decision, Ruleset};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentMode {
    Primary,
    Subagent,
}

#[derive(Clone, Debug)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub mode: AgentMode,
    pub system_prompt: String,
    pub model: Option<String>,
    pub tools: BTreeMap<String, bool>,
    pub temperature: Option<f32>,
    pub permission: Ruleset,
    pub hidden: bool,
}

#[derive(Clone, Default)]
pub struct AgentRegistry {
    agents: BTreeMap<String, Agent>,
}

impl AgentRegistry {
    pub fn from_config(config: &ConfigManager) -> Self {
        let mut registry = Self::default();
        registry.insert(Agent {
            id: "build".into(),
            name: "Build".into(),
            description:
                "Full-capability coding agent: reads, edits, runs commands, and verifies changes."
                    .into(),
            mode: AgentMode::Primary,
            system_prompt: BUILD_PROMPT.into(),
            model: None,
            tools: BTreeMap::new(),
            temperature: None,
            permission: Ruleset::default(),
            hidden: false,
        });
        registry.insert(Agent {
            id: "plan".into(),
            name: "Plan".into(),
            description: "Read-only architect: investigates the codebase and produces an implementation plan.".into(),
            mode: AgentMode::Primary,
            system_prompt: PLAN_PROMPT.into(),
            model: None,
            tools: BTreeMap::from([
                ("read".into(), true),
                ("ls".into(), true),
                ("glob".into(), true),
                ("grep".into(), true),
                ("webfetch".into(), true),
                ("question".into(), true),
            ]),
            temperature: None,
            permission: Ruleset::default(),
            hidden: false,
        });
        registry.insert(Agent {
            id: "general".into(),
            name: "General".into(),
            description: "General-purpose subagent for independent research and multi-step tasks."
                .into(),
            mode: AgentMode::Subagent,
            system_prompt: GENERAL_PROMPT.into(),
            model: None,
            tools: BTreeMap::new(),
            temperature: None,
            permission: Ruleset::default(),
            hidden: false,
        });
        registry.insert(Agent {
            id: "explore".into(),
            name: "Explore".into(),
            description: "Fast read-only search agent for locating code and answering questions."
                .into(),
            mode: AgentMode::Subagent,
            system_prompt: EXPLORE_PROMPT.into(),
            model: None,
            tools: BTreeMap::from([
                ("read".into(), true),
                ("ls".into(), true),
                ("glob".into(), true),
                ("grep".into(), true),
            ]),
            temperature: None,
            permission: Ruleset::default(),
            hidden: true,
        });

        registry.load_markdown(config);
        // Explicit JSON configuration outranks markdown files.
        for (id, agent_config) in &config.merged.agent {
            registry.apply(id, agent_config);
        }
        registry
    }

    /// Load opencode-style `agent(s)/**/*.md` and `mode(s)/*.md` definitions.
    /// Global files load first, then `.opencode` for compatibility, then the
    /// native `.recode` directory, so a later file wins on an id collision.
    fn load_markdown(&mut self, config: &ConfigManager) {
        let roots = [
            config.paths.config_dir.clone(),
            config.project_root.join(".opencode"),
            config.project_root.join(".recode"),
        ];
        for root in roots {
            for file in discover(&root, AGENT_SOURCES) {
                let Ok(content) = std::fs::read_to_string(&file.path) else {
                    continue;
                };
                let Some(id) = markdown_name(&root, &file.path) else {
                    continue;
                };
                let frontmatter = Frontmatter::parse(&content);
                self.apply(&id, &markdown_config(&frontmatter, file.primary));
            }
        }
    }

    fn insert(&mut self, agent: Agent) {
        self.agents.insert(agent.id.clone(), agent);
    }

    fn apply(&mut self, id: &str, config: &AgentConfig) {
        let base = self.agents.get(id).cloned().unwrap_or_else(|| Agent {
            id: id.to_owned(),
            name: id.to_owned(),
            description: String::new(),
            mode: AgentMode::Primary,
            system_prompt: BUILD_PROMPT.into(),
            model: None,
            tools: BTreeMap::new(),
            temperature: None,
            permission: Ruleset::default(),
            hidden: false,
        });
        if config.disable {
            self.agents.remove(id);
            return;
        }
        let mut agent = base;
        if !config.description.is_empty() {
            agent.description = config.description.clone();
        }
        if let Some(model) = &config.model {
            agent.model = Some(model.clone());
        }
        if let Some(prompt) = &config.prompt {
            agent.system_prompt = prompt.clone();
        }
        if let Some(temperature) = config.temperature {
            agent.temperature = Some(temperature);
        }
        if let Some(mode) = &config.mode {
            agent.mode = match mode.as_str() {
                "subagent" => AgentMode::Subagent,
                _ => AgentMode::Primary,
            };
        }
        agent.hidden |= config.hidden;
        for (tool, enabled) in &config.tools {
            agent.tools.insert(tool.clone(), *enabled);
        }
        agent.permission.extend(config.permission.to_ruleset());
        self.agents.insert(id.to_owned(), agent);
    }

    pub fn get(&self, id: &str) -> Option<&Agent> {
        self.agents.get(id)
    }

    pub fn get_or_default(&self, id: &str) -> &Agent {
        self.agents
            .get(id)
            .or_else(|| self.agents.get("build"))
            .expect("build agent is always registered")
    }

    pub fn list(&self) -> impl Iterator<Item = &Agent> {
        self.agents.values().filter(|agent| !agent.hidden)
    }

    /// Every registered agent, including hidden ones such as `explore`, which
    /// remain valid `task` targets.
    pub fn all(&self) -> impl Iterator<Item = &Agent> {
        self.agents.values()
    }

    pub fn default_id(&self) -> &str {
        "build"
    }
}

/// `agent(s)/**/*.md` and `mode(s)/*.md`; `mode(s)` files are always primary.
const AGENT_SOURCES: &[(&str, bool, bool)] = &[
    ("agent", true, false),
    ("agents", true, false),
    ("mode", false, true),
    ("modes", false, true),
];

fn markdown_config(frontmatter: &Frontmatter, primary: bool) -> AgentConfig {
    let mode = if primary {
        Some("primary".to_owned())
    } else {
        frontmatter
            .get("mode")
            .map(|mode| mode.to_ascii_lowercase())
            .filter(|mode| mode == "primary" || mode == "subagent")
    };
    AgentConfig {
        description: frontmatter
            .get("description")
            .unwrap_or_default()
            .to_owned(),
        mode,
        model: frontmatter.get("model").map(str::to_owned),
        temperature: frontmatter
            .get("temperature")
            .and_then(|value| value.parse().ok()),
        // The body is the system prompt even when empty; inheriting the build
        // prompt would silently give the agent capabilities it did not ask for.
        prompt: Some(frontmatter.body.clone()),
        tools: frontmatter
            .bool_map("tools")
            .into_iter()
            .collect::<IndexMap<_, _>>(),
        permission: permission_config(frontmatter),
        disable: frontmatter.boolean("disable"),
        hidden: frontmatter.boolean("hidden"),
    }
}

/// Flat `permission: { bash: ask }` entries. Nested resource maps are only
/// available through JSON configuration; silently ignore what we cannot map.
fn permission_config(frontmatter: &Frontmatter) -> PermissionConfig {
    let permissions = frontmatter
        .map("permission")
        .into_iter()
        .filter_map(|(action, value)| {
            let decision = match value.to_ascii_lowercase().as_str() {
                "allow" => Decision::Allow,
                "ask" => Decision::Ask,
                "deny" => Decision::Deny,
                _ => return None,
            };
            Some((action, PermissionValue::Action(decision)))
        })
        .collect();
    PermissionConfig(permissions)
}

const BUILD_PROMPT: &str = r#"You are Re:Code, an AI coding agent running in the user's terminal. You help with software engineering: understanding code, implementing features, fixing bugs, refactoring, running commands, and explaining what you did.

# How you work

- Be direct and concise. Prefer doing over describing. Do not narrate every tool call.
- Investigate before changing. Read the relevant files and search for usages before editing. Never guess at file contents or APIs.
- Make the smallest correct change. Match the existing style, naming, and patterns of the codebase. Do not add dependencies, abstractions, or files unless they are needed for the task.
- Verify your work. Run the project's formatter, type checker, and tests when available; report the actual result. If you cannot verify something, say so explicitly.
- Use dedicated tools for files: `read` before `edit`, `grep`/`glob` to locate code, `write` only for new files or full rewrites.
- Use `bash` for builds, tests, git, and package managers. Commands run in the project root on Windows PowerShell (or bash on Unix).
- When a task has several steps, use the todo list to track them and keep it current.
- If a permission is denied, adapt: do not try to bypass it. Ask the user only when genuinely blocked.
- Never fabricate command output, test results, or file contents. If a tool fails, read the error and try a different approach.
- Do not commit, push, or rewrite git history unless the user asks.

# Communication

- Final answers should state what changed, where, and how it was verified. Keep them short.
- Reference files with paths, e.g. `src/main.rs:42`.
- Do not use emojis unless the user does.

# Safety

- Never print secrets from the environment or files.
- Do not delete user data or run destructive commands (`rm -rf`, `git reset --hard`, force pushes) without explicit confirmation.
"#;

const PLAN_PROMPT: &str = r#"You are Re:Code in plan mode: a read-only architect. Investigate the codebase and produce a concrete implementation plan. You cannot edit files or run mutating commands.

- Read code, search, and inspect configuration before proposing changes.
- Identify the real constraints: existing patterns, call sites, tests, and build tooling.
- Produce a step-by-step plan with exact files and functions, ordered by dependency.
- Call out risks, migration concerns, and how the result should be verified.
- Do not write code to disk. Short illustrative snippets in your answer are fine.
- Ask clarifying questions only when the ambiguity blocks a correct plan.
"#;

const GENERAL_PROMPT: &str = r#"You are Re:Code, a general-purpose engineering subagent. Work autonomously on the task you were given. Investigate first, make focused changes, verify them, and return a concise report of what you found and changed. Do not expand scope beyond the task.
"#;

const EXPLORE_PROMPT: &str = r#"You are Re:Code's explore agent. You are read-only and optimized for fast, thorough codebase search. Locate definitions, usages, and relevant files, then answer the question with precise paths and line references. Do not propose large refactors or modify anything.
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::RecodePaths;

    #[test]
    fn builtin_agents_include_plan_and_build() {
        let dir = tempfile::tempdir().unwrap();
        let paths = RecodePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            db_path: dir.path().join("data/recode.db"),
            auth_path: dir.path().join("config/auth.json"),
            log_dir: dir.path().join("data/log"),
            global_config: dir.path().join("config/recode.json"),
        };
        let config = ConfigManager::load(paths, dir.path());
        let agents = AgentRegistry::from_config(&config);
        assert!(agents.get("build").is_some());
        assert!(agents
            .get("plan")
            .unwrap()
            .tools
            .get("read")
            .copied()
            .unwrap_or(false));
        assert!(!agents.get("plan").unwrap().tools.contains_key("write"));
    }

    #[test]
    fn loads_markdown_agents_from_global_and_project_directories() {
        let dir = tempfile::tempdir().unwrap();
        let paths = RecodePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            db_path: dir.path().join("data/recode.db"),
            auth_path: dir.path().join("config/auth.json"),
            log_dir: dir.path().join("data/log"),
            global_config: dir.path().join("config/recode.json"),
        };
        std::fs::create_dir_all(paths.config_dir.join("agents/team")).unwrap();
        std::fs::write(
            paths.config_dir.join("agents/team/security.md"),
            "---\ndescription: Security reviewer\nmode: subagent\ntools:\n  edit: false\n  read: true\n---\nYou review security.\n",
        )
        .unwrap();

        std::fs::create_dir_all(dir.path().join(".opencode/agent")).unwrap();
        std::fs::write(
            dir.path().join(".opencode/agent/reviewer.md"),
            "---\ndescription: From opencode\n---\nOld prompt.\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".recode/agent")).unwrap();
        std::fs::write(
            dir.path().join(".recode/agent/reviewer.md"),
            "---\ndescription: Code reviewer\ntemperature: 0.2\npermission:\n  bash: ask\n  edit: deny\ntools:\n  write: false\n---\nYou review code carefully.\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".recode/mode")).unwrap();
        std::fs::write(
            dir.path().join(".recode/mode/architect.md"),
            "---\nmode: subagent\n---\nDesign first.\n",
        )
        .unwrap();

        let config = ConfigManager::load(paths, dir.path());
        let agents = AgentRegistry::from_config(&config);

        let security = agents.get("team/security").expect("nested agent id");
        assert_eq!(security.description, "Security reviewer");
        assert_eq!(security.mode, AgentMode::Subagent);
        assert_eq!(security.tools.get("edit"), Some(&false));
        assert_eq!(security.system_prompt, "You review security.");

        let reviewer = agents.get("reviewer").expect("project agent");
        assert_eq!(reviewer.description, "Code reviewer");
        assert_eq!(reviewer.system_prompt, "You review code carefully.");
        assert_eq!(reviewer.temperature, Some(0.2));
        assert_eq!(reviewer.mode, AgentMode::Primary);
        assert_eq!(reviewer.tools.get("write"), Some(&false));
        assert_eq!(
            reviewer.permission.evaluate("bash", "cargo test"),
            Some(Decision::Ask)
        );
        assert_eq!(
            reviewer.permission.evaluate("edit", "src/main.rs"),
            Some(Decision::Deny)
        );

        let architect = agents.get("architect").expect("mode agent");
        assert_eq!(architect.mode, AgentMode::Primary);
    }

    #[test]
    fn json_agent_config_overrides_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let paths = RecodePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            db_path: dir.path().join("data/recode.db"),
            auth_path: dir.path().join("config/auth.json"),
            log_dir: dir.path().join("data/log"),
            global_config: dir.path().join("config/recode.json"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(
            &paths.global_config,
            r#"{"agent":{"reviewer":{"description":"JSON wins","temperature":0.9}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".recode/agent")).unwrap();
        std::fs::write(
            dir.path().join(".recode/agent/reviewer.md"),
            "---\ndescription: Markdown loses\n---\nReview.\n",
        )
        .unwrap();

        let config = ConfigManager::load(paths, dir.path());
        let agents = AgentRegistry::from_config(&config);
        let reviewer = agents.get("reviewer").expect("reviewer");
        assert_eq!(reviewer.description, "JSON wins");
        assert_eq!(reviewer.temperature, Some(0.9));
        // Markdown still supplies the prompt; JSON config has none.
        assert_eq!(reviewer.system_prompt, "Review.");
    }
}
