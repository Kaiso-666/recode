# Re:Code

**A Rust-native AI coding agent.** Re:Code is a ground-up port of the
[opencode](https://github.com/anomalyco/opencode) architecture — provider-neutral agent loop,
durable sessions, tools, permissions, LSP, MCP, TUI and HTTP server — from TypeScript/Bun/Effect
to a single async Rust binary.

```
cargo install --path crates/recode-cli
recode
```

> Status: early development. See `docs/plans/07-STATUS.md` for the honest live state.

## Why

- **No runtime.** No Node, no Bun, no JIT warmup, no `node_modules`.
- **Startup in milliseconds.** Small RSS, predictable performance.
- **Durable by design.** Every session is an append-only event log in SQLite.
- **Provider neutral.** Anthropic, OpenAI-compatible endpoints, Gemini — same loop.
- **Safe tools.** Typed capabilities with allow/ask/deny rules and path sandboxing.

## Workspace

| Crate | Responsibility |
|---|---|
| `recode-schema` | Domain types: IDs, messages, parts, events, LLM contracts, config |
| `recode-storage` | SQLite event store and projections |
| `recode-provider` | Anthropic / OpenAI-compatible / Gemini streaming clients |
| `recode-tool` | Tool registry and built-in tools |
| `recode-core` | Agent loop, sessions, config, permissions, LSP, MCP |
| `recode-server` | HTTP + SSE API |
| `recode-tui` | Terminal UI |
| `recode-cli` | `recode` binary |

## Development

```
cargo build
cargo test
cargo clippy --all-targets
cargo fmt
```

Configuration lives at `<config>/recode/recode.json`; data at `<data>/recode/recode.db`.
`RECODE_CONFIG`, `RECODE_DATA`, and `RECODE_LOG` override the locations.

## Credits

Architecture and product design are derived from opencode (MIT). Re:Code is an independent
Rust implementation and shares no runtime code with the original.

## License

MIT. See `LICENSE`.
