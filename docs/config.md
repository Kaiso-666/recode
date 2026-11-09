# Configuration

Re:Code reads JSONC (comments and trailing commas allowed). Two layers merge,
with the project winning:

| Layer | Path |
|---|---|
| Global | `<config>/recode/recode.json` (`%APPDATA%\recode` on Windows, `~/.config/recode` elsewhere) |
| Project | `recode.json`, then `recode.local.json` in the working tree |

`RECODE_CONFIG`, `RECODE_DATA`, `RECODE_DB`, and `RECODE_LOG` override the
directories. Credentials live in `<config>/recode/auth.json`; prefer
`recode auth login <provider>` over putting keys in a project file.

`recode config show` prints the merged result with secrets redacted, and
`recode config path` prints the active locations.

## Credentials

```sh
recode auth login anthropic          # prompts for an API key
recode auth login opencode           # OpenCode Console device flow
recode auth login opencode --key …   # API key instead of OAuth
recode auth login opencode --server https://opencode.ai/console
recode auth list
recode auth logout opencode
```

The OpenCode device flow prints a URL and a one-time code, waits for approval,
and stores the OAuth token in `auth.json` in opencode's shape. Access tokens are
refreshed automatically shortly before they expire and the rotated refresh token
is saved. The Console URL can also be set with `OPENCODE_CONSOLE_URL`.

## Minimal example

```jsonc
{
  "$schema": "https://recode.dev/schema.json", // informational only
  "model": "anthropic/claude-sonnet-4-20250514",
  "smallModel": "anthropic/claude-3-5-haiku-20241022",
  "defaultAgent": "build",
  "permission": {
    "read": "allow",
    "edit": { "*.env": "deny", "*": "ask" },
    "bash": "ask"
  }
}
```

## Models and providers

A model reference is `<provider>/<model>`. Built-in providers are always
listed; adding an entry can override their endpoint or models.

```jsonc
{
  "provider": {
    "openai": {
      "api": "openai",
      "options": {
        "apiKey": "sk-…",              // or use `recode auth login openai`
        "baseUrl": "https://api.openai.com/v1",
        "headers": { "X-Custom": "value" },
        "timeoutMs": 120000,
        "body": { "service_tier": "auto" }
      },
      "models": {
        "gpt-4.1": {
          "name": "GPT-4.1",
          "limit": { "context": 1000000, "output": 32768 },
          "cost": { "input": 2.0, "output": 8.0, "cacheRead": 0.5, "cacheWrite": 0 }
        }
      }
    },
    "local": {
      "api": "openai",
      "options": { "baseUrl": "http://127.0.0.1:11434/v1", "apiKey": "ollama" },
      "models": { "qwen2.5-coder:14b": {} }
    }
  }
}
```

`api` selects the wire protocol: `openai` or `anthropic`. Gemini is reached
through its OpenAI-compatible endpoint. When `smallModel` is set, compaction
summaries use it; otherwise they use the session model.

### models.dev catalog

`recode models --refresh` downloads the models.dev snapshot
(`https://models.opencode.ai/experimental/v2.0/api.json`) and writes a compact
copy to `<data>/models.json`. The provider list, model picker, bare model ids,
context limits and costs all come from that snapshot; the built-in list remains
as a fallback when no snapshot exists. Only models that advertise the
`chat-completions` or `messages` protocol are executable. `recode models` lists
providers with a credential (including the free OpenCode tier); add `--all` to
see every provider, or pass a provider id to filter.

```sh
recode models --refresh
recode models
recode models --all openrouter
```

### OpenCode Zen and free models

The built-in `opencode` provider points at
`https://opencode.ai/inference/openai/v1`. Its free models work without an
account, matching opencode's free tier:

```sh
recode models
recode run --model opencode/big-pickle "hello"
```

No `auth login` is required; when no `OPENCODE_API_KEY` or stored credential is
present, requests use the literal free-tier key `public`. Without a catalog
snapshot the built-in fallback lists `big-pickle` and the current free models;
after `recode models --refresh` the full free set is read from models.dev, and
paid models stay hidden until a key is supplied. Paid Zen models need a key from
`recode auth login opencode` or `OPENCODE_API_KEY`. Set
`OPENCODE_DISABLE_FREE_MODELS=1` to hide the free tier and require a key, the
same switch opencode uses. Free-tier requests are rate-limited by the service;
the provider returns its `FreeUsageLimitError` message unchanged.

## Agents

Built-ins: `build`, `plan`, `general`, and `explore`. Configure or add one:

```jsonc
{
  "agent": {
    "reviewer": {
      "description": "Reviews diffs without editing",
      "mode": "subagent",                 // primary | subagent | all
      "model": "anthropic/claude-sonnet-4-20250514",
      "temperature": 0.2,
      "prompt": "You review code. Never edit files.",
      "tools": { "read": true, "grep": true, "edit": false },
      "permission": { "bash": "deny" }
    }
  }
}
```

Markdown agents use opencode's layout: `agent(s)/**/*.md` and `mode(s)/*.md`
under the global config directory, `.opencode`, or `.recode`. The relative path
is the id (`agent/review/security.md` becomes `review/security`), and `mode(s)`
files are always primary agents. Frontmatter accepts `description`, `mode`,
`model`, `temperature`, `tools`, `permission` (flat allow/ask/deny entries),
`hidden`, and `disable`; the body becomes the system prompt. Precedence is
global markdown, `.opencode`, `.recode`, then JSON `agent` entries, so JSON
configuration always wins.

Project instructions are loaded from `AGENTS.md` and `CLAUDE.md` (plus
`instructions` globs) and prepended to the system prompt.

## Commands

Markdown files under `command(s)/**/*.md` (global config directory, `.opencode`,
or `.recode`) become slash commands. The relative path is the name, the body is
the prompt template, and frontmatter can set `description`, `agent`, `model`,
and `subagent` (the last one is parsed but not executed yet). Project files
override global files with the same name.

```markdown
---
description: Review a change
agent: plan
---
Review `$1` for correctness and edge cases.

Extra context from the caller: $ARGUMENTS
```

Templates support `$ARGUMENTS`, positional `$1`…`$N` (the highest placeholder
consumes the remaining arguments), and inline shell interpolation with
``!`command` ``. With no placeholders the typed arguments are appended to the
template. Typing `/` in the TUI completes built-in and custom commands, and F1
lists the custom ones. Run a command from the TUI (`/review src/main.rs`) or the
CLI:

```sh
recode run "/review src/main.rs"
```

The TUI and CLI expand commands against the local engine, or through
`POST /api/commands/{name}` when attached to `recode serve`, so inline shell
always runs on the machine that owns the session.

## Permissions

Keys are permission names (`read`, `edit`, `bash`, `webfetch`, `task`,
`question`, `mcp`, or a tool name). Values are `allow`, `ask`, or `deny`, or a
map of glob patterns to decisions.

```jsonc
{
  "permission": {
    "bash": { "git status*": "allow", "git push*": "ask", "*": "deny" },
    "edit": "allow",
    "read": { "*.pem": "deny", "*": "allow" }
  }
}
```

Deny always wins; any matching `ask` forces approval; an unmatched pattern
falls back to the tool's default. The TUI, CLI, and server can answer `once`,
`always` (adds a session rule), or `reject`.

## File snapshots

Before each parent prompt the worktree is committed to a private shadow git
repository under `<data>/recode/snapshots/<project-id>`. This enables
`sessions restore`, `/undo`, and the `files: true` revert option. The user's
own `.git` is never touched. Disable with:

```jsonc
{ "snapshot": false }
```

`git` must be on `PATH`; when it is missing snapshots are skipped with a
warning and runs continue.

## Language servers

Servers start on demand for edited files. Built-ins cover Rust, TypeScript,
Python, Go, and C/C++. Override or disable one by key:

```jsonc
{
  "lsp": {
    "rust": { "enabled": false },
    "custom": {
      "command": ["my-lsp", "--stdio"],
      "extensions": [".my"],
      "languageId": "mylang",
      "rootMarkers": ["my-project.toml"],
      "initialization": { "feature": true }
    }
  }
}
```

Diagnostics are appended to the tool result after a write/edit, capped at 20
entries.

## Formatters

Opt-in, run after a successful edit. The file path is appended to the command.

```jsonc
{
  "formatter": {
    "rustfmt": { "enabled": true },
    "prettier": { "enabled": true },
    "custom": { "command": ["myfmt", "--write"], "extensions": [".my"] }
  }
}
```

Built-ins: `rustfmt`, `gofmt`, `ruff`, `prettier`, `biome` (all disabled by
default).

## MCP servers

Tools are exposed to the model as `mcp__<server>__<tool>` and use the `mcp`
permission key.

```jsonc
{
  "mcp": {
    "files": {
      "type": "local",
      "command": ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."],
      "environment": { "LOG_LEVEL": "warn" }
    },
    "remote": {
      "type": "remote",
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer …" }
    }
  }
}
```

Local servers use line-delimited JSON-RPC over stdio; remote servers use
streamable HTTP with `Mcp-Session-Id`. Servers are contacted once before the
first prompt; a failed server is logged and skipped.

## TUI and misc

```jsonc
{
  "theme": "default",
  "autoupdate": false,
  "share": "manual",              // reserved; no hosted share yet
  "username": "you",
  "telemetry": false,
  "disabledProviders": ["provider-id"]
}
```
