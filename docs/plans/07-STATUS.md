# 07 — Live status

Last updated: 2026-11-04 (verified build)

## Current state

**Phase:** M2 complete, M3 complete, M4/M5 verified; M7 next.

```
cargo test --workspace       75 passed, 0 failed
cargo clippy --workspace --all-targets -- -D warnings   clean
cargo fmt --all --check       clean
```

### Milestones

| Milestone | State | Evidence |
|---|---|---|
| M0 foundation | ✅ done | 8 crates, CI, fmt/clippy, MIT attribution, `recode --version` |
| M1 providers + loop | ✅ done | OpenAI + Anthropic SSE; 2 streaming integration tests; mock-provider E2E |
| M2 tools | ✅ done | bash, read, write, edit, multi_edit, patch, ls, glob, grep, webfetch, todowrite, task (subagent), question (TUI modal + HTTP broker) |
| M3 config/agents | ✅ done | JSONC (+BOM), JSONC merge, 4 built-in agents, AGENTS.md/CLAUDE.md, auth store |
| M4 TUI | ✅ verified | transcript, streaming markdown-lite, editor, pickers, permission modal, todo strip — driven in a real ConPTY (`script/tui_smoke.py`), including the approval modal |
| M5 server | ✅ core | axum API + SSE verified with curl; bearer auth; permission broker (`GET /api/permissions`, `POST /api/permissions/{id}`) verified end to end |
| M6 LSP/MCP | ⬜ missing | — |
| M7 sessions at scale | 🟡 partial | manual + automatic compaction (durable `session.compacted`), JSON export; fork/revert/snapshots missing |
| M8 polish | ⬜ missing | — |

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
  final answer to the parent (integration test).
- Patch: unified diffs with multi-file hunks, CRLF preservation, and all-or-nothing
  validation; `patch` is registered as an edit-class tool (unit tests).
- Questions: a `question` tool call surfaces a modal in the TUI (verified through
  `script/tui_smoke.py --question`) and a pending request in `GET /api/questions` that
  `POST /api/questions/{id}` resolves; the tool returns the selected labels to the model
  (integration test plus curl E2E).
- `config show` redacts credentials; `config rebuild` replays the event log.

## Next actions (in order)

1. Fork/revert sessions and markdown export; then file snapshots for pre-edit revert.
2. LSP diagnostics manager (M6) then MCP client.
3. TUI polish: `@file` completion, session search, scrollback paging.
4. Add `recode run --format json` streaming (currently final transcript only).
5. Release plumbing: installer, shell completions, benchmarks.

## Known issues / gotchas

- **Rust toolchain:** installed manually at `C:\Users\meow\rust\1.99.1` and linked into
  rustup as custom toolchain `local`. `rustup default local` is required. Clippy and
  rustfmt were copied into that toolchain's `bin`. Cargo uses the rsproxy sparse mirror
  (`~/.cargo/config.toml`); the network to static.rust-lang.org is throttled to ~20 KB/s.
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
- **DB is pre-1.0:** schema changes may require deleting `<data>/recode.db`. `config rebuild`
  reconstructs projections but not the event log itself.
- **TUI testing:** `script/tui_smoke.py` needs `pywinpty` (installed for CPython 3.15) and a
  running mock provider; it asserts on ANSI-stripped ConPTY output.
- **Subagents:** depth is carried in session metadata (`subagentDepth`), capped at 2; the
  `task` tool is permission-checked like any other tool.
- **Questions:** `QuestionService` uses an in-process responder when present, otherwise the
  bus broker; TUI answers call `Engine::questions.reply` directly from the modal. `--auto`
  selects the first option for each question; headless `sessions` commands reject.

## Architecture notes

- `Engine` owns `Store`, `ConfigManager`, `Project`, `AgentRegistry`, `ToolRegistry`,
  `Providers`, `PermissionService`, a broadcast bus, and the active-run map.
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
