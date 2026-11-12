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
use recode_schema::question::QuestionReply;
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
            .route("/api/agents", get(agents))
            .route("/api/commands", get(commands))
            .route("/api/commands/{name}", post(expand_command))
            .route("/api/files", get(files))
            .route("/api/files/content", get(file_content))
            .route("/api/sessions", get(list_sessions).post(create_session))
            .route(
                "/api/sessions/{id}",
                get(get_session).delete(delete_session),
            )
            .route("/api/sessions/{id}/messages", get(messages))
            .route("/api/sessions/{id}/prompt", post(prompt))
            .route("/api/sessions/{id}/abort", post(abort))
            .route("/api/sessions/{id}/model", post(set_model))
            .route("/api/sessions/{id}/agent", post(set_agent))
            .route("/api/sessions/{id}/compact", post(compact))
            .route("/api/sessions/{id}/fork", post(fork_session))
            .route("/api/sessions/{id}/revert", post(revert_session))
            .route("/api/sessions/{id}/restore", post(restore_session))
            .route("/api/sessions/{id}/events", get(session_events))
            .route("/api/events", get(events))
            .route("/api/permissions", get(list_permissions))
            .route("/api/permissions/{id}", post(reply_permission))
            .route("/api/questions", get(list_questions))
            .route("/api/questions/{id}", post(reply_question))
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

async fn agents(State(state): State<AppState>) -> Json<Value> {
    let agents: Vec<Value> = state
        .engine
        .agents
        .list()
        .map(|agent| {
            json!({
                "id": agent.id,
                "name": agent.name,
                "description": agent.description,
            })
        })
        .collect();
    Json(json!({ "agents": agents }))
}

async fn commands(State(state): State<AppState>) -> Json<Value> {
    let commands: Vec<Value> = state
        .engine
        .commands
        .list()
        .map(|command| {
            json!({
                "name": command.name,
                "description": command.description,
                "agent": command.agent,
                "model": command.model,
                "subagent": command.subagent,
            })
        })
        .collect();
    Json(json!({ "commands": commands }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpandCommandRequest {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    arguments: String,
}

/// Expand a markdown command server-side so inline shell runs in the project
/// on the machine that owns the session.
async fn expand_command(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<ExpandCommandRequest>,
) -> Result<Json<Value>, ApiError> {
    let Some(command) = state.engine.commands.get(&name) else {
        return Err(ApiError::not_found(format!("unknown command {name:?}")));
    };
    let directory = match &body.session_id {
        Some(id) => {
            state
                .engine
                .session(&recode_schema::id::SessionId::from_existing(id.clone()))
                .map_err(ApiError::bad_request)?
                .ok_or_else(|| ApiError::not_found(format!("session {id:?} not found")))?
                .directory
        }
        None => state.engine.project.root.clone(),
    };
    let expansion = command
        .expand(&body.arguments, &directory)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({
        "text": expansion.text,
        "agent": expansion.agent,
        "model": expansion.model,
        "description": expansion.description,
        "subagent": expansion.subagent,
    })))
}

async fn files(State(state): State<AppState>) -> Json<Value> {
    let files = recode_tool::list_files(&state.engine.project.root, 5_000);
    Json(json!({ "files": files }))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

async fn file_content(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Value>, ApiError> {
    let root = recode_tool::canonicalish(&state.engine.project.root);
    let path = root.join(&query.path);
    let path = recode_tool::canonicalish(&path);
    if !recode_tool::is_within(&path, &root) {
        return Err(ApiError::bad_request("path escapes the project directory"));
    }
    if !path.is_file() {
        return Err(ApiError::not_found("file not found"));
    }
    let metadata =
        std::fs::metadata(&path).map_err(|error| ApiError::bad_request(error.to_string()))?;
    if metadata.len() > 256 * 1024 {
        return Err(ApiError::bad_request("file is larger than 256 KiB"));
    }
    let content =
        std::fs::read_to_string(&path).map_err(|error| ApiError::bad_request(error.to_string()))?;
    if content.contains('\0') {
        return Err(ApiError::bad_request("file is binary"));
    }
    Ok(Json(json!({ "path": query.path, "content": content })))
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
    let session_id = SessionId::from_existing(id);
    let busy = state.engine.is_busy(&session_id);
    let session = state
        .engine
        .session(&session_id)
        .map_err(ApiError::bad_request)?
        .ok_or_else(|| ApiError::not_found("session not found"))?;
    Ok(Json(json!({"session": session, "busy": busy})))
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
    let (messages, last_seq) = state
        .engine
        .messages_with_seq(&SessionId::from_existing(id))
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"messages": messages, "lastSeq": last_seq})))
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
struct ModelRequest {
    model: String,
}

async fn set_model(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ModelRequest>,
) -> Result<Json<Value>, ApiError> {
    let model = state
        .engine
        .config
        .resolve_model(Some(&body.model))
        .map_err(ApiError::bad_request)?;
    state
        .engine
        .set_model(&SessionId::from_existing(id), model)
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct AgentRequest {
    agent: String,
}

async fn set_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AgentRequest>,
) -> Result<Json<Value>, ApiError> {
    if state.engine.agents.get(&body.agent).is_none() {
        return Err(ApiError::bad_request(format!(
            "unknown agent {:?}",
            body.agent
        )));
    }
    state
        .engine
        .set_agent(&SessionId::from_existing(id), &body.agent)
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"ok": true})))
}

async fn compact(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let record = state
        .engine
        .compact(&SessionId::from_existing(id))
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(match record {
        Some(record) => json!({
            "compacted": true,
            "tokensBefore": record.tokens_before,
            "tokensAfter": record.tokens_after,
            "through": record.through,
        }),
        None => json!({"compacted": false}),
    }))
}

#[derive(Deserialize, Default)]
struct ForkRequest {
    #[serde(default)]
    title: Option<String>,
}

async fn fork_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ForkRequest>,
) -> Result<Json<Value>, ApiError> {
    let forked = state
        .engine
        .fork_session(&SessionId::from_existing(id), body.title)
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"session": forked})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevertRequest {
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    message: Option<String>,
    /// Also restore the worktree to the snapshot attached to the message.
    #[serde(default)]
    files: bool,
}

async fn revert_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RevertRequest>,
) -> Result<Json<Value>, ApiError> {
    let session_id = SessionId::from_existing(id);
    let message = body
        .message_id
        .or(body.message)
        .ok_or_else(|| ApiError::bad_request("messageId is required"))?;
    let message_id = recode_schema::MessageId::from_existing(message);
    let removed = state
        .engine
        .revert(&session_id, &message_id)
        .map_err(ApiError::bad_request)?;
    let snapshot = if body.files {
        Some(
            state
                .engine
                .restore_files(&session_id, Some(&message_id))
                .await
                .map_err(ApiError::bad_request)?,
        )
    } else {
        None
    };
    Ok(Json(json!({"removed": removed, "snapshot": snapshot})))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RestoreRequest {
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Restore the server's worktree to the snapshot taken with a message.
async fn restore_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RestoreRequest>,
) -> Result<Json<Value>, ApiError> {
    let session_id = SessionId::from_existing(id);
    let message = body
        .message_id
        .or(body.message)
        .map(recode_schema::MessageId::from_existing);
    let snapshot = state
        .engine
        .restore_files(&session_id, message.as_ref())
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"snapshot": snapshot})))
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

async fn list_questions(
    State(state): State<AppState>,
    Query(query): Query<PermissionQuery>,
) -> Json<Value> {
    let session = query
        .session
        .as_deref()
        .map(|id| SessionId::from_existing(id.to_owned()));
    let pending = state.engine.questions.pending_requests(session.as_ref());
    Json(json!({"questions": pending}))
}

#[derive(Deserialize)]
struct QuestionReplyRequest {
    reply: QuestionReply,
}

async fn reply_question(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<QuestionReplyRequest>,
) -> Result<Json<Value>, ApiError> {
    if !state.engine.questions.reply(&id, body.reply) {
        return Err(ApiError::not_found(format!(
            "question request {id} is not pending"
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
        | LiveEvent::PermissionReplied { session_id: id, .. }
        | LiveEvent::QuestionReplied { session_id: id, .. } => id == session_id,
        LiveEvent::PermissionAsked(request) => &request.session_id == session_id,
        LiveEvent::QuestionAsked(request) => &request.session_id == session_id,
    }
}

async fn openapi() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {"title": "Re:Code API", "version": env!("CARGO_PKG_VERSION")},
        "paths": {
            "/health": {"get": {"operationId": "health"}},
            "/api/providers": {"get": {"operationId": "providers"}},
            "/api/agents": {"get": {"operationId": "agents"}},
            "/api/commands": {"get": {"operationId": "commands.list"}},
            "/api/commands/{name}": {"post": {"operationId": "commands.expand"}},
            "/api/files": {"get": {"operationId": "files.list"}},
            "/api/files/content": {"get": {"operationId": "files.content"}},
            "/api/sessions": {"get": {"operationId": "sessions.list"}, "post": {"operationId": "sessions.create"}},
            "/api/sessions/{id}": {"get": {"operationId": "sessions.get"}, "delete": {"operationId": "sessions.delete"}},
            "/api/sessions/{id}/messages": {"get": {"operationId": "sessions.messages"}},
            "/api/sessions/{id}/prompt": {"post": {"operationId": "sessions.prompt"}},
            "/api/sessions/{id}/abort": {"post": {"operationId": "sessions.abort"}},
            "/api/sessions/{id}/model": {"post": {"operationId": "sessions.model"}},
            "/api/sessions/{id}/agent": {"post": {"operationId": "sessions.agent"}},
            "/api/sessions/{id}/compact": {"post": {"operationId": "sessions.compact"}},
            "/api/sessions/{id}/fork": {"post": {"operationId": "sessions.fork"}},
            "/api/sessions/{id}/revert": {"post": {"operationId": "sessions.revert"}},
            "/api/sessions/{id}/restore": {"post": {"operationId": "sessions.restore"}},
            "/api/sessions/{id}/events": {"get": {"operationId": "sessions.events"}},
            "/api/events": {"get": {"operationId": "events"}},
            "/api/permissions": {"get": {"operationId": "permissions.list"}},
            "/api/permissions/{id}": {"post": {"operationId": "permissions.reply"}},
            "/api/questions": {"get": {"operationId": "questions.list"}},
            "/api/questions/{id}": {"post": {"operationId": "questions.reply"}}
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
