# 03 — Roadmap

Each milestone ends with a green `cargo test` and a usable binary. No milestone is skipped.

## M0 — Foundation (scaffold)
- [x] Cargo workspace, CI config, rustfmt/clippy, license/attribution.
- [x] `recode --version`, `recode --help`.
- [x] `recode-schema`: ID newtypes, message/part/event types, serde round-trip tests.
- [x] `recode-storage`: SQLite migrations, append/load events, projection rebuild.
- **Exit:** `cargo test` green; events survive reopen. ✅

## M1 — Provider + loop (headless core)
- [x] OpenAI-compatible streaming client (SSE, tool calls, usage).
- [x] Anthropic Messages streaming client.
- [x] `recode run "prompt"` end-to-end against a local mock SSE server.
- [x] Retry/backoff, cancellation, token accounting.
- **Exit:** scripted conversation with 2 tool calls completes; transcript persisted. ✅

## M2 — Tools
- [x] Registry with JSON Schema, permission action, timeout, truncation.
- [x] `bash`, `read`, `write`, `edit`, `multi_edit`, `glob`, `grep`, `ls`, `webfetch`, `todowrite`.
- [ ] `patch`, `task` (subagent), `question`.
- [x] Permission engine (allow/ask/deny, wildcard patterns, per-project overrides).
- [x] Sandbox path policy + bash env scrub + output caps.
- **Exit:** agent can fix a seeded bug in a temp repo without human input (auto-approve). 🟡 (tool loop proven; seeded-bug fixture pending)

## M3 — Config, agents, instructions
- [x] JSONC config, env expansion, provider/model registry, default model selection.
- [x] Agent definitions (build/plan/general/explore), per-agent prompts/tools/permissions.
- [x] `AGENTS.md` / `CLAUDE.md` discovery + instructions injection.
- [x] `recode auth login|logout|list`, key storage.
- **Exit:** `recode run --agent plan` restricts writes; project instructions respected. ✅

## M4 — TUI
- [x] ratatui app: transcript, streaming markdown-lite, tool cards, input editor.
- [x] session picker/resume, model picker, agent switcher, permission modal.
- [x] keybinds, mouse scroll, paste, history, `/commands`; `@file` completion pending.
- [ ] Hand-test in a real terminal: resize, paste, permission modal, pickers.
- **Exit:** full interactive session, resumable after restart, no flicker at 60fps. 🟡

## M5 — Server + clients
- [x] axum API: sessions, messages, prompt, abort, config, providers, health, events SSE.
- [x] auth token, CORS, request tracing, OpenAPI JSON stub.
- [ ] Server-side permission replies (`permission.asked` SSE + POST reply).
- [ ] TUI/CLI can attach to `recode serve` via `--server`.
- **Exit:** two clients observe one session live.

## M6 — Code intelligence
- [ ] LSP manager: spawn per-project servers, diagnostics after edits, hover/symbols.
- [ ] MCP client: stdio + streamable HTTP; tools/resources/prompts bridged.
- [ ] Formatter hooks (rustfmt/prettier/biome) after writes, opt-in.
- **Exit:** diagnostics appear in tool results; an MCP filesystem server's tool is callable.

## M7 — Sessions at scale
- [ ] Fork/revert/compaction, summaries, token-budget auto-compact.
- [ ] File snapshots + revert (shadow git), patch tracking.
- [ ] Share/export (markdown/JSON) — export exists; markdown/share pending.
- **Exit:** compact a 200k-token session; revert a file to pre-edit state.

## M8 — Polish / release
- [ ] Windows installer, shell completions, `recode upgrade`.
- [ ] Benchmarks (startup, RSS, tokens/s overhead), fuzz SSE/config parsers.
- [ ] Docs site content, examples, CONTRIBUTING.
- **Exit:** signed release binaries for win/mac/linux; v1.0.0 tag.
