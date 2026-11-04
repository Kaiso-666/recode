//! Re:Code HTTP + SSE API.
//!
//! The API is a thin, typed facade over [`recode_core::Engine`]. One process
//! owns one project and any number of sessions; clients subscribe to a single
//! SSE feed or per-session feeds.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::Stream;
use recode_core::engine::Engine;
use recode_schema::event::LiveEvent;
use recode_schema::llm::ContentBlock;
use recode_schema::permission::PermissionReply;
use recode_schema::SessionId;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct Server {
    engine: Arc<Engine>,
    token: String,
}

impl Server {
    pub fn new(engine: Arc<Engine>, token: String) -> Self {
        Self { engine, token }
    }

    pub fn router(self) -> Router {
        let state = AppState {
            engine: self.engine,
            token: self.token,
        };
        Router::new()
            .route("/health", get(health))
            .route("/api/providers", get(providers))
            .route("/api/sessions", get(list_sessions).post(create_session))
            .route(
                "/api/sessions/{id}",
                get(get_session).delete(delete_session),
            )
            .route("/api/sessions/{id}/messages", get(messages))
            .route("/api/sessions/{id}/prompt", post(prompt))
            .route("/api/sessions/{id}/abort", post(abort))
            .route("/api/sessions/{id}/events", get(session_events))
            .route("/api/events", get(events))
            .route("/api/permissions", get(list_permissions))
            .route("/api/permissions/{id}", post(reply_permission))
            .route("/api/openapi.json", get(openapi))
            .layer(middleware::from_fn_with_state(state.clone(), authorize))
            .layer(CorsLayer::permissive())
            .layer(TraceLayer::new_for_http())
            .with_state(state)
    }
}

pub async fn serve(server: Server, listener: tokio::net::TcpListener) -> anyhow::Result<()> {
    axum::serve(listener, server.router()).await?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    engine: Arc<Engine>,
    token: String,
}

async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }
    let supplied = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if supplied == state.token {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        )
            .into_response()
    }
}

async fn health() -> Json<Value> {
    Json(json!({
        "ok": true,
        "name": "recode",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn providers(State(state): State<AppState>) -> Json<Value> {
    let providers: Vec<Value> = state
        .engine
        .config
        .providers()
        .into_iter()
        .map(|provider| {
            json!({
                "id": provider.id,
                "name": provider.name,
                "configured": provider.configured,
                "models": provider.models,
            })
        })
        .collect();
    Json(json!({
        "providers": providers,
        "defaultModel": state.engine.config.merged.model,
        "agent": state.engine.config.default_agent(),
        "project": state.engine.project.info,
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    all: bool,
    #[serde(default)]
    archived: bool,
}

async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let sessions = if query.all {
        state.engine.all_sessions(query.archived)
    } else {
        state.engine.sessions(query.archived)
    }
    .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"sessions": sessions})))
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

async fn create_session(
    State(state): State<AppState>,
    Json(body): Json<CreateSessionRequest>,
) -> Result<Json<Value>, ApiError> {
    let model = match body.model {
        Some(model) => Some(
            state
                .engine
                .config
                .resolve_model(Some(&model))
                .map_err(ApiError::bad_request)?,
        ),
        None => None,
    };
    let session = state
        .engine
        .create_session(body.agent, model, body.title)
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"session": session})))
}

async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let session = state
        .engine
        .session(&SessionId::from_existing(id))
        .map_err(ApiError::bad_request)?
        .ok_or_else(|| ApiError::not_found("session not found"))?;
    Ok(Json(json!({"session": session})))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state
        .engine
        .delete_session(&SessionId::from_existing(id))
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"ok": true})))
}

async fn messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let messages = state
        .engine
        .messages(&SessionId::from_existing(id))
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"messages": messages})))
}

#[derive(Deserialize)]
struct PromptRequest {
    text: String,
    #[serde(default)]
    files: Vec<FileInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileInput {
    mime: String,
    filename: String,
    url: String,
}

async fn prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PromptRequest>,
) -> Result<Json<Value>, ApiError> {
    let files: Vec<ContentBlock> = body
        .files
        .into_iter()
        .map(|file| ContentBlock::File {
            mime: file.mime,
            filename: file.filename,
            url: file.url,
        })
        .collect();
    state
        .engine
        .prompt(&SessionId::from_existing(id), body.text, files)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"ok": true})))
}

async fn abort(State(state): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    let aborted = state.engine.abort(&SessionId::from_existing(id));
    Json(json!({"aborted": aborted}))
}

#[derive(Deserialize)]
struct PermissionQuery {
    #[serde(default)]
    session: Option<String>,
}

async fn list_permissions(
    State(state): State<AppState>,
    Query(query): Query<PermissionQuery>,
) -> Json<Value> {
    let session = query
        .session
        .as_deref()
        .map(|id| SessionId::from_existing(id.to_owned()));
    let pending = state.engine.permissions.pending_requests(session.as_ref());
    Json(json!({"permissions": pending}))
}

#[derive(Deserialize)]
struct PermissionReplyRequest {
    reply: PermissionReply,
}

async fn reply_permission(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PermissionReplyRequest>,
) -> Result<Json<Value>, ApiError> {
    if !state.engine.permissions.reply(&id, body.reply) {
        return Err(ApiError::not_found(format!(
            "permission request {id} is not pending"
        )));
    }
    Ok(Json(json!({"ok": true})))
}

async fn session_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let session_id = SessionId::from_existing(id);
    let receiver = state.engine.subscribe();
    let stream = futures_util::stream::unfold(receiver, move |mut receiver| {
        let session_id = session_id.clone();
        async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        if !belongs_to(&event, &session_id) {
                            continue;
                        }
                        let data = serde_json::to_string(&event).unwrap_or_default();
                        return Some((
                            Ok::<_, Infallible>(Event::default().event("recode").data(data)),
                            receiver,
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.engine.subscribe();
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    return Some((
                        Ok::<_, Infallible>(Event::default().event("recode").data(data)),
                        receiver,
                    ));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn belongs_to(event: &LiveEvent, session_id: &SessionId) -> bool {
    match event {
        LiveEvent::Durable(envelope) => &envelope.session_id == session_id,
        LiveEvent::SessionStatus { session_id: id, .. }
        | LiveEvent::Delta { session_id: id, .. }
        | LiveEvent::PermissionReplied { session_id: id, .. } => id == session_id,
        LiveEvent::PermissionAsked(request) => &request.session_id == session_id,
    }
}

async fn openapi() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {"title": "Re:Code API", "version": env!("CARGO_PKG_VERSION")},
        "paths": {
            "/health": {"get": {"operationId": "health"}},
            "/api/providers": {"get": {"operationId": "providers"}},
            "/api/sessions": {"get": {"operationId": "sessions.list"}, "post": {"operationId": "sessions.create"}},
            "/api/sessions/{id}/messages": {"get": {"operationId": "sessions.messages"}},
            "/api/sessions/{id}/prompt": {"post": {"operationId": "sessions.prompt"}},
            "/api/sessions/{id}/abort": {"post": {"operationId": "sessions.abort"}},
            "/api/sessions/{id}/events": {"get": {"operationId": "sessions.events"}},
            "/api/events": {"get": {"operationId": "events"}},
            "/api/permissions": {"get": {"operationId": "permissions.list"}},
            "/api/permissions/{id}": {"post": {"operationId": "permissions.reply"}}
        }
    }))
}

struct ApiError(StatusCode, String);

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, message.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
