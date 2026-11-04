# 02 — Architecture

## Crate graph

```
recode-schema      IDs, messages, parts, events, config, tool contracts. No I/O.
     ▲
recode-storage     SQLite event store + projections. Depends on schema only.
recode-provider    Wire clients (Anthropic, OpenAI-compatible, Gemini). schema only.
recode-tool        Tool registry + built-in tools. schema + storage-free.
     ▲
recode-core        Session runner, agent loop, config, permissions, instructions,
                   LSP/MCP managers, project discovery. Glues everything.
     ▲
recode-server      axum HTTP/SSE API over core. Optional.
recode-tui         ratatui client. Talks to core directly or a server.
recode-cli         clap entrypoint: run | tui | serve | auth | models | sessions | mcp | lsp | update
```

Rules:
- `schema` must not depend on anything but serde/tokio-util-free primitives.
- `provider` never sees session storage; it turns an `LlmRequest` into a stream of `LlmEvent`.
- `tool` never writes session events; it returns a `ToolOutcome`, the core persists it.
- `server` and `tui` are clients of the same `SessionService` trait.

## Session model (event-sourced)

Durable events in `event` table: `id, session_id, seq, type, data(json), created_at`.
Projections: `session`, `message`, `part`, `session_inbox`.

Message = ordered list of parts. Part types:
`text`, `reasoning`, `tool_call`, `tool_result`, `file`, `step_start`, `step_finish`,
`patch`, `snapshot`, `agent_switch`, `model_switch`, `compaction`, `error`.

The runner loop:

1. Admit prompt → `session.prompt` event → inbox row.
2. `run_session`: load history → build LLM request (instructions + tools + messages).
3. Stream provider events → append parts incrementally → publish to bus.
4. On `finish` with tool calls: execute tools (permission-checked), append tool results,
   continue with step 2. Otherwise go idle.
5. Every step is exactly one provider call. No hidden tool loop inside the provider.

Cancellation: `CancellationToken` per session; child processes killed via process group.

## Storage schema (SQLite, WAL)

```sql
event(id TEXT PK, session_id TEXT, seq INTEGER, type TEXT, data TEXT, created_at INTEGER)
session(id TEXT PK, project_id TEXT, directory TEXT, title TEXT, parent_id TEXT,
        created_at INTEGER, updated_at INTEGER, archived INTEGER, metadata TEXT)
message(id TEXT PK, session_id TEXT, seq INTEGER, role TEXT, created_at INTEGER,
        agent TEXT, model TEXT, metadata TEXT)
part(id TEXT PK, message_id TEXT, seq INTEGER, type TEXT, data TEXT, created_at INTEGER)
session_inbox(id TEXT PK, session_id TEXT, type TEXT, data TEXT, created_at INTEGER)
kv(key TEXT PK, value TEXT)
```

`event` is append-only and authoritative. Projection tables are rebuilt on schema bump.
Database: `<data_dir>/recode.db`; override with `RECODE_DB`.

## Provider wire layer

- `LlmRequest { model, system, messages, tools, tool_choice, temperature, max_tokens, stream }`
- `LlmEvent`:
  - `StepStart`, `TextDelta`, `ReasoningDelta`, `ToolCallStart`, `ToolCallDelta`, `ToolCallEnd`,
    `Usage`, `StepFinish`, `Finish`, `Error`.
- Adapters own message translation; the runner only sees canonical events.
- SSE parsing is incremental and tolerant of comments, multi-line data, CRLF.
- Retry: 429/5xx with jittered exponential backoff, honoring `retry-after`; abortable.

## Config

Precedence (low → high): built-in defaults, `~/.config/recode/recode.json`,
`<project>/recode.json`, `<project>/recode.local.json`, env `RECODE_CONFIG`, CLI flags.
JSONC parsing (comments + trailing commas) via a small normalizer.

Projects are located by walking up for `.git` or the config file; not required.

## Security

- Path resolution: every file tool canonicalizes, rejects `..` escapes unless
  `permission.filesystem.outside_project = allow`.
- Bash runs with a scrubbed env (no inherited secrets except allowlist), cwd pinned,
  timeout, output caps, process-tree kill.
- Secrets never logged. Auth store is `auth.json` with 0600 permissions.

## Observability

- `tracing` spans: `session.run`, `llm.stream`, `tool.execute`.
- `RECODE_LOG=debug` writes JSON logs to `<data_dir>/log/recode.log`.
- `/health`, `/metrics` (Prometheus text) on the server.
