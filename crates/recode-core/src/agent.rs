use crate::config::ConfigManager;
use recode_schema::config::AgentConfig;
use recode_schema::permission::Ruleset;
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

        for (id, agent_config) in &config.merged.agent {
            registry.apply(id, agent_config);
        }
        registry
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

    pub fn default_id(&self) -> &str {
        "build"
    }
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
}
