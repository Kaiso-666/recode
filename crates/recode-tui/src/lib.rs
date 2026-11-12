//! Re:Code terminal UI.
//!
//! The TUI is a client of a session engine. It owns no domain state of its own
//! beyond presentation: with a local engine transcripts come from the event
//! store and deltas from the bus; with `--server` the same view is polled from
//! the HTTP API.

pub mod app;
pub mod backend;
pub mod input;
pub mod theme;
pub mod ui;

pub use app::{App, Overlay};
pub use backend::{AgentView, Backend};
pub use theme::Theme;

use async_trait::async_trait;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use recode_core::permission::PermissionResponder;
use recode_schema::permission::{PermissionReply, PermissionRequest};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub enum AppEvent {
    Permission {
        request: PermissionRequest,
        reply: oneshot::Sender<PermissionReply>,
    },
}

/// Bridges the engine's permission checks into the UI thread's modals.
pub struct TuiResponder {
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<AppEvent>>>,
}

impl TuiResponder {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }

    pub fn take_receiver(&self) -> Option<mpsc::UnboundedReceiver<AppEvent>> {
        self.rx.lock().ok().and_then(|mut receiver| receiver.take())
    }
}

impl Default for TuiResponder {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PermissionResponder for TuiResponder {
    async fn ask(&self, request: PermissionRequest) -> PermissionReply {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .tx
            .send(AppEvent::Permission {
                request,
                reply: reply_tx,
            })
            .is_err()
        {
            return PermissionReply::Reject;
        }
        reply_rx.await.unwrap_or(PermissionReply::Reject)
    }
}

pub async fn run(
    backend: Backend,
    responder: Option<Arc<TuiResponder>>,
    session_id: recode_schema::SessionId,
    theme: Theme,
) -> anyhow::Result<()> {
    let (keep_tx, keep_rx) = mpsc::unbounded_channel();
    let mut app_events = match responder.and_then(|responder| responder.take_receiver()) {
        Some(receiver) => receiver,
        None => keep_rx,
    };
    // `keep_tx` keeps the fallback channel open for the lifetime of the loop.
    let _keep_tx = keep_tx;
    let mut bus = backend.subscribe();
    let mut app = App::new(backend, session_id, theme).map_err(anyhow::Error::msg)?;

    let stop = Arc::new(AtomicBool::new(false));
    let (key_tx, mut key_rx) = mpsc::unbounded_channel();
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match crossterm::event::poll(Duration::from_millis(100)) {
                    Ok(true) => match crossterm::event::read() {
                        Ok(event) => {
                            if key_tx.send(event).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    },
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
        });
    }

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    let mut tick = tokio::time::interval(Duration::from_millis(80));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let outcome: anyhow::Result<()> = async {
        loop {
            terminal.draw(|frame| ui::draw(frame, &app))?;
            tokio::select! {
                key = key_rx.recv() => {
                    match key {
                        Some(crossterm::event::Event::Key(key)) => app.on_key(key).await,
                        Some(crossterm::event::Event::Paste(text)) => app.on_paste(&text),
                        Some(crossterm::event::Event::Mouse(mouse)) => app.on_mouse(mouse),
                        Some(_) => {}
                        None => break,
                    }
                }
                event = app_events.recv() => {
                    match event {
                        Some(event) => app.on_app_event(event),
                        None => break,
                    }
                }                event = bus.recv() => {
                    match event {
                        Ok(event) => app.on_live(event),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => app.reload(),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = tick.tick() => app.on_tick().await,
            }
            if app.should_quit {
                break;
            }
        }
        Ok(())
    }
    .await;

    stop.store(true, Ordering::Relaxed);
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
    ratatui::restore();
    outcome
}
