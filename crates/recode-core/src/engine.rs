use crate::agent::{Agent, AgentRegistry};
use crate::config::ConfigManager;
use crate::instructions::Instructions;
use crate::paths::RecodePaths;
use crate::permission::PermissionResponder;
use crate::project::Project;
use crate::providers::Providers;
use crate::runner;
use recode_schema::event::{Event, LiveEvent, SessionInfo, SessionStatus};
use recode_schema::id::SessionId;
use recode_schema::message::{Message, MessageInfo};
use recode_schema::model::ModelRef;
use recode_schema::part::Part;
use recode_storage::{Store, StoredMessage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;

pub struct EngineOptions {
    pub paths: RecodePaths,
    pub directory: PathBuf,
    /// In-process approval bridge. `None` leaves approvals to the bus broker
    /// (server mode), where a client replies over HTTP.
    pub responder: Option<Arc<dyn PermissionResponder>>,
}

struct ActiveRun {
    cancel: CancellationToken,
    done: watch::Sender<bool>,
}

/// The in-process engine: owns storage, configuration, providers, tools, and
/// the set of active session runs. Cheap to clone via `Arc`.
pub struct Engine {
    pub store: Arc<Store>,
    pub config: Arc<ConfigManager>,
    pub project: Arc<Project>,
    pub agents: Arc<AgentRegistry>,
    pub tools: Arc<recode_tool::ToolRegistry>,
    pub providers: Arc<Providers>,
    pub permissions: Arc<crate::permission::PermissionService>,
    pub bus: broadcast::Sender<LiveEvent>,
    runs: parking_lot::Mutex<HashMap<SessionId, ActiveRun>>,
}

impl Engine {
    pub fn new(options: EngineOptions) -> Result<Arc<Self>, String> {
        options
            .paths
            .ensure()
            .map_err(|error| format!("cannot create data directories: {error}"))?;
        let project = Project::discover(&options.directory)?;
        let config = Arc::new(ConfigManager::load(options.paths.clone(), &project.root));
        let store = Arc::new(Store::open(&options.paths.db_path).map_err(|error| {
            format!(
                "cannot open database {}: {error}",
                options.paths.db_path.display()
            )
        })?);
        let project_info = match store
            .project_by_directory(&project.root)
            .map_err(|error| error.to_string())?
        {
            Some(mut existing) => {
                existing.directory = project.root.clone();
                existing.name = project.info.name.clone();
                existing.vcs = project.info.vcs.clone();
                store
                    .upsert_project(&existing)
                    .map_err(|error| error.to_string())?;
                existing
            }
            None => {
                store
                    .upsert_project(&project.info)
                    .map_err(|error| error.to_string())?;
                project.info.clone()
            }
        };
        let mut project = project;
        project.info = project_info;
        let agents = Arc::new(AgentRegistry::from_config(&config));
        let tools = Arc::new(recode_tool::ToolRegistry::builtin());
        let providers = Arc::new(Providers::new(config.clone()));
        let mut rules = config.ruleset();
        // Baseline protections. Explicit user rules are appended later and win.
        rules.push(recode_schema::permission::Rule::deny("edit", ".git/**"));
        rules.push(recode_schema::permission::Rule::deny("edit", "**/.git/**"));
        let (bus, _) = broadcast::channel(2048);
        let permissions = Arc::new(crate::permission::PermissionService::new(
            rules,
            options.responder,
            Some(bus.clone()),
            std::time::Duration::from_secs(300),
        ));
        Ok(Arc::new(Self {
            store,
            config,
            project: Arc::new(project),
            agents,
            tools,
            providers,
            permissions,
            bus,
            runs: parking_lot::Mutex::new(HashMap::new()),
        }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LiveEvent> {
        self.bus.subscribe()
    }

    pub fn agent(&self, id: &str) -> &Agent {
        self.agents.get_or_default(id)
    }

    pub fn create_session(
        &self,
        agent: Option<String>,
        model: Option<ModelRef>,
        title: Option<String>,
    ) -> Result<SessionInfo, String> {
        let agent = agent.unwrap_or_else(|| self.config.default_agent());
        let model = match model {
            Some(model) => Some(model),
            None => self.config.resolve_model(None).ok(),
        };
        let mut session = SessionInfo::new(self.project.info.id.clone(), self.project.root.clone());
        session.agent = Some(agent);
        session.model = model;
        session.title = title.unwrap_or_default();
        self.store
            .create_session(session.clone())
            .map_err(|error| error.to_string())?;
        Ok(session)
    }

    pub fn session(&self, session_id: &SessionId) -> Result<Option<SessionInfo>, String> {
        self.store
            .get_session(session_id)
            .map_err(|error| error.to_string())
    }

    pub fn sessions(&self, include_archived: bool) -> Result<Vec<SessionInfo>, String> {
        self.store
            .list_sessions(Some(&self.project.info.id), include_archived, 200)
            .map_err(|error| error.to_string())
    }

    pub fn all_sessions(&self, include_archived: bool) -> Result<Vec<SessionInfo>, String> {
        self.store
            .list_sessions(None, include_archived, 500)
            .map_err(|error| error.to_string())
    }

    pub fn messages(&self, session_id: &SessionId) -> Result<Vec<Message>, String> {
        let stored: Vec<StoredMessage> = self
            .store
            .messages(session_id, 10_000)
            .map_err(|error| error.to_string())?;
        Ok(stored
            .into_iter()
            .map(StoredMessage::into_message)
            .collect())
    }

    pub fn delete_session(&self, session_id: &SessionId) -> Result<(), String> {
        self.store
            .delete_session(session_id)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn update_title(
        &self,
        session_id: &SessionId,
        title: impl Into<String>,
    ) -> Result<(), String> {
        let Some(mut session) = self.session(session_id)? else {
            return Err("session not found".into());
        };
        session.title = title.into();
        session.updated_at = recode_schema::now_ms();
        self.store
            .update_session(session)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn set_model(&self, session_id: &SessionId, model: ModelRef) -> Result<(), String> {
        let Some(mut session) = self.session(session_id)? else {
            return Err("session not found".into());
        };
        session.model = Some(model);
        session.updated_at = recode_schema::now_ms();
        self.store
            .update_session(session)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn set_agent(
        &self,
        session_id: &SessionId,
        agent: impl Into<String>,
    ) -> Result<(), String> {
        let Some(mut session) = self.session(session_id)? else {
            return Err("session not found".into());
        };
        session.agent = Some(agent.into());
        session.updated_at = recode_schema::now_ms();
        self.store
            .update_session(session)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Summarize older history and persist a compaction event. The full
    /// transcript stays readable; only the model-facing history is shortened.
    pub async fn compact(
        self: &Arc<Self>,
        session_id: &SessionId,
    ) -> Result<Option<recode_storage::CompactionRecord>, String> {
        if self.is_busy(session_id) {
            return Err("session is busy; wait for the current run to finish".into());
        }
        let session = self
            .session(session_id)?
            .ok_or_else(|| "session not found".to_owned())?;
        let agent_id = session
            .agent
            .clone()
            .unwrap_or_else(|| self.config.default_agent());
        let model = match session.model.clone() {
            Some(model) => model,
            None => crate::engine::initial_model(self, self.agent(&agent_id))?,
        };
        let cancel = CancellationToken::new();
        crate::compaction::compact_session(self, session_id, &model, &cancel).await
    }

    /// Admit a user prompt and start a run. Fails when the session is busy.
    pub async fn prompt(
        self: &Arc<Self>,
        session_id: &SessionId,
        text: String,
        files: Vec<recode_schema::llm::ContentBlock>,
    ) -> Result<(), String> {
        if text.trim().is_empty() && files.is_empty() {
            return Err("prompt must not be empty".into());
        }
        let session = self
            .session(session_id)?
            .ok_or_else(|| "session not found".to_owned())?;
        let agent_id = session
            .agent
            .clone()
            .unwrap_or_else(|| self.config.default_agent());
        let model = match session.model.clone() {
            Some(model) => model,
            None => crate::engine::initial_model(self, self.agent(&agent_id))?,
        };
        let cancel = CancellationToken::new();
        let (done, _receiver) = watch::channel(false);
        {
            let mut runs = self.runs.lock();
            if runs.contains_key(session_id) {
                return Err("session is busy".into());
            }
            runs.insert(
                session_id.clone(),
                ActiveRun {
                    cancel: cancel.clone(),
                    done,
                },
            );
        }

        let message = MessageInfo::user(session_id.clone());
        let appended = self
            .store
            .append(Event::MessageAdded {
                message: message.clone(),
            })
            .map_err(|error| error.to_string());
        let appended = match appended {
            Ok(envelope) => envelope,
            Err(error) => {
                self.runs.lock().remove(session_id);
                return Err(error);
            }
        };
        let _ = self.bus.send(LiveEvent::Durable(appended));
        if !text.is_empty() {
            let part = Part::text(text.clone());
            match self.store.append(Event::PartAdded {
                session_id: session_id.clone(),
                message_id: message.id.clone(),
                part,
            }) {
                Ok(envelope) => {
                    let _ = self.bus.send(LiveEvent::Durable(envelope));
                }
                Err(error) => {
                    self.runs.lock().remove(session_id);
                    return Err(error.to_string());
                }
            }
        }
        for block in files {
            if let recode_schema::llm::ContentBlock::File {
                mime,
                filename,
                url,
            } = block
            {
                let part = Part::File(recode_schema::part::FilePart {
                    id: recode_schema::id::PartId::new(),
                    mime,
                    filename,
                    url,
                });
                match self.store.append(Event::PartAdded {
                    session_id: session_id.clone(),
                    message_id: message.id.clone(),
                    part,
                }) {
                    Ok(envelope) => {
                        let _ = self.bus.send(LiveEvent::Durable(envelope));
                    }
                    Err(error) => {
                        self.runs.lock().remove(session_id);
                        return Err(error.to_string());
                    }
                }
            }
        }
        if session.title.is_empty() {
            let title = text.lines().next().unwrap_or("New session").trim();
            let title = if title.chars().count() > 70 {
                format!("{}…", title.chars().take(70).collect::<String>())
            } else {
                title.to_owned()
            };
            let _ = self.update_title(session_id, title);
        }
        let engine = self.clone();
        let id = session_id.clone();
        tokio::spawn(async move {
            runner::run(engine.clone(), id.clone(), agent_id, model, cancel).await;
            if let Some(run) = engine.runs.lock().get(&id) {
                let _ = run.done.send(true);
            }
            engine.runs.lock().remove(&id);
        });
        Ok(())
    }

    pub fn is_busy(&self, session_id: &SessionId) -> bool {
        self.runs.lock().contains_key(session_id)
    }

    pub fn abort(&self, session_id: &SessionId) -> bool {
        if let Some(run) = self.runs.lock().get(session_id) {
            run.cancel.cancel();
            true
        } else {
            false
        }
    }

    /// Wait until the active run for a session finishes. Returns immediately
    /// when the session is idle.
    pub async fn wait_idle(&self, session_id: &SessionId) {
        loop {
            let receiver = {
                let runs = self.runs.lock();
                let Some(run) = runs.get(session_id) else {
                    return;
                };
                run.done.subscribe()
            };
            let mut receiver = receiver;
            let _ = receiver.changed().await;
            if *receiver.borrow() {
                return;
            }
        }
    }

    pub fn set_status(&self, session_id: &SessionId, status: SessionStatus) {
        let _ = self.store.set_status(session_id, status);
        let _ = self.bus.send(LiveEvent::SessionStatus {
            session_id: session_id.clone(),
            status,
        });
    }

    pub fn instructions_for(&self, session: &SessionInfo) -> Instructions {
        Instructions::load(&self.config, &self.project.root, &session.directory)
    }
}

/// Build the canonical starting model for a session, if one is available.
pub fn initial_model(engine: &Engine, agent: &Agent) -> Result<ModelRef, String> {
    if let Some(model) = &agent.model {
        ModelRef::parse(model).ok_or_else(|| format!("agent has invalid model {model:?}"))
    } else {
        engine.config.resolve_model(None)
    }
}
