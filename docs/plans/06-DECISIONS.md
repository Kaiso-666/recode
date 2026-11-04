# 06 — Decisions (ADR log)

Append-only. Number, date, decision, context, consequences.

## ADR-001 — Separate repository, not a branch of the fork
2026-11-04. The opencode fork is kept as read-only reference at `projects/opencode`.
Re:Code lives at `projects/recode` with its own history and MIT attribution.
**Why:** the Rust port shares architecture, not source; mixing 9k TS files makes builds,
licensing review, and release artifacts confusing. **Consequence:** we cherry-pick ideas,
never code; upstream changes are reviewed manually.

## ADR-002 — rusqlite (bundled) over sqlx
2026-11-04. Sync `rusqlite` behind `spawn_blocking` + a small connection pool.
**Why:** no async SQL build-time macros, far lower compile cost on this machine, full SQLite
control (WAL, FTS5 later). **Consequence:** write path is serialized per connection; use WAL
and a single writer with a mutex. Revisit if write throughput matters.

## ADR-003 — Event log is authoritative
2026-11-04. All mutations append an event; projections are derived tables.
**Why:** resumability, replay, fork, audit, and server fan-out come free. **Consequence:**
every new feature needs an event + projection test; projections carry a schema version.

## ADR-004 — Provider wire protocols, no vendor SDK
2026-11-04. Implement Anthropic Messages and OpenAI Chat Completions over HTTP/SSE directly.
**Why:** SDKs assume JS/Python runtimes; wire protocols are stable and cover ~all providers.
**Consequence:** maintenance burden for API drift; covered by fixture tests.

## ADR-005 — JSONC config, camelCase on disk, snake_case in Rust
2026-11-04. Use serde rename_all="camelCase" for config. Accept opencode-shaped keys where
sensible so users can migrate. **Consequence:** two config dialects documented explicitly.

## ADR-006 — TUI is a client, not the engine
2026-11-04. `recode-tui` depends on an `EngineClient` trait with in-process and HTTP impls.
**Why:** headless tests, remote sessions, no UI-coupled state. **Consequence:** TUI actions
must be expressible as API calls.

## ADR-007 — Windows-first development
2026-11-04. Primary dev machine is Windows (AMD A6, 7 GB). Keep dependency tree lean;
avoid crates requiring heavy C++ builds; test path/PTY behavior on Windows first.
**Consequence:** Unix-specific features gated with `cfg`; CI later covers 3 OSes.
