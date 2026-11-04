use crate::input::Input;
use crate::AppEvent;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use recode_core::engine::Engine;
use recode_schema::event::{DeltaKind, Event, LiveEvent, SessionInfo, SessionStatus};
use recode_schema::message::Message;
use recode_schema::part::Part;
use recode_schema::permission::{PermissionReply, PermissionRequest};
use recode_schema::question::{QuestionReply, QuestionRequest};
use recode_schema::Role;
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
    Question,
}

#[derive(Clone, Debug)]
pub struct PickerItem {
    pub label: String,
    pub value: String,
}

/// Inline `@path` completion state.
#[derive(Clone, Debug)]
pub struct Completion {
    /// Character index of the `@` in the current input line.
    pub start: usize,
    /// Text typed after `@`, used for filtering.
    pub prefix: String,
    pub items: Vec<String>,
    pub index: usize,
}

struct PendingPermission {
    request: PermissionRequest,
    reply: oneshot::Sender<PermissionReply>,
}

/// Progress through a multi-question request. The answers collected so far are
/// only submitted when the last question is confirmed.
#[derive(Clone, Debug)]
pub struct QuestionProgress {
    pub index: usize,
    pub cursor: usize,
    pub selected: Vec<usize>,
    pub answers: Vec<Vec<String>>,
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
    pub picker_filter: String,
    pub should_quit: bool,
    pub scroll_back: usize,
    pub notice: Option<(String, u16)>,
    pub tick: u64,
    history: Vec<String>,
    history_pos: Option<usize>,
    pub show_reasoning: bool,
    pending: Vec<PendingPermission>,
    pub pending_questions: Vec<QuestionRequest>,
    pub question_state: Option<QuestionProgress>,
    pub completion: Option<Completion>,
    file_cache: Vec<String>,
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
            picker_filter: String::new(),
            should_quit: false,
            scroll_back: 0,
            notice: None,
            tick: 0,
            history: Vec::new(),
            history_pos: None,
            show_reasoning: true,
            pending: Vec::new(),
            pending_questions: Vec::new(),
            question_state: None,
            completion: None,
            file_cache: Vec::new(),
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
            LiveEvent::QuestionAsked(request) => {
                if !self
                    .pending_questions
                    .iter()
                    .any(|pending| pending.id == request.id)
                {
                    self.pending_questions.push(request);
                }
                if self.question_state.is_none() {
                    self.activate_question();
                }
            }
            LiveEvent::QuestionReplied { .. } => {}
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

        if self.completion.is_some() {
            match key.code {
                KeyCode::Up => {
                    if let Some(completion) = &mut self.completion {
                        completion.index = completion.index.saturating_sub(1);
                    }
                    return;
                }
                KeyCode::Down => {
                    if let Some(completion) = &mut self.completion {
                        completion.index =
                            (completion.index + 1).min(completion.items.len().saturating_sub(1));
                    }
                    return;
                }
                KeyCode::Tab | KeyCode::Enter => {
                    self.accept_completion();
                    return;
                }
                KeyCode::Esc => {
                    self.completion = None;
                    return;
                }
                _ => {}
            }
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
            KeyCode::F(1) => {
                self.completion = None;
                self.overlay = Overlay::Help;
            }
            KeyCode::F(2) => self.open_model_picker(),
            KeyCode::F(3) => self.open_agent_picker(),
            KeyCode::F(4) => self.open_session_picker(),
            KeyCode::Char('r') if ctrl => self.show_reasoning = !self.show_reasoning,
            KeyCode::Char(character) if !ctrl && !alt => self.input.insert_char(character),
            _ => {}
        }
        self.refresh_completion();
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.overlay == Overlay::None {
            self.input.insert_str(&text.replace("\r\n", "\n"));
            self.refresh_completion();
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
            Overlay::Model | Overlay::Agent => match key.code {
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
            Overlay::Sessions => match key.code {
                KeyCode::Esc => self.overlay = Overlay::None,
                KeyCode::Up => {
                    self.picker_index = self.picker_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    let count = self.visible_picker().len();
                    if count > 0 {
                        self.picker_index = (self.picker_index + 1).min(count - 1);
                    }
                }
                KeyCode::Backspace => {
                    self.picker_filter.pop();
                    self.picker_index = 0;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.picker_filter.clear();
                    self.picker_index = 0;
                }
                KeyCode::Enter => self.apply_picker().await,
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.picker_filter.push(character);
                    self.picker_index = 0;
                }
                _ => {}
            },
            Overlay::Permission => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.reply_permission(PermissionReply::Once),
                KeyCode::Char('a') => self.reply_permission(PermissionReply::Always),
                KeyCode::Char('n') | KeyCode::Esc => self.reply_permission(PermissionReply::Reject),
                _ => {}
            },
            Overlay::Question => self.on_question_key(key),
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

    pub fn current_question(&self) -> Option<(&QuestionRequest, &QuestionProgress)> {
        let request = self.pending_questions.first()?;
        let progress = self.question_state.as_ref()?;
        Some((request, progress))
    }

    fn activate_question(&mut self) {
        if self.pending_questions.is_empty() {
            self.question_state = None;
            if self.overlay == Overlay::Question {
                self.overlay = Overlay::None;
            }
            return;
        }
        self.question_state = Some(QuestionProgress {
            index: 0,
            cursor: 0,
            selected: Vec::new(),
            answers: Vec::new(),
        });
        self.overlay = Overlay::Question;
    }

    fn on_question_key(&mut self, key: KeyEvent) {
        let Some(request) = self.pending_questions.first().cloned() else {
            self.activate_question();
            return;
        };
        let Some(mut progress) = self.question_state.take() else {
            self.activate_question();
            return;
        };
        let Some(question) = request.questions.get(progress.index) else {
            self.finish_question(&request, QuestionReply::Reject);
            return;
        };
        let option_count = question.options.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                self.finish_question(&request, QuestionReply::Reject);
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                progress.cursor = progress.cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if option_count > 0 {
                    progress.cursor = (progress.cursor + 1).min(option_count - 1);
                }
            }
            KeyCode::Char(' ') => {
                if question.multiple {
                    if let Some(position) = progress
                        .selected
                        .iter()
                        .position(|selected| *selected == progress.cursor)
                    {
                        progress.selected.remove(position);
                    } else {
                        progress.selected.push(progress.cursor);
                        progress.selected.sort_unstable();
                    }
                } else {
                    progress.selected = vec![progress.cursor];
                }
            }
            KeyCode::Char(character) if character.is_ascii_digit() => {
                let digit = character.to_digit(10).unwrap_or(1) as usize;
                if digit >= 1 && digit <= option_count {
                    progress.cursor = digit - 1;
                }
            }
            KeyCode::Enter => {
                let mut indices = if question.multiple && !progress.selected.is_empty() {
                    progress.selected.clone()
                } else {
                    vec![progress.cursor]
                };
                indices.retain(|index| *index < option_count);
                if indices.is_empty() {
                    self.set_notice("no option available — press Esc to decline");
                    self.question_state = Some(progress);
                    return;
                }
                let labels = indices
                    .into_iter()
                    .filter_map(|index| question.options.get(index))
                    .map(|option| option.label.clone())
                    .collect::<Vec<_>>();
                progress.answers.push(labels);
                if progress.index + 1 < request.questions.len() {
                    progress.index += 1;
                    progress.cursor = 0;
                    progress.selected.clear();
                    self.question_state = Some(progress);
                } else {
                    let answers = progress.answers.clone();
                    self.finish_question(&request, QuestionReply::Answer { answers });
                }
                return;
            }
            _ => {}
        }
        self.question_state = Some(progress);
    }

    fn finish_question(&mut self, request: &QuestionRequest, reply: QuestionReply) {
        let _ = self.engine.questions.reply(&request.id, reply);
        if let Some(position) = self
            .pending_questions
            .iter()
            .position(|pending| pending.id == request.id)
        {
            self.pending_questions.remove(position);
        }
        self.activate_question();
    }

    async fn submit(&mut self) {
        self.completion = None;
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
        self.file_cache.clear();
        let text = expand_file_references(&self.engine.project.root, text);
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

    /// Recompute the `@file` menu for the token under the cursor.
    fn refresh_completion(&mut self) {
        let Some((start, prefix)) = self.completion_token() else {
            self.completion = None;
            return;
        };
        if self.file_cache.is_empty() {
            self.file_cache = recode_tool::list_files(&self.engine.project.root, 5_000);
        }
        let needle = prefix.to_ascii_lowercase();
        let mut items: Vec<String> = self
            .file_cache
            .iter()
            .filter(|path| path.to_ascii_lowercase().starts_with(&needle))
            .take(50)
            .cloned()
            .collect();
        if items.is_empty() && !needle.is_empty() {
            items = self
                .file_cache
                .iter()
                .filter(|path| path.to_ascii_lowercase().contains(&needle))
                .take(50)
                .cloned()
                .collect();
        }
        if items.is_empty() {
            self.completion = None;
            return;
        }
        self.completion = Some(Completion {
            start,
            prefix,
            items,
            index: 0,
        });
    }

    /// The `@` token that ends at the cursor, if any: `(at_char_index, prefix)`.
    fn completion_token(&self) -> Option<(usize, String)> {
        let line = self.input.lines.get(self.input.row)?;
        at_token(line, self.input.col)
    }

    fn accept_completion(&mut self) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let Some(item) = completion.items.get(completion.index).cloned() else {
            return;
        };
        let line_len = self
            .input
            .lines
            .get(self.input.row)
            .map(|line| line.chars().count())
            .unwrap_or(0);
        let trailing = if self.input.col >= line_len { " " } else { "" };
        let text = format!("@{item}{trailing}");
        self.input
            .replace_token(completion.start, self.input.col, &text);
    }

    /// Picker rows after the session search filter is applied.
    pub fn visible_picker(&self) -> Vec<PickerItem> {
        if self.overlay == Overlay::Sessions && !self.picker_filter.trim().is_empty() {
            let needle = self.picker_filter.to_lowercase();
            self.picker
                .iter()
                .filter(|item| {
                    item.label.to_lowercase().contains(&needle)
                        || item.value.to_lowercase().contains(&needle)
                })
                .cloned()
                .collect()
        } else {
            self.picker.clone()
        }
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
            "/fork" => match self.engine.fork_session(&self.session.id, None) {
                Ok(session) => {
                    self.session = session;
                    self.reload();
                    self.scroll_back = 0;
                    self.set_notice("forked session");
                }
                Err(error) => self.set_notice(error),
            },
            "/undo" => {
                let last_user = self
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.info.role == Role::User)
                    .map(|message| message.info.id.clone());
                match last_user {
                    Some(message_id) => match self.engine.revert(&self.session.id, &message_id) {
                        Ok(removed) => {
                            let files = self
                                .engine
                                .restore_files(&self.session.id, Some(&message_id))
                                .await;
                            self.reload();
                            match files {
                                Ok(_) => self.set_notice(format!(
                                    "undid {removed} message(s) and restored files"
                                )),
                                Err(error) => self.set_notice(format!(
                                    "removed {removed} message(s); files not restored: {error}"
                                )),
                            }
                        }
                        Err(error) => self.set_notice(error),
                    },
                    None => self.set_notice("nothing to undo"),
                }
            }
            "/compact" => {
                self.set_notice("compacting…");
                match self.engine.compact(&self.session.id).await {
                    Ok(Some(record)) => {
                        self.reload();
                        self.set_notice(format!(
                            "compacted: {} → {} tokens",
                            record.tokens_before, record.tokens_after
                        ));
                    }
                    Ok(None) => self.set_notice("nothing to compact"),
                    Err(error) => self.set_notice(error),
                }
            }
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
        self.picker_filter.clear();
        self.completion = None;
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
        self.picker_filter.clear();
        self.completion = None;
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
        self.picker_filter.clear();
        self.completion = None;
        self.overlay = Overlay::Sessions;
    }

    async fn apply_picker(&mut self) {
        let items = self.visible_picker();
        let Some(item) = items.get(self.picker_index).cloned() else {
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

fn at_token(line: &str, cursor: usize) -> Option<(usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut start = cursor;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    if start >= cursor || chars.get(start) != Some(&'@') {
        return None;
    }
    Some((start, chars[start + 1..cursor].iter().collect()))
}

/// Append the contents of files referenced as `@relative/path`. Paths must
/// resolve inside the project; unreadable, binary and oversized files are
/// ignored. The total expansion is capped so one prompt cannot exhaust the
/// context window.
fn expand_file_references(root: &std::path::Path, text: String) -> String {
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
        let Ok(path) = root.join(name).canonicalize() else {
            continue;
        };
        if !recode_tool::is_within(
            &recode_tool::canonicalish(&path),
            &recode_tool::canonicalish(root),
        ) {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > 256 * 1024 {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if content.contains('\0') {
            continue;
        }
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

fn format_time(timestamp: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(timestamp)
        .single()
        .map(|time| time.format("%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_at_token_under_the_cursor() {
        assert_eq!(at_token("see @src/ma", 11), Some((4, "src/ma".to_owned())));
        assert_eq!(at_token("mail@host", 9), None);
        assert_eq!(at_token("plain", 5), None);
        assert_eq!(at_token("@a", 0), None);
    }

    #[test]
    fn expands_existing_file_references_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let expanded = expand_file_references(dir.path(), "read @a.txt".into());
        assert!(
            expanded.contains("<file path=\"a.txt\">\nhello\n</file>"),
            "expanded: {expanded:?}"
        );
        let unchanged = expand_file_references(dir.path(), "read @missing.txt".into());
        assert_eq!(unchanged, "read @missing.txt");
    }
}
