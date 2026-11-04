# Contributing to Re:Code

Thanks for helping. Re:Code is a ground-up Rust implementation of the opencode
architecture: a provider-neutral agent loop, durable sessions, typed tools,
LSP/MCP, a TUI, and an HTTP API — no Node, Bun, or JS runtime.

## Setup

- Rust 1.83 or newer (`rustup default stable`).
- `cargo`, `rustfmt`, and `clippy` on `PATH`.
- Optional: `git` (file snapshots), a language server, `python` for the mock
  providers used by the integration tests.

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

All four must pass before a PR. The CI runs the same gates on Linux, Windows,
and macOS.

## Architecture in one minute

| Crate | Role |
|---|---|
| `recode-schema` | IDs, messages, parts, events, LLM contracts, config types |
| `recode-storage` | SQLite event store + rebuildable projections |
| `recode-provider` | Anthropic / OpenAI-compatible SSE clients |
| `recode-tool` | Tool registry, permission metadata, built-in tools |
| `recode-core` | Config, agents, permissions, engine, runner, compaction, LSP, MCP, snapshots |
| `recode-client` | Typed HTTP client for attach-mode clients |
| `recode-server` | axum API + SSE feeds |
| `recode-tui` | ratatui app over a local engine or an HTTP snapshot |
| `recode-cli` | `recode` binary |

The event log is the source of truth. Tools never write events; the runner
persists tool results and broadcasts them. Projections are a pure fold and can
be rebuilt with `recode config rebuild`.

## Testing with the mock provider

The integration tests start tiny in-process HTTP/SSE servers, so they need no
credentials. For manual runs use the scripted provider:

```sh
python script/mock_provider.py --port 18890 --scenario read
```

Point a project `recode.json` at `http://127.0.0.1:18890/v1` (provider `mock`,
api `openai`) and run `recode run --auto "read a.txt"`.

The TUI has a real-ConPTY smoke test (requires `pywinpty`):

```sh
python script/tui_smoke.py --project <dir> --config <dir> --data <dir> \
  [--permission] [--question] [--write --undo] [--complete] [--search] \
  [--extra "tui --server http://127.0.0.1:4096 --token T"]
```

## Pull requests

- Keep changes focused; one feature or fix per PR.
- Add a test at the lowest layer that can prove the behaviour. Protocol code
  gets a mock server; UI behaviour gets a smoke assertion.
- Run `cargo fmt` before committing.
- Update `docs/plans/07-STATUS.md` (and the roadmap/parity matrix when a
  milestone changes) if the change alters the public state of the project.
- Do not add a dependency when the standard library or an existing crate is
  enough. Windows-first, small binaries, no async runtime beyond Tokio.

## Commit style

Imperative subject, optional `feat:`/`fix:`/`docs:` prefix. Examples from the
history: `feat: patch and interactive question tools`,
`feat(tui): @file completion and session search`.

## Licensing

Contributions are MIT. Re:Code is derived from the architecture of
[opencode](https://github.com/anomalyco/opencode) (MIT); keep attribution intact.
