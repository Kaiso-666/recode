use recode_schema::event::{DeltaKind, Event};
use recode_schema::part::Part;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

fn json<T: Serialize>(value: &T) -> rusqlite::Result<String> {
    serde_json::to_string(value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

/// Bump when projection semantics change; `migrate` rebuilds older databases.
pub const SCHEMA_VERSION: u32 = 1;

pub(crate) fn clear(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM part;
         DELETE FROM message;
         DELETE FROM permission;
         DELETE FROM session_inbox;
         DELETE FROM session;",
    )
}

pub(crate) fn apply(conn: &Connection, event: &Event) -> rusqlite::Result<()> {
    match event {
        Event::SessionCreated { session } | Event::SessionUpdated { session } => {
            conn.execute(
                "INSERT INTO session
                    (id, project_id, directory, title, parent_id, created_at, updated_at,
                     archived, agent, model, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(id) DO UPDATE SET
                    project_id = excluded.project_id,
                    directory = excluded.directory,
                    title = excluded.title,
                    parent_id = excluded.parent_id,
                    updated_at = excluded.updated_at,
                    archived = excluded.archived,
                    agent = excluded.agent,
                    model = excluded.model,
                    metadata = excluded.metadata",
                params![
                    session.id.0,
                    session.project_id.0,
                    session.directory.to_string_lossy(),
                    session.title,
                    session.parent_id.as_ref().map(|id| id.0.clone()),
                    session.created_at,
                    session.updated_at,
                    if session.archived { 1i64 } else { 0 },
                    session.agent,
                    session.model.as_ref().map(json).transpose()?,
                    json(&session.metadata)?,
                ],
            )?;
        }
        Event::SessionDeleted { session_id } => {
            conn.execute(
                "DELETE FROM part WHERE message_id IN (SELECT id FROM message WHERE session_id = ?1)",
                params![session_id.0],
            )?;
            conn.execute(
                "DELETE FROM message WHERE session_id = ?1",
                params![session_id.0],
            )?;
            conn.execute(
                "DELETE FROM permission WHERE session_id = ?1",
                params![session_id.0],
            )?;
            conn.execute(
                "DELETE FROM session_inbox WHERE session_id = ?1",
                params![session_id.0],
            )?;
            conn.execute("DELETE FROM session WHERE id = ?1", params![session_id.0])?;
            conn.execute(
                "DELETE FROM event WHERE session_id = ?1",
                params![session_id.0],
            )?;
        }
        Event::MessageAdded { message } => {
            let seq: i64 = conn.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM message WHERE session_id = ?1",
                params![message.session_id.0],
                |row| row.get(0),
            )?;
            conn.execute(
                "INSERT INTO message
                    (id, session_id, seq, role, created_at, agent, model, parent_id, error, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO NOTHING",
                params![
                    message.id.0,
                    message.session_id.0,
                    seq,
                    message.role.as_str(),
                    message.created_at,
                    message.agent,
                    message.model.as_ref().map(json).transpose()?,
                    message.parent_id.as_ref().map(|id| id.0.clone()),
                    message.error.as_ref().map(json).transpose()?,
                    json(&message.metadata)?,
                ],
            )?;
        }
        Event::MessageRemoved {
            session_id,
            message_id,
        } => {
            let _ = session_id;
            conn.execute(
                "DELETE FROM part WHERE message_id = ?1",
                params![message_id.0],
            )?;
            conn.execute("DELETE FROM message WHERE id = ?1", params![message_id.0])?;
        }
        Event::PartAdded {
            message_id, part, ..
        }
        | Event::PartUpdated {
            message_id, part, ..
        } => {
            let existing_seq: Option<i64> = conn
                .query_row(
                    "SELECT seq FROM part WHERE id = ?1",
                    params![part.id().0],
                    |row| row.get(0),
                )
                .optional()?;
            let seq = match existing_seq {
                Some(seq) => seq,
                None => conn.query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM part WHERE message_id = ?1",
                    params![message_id.0],
                    |row| row.get(0),
                )?,
            };
            conn.execute(
                "INSERT INTO part (id, message_id, seq, type, data, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET data = excluded.data",
                params![
                    part.id().0,
                    message_id.0,
                    seq,
                    part_type(part),
                    json(part)?,
                    recode_schema::now_ms(),
                ],
            )?;
        }
        Event::PartDelta {
            part_id,
            kind,
            delta,
            ..
        } => {
            let data: Option<String> = conn
                .query_row(
                    "SELECT data FROM part WHERE id = ?1",
                    params![part_id.0],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(data) = data else {
                return Ok(());
            };
            let mut part: Part = serde_json::from_str(&data)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            match kind {
                DeltaKind::Text => {
                    if let Some(text) = part.text_mut() {
                        text.text.push_str(delta);
                    } else {
                        return Ok(());
                    }
                }
                DeltaKind::Reasoning => {
                    if let Some(reasoning) = part.reasoning_mut() {
                        reasoning.text.push_str(delta);
                    } else {
                        return Ok(());
                    }
                }
            }
            conn.execute(
                "UPDATE part SET data = ?1 WHERE id = ?2",
                params![json(&part)?, part_id.0],
            )?;
        }
        Event::PartRemoved { part_id, .. } => {
            conn.execute("DELETE FROM part WHERE id = ?1", params![part_id.0])?;
        }
        Event::SessionIdle { .. } | Event::SessionError { .. } => {}
        Event::PermissionAsked { request } => {
            conn.execute(
                "INSERT INTO permission (id, session_id, data, reply, created_at)
                 VALUES (?1, ?2, ?3, NULL, ?4)
                 ON CONFLICT(id) DO NOTHING",
                params![
                    request.id,
                    request.session_id.0,
                    json(request)?,
                    request.created_at,
                ],
            )?;
        }
        Event::PermissionReplied {
            session_id,
            request_id,
            reply,
        } => {
            let _ = session_id;
            conn.execute(
                "UPDATE permission SET reply = ?1 WHERE id = ?2",
                params![json(reply)?, request_id],
            )?;
        }
    }
    Ok(())
}

fn part_type(part: &Part) -> &'static str {
    match part {
        Part::Text(_) => "text",
        Part::Reasoning(_) => "reasoning",
        Part::Tool(_) => "tool",
        Part::StepStart(_) => "step_start",
        Part::StepFinish(_) => "step_finish",
        Part::File(_) => "file",
        Part::Patch(_) => "patch",
        Part::Snapshot(_) => "snapshot",
        Part::Agent(_) => "agent",
        Part::Model(_) => "model",
        Part::Compaction(_) => "compaction",
        Part::Error(_) => "error",
    }
}
