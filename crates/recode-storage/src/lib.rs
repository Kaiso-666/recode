//! Durable storage for Re:Code.
//!
//! The event log is authoritative. Every append updates the projection tables
//! in the same transaction, so readers never observe a partially applied
//! change. `rebuild` replays the log into fresh projections after a schema
//! bump or corruption.

mod migrate;
mod projection;

pub use projection::SCHEMA_VERSION;

use crate::migrate::migrate;
use parking_lot::Mutex;
use recode_schema::event::{Event, EventEnvelope, SessionInfo, SessionStatus};
use recode_schema::id::{ProjectId, SessionId};
use recode_schema::message::Message;
use recode_schema::now_ms;
use recode_schema::permission::{PermissionReply, PermissionRequest};
use recode_schema::project::ProjectInfo;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("invalid stored data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A validated session transcript page.
#[derive(Clone, Debug)]
pub struct StoredMessage {
    pub info: recode_schema::message::MessageInfo,
    pub parts: Vec<recode_schema::part::Part>,
}

/// The latest summary of older history for a session.
#[derive(Clone, Debug)]
pub struct CompactionRecord {
    pub summary: String,
    pub through: recode_schema::MessageId,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub created_at: i64,
}

impl StoredMessage {
    pub fn into_message(self) -> Message {
        Message {
            info: self.info,
            parts: self.parts,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoreOptions {
    /// Maximum number of events returned by one query.
    pub max_page: u32,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self { max_page: 10_000 }
    }
}

/// Thread-safe handle to the SQLite database.
///
/// The database is intentionally single-writer: a mutex serializes writes,
/// which is more than enough for an interactive coding agent and keeps
/// ordering obvious. Reads go through the same connection for simplicity; WAL
/// still allows a second process (for example `recode serve`) to read.
pub struct Store {
    conn: Mutex<Connection>,
    path: PathBuf,
    options: StoreOptions,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        Self::from_connection(conn, path)
    }

    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn, PathBuf::from(":memory:"))
    }

    fn from_connection(conn: Connection, path: PathBuf) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
            options: StoreOptions::default(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one event and update projections atomically.
    pub fn append(&self, event: Event) -> Result<EventEnvelope> {
        let mut events = self.append_batch(vec![event])?;
        Ok(events.remove(0))
    }

    /// Append a batch in a single transaction. Preserves input order and
    /// assigns per-session sequence numbers.
    pub fn append_batch(&self, events: Vec<Event>) -> Result<Vec<EventEnvelope>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut envelopes = Vec::with_capacity(events.len());
        for event in events {
            let session_id = event.session_id().clone();
            let seq: u64 = tx.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM event WHERE session_id = ?1",
                params![session_id.0],
                |row| row.get(0),
            )?;
            let envelope = EventEnvelope {
                id: recode_schema::EventId::new(),
                session_id: session_id.clone(),
                seq,
                created_at: now_ms(),
                event,
            };
            let data = serde_json::to_string(&envelope.event)?;
            tx.execute(
                "INSERT INTO event (id, session_id, seq, type, data, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    envelope.id.0,
                    envelope.session_id.0,
                    envelope.seq,
                    event_type(&envelope.event),
                    data,
                    envelope.created_at
                ],
            )?;
            projection::apply(&tx, &envelope.event)?;
            tx.execute(
                "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                params![envelope.created_at, envelope.session_id.0],
            )?;
            envelopes.push(envelope);
        }
        tx.commit()?;
        Ok(envelopes)
    }

    pub fn events(&self, session_id: &SessionId, after_seq: u64) -> Result<Vec<EventEnvelope>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, seq, type, data, created_at FROM event
             WHERE session_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![session_id.0, after_seq, self.options.max_page],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        let mut events = Vec::new();
        for row in rows {
            let (id, seq, data, created_at) = row?;
            events.push(EventEnvelope {
                id: recode_schema::EventId::from_existing(id),
                session_id: session_id.clone(),
                seq,
                created_at,
                event: serde_json::from_str(&data)?,
            });
        }
        Ok(events)
    }

    pub fn event_count(&self) -> Result<u64> {
        let conn = self.conn.lock();
        Ok(conn.query_row("SELECT COUNT(*) FROM event", [], |row| row.get(0))?)
    }

    /// Rebuild all projections from the event log. Runs in one transaction.
    pub fn rebuild(&self) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        projection::clear(&tx)?;
        let mut stmt = tx.prepare(
            "SELECT session_id, seq, data, created_at FROM event ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut collected = Vec::new();
        for row in rows {
            collected.push(row?);
        }
        drop(stmt);
        for (_, _, data, _) in collected {
            let event: Event = serde_json::from_str(&data)?;
            projection::apply(&tx, &event)?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- projects -------------------------------------------------------

    pub fn upsert_project(&self, project: &ProjectInfo) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO project (id, directory, worktree, name, vcs, created_at, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
               directory = excluded.directory,
               worktree = excluded.worktree,
               name = excluded.name,
               vcs = excluded.vcs,
               metadata = excluded.metadata",
            params![
                project.id.0,
                project.directory.to_string_lossy(),
                project
                    .worktree
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
                project.name,
                project.vcs,
                project.created_at,
                serde_json::to_string(&project.metadata)?
            ],
        )?;
        Ok(())
    }

    pub fn project_by_directory(&self, directory: &Path) -> Result<Option<ProjectInfo>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, directory, worktree, name, vcs, created_at, metadata
             FROM project WHERE directory = ?1 ORDER BY created_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![directory.to_string_lossy()])?;
        match rows.next()? {
            Some(row) => Ok(Some(project_from_row(row)?)),
            None => Ok(None),
        }
    }

    // ---- sessions -------------------------------------------------------

    pub fn create_session(&self, session: SessionInfo) -> Result<EventEnvelope> {
        self.append(Event::SessionCreated { session })
    }

    pub fn update_session(&self, session: SessionInfo) -> Result<EventEnvelope> {
        self.append(Event::SessionUpdated { session })
    }

    pub fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.append(Event::SessionDeleted {
            session_id: session_id.clone(),
        })?;
        Ok(())
    }

    pub fn get_session(&self, session_id: &SessionId) -> Result<Option<SessionInfo>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, directory, title, parent_id, created_at, updated_at,
                    archived, agent, model, metadata
             FROM session WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![session_id.0])?;
        match rows.next()? {
            Some(row) => Ok(Some(session_from_row(row)?)),
            None => Ok(None),
        }
    }

    /// List sessions, newest first. `roots_only` excludes subagent children so
    /// pickers and session lists never show them; pass `false` to see every
    /// session (for example to inspect a child transcript).
    pub fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        include_archived: bool,
        roots_only: bool,
        limit: u32,
    ) -> Result<Vec<SessionInfo>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, directory, title, parent_id, created_at, updated_at,
                    archived, agent, model, metadata
             FROM session
             WHERE (?1 IS NULL OR project_id = ?1)
               AND (?2 OR archived = 0)
               AND (NOT ?3 OR parent_id IS NULL)
             ORDER BY updated_at DESC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                project_id.map(|project_id| project_id.0.as_str()),
                include_archived,
                roots_only,
                limit
            ],
            session_from_row,
        )?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    // ---- messages -------------------------------------------------------

    pub fn messages(&self, session_id: &SessionId, limit: u32) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, role, created_at, agent, model, parent_id, error, metadata
             FROM message WHERE session_id = ?1 ORDER BY seq ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![session_id.0, limit], |row| {
            let model: Option<String> = row.get(4)?;
            let error: Option<String> = row.get(6)?;
            let metadata: Option<String> = row.get(7)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                model,
                row.get::<_, Option<String>>(5)?,
                error,
                metadata,
            ))
        })?;
        let mut messages = Vec::new();
        for row in rows {
            let (id, role, created_at, agent, model, parent_id, error, metadata) = row?;
            let info = recode_schema::message::MessageInfo {
                id: recode_schema::MessageId::from_existing(id.clone()),
                session_id: session_id.clone(),
                role: if role == "user" {
                    recode_schema::Role::User
                } else {
                    recode_schema::Role::Assistant
                },
                created_at,
                agent,
                model: model.as_deref().map(serde_json::from_str).transpose()?,
                parent_id: parent_id.map(recode_schema::MessageId::from_existing),
                error: error.as_deref().map(serde_json::from_str).transpose()?,
                metadata: metadata
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or(serde_json::Value::Null),
            };
            let parts = self.parts_for_message_locked(&conn, &info.id)?;
            messages.push(StoredMessage { info, parts });
        }
        Ok(messages)
    }

    pub fn message(&self, message_id: &recode_schema::MessageId) -> Result<Option<StoredMessage>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT session_id, role, created_at, agent, model, parent_id, error, metadata
             FROM message WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![message_id.0])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let session_id = SessionId::from_existing(row.get::<_, String>(0)?);
        let role: String = row.get(1)?;
        let info = recode_schema::message::MessageInfo {
            id: message_id.clone(),
            session_id,
            role: if role == "user" {
                recode_schema::Role::User
            } else {
                recode_schema::Role::Assistant
            },
            created_at: row.get(2)?,
            agent: row.get(3)?,
            model: row
                .get::<_, Option<String>>(4)?
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            parent_id: row
                .get::<_, Option<String>>(5)?
                .map(recode_schema::MessageId::from_existing),
            error: row
                .get::<_, Option<String>>(6)?
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            metadata: row
                .get::<_, Option<String>>(7)?
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or(serde_json::Value::Null),
        };
        let parts = self.parts_for_message_locked(&conn, message_id)?;
        Ok(Some(StoredMessage { info, parts }))
    }

    /// Latest compaction summary for a session, if any.
    pub fn compaction(&self, session_id: &SessionId) -> Result<Option<CompactionRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT summary, through, tokens_before, tokens_after, created_at
             FROM compaction WHERE session_id = ?1",
        )?;
        let mut rows = stmt.query(params![session_id.0])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(CompactionRecord {
            summary: row.get(0)?,
            through: recode_schema::MessageId::from_existing(row.get::<_, String>(1)?),
            tokens_before: row.get(2)?,
            tokens_after: row.get(3)?,
            created_at: row.get(4)?,
        }))
    }

    fn parts_for_message_locked(
        &self,
        conn: &Connection,
        message_id: &recode_schema::MessageId,
    ) -> Result<Vec<recode_schema::part::Part>> {
        let mut stmt =
            conn.prepare("SELECT data FROM part WHERE message_id = ?1 ORDER BY seq ASC")?;
        let rows = stmt.query_map(params![message_id.0], |row| row.get::<_, String>(0))?;
        let mut parts = Vec::new();
        for row in rows {
            parts.push(serde_json::from_str(&row?)?);
        }
        Ok(parts)
    }

    // ---- permissions ----------------------------------------------------

    pub fn pending_permissions(&self, session_id: &SessionId) -> Result<Vec<PermissionRequest>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT data FROM permission WHERE session_id = ?1 AND reply IS NULL ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![session_id.0], |row| row.get::<_, String>(0))?;
        let mut requests = Vec::new();
        for row in rows {
            requests.push(serde_json::from_str(&row?)?);
        }
        Ok(requests)
    }

    pub fn reply_permission(&self, request_id: &str, reply: PermissionReply) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE permission SET reply = ?1 WHERE id = ?2",
            params![serde_json::to_string(&reply)?, request_id],
        )?;
        Ok(())
    }

    // ---- kv -------------------------------------------------------------

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Session status is volatile (not durable) but useful across short-lived
    /// server restarts; store it in `kv` under a namespaced key.
    pub fn set_status(&self, session_id: &SessionId, status: SessionStatus) -> Result<()> {
        self.kv_set(
            &format!("session.status.{}", session_id.0),
            status_str(status),
        )
    }

    pub fn get_status(&self, session_id: &SessionId) -> Result<Option<SessionStatus>> {
        Ok(self
            .kv_get(&format!("session.status.{}", session_id.0))?
            .and_then(|value| match value.as_str() {
                "idle" => Some(SessionStatus::Idle),
                "busy" => Some(SessionStatus::Busy),
                "retry" => Some(SessionStatus::Retry),
                _ => None,
            }))
    }
}

fn status_str(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Busy => "busy",
        SessionStatus::Retry => "retry",
    }
}

pub(crate) fn event_type(event: &Event) -> &'static str {
    match event {
        Event::SessionCreated { .. } => "session.created",
        Event::SessionUpdated { .. } => "session.updated",
        Event::SessionDeleted { .. } => "session.deleted",
        Event::MessageAdded { .. } => "message.added",
        Event::MessageRemoved { .. } => "message.removed",
        Event::PartAdded { .. } => "part.added",
        Event::PartUpdated { .. } => "part.updated",
        Event::PartDelta { .. } => "part.delta",
        Event::PartRemoved { .. } => "part.removed",
        Event::SessionIdle { .. } => "session.idle",
        Event::SessionError { .. } => "session.error",
        Event::PermissionAsked { .. } => "permission.asked",
        Event::PermissionReplied { .. } => "permission.replied",
        Event::SessionCompacted { .. } => "session.compacted",
    }
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionInfo> {
    let model: Option<String> = row.get(9)?;
    let metadata: Option<String> = row.get(10)?;
    Ok(SessionInfo {
        id: SessionId::from_existing(row.get::<_, String>(0)?),
        project_id: ProjectId::from_existing(row.get::<_, String>(1)?),
        directory: PathBuf::from(row.get::<_, String>(2)?),
        title: row.get(3)?,
        parent_id: row
            .get::<_, Option<String>>(4)?
            .map(SessionId::from_existing),
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        archived: row.get::<_, i64>(7)? != 0,
        agent: row.get(8)?,
        model: model
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
        metadata: metadata
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
            .unwrap_or(serde_json::Value::Null),
    })
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectInfo> {
    let metadata: Option<String> = row.get(6)?;
    Ok(ProjectInfo {
        id: ProjectId::from_existing(row.get::<_, String>(0)?),
        directory: PathBuf::from(row.get::<_, String>(1)?),
        worktree: row.get::<_, Option<String>>(2)?.map(PathBuf::from),
        name: row.get(3)?,
        vcs: row.get(4)?,
        created_at: row.get(5)?,
        metadata: metadata
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
            .unwrap_or(serde_json::Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use recode_schema::part::Part;
    use recode_schema::project::ProjectInfo;

    fn setup() -> (Store, SessionInfo) {
        let store = Store::open_memory().unwrap();
        let project = ProjectInfo {
            id: ProjectId::new(),
            directory: PathBuf::from("C:/work/demo"),
            worktree: None,
            name: "demo".into(),
            vcs: Some("git".into()),
            created_at: now_ms(),
            metadata: serde_json::Value::Null,
        };
        store.upsert_project(&project).unwrap();
        let session = SessionInfo::new(project.id.clone(), project.directory.clone());
        store.create_session(session.clone()).unwrap();
        (store, session)
    }

    #[test]
    fn appends_and_replays_events() {
        let (store, session) = setup();
        let message = recode_schema::message::MessageInfo::user(session.id.clone());
        store
            .append(Event::MessageAdded {
                message: message.clone(),
            })
            .unwrap();
        let part = Part::text("hello");
        store
            .append(Event::PartAdded {
                session_id: session.id.clone(),
                message_id: message.id.clone(),
                part: part.clone(),
            })
            .unwrap();
        store
            .append(Event::PartDelta {
                session_id: session.id.clone(),
                message_id: message.id.clone(),
                part_id: part.id().clone(),
                kind: recode_schema::event::DeltaKind::Text,
                delta: " world".into(),
            })
            .unwrap();

        let messages = store.messages(&session.id, 100).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].parts.len(), 1);
        assert_eq!(messages[0].parts[0].plain_text(), Some("hello world"));

        let events = store.events(&session.id, 0).unwrap();
        assert_eq!(events.len(), 4); // session.created + 3
    }

    #[test]
    fn rebuild_reproduces_projections() {
        let (store, session) = setup();
        let message = recode_schema::message::MessageInfo::user(session.id.clone());
        store
            .append(Event::MessageAdded {
                message: message.clone(),
            })
            .unwrap();
        store
            .append(Event::PartAdded {
                session_id: session.id.clone(),
                message_id: message.id.clone(),
                part: Part::text("persist me"),
            })
            .unwrap();
        store.rebuild().unwrap();
        let messages = store.messages(&session.id, 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].parts[0].plain_text(), Some("persist me"));
    }

    #[test]
    fn sessions_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recode.db");
        let session_id = {
            let store = Store::open(&path).unwrap();
            let project = ProjectInfo {
                id: ProjectId::new(),
                directory: PathBuf::from("C:/work/demo"),
                worktree: None,
                name: "demo".into(),
                vcs: None,
                created_at: now_ms(),
                metadata: serde_json::Value::Null,
            };
            store.upsert_project(&project).unwrap();
            let session = SessionInfo::new(project.id.clone(), project.directory.clone());
            store.create_session(session.clone()).unwrap();
            session.id
        };
        let store = Store::open(&path).unwrap();
        assert!(store.get_session(&session_id).unwrap().is_some());
    }
}
