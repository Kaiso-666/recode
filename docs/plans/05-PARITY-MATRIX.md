# 05 — Parity matrix (opencode → Re:Code)

Status legend: `missing` · `partial` · `done` · `n/a` (explicit non-goal)

## Packages

| opencode package | Re:Code crate/module | Status | Notes |
|---|---|---|---|
| `schema` | `recode-schema` | done | Core message/part/tool/llm/config types; v1/compat schemas intentionally skipped |
| `core` | `recode-core` + `recode-tool` + `recode-storage` | partial | Runner, config, agents, permissions, compaction, subagents; LSP/MCP pending |
| `ai` | `recode-provider` | partial | Canonical LLM events; anthropic/openai-compatible (Gemini via OpenAI endpoint) |
| `server` | `recode-server` | partial | axum + SSE + permission broker; no editor clients yet |
| `cli` | `recode-cli` | done | run/tui/serve/auth/models/sessions/config |
| `tui` | `recode-tui` | partial | ratatui app verified in ConPTY; `@file`, search pending |
| `protocol` | `recode-server::routes` | partial | Reimplemented public HTTP surface, not exact |
| `client` | internal `EngineClient` | missing | Rust trait, no codegen |
| `sdk` | n/a | n/a | No JS SDK in v1 |
| `web`, `app`, `desktop`, `console`, `stats` | n/a | n/a | Out of scope |
| `plugin`, `plugins` | `recode-core::hooks` | missing | Rust traits + MCP; no JS sandbox |
| `pty` | `recode-tool::bash` + server PTY | partial | Non-interactive first |
| `codemode` | — | missing | Consider later (Lua/WASM sandbox) |

## Capabilities

| Capability | opencode | Re:Code |
|---|---|---|
| Streaming text/reasoning | yes | done |
| Parallel tool calls | yes | done |
| Provider retry/failover | yes | done (retry; no cross-provider failover yet) |
| Session persistence | SQLite | SQLite event store (done) |
| Resume a session | yes | done (`--session`, TUI picker) |
| Fork / revert | yes | M7 |
| Session compaction | yes | done (manual + automatic) |
| Subagents (`task`) | yes | done (depth-limited child sessions) |
| Permissions (allow/ask/deny) | yes | done (in-process + HTTP broker) |
| Model questions | yes | done (`question` tool: TUI modal, CLI prompt, HTTP broker) |
| AGENTS.md instructions | yes | done |
| Custom agents | yes | M3 |
| Custom commands | yes | M3 |
| LSP diagnostics | yes | M6 |
| MCP | yes | M6 |
| Plugins (JS) | yes | n/a (Rust hooks) |
| Worktrees | yes | M7 |
| File snapshots | yes | M7 |
| Share/export | yes | M7 |
| TUI mouse/paste/themes | rich | M4 (subset) |
| Web/desktop | yes | n/a |

## Architectural differences (intentional)

1. **Rust async instead of Effect.** Cancellation via `tokio_util::sync::CancellationToken`;
   retries via `backoff`-style helper; no dependency injection framework.
2. **Event-sourced storage from day one.** opencode folds durable events too; Re:Code makes
   the event log the sole write path and rebuilds projections on version bumps.
3. **Client/server optional.** In-process TUI path avoids HTTP for local use; server is for
   remote/editor clients.
4. **No JS plugin sandbox.** Extension surface is compile-time traits + MCP + custom agents.
