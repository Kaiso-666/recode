//! Re:Code core engine: configuration, projects, agents, permissions, session
//! persistence orchestration, LSP/MCP managers (later milestones), and the
//! model ↔ tool agent loop.

pub mod agent;
pub mod auth;
pub mod compaction;
pub mod config;
pub mod engine;
pub mod formatter;
pub mod instructions;
pub mod jsonc;
pub mod lsp;
pub mod mcp;
pub mod oauth;
pub mod paths;
pub mod permission;
pub mod project;
pub mod prompt;
pub mod providers;
pub mod question;
pub mod runner;
pub mod snapshot;
pub mod subagent;

pub use agent::{Agent, AgentMode, AgentRegistry};
pub use config::ConfigManager;
pub use engine::{Engine, EngineOptions};
pub use paths::RecodePaths;
pub use permission::{PermissionResponder, PermissionService};
pub use project::Project;
