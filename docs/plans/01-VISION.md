# 01 — Vision

## What

**Re:Code** is a from-scratch, Rust-native terminal AI coding agent. It is a port of the
opencode architecture (TypeScript/Bun/Effect) to a single static binary with an async Rust core.

It keeps what makes opencode good — provider neutrality, a real agent loop with tools,
sessions that survive restarts, a client/server split, permissions, LSP awareness, MCP —
and removes the Bun/Node runtime, the Effect runtime, and the JS dependency graph from the
critical path.

## Goals

1. **One binary, zero runtime deps.** `recode` runs on a fresh Windows/macOS/Linux box.
2. **Fast startup and low memory.** Target < 30 ms cold start for `--version`; < 80 MB RSS idle.
3. **Provider neutral.** Anthropic, OpenAI-compatible (OpenRouter, Groq, DeepSeek, Ollama,
   LM Studio, vLLM), Google Gemini. Streaming tool calls everywhere.
4. **Durable sessions.** SQLite event log; resume, fork, export, replay.
5. **Safe by default.** Permission rules per tool/pattern, project sandbox for file edits,
   explicit approval prompts in interactive mode.
6. **Headless + interactive.** Same engine: `recode run` (scriptable), `recode` (TUI),
   `recode serve` (HTTP+SSE for editor/remote clients).
7. **Extensible.** MCP client (stdio/HTTP), LSP diagnostics, custom agents/commands/skills.
8. **Correct concurrency.** Cancellation-safe tool execution, no orphaned child processes.

## Non-goals (v1)

- Web/desktop GUI parity with opencode's Solid app.
- Byte-for-byte compatibility with opencode's database or plugin API.
- Reimplementing every provider SDK; use wire protocols directly.
- A JavaScript plugin sandbox (Rust-native extension traits + MCP instead).

## Principles

- **Boring, explicit Rust.** Own the loop; no hidden frameworks.
- **Events are the truth.** Every state change is an event; projections are derived.
- **Boundaries are typed.** Validate once at the edge (serde), pass typed values inward.
- **Tools are capabilities.** A tool declares its schema, permission action, and sandbox needs.
- **Small crates, clear edges.** Schema → core → provider/tool → server/TUI/CLI.
- **Test the loop, not the mock.** Wire-level provider tests with recorded SSE fixtures.
- **Every phase leaves a usable binary.**
