use crate::projection::SCHEMA_VERSION;
use rusqlite::Connection;

pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if version < 1 {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS project (
                id          TEXT PRIMARY KEY,
                directory   TEXT NOT NULL,
                worktree    TEXT,
                name        TEXT NOT NULL DEFAULT '',
                vcs         TEXT,
                created_at  INTEGER NOT NULL,
                metadata    TEXT
            );
            CREATE UNIQUE INDEX IF NOT EXISTS project_directory ON project(directory);

            CREATE TABLE IF NOT EXISTS session (
                id          TEXT PRIMARY KEY,
                project_id  TEXT NOT NULL,
                directory   TEXT NOT NULL,
                title       TEXT NOT NULL DEFAULT '',
                parent_id   TEXT,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL,
                archived    INTEGER NOT NULL DEFAULT 0,
                agent       TEXT,
                model       TEXT,
                metadata    TEXT
            );
            CREATE INDEX IF NOT EXISTS session_project ON session(project_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS message (
                id          TEXT PRIMARY KEY,
                session_id  TEXT NOT NULL,
                seq         INTEGER NOT NULL,
                role        TEXT NOT NULL,
                created_at  INTEGER NOT NULL,
                agent       TEXT,
                model       TEXT,
                parent_id   TEXT,
                error       TEXT,
                metadata    TEXT
            );
            CREATE INDEX IF NOT EXISTS message_session ON message(session_id, seq);

            CREATE TABLE IF NOT EXISTS part (
                id          TEXT PRIMARY KEY,
                message_id  TEXT NOT NULL,
                seq         INTEGER NOT NULL,
                type        TEXT NOT NULL,
                data        TEXT NOT NULL,
                created_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS part_message ON part(message_id, seq);

            CREATE TABLE IF NOT EXISTS event (
                id          TEXT PRIMARY KEY,
                session_id  TEXT NOT NULL,
                seq         INTEGER NOT NULL,
                type        TEXT NOT NULL,
                data        TEXT NOT NULL,
                created_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS event_session ON event(session_id, seq);
            CREATE INDEX IF NOT EXISTS event_created ON event(created_at);

            CREATE TABLE IF NOT EXISTS permission (
                id          TEXT PRIMARY KEY,
                session_id  TEXT NOT NULL,
                data        TEXT NOT NULL,
                reply       TEXT,
                created_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS permission_session ON permission(session_id, created_at);

            CREATE TABLE IF NOT EXISTS session_inbox (
                id          TEXT PRIMARY KEY,
                session_id  TEXT NOT NULL,
                kind        TEXT NOT NULL,
                data        TEXT NOT NULL,
                created_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS inbox_session ON session_inbox(session_id, created_at);

            CREATE TABLE IF NOT EXISTS kv (
                key         TEXT PRIMARY KEY,
                value       TEXT NOT NULL
            );
            "#,
        )?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}
