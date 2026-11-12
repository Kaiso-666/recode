//! Storage for the TUI's domain data.
//!
//! The TUI is a client of a session engine, not the engine itself: it renders a
//! snapshot, applies streaming deltas, and sends commands. With a local engine
//! the snapshot is read from SQLite; with `--server` the same snapshot is
//! polled from the HTTP API. Everything above this module is identical in both
//! modes, which is what makes attach mode a client feature rather than a
//! second UI.

use recode_client::Client;
use recode_core::command::Expansion;
use recode_core::config::ProviderSummary;
use recode_core::engine::Engine;
use recode_schema::event::{LiveEvent, SessionInfo, SessionStatus};
use recode_schema::message::Message;
use recode_schema::permission::{PermissionReply, PermissionRequest};
use recode_schema::question::{QuestionReply, QuestionRequest};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentView {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandView {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Default)]
pub struct Poll {
    pub permissions: Vec<PermissionRequest>,
    pub questions: Vec<QuestionRequest>,
}

#[derive(Clone)]
struct RemoteCache {
    session: SessionInfo,
    messages: Vec<Message>,
    sessions: Vec<SessionInfo>,
    providers: Vec<ProviderSummary>,
    agents: Vec<AgentView>,
    commands: Vec<CommandView>,
    files: Vec<String>,
    busy: bool,
    /// Sequence number of the last event folded into `messages`.
    last_seq: u64,
    /// A live event could not be folded in; the transcript must be refetched.
    stale: bool,
}

struct Remote {
    client: Client,
    cache: RwLock<RemoteCache>,
    /// Live events from the server's event stream.
    bus: tokio::sync::broadcast::Sender<recode_schema::event::LiveEvent>,
    /// Whether the event stream is currently connected.
    connected: Arc<AtomicBool>,
    /// Bumped when the visible session changes; readers of an older session
    /// stop when they see a newer generation.
    generation: Arc<AtomicU64>,
}

/// Fold the server's live events into the cached transcript and forward them
/// to the TUI.
fn spawn_event_reader(
    remote: Arc<Remote>,
    mut stream: recode_client::EventStream,
    session_id: String,
    generation: u64,
) {
    tokio::spawn(async move {
        while let Some(item) = stream.next().await {
            if remote.generation.load(Ordering::Acquire) != generation {
                return;
            }
            let event = match item {
                Ok(event) => event,
                Err(error) => {
                    tracing::debug!(%error, "server event stream failed");
                    break;
                }
            };
            {
                let mut cache = remote.cache.write().unwrap();
                // The visible session may have changed while we waited for
                // the lock; the reader of the old session stops here.
                if remote.generation.load(Ordering::Acquire) != generation {
                    return;
                }
                match &event {
                    LiveEvent::Durable(envelope) => {
                        if envelope.session_id.0 == session_id && envelope.seq > cache.last_seq {
                            cache.last_seq = envelope.seq;
                            if !event.apply(&mut cache.messages) {
                                // The event does not fit the transcript we
                                // hold; the next poll refetches it.
                                cache.stale = true;
                            }
                        }
                    }
                    LiveEvent::SessionStatus { status, .. } => {
                        cache.busy = *status == SessionStatus::Busy;
                    }
                    _ => {}
                }
            }
            let _ = remote.bus.send(event);
        }
        remote.connected.store(false, Ordering::Release);
        tracing::debug!("server event stream closed");
    });
}

#[derive(Clone)]
pub struct Backend {
    local: Option<Arc<Engine>>,
    remote: Option<Arc<Remote>>,
    bus: tokio::sync::broadcast::Sender<recode_schema::event::LiveEvent>,
}

impl Backend {
    pub fn local(engine: Arc<Engine>) -> Self {
        let bus = engine.bus.clone();
        Self {
            local: Some(engine),
            remote: None,
            bus,
        }
    }

    /// Connect to a running server and load the initial snapshot. Fails before
    /// the terminal is touched, so a bad URL leaves a usable shell.
    pub async fn remote(client: Client, session_id: &str) -> Result<Self, String> {
        client.health().await?;
        // Subscribe before reading the snapshot: events that arrive while the
        // snapshot is fetched are buffered by the stream, and the sequence
        // number of the snapshot tells us which of them are already included.
        let mut stream = match client.events(session_id).await {
            Ok(stream) => Some(stream),
            Err(error) => {
                tracing::debug!(%error, "cannot subscribe to server events");
                None
            }
        };
        let snapshot = client.session(session_id).await?;
        let (messages, last_seq) = client.messages_with_seq(session_id).await?;
        let sessions = client.sessions(true, true).await.unwrap_or_default();
        let providers = client.providers().await.unwrap_or_default();
        let agents = client.agents().await.unwrap_or_default();
        let commands = client.commands().await.unwrap_or_default();
        let files = client.files().await.unwrap_or_default();
        let (bus, _) = tokio::sync::broadcast::channel(256);
        let cache = RemoteCache {
            session: snapshot.session,
            messages,
            sessions,
            providers: providers
                .into_iter()
                .map(|provider| ProviderSummary {
                    id: provider.id,
                    name: provider.name,
                    configured: provider.configured,
                    models: provider.models,
                })
                .collect(),
            agents: agents
                .into_iter()
                .map(|agent| AgentView {
                    id: agent.id,
                    name: agent.name,
                    description: agent.description,
                })
                .collect(),
            commands: commands
                .into_iter()
                .map(|command| CommandView {
                    name: command.name,
                    description: command.description,
                })
                .collect(),
            files,
            busy: snapshot.busy,
            last_seq,
            stale: false,
        };
        let connected = Arc::new(AtomicBool::new(stream.is_some()));
        let remote = Arc::new(Remote {
            client,
            cache: RwLock::new(cache),
            bus: bus.clone(),
            connected: connected.clone(),
            generation: Arc::new(AtomicU64::new(0)),
        });
        if let Some(stream) = stream.take() {
            spawn_event_reader(remote.clone(), stream, session_id.to_owned(), 0);
        }
        Ok(Self {
            local: None,
            remote: Some(remote),
            bus,
        })
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<recode_schema::event::LiveEvent> {
        self.bus.subscribe()
    }

    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// The sequence number of the transcript the backend currently holds.
    /// Local mode has no sequence numbers; the counter stays at zero.
    pub fn transcript_seq(&self) -> u64 {
        match (&self.local, &self.remote) {
            (Some(_), _) => 0,
            (_, Some(remote)) => remote.cache.read().unwrap().last_seq,
            _ => 0,
        }
    }

    pub fn root(&self) -> PathBuf {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.project.root.clone(),
            (_, Some(remote)) => remote.cache.read().unwrap().session.directory.clone(),
            _ => PathBuf::new(),
        }
    }

    pub fn session(&self, id: &recode_schema::SessionId) -> Option<SessionInfo> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.session(id).ok().flatten(),
            (_, Some(remote)) => {
                let cache = remote.cache.read().unwrap();
                if &cache.session.id == id {
                    Some(cache.session.clone())
                } else {
                    cache
                        .sessions
                        .iter()
                        .find(|session| &session.id == id)
                        .cloned()
                }
            }
            _ => None,
        }
    }

    pub fn messages(&self, id: &recode_schema::SessionId) -> Result<Vec<Message>, String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.messages(id),
            (_, Some(remote)) => {
                let cache = remote.cache.read().unwrap();
                if &cache.session.id == id {
                    Ok(cache.messages.clone())
                } else {
                    Ok(Vec::new())
                }
            }
            _ => Err("no backend".into()),
        }
    }

    pub fn is_busy(&self, id: &recode_schema::SessionId) -> bool {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.is_busy(id),
            (_, Some(remote)) => {
                let cache = remote.cache.read().unwrap();
                &cache.session.id == id && cache.busy
            }
            _ => false,
        }
    }

    pub fn providers(&self) -> Vec<ProviderSummary> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.config.providers(),
            (_, Some(remote)) => remote.cache.read().unwrap().providers.clone(),
            _ => Vec::new(),
        }
    }

    pub fn agents(&self) -> Vec<AgentView> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine
                .agents
                .list()
                .map(|agent| AgentView {
                    id: agent.id.clone(),
                    name: agent.name.clone(),
                    description: agent.description.clone(),
                })
                .collect(),
            (_, Some(remote)) => remote.cache.read().unwrap().agents.clone(),
            _ => Vec::new(),
        }
    }

    pub fn has_agent(&self, id: &str) -> bool {
        self.agents().iter().any(|agent| agent.id == id)
    }

    /// Custom slash commands, used for completion and to decide whether a
    /// leading `/name` should be expanded instead of sent as text.
    pub fn commands(&self) -> Vec<CommandView> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine
                .commands
                .list()
                .map(|command| CommandView {
                    name: command.name.clone(),
                    description: command.description.clone(),
                })
                .collect(),
            (_, Some(remote)) => remote.cache.read().unwrap().commands.clone(),
            _ => Vec::new(),
        }
    }

    /// Expand a markdown slash command. With a local engine the template runs
    /// in-process; in attach mode the server expands it so shell interpolation
    /// uses the machine that owns the session.
    pub async fn expand_command(
        &self,
        name: &str,
        arguments: &str,
        directory: &Path,
    ) -> Result<Option<Expansion>, String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => {
                let Some(command) = engine.commands.get(name) else {
                    return Ok(None);
                };
                command.expand(arguments, directory).await.map(Some)
            }
            (_, Some(remote)) => {
                if !remote
                    .client
                    .commands()
                    .await?
                    .iter()
                    .any(|command| command.name == name)
                {
                    return Ok(None);
                }
                let session = remote.cache.read().unwrap().session.id.0.clone();
                let expansion = remote
                    .client
                    .expand_command(&session, name, arguments)
                    .await?;
                Ok(Some(Expansion {
                    text: expansion.text,
                    agent: expansion.agent,
                    model: expansion.model,
                    description: expansion.description,
                    subagent: expansion.subagent,
                }))
            }
            _ => Ok(None),
        }
    }

    pub fn files(&self) -> Vec<String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => recode_tool::list_files(&engine.project.root, 5_000),
            (_, Some(remote)) => remote.cache.read().unwrap().files.clone(),
            _ => Vec::new(),
        }
    }

    pub async fn sessions(&self) -> Result<Vec<SessionInfo>, String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.sessions(true),
            (_, Some(remote)) => {
                let sessions = remote.client.sessions(true, true).await?;
                remote.cache.write().unwrap().sessions = sessions.clone();
                Ok(sessions)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Reload the visible session; for remote clients this is the polling
    /// refresh. `Poll` carries pending interactive requests so the caller can
    /// keep its modals in sync.
    pub async fn poll(&self, id: &recode_schema::SessionId) -> Result<Poll, String> {
        let (Some(remote), None) = (&self.remote, &self.local) else {
            return Ok(Poll::default());
        };
        let snapshot = remote.client.session(&id.0).await?;
        let permissions = remote
            .client
            .pending_permissions(&id.0)
            .await
            .unwrap_or_default();
        let questions = remote
            .client
            .pending_questions(&id.0)
            .await
            .unwrap_or_default();
        // While the event stream is connected the transcript is kept current
        // by the events themselves; refetching would race with them. The
        // transcript is refetched when the run is over, when the stream is
        // down, or when an event could not be folded in.
        let connected = remote.connected.load(Ordering::Acquire);
        let stale = remote.cache.read().unwrap().stale;
        if !snapshot.busy || !connected || stale {
            let (messages, last_seq) = remote.client.messages_with_seq(&id.0).await?;
            let mut cache = remote.cache.write().unwrap();
            // Never replace a transcript that already includes newer events.
            if last_seq >= cache.last_seq {
                cache.messages = messages;
                cache.last_seq = last_seq;
                cache.stale = false;
            }
            cache.session = snapshot.session;
            cache.busy = snapshot.busy;
        }
        Ok(Poll {
            permissions,
            questions,
        })
    }

    /// Switch the visible session (used by the session picker).
    pub async fn open_session(&self, id: &recode_schema::SessionId) -> Result<SessionInfo, String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine
                .session(id)?
                .ok_or_else(|| "session not found".to_owned()),
            (_, Some(remote)) => {
                // Subscribe before the snapshot so no event is missed, then
                // replace the reader of the previous session.
                let stream = remote.client.events(&id.0).await.ok();
                let snapshot = remote.client.session(&id.0).await?;
                let (messages, last_seq) = remote.client.messages_with_seq(&id.0).await?;
                let generation = remote.generation.fetch_add(1, Ordering::AcqRel) + 1;
                {
                    let mut cache = remote.cache.write().unwrap();
                    cache.session = snapshot.session.clone();
                    cache.messages = messages;
                    cache.busy = snapshot.busy;
                    cache.last_seq = last_seq;
                    cache.stale = false;
                }
                remote.connected.store(stream.is_some(), Ordering::Release);
                if let Some(stream) = stream {
                    spawn_event_reader(remote.clone(), stream, id.0.clone(), generation);
                }
                Ok(snapshot.session)
            }
            _ => Err("no backend".into()),
        }
    }

    pub async fn create_session(
        &self,
        agent: Option<String>,
        model: Option<String>,
        title: Option<String>,
    ) -> Result<SessionInfo, String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => {
                let model = match model {
                    Some(model) => Some(engine.config.resolve_model(Some(&model))?),
                    None => None,
                };
                engine.create_session(agent, model, title)
            }
            (_, Some(remote)) => {
                let session = remote.client.create_session(agent, model, title).await?;
                remote.cache.write().unwrap().session = session.clone();
                Ok(session)
            }
            _ => Err("no backend".into()),
        }
    }

    pub async fn prompt(&self, id: &recode_schema::SessionId, text: String) -> Result<(), String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.prompt(id, text, Vec::new()).await,
            (_, Some(remote)) => remote.client.prompt(&id.0, &text).await,
            _ => Err("no backend".into()),
        }
    }

    pub async fn abort(&self, id: &recode_schema::SessionId) -> bool {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.abort(id),
            (_, Some(remote)) => remote.client.abort(&id.0).await.unwrap_or(false),
            _ => false,
        }
    }

    pub async fn fork(
        &self,
        id: &recode_schema::SessionId,
        title: Option<String>,
    ) -> Result<SessionInfo, String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.fork_session(id, title),
            (_, Some(remote)) => {
                let session = remote.client.fork(&id.0, title).await?;
                remote.cache.write().unwrap().session = session.clone();
                Ok(session)
            }
            _ => Err("no backend".into()),
        }
    }

    /// Remove messages after `message`; `files` also restores the pre-prompt
    /// worktree snapshot.
    pub async fn revert(
        &self,
        id: &recode_schema::SessionId,
        message: &recode_schema::MessageId,
        files: bool,
    ) -> Result<usize, String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => {
                let removed = engine.revert(id, message)?;
                if files {
                    if let Err(error) = engine.restore_files(id, Some(message)).await {
                        return Err(format!(
                            "removed {removed} message(s); files not restored: {error}"
                        ));
                    }
                }
                Ok(removed)
            }
            (_, Some(remote)) => {
                let response = remote.client.revert(&id.0, &message.0, files).await?;
                Ok(response.removed)
            }
            _ => Err("no backend".into()),
        }
    }

    pub async fn set_model(
        &self,
        id: &recode_schema::SessionId,
        model: &str,
    ) -> Result<(), String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => {
                let model = engine.config.resolve_model(Some(model))?;
                engine.set_model(id, model)?;
                Ok(())
            }
            (_, Some(remote)) => remote.client.set_model(&id.0, model).await,
            _ => Err("no backend".into()),
        }
    }

    pub async fn set_agent(
        &self,
        id: &recode_schema::SessionId,
        agent: &str,
    ) -> Result<(), String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => engine.set_agent(id, agent),
            (_, Some(remote)) => remote.client.set_agent(&id.0, agent).await,
            _ => Err("no backend".into()),
        }
    }

    pub async fn compact(
        &self,
        id: &recode_schema::SessionId,
    ) -> Result<Option<(u64, u64)>, String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => Ok(engine
                .compact(id)
                .await?
                .map(|record| (record.tokens_before, record.tokens_after))),
            (_, Some(remote)) => {
                let summary = remote.client.compact(&id.0).await?;
                Ok(if summary.compacted {
                    Some((
                        summary.tokens_before.unwrap_or(0),
                        summary.tokens_after.unwrap_or(0),
                    ))
                } else {
                    None
                })
            }
            _ => Err("no backend".into()),
        }
    }

    pub async fn reply_permission(&self, id: &str, reply: PermissionReply) -> Result<(), String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => {
                if engine.permissions.reply(id, reply) {
                    Ok(())
                } else {
                    Err("permission request is no longer pending".into())
                }
            }
            (_, Some(remote)) => remote.client.reply_permission(id, reply).await,
            _ => Err("no backend".into()),
        }
    }

    pub async fn reply_question(&self, id: &str, reply: QuestionReply) -> Result<(), String> {
        match (&self.local, &self.remote) {
            (Some(engine), _) => {
                if engine.questions.reply(id, reply) {
                    Ok(())
                } else {
                    Err("question is no longer pending".into())
                }
            }
            (_, Some(remote)) => remote.client.reply_question(id, reply).await,
            _ => Err("no backend".into()),
        }
    }

    /// Expand `@path` references using the machine that owns the worktree.
    /// Local mode reads disk; attach mode asks the server, so the TUI can run
    /// anywhere.
    pub async fn expand_file_references(&self, text: String) -> String {
        let mut seen = std::collections::HashSet::new();
        let mut attachments = String::new();
        let mut total = 0usize;
        for token in text.split_whitespace() {
            let Some(name) = token.strip_prefix('@') else {
                continue;
            };
            let name = name.trim_end_matches(|character: char| ".,;)]".contains(character));
            if name.is_empty() || !seen.insert(name.to_owned()) {
                continue;
            }
            let content = match &self.local {
                Some(_) => read_local(&self.root(), name),
                None => match &self.remote {
                    Some(remote) => remote.client.file_content(name).await.ok(),
                    None => None,
                },
            };
            let Some(content) = content else {
                continue;
            };
            if total + content.len() > 512 * 1024 {
                break;
            }
            total += content.len();
            attachments.push_str(&format!("\n\n<file path=\"{name}\">\n{content}\n</file>"));
        }
        if attachments.is_empty() {
            text
        } else {
            format!("{text}{attachments}")
        }
    }
}

fn read_local(root: &Path, name: &str) -> Option<String> {
    let path = root.join(name).canonicalize().ok()?;
    if !recode_tool::is_within(
        &recode_tool::canonicalish(&path),
        &recode_tool::canonicalish(root),
    ) {
        return None;
    }
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    (!content.contains('\0')).then_some(content)
}
