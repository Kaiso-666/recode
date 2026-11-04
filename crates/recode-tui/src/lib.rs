//! Re:Code terminal UI.
//!
//! The TUI is a client of the in-process engine. It owns no domain state of
//! its own beyond presentation; transcripts are reloaded from the event store
//! and live updates arrive on the engine bus.

pub mod app;
pub mod input;
pub mod ui;

pub use app::{App, Overlay};

use async_trait::async_trait;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use recode_core::engine::Engine;
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
    engine: Arc<Engine>,
    responder: Arc<TuiResponder>,
    session_id: recode_schema::SessionId,
) -> anyhow::Result<()> {
    let mut app_events = responder
        .take_receiver()
        .ok_or_else(|| anyhow::anyhow!("TUI responder receiver already taken"))?;
    let mut bus = engine.subscribe();
    let mut app = App::new(engine.clone(), session_id).map_err(anyhow::Error::msg)?;

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
                }
                event = bus.recv() => {
                    match event {
                        Ok(event) => app.on_live(event),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => app.reload(),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = tick.tick() => app.on_tick(),
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
