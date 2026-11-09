# 05 — Parity matrix (opencode → Re:Code)

Status legend: `missing` · `partial` · `done` · `n/a` (explicit non-goal)

## Packages

| opencode package | Re:Code crate/module | Status | Notes |
|---|---|---|---|
| `schema` | `recode-schema` | done | Core message/part/tool/llm/config types; v1/compat schemas intentionally skipped |
| `core` | `recode-core` + `recode-tool` + `recode-storage` | partial | Runner, config, agents, permissions, compaction, subagents, snapshots, LSP, MCP, opt-in formatters |
| `ai` | `recode-provider` | partial | Canonical LLM events; anthropic/openai-compatible (Gemini via OpenAI endpoint); OpenCode Zen free tier; models.dev snapshot/routing |
| `server` | `recode-server` | done | axum + SSE, permission/question brokers, file + model/agent + compact routes |
| `cli` | `recode-cli` | done | run/tui/serve/auth/models/sessions/config; `--server` attach |
| `tui` | `recode-tui` | done | local + attach modes, `@file`, session search, modals |
| `protocol` | `recode-server` | partial | Reimplemented public HTTP surface, not exact |
| `client` | `recode-client` | done | Typed HTTP client used by TUI/CLI attach; no codegen |
| `sdk` | n/a | n/a | No JS SDK in v1 |
| `web`, `app`, `desktop`, `console`, `stats` | n/a | n/a | Out of scope |
| `plugin`, `plugins` | `recode-core::hooks` | missing | Rust traits + MCP; no JS sandbox |
| `pty` | `recode-tool::bash` + server PTY | partial | Non-interactive first |
| `codemode` | — | missing | Consider later (Lua/WASM sandbox) |

## Capabilities

| Capability | opencode | Re:Code |
|---|---|---|
| Streaming text/reasoning | yes | done |
| models.dev provider/model catalog | yes | done (chat-completions/messages routes; `recode models --refresh`) |
| OpenCode Zen free tier | yes | done (no account; paid models hidden until a key exists) |
| Console device OAuth | yes | done (device flow, auth.json-compatible storage, automatic refresh) |
| Parallel tool calls | yes | done |
| Provider retry/failover | yes | done (retry; no cross-provider failover yet) |
| Session persistence | SQLite | SQLite event store (done) |
| Resume a session | yes | done (`--session`, TUI picker) |
| Fork / revert | yes | done (transcript fork, message revert, file restore) |
| Session compaction | yes | done (manual + automatic) |
| Subagents (`task`) | yes | done (depth-limited child sessions) |
| Permissions (allow/ask/deny) | yes | done (in-process + HTTP broker) |
| Model questions | yes | done (`question` tool: TUI modal, CLI prompt, HTTP broker) |
| AGENTS.md instructions | yes | done |
| Custom agents | yes | done (JSON, plus opencode-style `agent(s)`/`mode(s)` markdown) |
| Custom commands | yes | done (markdown `command(s)`, arguments, shell interpolation; subagent flag parsed, not executed) |
| LSP diagnostics | yes | done (on-demand client; diagnostics in tool results) |
| MCP | yes | done (stdio + streamable HTTP, tools bridged) |
| Plugins (JS) | yes | n/a (Rust hooks) |
| Worktrees | yes | missing (post-1.0) |
| File snapshots | yes | done (shadow git + restore) |
| Share/export | yes | partial (local JSON/Markdown; no hosted share) |
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
