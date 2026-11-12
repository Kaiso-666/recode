# 07 — Live status

Last updated: 2026-11-09 (verified release build + benchmark)

## Current state

**Phase:** M2–M8 complete (signing pending); M9 opencode parity started with the
OpenCode Zen free tier, models.dev, Console OAuth, and markdown agents/commands.

```
cargo test --workspace       120 passed, 0 failed
cargo clippy --workspace --all-targets -- -D warnings   clean
cargo fmt --all --check       clean
cargo build --release --locked --bin recode             verified (20m07s, jobs=1)
startup                      mean 39.1 ms, p50 38.6 ms, p95 46.7 ms (20 runs)
binary                       18.0 MiB
```

### Milestones

| Milestone | State | Evidence |
|---|---|---|
| M0 foundation | ✅ done | 8 crates, CI, fmt/clippy, MIT attribution, `recode --version` |
| M1 providers + loop | ✅ done | OpenAI + Anthropic SSE; 2 streaming integration tests; mock-provider E2E |
| M2 tools | ✅ done | bash, read, write, edit, multi_edit, patch, ls, glob, grep, webfetch, todowrite, task (subagent), question (TUI modal + HTTP broker) |
| M3 config/agents | ✅ done | JSONC (+BOM), JSONC merge, 4 built-in agents, opencode-style markdown agents and slash commands (`.opencode`/`.recode`/global), AGENTS.md/CLAUDE.md, auth store |
| M4 TUI | ✅ verified | transcript, streaming markdown-lite, editor, pickers, `/` command completion, permission modal, todo strip — driven in a real ConPTY (`script/tui_smoke.py`), including the approval modal |
| M5 server | ✅ done | axum API + SSE verified with curl; bearer auth; permission and question brokers; `recode-client`; TUI/CLI attach (`--server`) verified in ConPTY |
| M6 LSP/MCP | ✅ done | on-demand LSP diagnostics in tool results; MCP stdio + streamable HTTP tools (`mcp__*`); opt-in formatter hooks |
| M7 sessions at scale | ✅ done | compaction (uses `smallModel` when set); fork/revert; shadow-git snapshots + file restore; patch tracking; JSON/Markdown export |
| M8 polish | 🟡 signing pending | installers, completions, checksum-verified upgrade, release workflow, docs; release build and startup benchmark measured; signing pending |
| M9 parity | 🟡 started | OpenCode Zen free models work without an account; models.dev catalog refresh, per-model routing and Console device OAuth with refresh are wired |

### Verified end-to-end

- `recode run --auto "read a.txt"` against `script/mock_provider.py`:
  tool call executed, transcript persisted, exit 0, token summary printed.
- `recode sessions list` / `sessions show` read the persisted transcript back.
- `recode serve` + curl: `/health`, `/api/providers`, session create, prompt,
  `/messages`, and the SSE event stream all returned the expected payloads.
- Permission broker: with no `--auto`, a prompt produced a pending request, the TUI modal
  and the HTTP reply endpoint both approved it, and the tool completed.
- TUI: `script/tui_smoke.py` drives the real binary in a ConPTY (via pywinpty), sends a
  prompt, waits for the streamed reply, opens help, and quits; the permission variant also
  approves the modal.
- Compaction: `recode sessions compact <id>` writes a `session.compacted` event; a following
  request contains the summary instead of the original tool transcript (integration test).
- Subagents: a `task` tool call creates a child session, runs it to idle, and returns its
  final answer to the parent (integration test). Child sessions stay out of
  `Engine::sessions`/`all_sessions`, so the TUI picker, `/api/sessions`, `sessions list`,
  and `run --continue` only see top-level sessions; `Engine::child_sessions` exposes a
  parent's children for inspection.
- Patch: unified diffs with multi-file hunks, CRLF preservation, and all-or-nothing
  validation; `patch` is registered as an edit-class tool (unit tests).
- Questions: a `question` tool call surfaces a modal in the TUI (verified through
  `script/tui_smoke.py --question`) and a pending request in `GET /api/questions` that
  `POST /api/questions/{id}` resolves; the tool returns the selected labels to the model
  (integration test plus curl E2E).
- Sessions: `sessions fork` copies a transcript with fresh IDs, `sessions revert` removes
  messages after a boundary, and `sessions export --format md` renders a shareable
  transcript (verified on the mock-provider transcript). `POST /api/sessions/{id}/fork`
  and `/revert` (with `files: true`) work over HTTP.
- Remote sessions: `recode sessions --server URL --token T <list|show|delete|export|fork|revert|restore|compact>`
  drives a `recode serve` instance over HTTP. Verified end to end against the mock
  provider: `run --server` created the session, then list/show/export/fork/restore/compact/revert/delete
  all returned the expected output. `POST /api/sessions/{id}/restore` is the new server
  route behind `sessions restore`.
- File snapshots: every parent prompt commits the worktree to a shadow git repo under
  `<data>/snapshots/<project-id>`. A `write` tool edit was reverted by `sessions restore`
  and by the TUI `/undo` command (`script/tui_smoke.py --write --undo`), and the
  integration test asserts the snapshot part on the user message.
- Patch tracking: when a run changes files, the last assistant message receives a
  `PatchPart` listing them (integration test); the TUI and Markdown export render it.
- LSP: a mock language server publishes a diagnostic on `didOpen`; the integration test
  asserts the `write` tool's result contains `--- diagnostics ---` and the server's
  message (`tests/lsp.rs`). Unit tests cover diagnostic parsing and Windows file URIs.
- MCP: both transports are exercised end to end against protocol-accurate mock servers
  (`script/mock_mcp.py` and an in-test HTTP server). The model calls
  `mcp__mock__echo`, the registry routes it to the server, and the echoed text is
  persisted in the tool part (`tests/mcp.rs`).
- Markdown agents: `.recode/agent/reviewer.md` with `mode: primary` was selected with
  `recode run --agent reviewer` against the mock provider; the request log shows the file
  body as the system prompt, proving discovery, frontmatter parsing and precedence.
  Unit tests cover global `.opencode`/`.recode` precedence, `mode(s)` primary agents,
  nested ids, tool/permission maps, and JSON config winning over markdown.
- Agent models: an agent's `model` now drives new sessions and is adopted when switching
  agents through the TUI/CLI; an explicit `--model` still wins. Fixed a bug where
  `create_session` always resolved the global default instead.
- Custom commands: `.recode/command/review.md` with `agent: reviewer`, `$1`/`$ARGUMENTS`
  and ``!`echo shell-win` `` was run as `recode run "/review a.txt second"`; the model
  request carried the Reviewer system prompt and the expanded prompt with shell output.
  The same command was typed in the TUI and passed `script/tui_smoke.py` (prompt flag);
  its `/rev` completion row was asserted in the ConPTY, and F1 lists custom commands.
  Unit tests cover precedence, quoted arguments, positional/named substitution,
  append-without-placeholders, shell interpolation, and agent/model selection.
- Server-side commands: `GET /api/commands` lists markdown commands and
  `POST /api/commands/{name}` expands one in the session directory. `recode run --server`
  expanded `/review` over HTTP (curl returned the shell-expanded text) and the attached
  TUI uses the same endpoint, so shell interpolation runs on the server.
- Task agents: the `task` schema enum is injected from `AgentRegistry::all()` at engine
  startup, so custom markdown agents can be delegated to instead of the old hardcoded
  `general`/`explore` pair (unit tests).
- Release benchmark: `script/bench.ps1 -Runs 20` on the release binary measured startup at
  mean 39.1 ms / p50 38.6 ms / p95 46.7 ms and an 18.0 MiB executable; the release build
  took 20m07s with `CARGO_BUILD_JOBS=1`. The release binary also passed the markdown
  agent/command mock-provider run and the ConPTY TUI smoke (`--slash`).
- Formatters: an opt-in formatter command runs after a successful edit and rewrites the
  file (unit test with a Python one-liner); built-ins stay disabled until enabled.
- JSON output: `recode run --format json` streams one object per line (`delta`, `event`,
  `idle`) and ends with a `result` carrying the final text and usage; stdout stays pure
  JSONL while prompts/logs go to stderr (verified against `script/mock_provider.py`).
- TUI `@file` completion: typing `@` opens a filtered project-file menu (↑/↓, Tab/Enter, Esc);
  submitting expands `@path` into `<file path="…">` blocks (unit test + `script/tui_smoke.py
  --complete`, which types `read @a`, accepts the completion, and gets the mock reply).
- TUI session search: F4 opens the session picker and typing filters by title or id
  (`Ctrl+U` clears); verified in the ConPTY smoke test (`--search`).
- Attach mode: `recode tui --server URL --token T` runs the same TUI against
  `recode serve`. The client loads a snapshot, polls the API while busy, and answers
  permission/question requests through the HTTP brokers. Verified in ConPTY for a normal
  prompt, a `write` behind an `ask` rule (y → tool runs), and a `question` (Enter → answer
  returned to the model). `recode run --server URL --auto "…"` streams the assistant text
  and prints usage (text and JSON modes verified).
- `config show` redacts credentials; `config rebuild` replays the event log.
- Release plumbing: `recode completions <shell>` emits clap-generated scripts (1,268 lines
  for bash; PowerShell/zsh/fish verified to produce output). `recode upgrade --check` and
  `recode upgrade --yes` were exercised against a local GitHub-shaped API: lookup, checksum
  verification, and self-replacement on Windows all succeeded. `script/install.ps1`,
  `script/install.sh`, and `script/package.ps1` are in place, and
  `.github/workflows/release.yml` builds win/linux/macOS artifacts on a tag. The release
  profile (`--release --locked --bin recode`, thin LTO) built and the binary ran
  (`recode 0.1.0`) after `CARGO_BUILD_JOBS=1`; the earlier `lib.exe 0xc0000142` failure was
  concurrency-related and did not recur.
- models.dev catalog: `recode models --refresh` downloads the live catalog (228
  providers / 7,231 supported chat models at the last run), keeps a compact 2.4 MB snapshot at
  `<data>/models.json`, and reloads it. Provider lists, bare model ids, context limits, costs
  and per-model protocol/base-URL routing now come from it. The TUI picker shows configured
  providers only; `recode models --all` shows the full catalog. An E2E run used a synthetic
  catalog provider pointed at `script/mock_provider.py`: the `read` tool ran and the cost was
  zero, proving catalog route selection and `Providers::get_for` work end to end.
- OpenCode Console OAuth: `recode auth login opencode` runs the RFC 8628 device flow,
  stores the token in opencode-compatible `auth.json`, and `Providers` refreshes it shortly
  before expiry (serialized so concurrent steps cannot rotate the refresh token twice).
  Verified against a mock Console: login stored `device-access`; an expired credential was
  refreshed to `refreshed-access`, the inference request carried `Bearer refreshed-access`,
  and the rotated refresh token was persisted. `--key` keeps the API-key path.
- OpenCode Zen free tier: the built-in `opencode` provider targets
  `https://opencode.ai/inference/openai/v1`, lists the current free model set, and falls
  back to the literal free key `public` when no credential exists. `recode models` shows
  `opencode/big-pickle` and the others; bare `--model big-pickle` resolves too.
  `OPENCODE_DISABLE_FREE_MODELS=1` disables the fallback. The endpoint accepts the key; a
  live request from this machine returned the service's `FreeUsageLimitError`, so a
  successful free completion could not be captured here. Unit tests cover the wiring.

## Next actions (in order)

1. M9 parity: diff the CLI/config/TUI surface against the upstream fork and close gaps
   (themes/keybinds, share, subagent commands).
2. Wire the benchmark into CI and track regressions.
3. TUI attach over SSE (replace 4–20 Hz polling).
4. Signed release binaries and parser fuzz tests.

## Known issues / gotchas

- **Rust toolchain:** installed manually at `C:\Users\meow\rust\1.99.1` and linked into
  rustup as custom toolchain `local`. `rustup default local` is required. Clippy and
  rustfmt were copied into that toolchain's `bin`. Cargo uses the rsproxy sparse mirror
  (`~/.cargo/config.toml`); the network to static.rust-lang.org is throttled to ~20 KB/s.
- **Release builds:** use `CARGO_BUILD_JOBS=1` on this machine. The parallel build once
  failed in `cc-rs` with `lib.exe` status `0xc0000142` (`STATUS_DLL_INIT_FAILED`); the
  serial build completed in 16m51s. PowerShell treats cargo's stderr progress as an error
  record, so run builds through `cmd /c` or ignore the `NativeCommandError` wrapper.
- **Windows paths:** `Project::discover` canonicalizes to `\\?\C:\...`. Tool path checks
  strip the verbatim prefix and compare case-insensitively (`recode_tool::is_within`).
- **Config BOM:** PowerShell `Set-Content -Encoding utf8` writes a BOM; `jsonc::normalize`
  strips it (covered by a test).
- **Events on the bus:** `Engine::prompt` now broadcasts the user `MessageAdded`/`PartAdded`
  events; remote SSE clients see the prompt immediately.
- **Permissions:** `authorize` is conservative — a pattern that matches no rule falls back to
  the tool default; any `ask` forces approval; deny always wins. `always` replies add a
  session rule.
- **Streaming:** deltas broadcast immediately but persist in 2 KB / 60 ms batches as
  `PartDelta`; clients reload from the event log on durable events and on idle.
- **Casing:** event JSON currently mixes camelCase (inner structs) with snake_case (variant
  fields). Harmless internally; normalize before publishing a wire protocol.
- **OAuth credentials:** `auth.json` entries may be `{type:"api",key}` or
  `{type:"oauth",access,refresh,expires,metadata}`; `expires` is Unix milliseconds. A token
  counts as expired 60 s before its deadline. Refresh happens on the first provider build
  after expiry, updates `auth.json`, and serializes concurrent refreshes; a failed refresh
  asks the user to log in again. The engine reads `auth.json` at startup, so a login in
  another process needs a restart.
- **models.dev cache:** `<data>/models.json` is Re:Code's compact format, not the upstream
  document; delete it to fall back to built-ins. It tolerates a UTF-8 BOM. Only models whose
  catalog entry offers `chat-completions` or `messages` are imported, and deprecated entries
  are skipped. `recode models --refresh` is a manual network call; startup never fetches.
- **DB is pre-1.0:** schema changes may require deleting `<data>/recode.db`. `config rebuild`
  reconstructs projections but not the event log itself.
- **TUI testing:** `script/tui_smoke.py` needs `pywinpty` (installed for CPython 3.15) and a
  running mock provider; it asserts on ANSI-stripped ConPTY output.
- **Subagents:** depth is carried in session metadata (`subagentDepth`), capped at 2; the
  `task` tool is permission-checked like any other tool.
- **Questions:** `QuestionService` uses an in-process responder when present, otherwise the
  bus broker; TUI answers call `Engine::questions.reply` directly from the modal. `--auto`
  selects the first option for each question; headless `sessions` commands reject.
- **JSON output:** `--format json` is line-delimited: `delta`, `event`, `idle`, then one
  `result`. The final result intentionally omits the full transcript (use `sessions show`
  or the unchanged text mode for that).
- **File snapshots:** require `git` on `PATH` (missing git degrades to a warning, the run
  continues); disable with `"snapshot": false` in config. The shadow repo lives under
  `<data>/snapshots/<project-id>` with `GIT_DIR`/`GIT_WORK_TREE` env vars, so the user's
  `.git` is never touched. `.git` is always excluded; the project `.gitignore` is used as
  the excludes file. Restore removes files created after the snapshot but leaves ignored
  files alone, and does not recurse into nested repositories (gitlink limitation).
- **Patch parts:** computed with `git diff --name-only <pre-prompt snapshot>` at run end and
  attached to the last assistant message; no Patch part is written when nothing changed.
- **LSP:** servers start on demand for configured extensions (built-ins: rust-analyzer,
  typescript-language-server, pyright-langserver, gopls, clangd) and are cached per
  process. `lsp.<name>.enabled: false` disables one. Diagnostics wait at most 1.5 s and
  initialization at most 15 s; a failed server is not retried until restart. The client
  handles initialize, didOpen/didChange/didSave and publishDiagnostics only.
- **MCP:** enabled servers are contacted once before the first prompt, so the first prompt
  includes their startup time. Tools are named `mcp__<server>__<tool>` (sanitized, 64
  chars), use permission key `mcp` (default ask), and are visible to agent allowlists
  unless `"mcp": false` is set. Local transport is line-delimited JSON-RPC; remote uses
  POST + `Mcp-Session-Id`. Only tools are bridged (resources/prompts are not yet).
- **Formatters:** disabled by default; enable by key under `formatter`
  (`{"rustfmt": {"enabled": true}}`). The file path is appended as the last argument and
  commands time out after 15 s. Missing binaries are remembered for the process.
  Formatting runs before LSP diagnostics, so diagnostics reflect the formatted file.
- **Attach mode:** `recode tui --server …` (the `tui` subcommand is required; top-level
  flags are not global). The client polls; while busy it refreshes every ~320 ms, when
  idle every ~1.6 s. Polling replaces the rendered transcript from the server, so remote
  streaming is chunky but durable events are never lost. Server-side permissions/questions
  appear as normal modals and replies go through `POST /api/permissions|questions/{id}`.
  `@file` completion uses `GET /api/files`; submitting expands files with
  `GET /api/files/content`, so the TUI can run on a different machine. `/` completion
  fetches `GET /api/commands`, and entering one expands it through
  `POST /api/commands/{name}`. `run --server` rejects `--file` and, without `--auto`,
  rejects tool approvals (use the TUI to attach interactively). `recode sessions --server …`
  performs the same list/show/delete/export/fork/revert/restore/compact commands against
  the server, so a remote session store can be inspected without a local database.

## Architecture notes

- `Engine` owns `Store`, `ConfigManager`, `Project`, `AgentRegistry`, `CommandRegistry`,
  `ToolRegistry`, `Providers`, `PermissionService`, `QuestionService`, `SnapshotService`,
  `LspManager`, `McpManager`, `FormatterService`, a broadcast bus, and the active-run map.
- `recode-client` is the HTTP facade used by attach clients; the TUI's `Backend` wraps
  either an `Arc<Engine>` or a cached `Client`, so the app renders one session model in
  both modes. Writes are async; reads come from the cache or SQLite synchronously.
- Every durable write goes through `Store::append`/`append_batch`; projections update in the
  same SQLite transaction. `Store::rebuild` replays the log.
- The runner creates one assistant message per step; tool results live in `ToolPart.state`
  and are projected into provider messages by `prompt::to_llm_messages`.
- Tools never write events; the runner persists outcomes and broadcasts them.
- Windows `bash` tool runs PowerShell with `CREATE_NO_WINDOW`, scrubs credential-like env
  vars, caps output, and kills the process tree on timeout/abort.
- `recode-tool::todo` keeps a process-global per-session todo list; the TUI renders the first
  five items above the composer.

## Repo

- Local: `C:\Users\meow\projects\recode` (separate repo from the opencode fork by ADR-001).
- Reference: `C:\Users\meow\projects\opencode` (fork `Kaiso-666/opencode`).
