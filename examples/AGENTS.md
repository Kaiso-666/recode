# Project instructions

These lines are loaded from `AGENTS.md` (or `CLAUDE.md`) at the project root and
prepended to every primary agent's system prompt. Keep them short and specific.

- Build and test with `cargo test --workspace`; never commit generated files.
- Prefer small, focused changes with a test that proves the behaviour.
- Do not touch files under `secrets/`.
