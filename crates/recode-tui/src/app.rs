use crate::input::Input;
use crate::AppEvent;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use recode_core::engine::Engine;
use recode_schema::event::{DeltaKind, Event, LiveEvent, SessionInfo, SessionStatus};
use recode_schema::message::Message;
use recode_schema::part::Part;
use recode_schema::permission::{PermissionReply, PermissionRequest};
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    Model,
    Agent,
    Sessions,
    Permission,
}

#[derive(Clone, Debug)]
pub struct PickerItem {
    pub label: String,
    pub value: String,
}

struct PendingPermission {
    request: PermissionRequest,
    reply: oneshot::Sender<PermissionReply>,
}

pub struct App {
    pub engine: Arc<Engine>,
    pub session: SessionInfo,
    pub messages: Vec<Message>,
    pub input: Input,
    pub status: SessionStatus,
    pub overlay: Overlay,
    pub picker: Vec<PickerItem>,
    pub picker_index: usize,
    pub should_quit: bool,
    pub scroll_back: usize,
    pub notice: Option<(String, u16)>,
    pub tick: u64,
    history: Vec<String>,
    history_pos: Option<usize>,
    pub show_reasoning: bool,
    pending: Vec<PendingPermission>,
}

impl App {
    pub fn new(engine: Arc<Engine>, session_id: recode_schema::SessionId) -> Result<Self, String> {
        let session = engine
            .session(&session_id)?
            .ok_or_else(|| "session not found".to_owned())?;
        let messages = engine.messages(&session_id)?;
        let mut app = Self {
            engine,
            session,
            messages,
            input: Input::new(),
            status: SessionStatus::Idle,
            overlay: Overlay::None,
            picker: Vec::new(),
            picker_index: 0,
            should_quit: false,
            scroll_back: 0,
            notice: None,
            tick: 0,
            history: Vec::new(),
            history_pos: None,
            show_reasoning: true,
            pending: Vec::new(),
        };
        if app.engine.is_busy(&app.session.id) {
            app.status = SessionStatus::Busy;
        }
        Ok(app)
    }

    pub fn reload(&mut self) {
        match self.engine.messages(&self.session.id) {
            Ok(messages) => self.messages = messages,
            Err(error) => self.set_notice(error),
        }
        if let Ok(Some(session)) = self.engine.session(&self.session.id) {
            self.session = session;
        }
    }

    pub fn set_notice(&mut self, message: impl Into<String>) {
        self.notice = Some((message.into(), 70));
    }

    pub fn usage(&self) -> recode_schema::llm::Usage {
        let mut usage = recode_schema::llm::Usage::default();
        for message in &self.messages {
            for part in &message.parts {
                if let Part::StepFinish(finish) = part {
                    usage.merge(&finish.tokens);
                }
            }
        }
        usage
    }

    pub fn model_label(&self) -> String {
        self.session
            .model
            .as_ref()
            .map(|model| model.qualified())
            .unwrap_or_else(|| "no model".to_owned())
    }

    pub fn agent_label(&self) -> String {
        self.session
            .agent
            .clone()
            .unwrap_or_else(|| self.engine.config.default_agent())
    }

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if let Some((_, remaining)) = &mut self.notice {
            if *remaining == 0 {
                self.notice = None;
            } else {
                *remaining -= 1;
            }
        }
    }

    pub fn on_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Permission { request, reply } => {
                self.pending.push(PendingPermission { request, reply });
                if self.pending.len() == 1 {
                    self.overlay = Overlay::Permission;
                }
            }
        }
    }

    pub fn on_live(&mut self, event: LiveEvent) {
        match event {
            LiveEvent::Delta {
                session_id,
                message_id,
                part_id,
                kind,
                delta,
            } if session_id == self.session.id => {
                let mut applied = false;
                if let Some(message) = self
                    .messages
                    .iter_mut()
                    .find(|message| message.info.id == message_id)
                {
                    if let Some(part) = message.parts.iter_mut().find(|part| part.id() == &part_id)
                    {
                        match kind {
                            DeltaKind::Text => {
                                if let Some(text) = part.text_mut() {
                                    text.text.push_str(&delta);
                                    applied = true;
                                }
                            }
                            DeltaKind::Reasoning => {
                                if let Some(reasoning) = part.reasoning_mut() {
                                    reasoning.text.push_str(&delta);
                                    applied = true;
                                }
                            }
                        }
                    }
                }
                if !applied {
                    self.reload();
                }
            }
            LiveEvent::Durable(envelope) => {
                if envelope.session_id != self.session.id {
                    return;
                }
                match envelope.event {
                    Event::SessionUpdated { session } => {
                        self.session = session;
                    }
                    Event::PartDelta { .. } => {
                        // Live deltas already carry the text; the durable copy
                        // is reconciled on idle.
                    }
                    Event::SessionIdle { .. } => {
                        self.reload();
                    }
                    Event::SessionError { error, .. } => {
                        self.set_notice(error.message);
                        self.reload();
                    }
                    Event::SessionDeleted { .. } => {
                        self.should_quit = true;
                    }
                    _ => self.reload(),
                }
            }
            LiveEvent::SessionStatus { session_id, status } if session_id == self.session.id => {
                self.status = status;
                if status == SessionStatus::Idle {
                    self.reload();
                }
            }
            LiveEvent::PermissionAsked(_) | LiveEvent::PermissionReplied { .. } => {}
            _ => {}
        }
    }

    pub async fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if ctrl && key.code == KeyCode::Char('c') {
            if self.engine.is_busy(&self.session.id) {
                self.engine.abort(&self.session.id);
                self.set_notice("aborting…");
            } else {
                self.should_quit = true;
            }
            return;
        }

        if self.overlay != Overlay::None {
            self.on_overlay_key(key).await;
            return;
        }

        match key.code {
            KeyCode::Esc => {
                if self.engine.is_busy(&self.session.id) {
                    self.engine.abort(&self.session.id);
                    self.set_notice("aborting…");
                } else {
                    self.input.clear();
                }
            }
            KeyCode::Enter if shift || alt => self.input.newline(),
            KeyCode::Enter => self.submit().await,
            KeyCode::Char('j') if ctrl => self.input.newline(),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Up => {
                if self.input.is_empty() {
                    self.history_previous();
                } else {
                    self.input.up();
                }
            }
            KeyCode::Down => {
                if self.input.is_empty() {
                    self.history_next();
                } else {
                    self.input.down();
                }
            }
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Tab => self.input.insert_str("  "),
            KeyCode::PageUp => self.scroll_back = self.scroll_back.saturating_add(8),
            KeyCode::PageDown => self.scroll_back = self.scroll_back.saturating_sub(8),
            KeyCode::F(1) => self.overlay = Overlay::Help,
            KeyCode::F(2) => self.open_model_picker(),
            KeyCode::F(3) => self.open_agent_picker(),
            KeyCode::F(4) => self.open_session_picker(),
            KeyCode::Char('r') if ctrl => self.show_reasoning = !self.show_reasoning,
            KeyCode::Char(character) if !ctrl && !alt => self.input.insert_char(character),
            _ => {}
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.overlay == Overlay::None {
            self.input.insert_str(&text.replace("\r\n", "\n"));
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_back = self.scroll_back.saturating_add(3),
            MouseEventKind::ScrollDown => self.scroll_back = self.scroll_back.saturating_sub(3),
            _ => {}
        }
    }

    async fn on_overlay_key(&mut self, key: KeyEvent) {
        match self.overlay {
            Overlay::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.overlay = Overlay::None;
                }
            }
            Overlay::Model | Overlay::Agent | Overlay::Sessions => match key.code {
                KeyCode::Esc => self.overlay = Overlay::None,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.picker_index = self.picker_index.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !self.picker.is_empty() {
                        self.picker_index = (self.picker_index + 1).min(self.picker.len() - 1);
                    }
                }
                KeyCode::Enter => self.apply_picker().await,
                _ => {}
            },
            Overlay::Permission => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.reply_permission(PermissionReply::Once),
                KeyCode::Char('a') => self.reply_permission(PermissionReply::Always),
                KeyCode::Char('n') | KeyCode::Esc => self.reply_permission(PermissionReply::Reject),
                _ => {}
            },
            Overlay::None => {}
        }
    }

    fn reply_permission(&mut self, reply: PermissionReply) {
        if !self.pending.is_empty() {
            let pending = self.pending.remove(0);
            let _ = pending.reply.send(reply);
        }
        self.overlay = if self.pending.is_empty() {
            Overlay::None
        } else {
            Overlay::Permission
        };
    }

    pub fn current_permission(&self) -> Option<&PermissionRequest> {
        self.pending.first().map(|pending| &pending.request)
    }

    async fn submit(&mut self) {
        let text = self.input.take();
        if text.trim().is_empty() {
            self.input.clear();
            return;
        }
        if text.starts_with('/') {
            self.handle_command(text.trim()).await;
            return;
        }
        if self.engine.is_busy(&self.session.id) {
            self.set_notice("session is busy — press Esc to abort");
            self.input.set_text(&text);
            return;
        }
        self.history.push(text.clone());
        self.history_pos = None;
        self.status = SessionStatus::Busy;
        if let Err(error) = self
            .engine
            .prompt(&self.session.id, text.clone(), Vec::new())
            .await
        {
            self.set_notice(error);
            self.input.set_text(&text);
            self.status = SessionStatus::Idle;
            return;
        }
        self.scroll_back = 0;
        self.reload();
    }

    async fn handle_command(&mut self, command: &str) {
        let (name, argument) = match command.split_once(char::is_whitespace) {
            Some((name, argument)) => (name, argument.trim()),
            None => (command, ""),
        };
        match name {
            "/quit" | "/q" => self.should_quit = true,
            "/help" | "/?" => self.overlay = Overlay::Help,
            "/new" => {
                let agent = self.session.agent.clone();
                match self
                    .engine
                    .create_session(agent, self.session.model.clone(), None)
                {
                    Ok(session) => {
                        self.session = session;
                        self.reload();
                        self.scroll_back = 0;
                        self.set_notice("new session");
                    }
                    Err(error) => self.set_notice(error),
                }
            }
            "/model" if argument.is_empty() => self.open_model_picker(),
            "/model" => match self.engine.config.resolve_model(Some(argument)) {
                Ok(model) => {
                    let label = model.qualified();
                    if let Err(error) = self.engine.set_model(&self.session.id, model) {
                        self.set_notice(error);
                    } else {
                        self.reload();
                        self.set_notice(format!("model: {label}"));
                    }
                }
                Err(error) => self.set_notice(error),
            },
            "/agent" if argument.is_empty() => self.open_agent_picker(),
            "/agent" => {
                if self.engine.agents.get(argument).is_none() {
                    self.set_notice(format!("unknown agent {argument:?}"));
                } else if let Err(error) = self.engine.set_agent(&self.session.id, argument) {
                    self.set_notice(error);
                } else {
                    self.reload();
                    self.set_notice(format!("agent: {argument}"));
                }
            }
            "/sessions" => self.open_session_picker(),
            "/abort" => {
                if self.engine.abort(&self.session.id) {
                    self.set_notice("aborting…");
                }
            }
            "/reasoning" => {
                self.show_reasoning = !self.show_reasoning;
                self.set_notice(if self.show_reasoning {
                    "reasoning shown"
                } else {
                    "reasoning hidden"
                });
            }
            "/clear" => {
                self.input.clear();
                self.set_notice("input cleared");
            }
            other => self.set_notice(format!("unknown command {other}; press F1 for help")),
        }
    }

    fn open_model_picker(&mut self) {
        let current = self.session.model.as_ref().map(|model| model.qualified());
        let mut items = Vec::new();
        for provider in self.engine.config.providers() {
            for model in provider.models {
                let value = format!("{}/{}", provider.id, model);
                let marker = if current.as_deref() == Some(value.as_str()) {
                    "●"
                } else {
                    " "
                };
                let status = if provider.configured {
                    ""
                } else {
                    "  (no key)"
                };
                items.push(PickerItem {
                    label: format!("{marker} {value}{status}"),
                    value,
                });
            }
        }
        if items.is_empty() {
            self.set_notice("no models configured; edit recode.json or run `recode auth login`");
            return;
        }
        self.picker = items;
        self.picker_index = self
            .picker
            .iter()
            .position(|item| item.label.starts_with('●'))
            .unwrap_or(0);
        self.overlay = Overlay::Model;
    }

    fn open_agent_picker(&mut self) {
        let current = self.agent_label();
        self.picker = self
            .engine
            .agents
            .list()
            .map(|agent| PickerItem {
                label: format!(
                    "{} {:<10} {}",
                    if agent.id == current { "●" } else { " " },
                    agent.id,
                    agent.description
                ),
                value: agent.id.clone(),
            })
            .collect();
        self.picker_index = self
            .picker
            .iter()
            .position(|item| item.label.starts_with('●'))
            .unwrap_or(0);
        self.overlay = Overlay::Agent;
    }

    fn open_session_picker(&mut self) {
        let sessions = match self.engine.sessions(true) {
            Ok(sessions) => sessions,
            Err(error) => {
                self.set_notice(error);
                return;
            }
        };
        let current = self.session.id.clone();
        self.picker = sessions
            .into_iter()
            .map(|session| PickerItem {
                label: format!(
                    "{} {}  ·  {}  ·  {}",
                    if session.id == current { "●" } else { " " },
                    session.display_title(),
                    &session.id.0[..session.id.0.len().min(12)],
                    format_time(session.updated_at)
                ),
                value: session.id.0.clone(),
            })
            .collect();
        self.picker_index = self
            .picker
            .iter()
            .position(|item| item.label.starts_with('●'))
            .unwrap_or(0);
        self.overlay = Overlay::Sessions;
    }

    async fn apply_picker(&mut self) {
        let Some(item) = self.picker.get(self.picker_index).cloned() else {
            self.overlay = Overlay::None;
            return;
        };
        match self.overlay {
            Overlay::Model => match self.engine.config.resolve_model(Some(&item.value)) {
                Ok(model) => {
                    if let Err(error) = self.engine.set_model(&self.session.id, model) {
                        self.set_notice(error);
                    } else {
                        self.reload();
                        self.set_notice(format!("model: {}", item.value));
                    }
                }
                Err(error) => self.set_notice(error),
            },
            Overlay::Agent => {
                if let Err(error) = self.engine.set_agent(&self.session.id, item.value.clone()) {
                    self.set_notice(error);
                } else {
                    self.reload();
                    self.set_notice(format!("agent: {}", item.value));
                }
            }
            Overlay::Sessions => {
                let id = recode_schema::SessionId::from_existing(item.value.clone());
                match self.engine.session(&id) {
                    Ok(Some(session)) => {
                        self.session = session;
                        self.messages = self.engine.messages(&id).unwrap_or_default();
                        self.scroll_back = 0;
                    }
                    Ok(None) => self.set_notice("session not found"),
                    Err(error) => self.set_notice(error),
                }
            }
            _ => {}
        }
        self.overlay = Overlay::None;
    }

    fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_pos {
            Some(0) => 0,
            Some(index) => index - 1,
            None => self.history.len() - 1,
        };
        self.history_pos = Some(next);
        self.input.set_text(&self.history[next]);
    }

    fn history_next(&mut self) {
        let Some(index) = self.history_pos else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_pos = None;
            self.input.clear();
            return;
        }
        let next = index + 1;
        self.history_pos = Some(next);
        self.input.set_text(&self.history[next]);
    }
}

fn format_time(timestamp: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(timestamp)
        .single()
        .map(|time| time.format("%m-%d %H:%M").to_string())
        .unwrap_or_default()
}
