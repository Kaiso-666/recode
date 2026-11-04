# Tools

Every tool has a JSON Schema, a permission key, a timeout, and a path sandbox.
Paths must resolve inside the project directory (symlinks included); the only
exception is the subagent/fork session, which shares the parent worktree.

| Tool | Permission | Default | Purpose |
|---|---|---|---|
| `read` | `read` | allow | Read a file with line numbers; images/PDFs become attachments |
| `write` | `edit` | ask | Create or overwrite a whole file |
| `edit` | `edit` | ask | Replace one exact string (fails on ambiguous matches) |
| `multi_edit` | `edit` | ask | Apply several string replacements atomically |
| `patch` | `edit` | ask | Apply a unified diff to one or more files |
| `ls` | `ls` | allow | List a directory |
| `glob` | `glob` | allow | Find files by pattern, honouring ignores |
| `grep` | `grep` | allow | Regex search with context |
| `bash` | `bash` | ask | Run PowerShell (Windows) or `sh`, capped and cancellable |
| `webfetch` | `webfetch` | ask | Fetch a URL and convert to text/markdown |
| `todowrite` | `todowrite` | allow | Replace the session todo list |
| `task` | `task` | ask | Run a child session (subagent), returns its final answer |
| `question` | `question` | allow | Ask the user one or more multiple-choice questions |
| `mcp__*` | `mcp` | ask | Any tool exposed by a configured MCP server |

## Behaviour notes

- `read` refuses binary files and off-project paths. Line numbers are included
  so the model can cite locations.
- `edit` requires the old string to occur exactly once; use `patch` or
  `multi_edit` for larger changes. CRLF is preserved.
- `bash` starts the process in the project root, scrubs credential-like
  environment variables, caps combined output, and kills the process tree on
  timeout or abort. It is not an interactive PTY.
- `webfetch` follows redirects with a size cap and a timeout.
- `task` runs a subagent (`general` or `explore` by default) in a child session.
  Depth is stored in session metadata and capped at 2.
- `question` returns the selected option labels to the model; the TUI shows a
  modal and the server exposes `GET /api/questions` + `POST /api/questions/{id}`.
- After a successful `write`/`edit`/`patch`, an enabled formatter runs first,
  then LSP diagnostics for the changed files are appended to the tool result
  (`--- diagnostics ---`).

## Permission patterns

`permission_patterns` supplies values for the glob rules. For example `bash`
uses the command string, `edit` uses the resolved path, and MCP tools use
`<server>:<tool>`. A rule such as `{"bash": {"git status*": "allow"}}` matches
those values.

## Adding a tool

1. Implement `recode_tool::Tool` in `crates/recode-tool/src`.
2. Register it in `ToolRegistry::builtin`.
3. Pick a permission key and default decision; call `context.resolve_path` for
   every filesystem argument.
4. Add unit tests for the schema, sandbox escapes, and the dangerous path.
5. Mention the permission key in `docs/config.md` if it is new.
