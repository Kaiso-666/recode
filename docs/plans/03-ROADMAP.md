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
- [x] `bash`, `read`, `write`, `edit`, `multi_edit`, `patch`, `glob`, `grep`, `ls`, `webfetch`, `todowrite`, `task` (subagent), `question`.
- [x] Permission engine (allow/ask/deny, wildcard patterns, per-project overrides).
- [x] Sandbox path policy + bash env scrub + output caps.
- **Exit:** agent can fix a seeded bug in a temp repo without human input (auto-approve). 🟡 (tool loop proven; seeded-bug fixture pending)

## M3 — Config, agents, instructions
- [x] JSONC config, env expansion, provider/model registry, default model selection.
- [x] Agent definitions (build/plan/general/explore), per-agent prompts/tools/permissions.
- [x] opencode-style markdown agents: `agent(s)/**/*.md`, `mode(s)/*.md`, frontmatter
  fields, global/`.opencode`/`.recode` precedence, project overrides global.
- [x] custom markdown commands: `command(s)/**/*.md`, `$ARGUMENTS`/`$1..$N`, inline
  shell interpolation, agent/model selection, TUI and CLI expansion.
- [x] `AGENTS.md` / `CLAUDE.md` discovery + instructions injection.
- [x] `recode auth login|logout|list`, key storage.
- **Exit:** `recode run --agent plan` restricts writes; project instructions respected. ✅

## M4 — TUI
- [x] ratatui app: transcript, streaming markdown-lite, tool cards, input editor.
- [x] session picker/resume, model picker, agent switcher, permission modal.
- [x] keybinds, mouse scroll, paste, history, `/commands`, `@file` completion, session search, scrollback paging.
- [x] Hand-test in a real terminal: resize, paste, permission modal, pickers.
- **Exit:** full interactive session, resumable after restart, no flicker at 60fps. ✅

## M5 — Server + clients
- [x] axum API: sessions, messages, prompt, abort, config, providers, health, events SSE.
- [x] auth token, CORS, request tracing, OpenAPI JSON stub.
- [x] Server-side permission replies (`permission.asked` SSE + POST reply).
- [x] TUI/CLI attach to `recode serve` via `--server` (`recode tui --server URL --token T`, `recode run --server URL`).
- **Exit:** two clients observe one session live. ✅ (attach TUI polls the API and follows a server-side run; verified in ConPTY for prompts, permission and question modals)

## M6 — Code intelligence
- [x] LSP manager: spawn per-project servers on demand, diagnostics after edits (built-ins for Rust, TypeScript, Python, Go, C/C++).
- [x] MCP client: stdio + streamable HTTP; tools registered as `mcp__<server>__<tool>`.
- [x] Formatter hooks (rustfmt/gofmt/ruff/prettier/biome) after writes, opt-in.
- **Exit:** diagnostics appear in tool results; an MCP filesystem server's tool is callable. ✅ (LSP and MCP verified against protocol-accurate mock servers; real servers use the same code path)

## M7 — Sessions at scale
- [x] Compaction, summaries, token-budget auto-compact.
- [x] Fork/revert (transcript fork, message revert, file restore).
- [x] File snapshots + revert (shadow git), patch tracking.
- [x] Share/export (markdown/JSON).
- **Exit:** compact a 200k-token session; revert a file to pre-edit state. ✅ (file restore verified end to end; large-session budget exercised with the compaction fixture)

## M8 — Polish / release
- [x] Windows/macOS/Linux installers, shell completions, `recode upgrade` (SHA-256 verified).
- [x] GitHub release workflow publishing per-platform binaries.
- [x] Docs: `docs/config.md`, `docs/tools.md`, `CONTRIBUTING.md`, examples.
- [x] Release profile builds and runs locally (`--release --locked`, thin LTO).
- [ ] Benchmarks in CI, fuzz SSE/config parsers, signed binaries.
- **Exit:** signed release binaries for win/mac/linux; v1.0.0 tag. 🟡 (unsigned artifacts; signing key pending)

## M9 — opencode parity
- [x] OpenCode Zen provider: current free models, automatic `public` key when no credential,
  `OPENCODE_DISABLE_FREE_MODELS` switch, picker + bare model ids.
- [x] models.dev catalog: `recode models --refresh`, compact cache, provider/model routing,
  context limits and costs. Only the OpenAI-compatible and Anthropic protocols are imported.
- [x] OpenCode Console device OAuth for paid Zen models, with token refresh.
- [ ] Diff the CLI/config/TUI surface against the upstream fork and close gaps.
- **Exit:** an opencode user can move their config and habits over without surprises.
