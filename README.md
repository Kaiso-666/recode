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
| `recode-client` | Typed HTTP client for attach-mode clients |
| `recode-server` | HTTP + SSE API |
| `recode-tui` | Terminal UI (local or attached) |
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

### Attach to a server

`recode serve` owns the engine and can serve several clients. Run the TUI from
another terminal or machine against that process:

```
recode serve --port 4096 --token secret      # one terminal
recode tui --server http://127.0.0.1:4096 --token secret
```

The attached TUI shows the same sessions, streams local runs, and answers
permission and question prompts over HTTP. `recode run --server URL --token T
--auto "…"` is the headless equivalent; without `--auto` tool approvals are
rejected. Tokens may also come from `RECODE_SERVER_TOKEN`.

### Language servers and MCP

LSP servers start on demand for edited files. Built-ins cover Rust, TypeScript,
Python, Go and C/C++; disable or add one under `lsp`:

```jsonc
{
  "lsp": {
    "rust": { "enabled": false },
    "custom": { "command": ["my-lsp", "--stdio"], "extensions": [".my"] }
  }
}
```

MCP servers are connected before the first prompt. Their tools appear to the
model as `mcp__<server>__<tool>` and use the `mcp` permission key:

```jsonc
{
  "mcp": {
    "files": {
      "type": "local",
      "command": ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
    },
    "remote": {
      "type": "remote",
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer …" }
    }
  }
}
```

## Credits

Architecture and product design are derived from opencode (MIT). Re:Code is an independent
Rust implementation and shares no runtime code with the original.

## License

MIT. See `LICENSE`.
