# 04 — Crate map

## recode-schema
- `id.rs` — typed IDs (`SessionId`, `MessageId`, `PartId`, `ToolCallId`) with prefixes, ULID-like ordering.
- `message.rs` — `Role`, `Message`, `MessageTime`, `MessageMeta`.
- `part.rs` — `Part` enum + payloads, `ToolState` (pending/running/completed/error).
- `event.rs` — durable event enum, `EventEnvelope`, projection types.
- `llm.rs` — `LlmRequest`, `LlmEvent`, `Usage`, `FinishReason`, `ToolSpec`, `ToolChoice`.
- `tool.rs` — `ToolOutcome`, `ToolContent`, `ToolError`, `ToolContext`.
- `permission.rs` — `Rule`, `Ruleset`, `Decision`, wildcard matching.
- `config.rs` — config structs + defaults (serde), model/provider entries.
- `project.rs` — project/location IDs, paths.

## recode-storage
- `db.rs` — pool/connection, pragmas, migrations.
- `event_store.rs` — append with per-session seq (transactional), load since, iterate.
- `projection.rs` — fold events into session/message/part tables; rebuild command.
- `session_store.rs` — queries for TUI/API (list, transcript, search, inbox).
- `kv.rs` — small key/value for UI state.

## recode-provider
- `mod.rs` — `Provider` trait, registry, model resolution.
- `openai.rs` — `/chat/completions` streaming; also `/responses` later.
- `anthropic.rs` — `/v1/messages` streaming with tool_use blocks.
- `gemini.rs` — `streamGenerateContent`.
- `sse.rs` — incremental SSE decoder.
- `retry.rs` — backoff policy.
- `mock.rs` — test server fixtures.

## recode-tool
- `registry.rs` — registration, JSON schema, name/namespace, snapshot per agent.
- `bash.rs` — shell exec, timeout, tree-kill, output caps.
- `fs.rs` — read/write/edit/multi_edit/ls/glob/grep (ignore-aware).
- `webfetch.rs` — GET → markdown/text, size cap, allowlist.
- `todo.rs` — per-session todo list.
- `task.rs` — subagent spawn via core callback trait.
- `patch.rs` — apply unified diffs.
- `question.rs` — structured user question (client callback).

## recode-core
- `config.rs` — load/merge JSONC, env expansion, validation.
- `auth.rs` — credential store (auth.json, 0600).
- `project.rs` — root discovery, git info, worktree.
- `instructions.rs` — AGENTS.md/CLAUDE.md/rules discovery.
- `agent.rs` — agent registry, prompts, tool filters.
- `permission.rs` — ask/allow/deny engine + pending request broker.
- `session.rs` — `SessionService`: create, prompt, abort, fork, revert, list.
- `runner.rs` — the agent loop: LLM ↔ tools, event emission.
- `bus.rs` — broadcast event bus for live subscribers.
- `lsp.rs` — LSP client/manager.
- `mcp.rs` — MCP client/manager.
- `snapshot.rs` — shadow-git file snapshots.
- `compaction.rs` — summarization.
- `command.rs` — custom slash commands from config/markdown.

## recode-server
- `lib.rs` — axum router, bearer middleware, JSON handlers, SSE feeds, OpenAPI stub.

## recode-client
- `lib.rs` — typed HTTP client (`Client`): sessions, messages, prompt/abort, fork/revert,
  compact, model/agent switches, permissions, questions, project files.

## recode-tui
- `app.rs` — state machine, commands, overlays, polling for attach mode.
- `backend.rs` — `Backend` enum: in-process engine or cached HTTP client.
- `ui.rs` — transcript, editor, status, dialogs, pickers, completion menu.
- `input.rs` — keybinds, paste, history, cursor editing.

## recode-cli
- `main.rs` — clap, telemetry init, dispatch.
- `cmd/run.rs`, `cmd/tui.rs`, `cmd/serve.rs`, `cmd/auth.rs`, `cmd/models.rs`,
  `cmd/sessions.rs`, `cmd/mcp.rs`, `cmd/lsp.rs`, `cmd/upgrade.rs`.
- `output.rs` — non-interactive formatting (text/json).
