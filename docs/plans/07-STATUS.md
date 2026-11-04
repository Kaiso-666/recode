# 07 — Live status

Last updated: 2026-11-04 (verified build)

## Current state

**Phase:** M3 complete, M4/M5 partial, all tests green.

```
cargo test --workspace       60 passed, 0 failed
cargo clippy --workspace --all-targets -- -D warnings   clean
cargo fmt --all --check       clean
```

### Milestones

| Milestone | State | Evidence |
|---|---|---|
| M0 foundation | ✅ done | 8 crates, CI, fmt/clippy, MIT attribution, `recode --version` |
| M1 providers + loop | ✅ done | OpenAI + Anthropic SSE; 2 streaming integration tests; mock-provider E2E |
| M2 tools | 🟡 11/12 | bash, read, write, edit, multi_edit, ls, glob, grep, webfetch, todowrite (patch/task/question missing) |
| M3 config/agents | ✅ done | JSONC (+BOM), JSONC merge, 4 built-in agents, AGENTS.md/CLAUDE.md, auth store |
| M4 TUI | 🟡 written | transcript, streaming markdown-lite, editor, pickers, permission modal, todo strip — not yet hand-tested in a real terminal |
| M5 server | 🟡 partial | axum API + SSE verified with curl; bearer auth; no permission reply endpoint |
| M6 LSP/MCP | ⬜ missing | — |
| M7 sessions at scale | 🟡 export only | — |
| M8 polish | ⬜ missing | — |

### Verified end-to-end

- `recode run --auto "read a.txt"` against `script/mock_provider.py`:
  tool call executed, transcript persisted, exit 0, token summary printed.
- `recode sessions list` / `sessions show` read the persisted transcript back.
- `recode serve` + curl: `/health`, `/api/providers`, session create, prompt,
  `/messages`, and the SSE event stream all returned the expected payloads.
- `config show` redacts credentials; `config rebuild` replays the event log.

## Next actions (in order)

1. Wire server-side permission replies: `permission.asked` on SSE + `POST /api/permissions/{id}`;
   today `serve` only auto-approves (`--auto`) or rejects.
2. Hand-test the TUI in Windows Terminal (resize, paste, permission modal, F2/F3/F4).
3. Session compaction (`/compact`, auto at token budget) and fork/revert.
4. `patch` tool and `task` subagent tool.
5. LSP diagnostics manager (M6) then MCP client.
6. Add `recode run --format json` event streaming (currently final transcript only).
7. Release plumbing: `cargo dist`/installer, completions, benchmarks.

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
