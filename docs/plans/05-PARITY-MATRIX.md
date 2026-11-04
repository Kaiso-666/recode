# 05 — Parity matrix (opencode → Re:Code)

Status legend: `missing` · `partial` · `done` · `n/a` (explicit non-goal)

## Packages

| opencode package | Re:Code crate/module | Status | Notes |
|---|---|---|---|
| `schema` | `recode-schema` | partial | Core message/part/tool/llm types first; v1/compat schemas skipped |
| `core` | `recode-core` + `recode-tool` + `recode-storage` | partial | Runner, config, agents, permissions, LSP, MCP |
| `ai` | `recode-provider` | partial | Canonical LLM events; anthropic/openai/gemini |
| `server` | `recode-server` | missing | axum + SSE |
| `cli` | `recode-cli` | partial | run/serve/tui/auth/models/sessions |
| `tui` | `recode-tui` | missing | ratatui |
| `protocol` | `recode-server::routes` | missing | Reimplement public HTTP surface, not exact |
| `client` | internal `EngineClient` | missing | Rust trait, no codegen |
| `sdk` | n/a | n/a | No JS SDK in v1 |
| `web`, `app`, `desktop`, `console`, `stats` | n/a | n/a | Out of scope |
| `plugin`, `plugins` | `recode-core::hooks` | missing | Rust traits + MCP; no JS sandbox |
| `pty` | `recode-tool::bash` + server PTY | partial | Non-interactive first |
| `codemode` | — | missing | Consider later (Lua/WASM sandbox) |

## Capabilities

| Capability | opencode | Re:Code |
|---|---|---|
| Streaming text/reasoning | yes | planned (M1) |
| Parallel tool calls | yes | planned (M2) |
| Provider retry/failover | yes | planned (M1) |
| Session persistence | SQLite | SQLite event store (M0) |
| Resume / fork / revert | yes | M5/M7 |
| Compaction | yes | M7 |
| Permissions (allow/ask/deny) | yes | M2 |
| AGENTS.md instructions | yes | M3 |
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
