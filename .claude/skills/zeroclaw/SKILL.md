---
name: zeroclaw
description: "Help users operate and interact with their ZeroClaw agent instance — through both the CLI (`zeroclaw` commands) and the REST/WebSocket gateway API. Use this skill whenever the user wants to: send messages to ZeroClaw, manage memory or cron jobs, check system status, configure channels or providers, hit the gateway API, troubleshoot their ZeroClaw setup, build from source, or do anything involving the `zeroclaw` binary or its HTTP endpoints. Trigger this even if the user just says things like 'check my agent status', 'schedule a reminder', 'store this in memory', 'list my cron jobs', 'send a message to my bot', 'set up Telegram', 'build zeroclaw', or 'my bot is broken' — these are all ZeroClaw operations."
---

# ZeroClaw Skill

You are helping a user operate their ZeroClaw agent instance. ZeroClaw is an autonomous agent runtime with a CLI and an HTTP/WebSocket gateway.

Your job is to understand what the user wants to accomplish and then **execute it** — run the command, make the API call, report the result. Do not just show commands for the user to copy-paste. Actually run them via the Bash tool and tell the user what happened. The only exception is destructive operations (clearing all memory, estop kill-all) where you should confirm first.

## Adaptive Expertise

Pay attention to how the user talks. Someone who says "can you hit the webhook endpoint with a POST" is telling you they know what they're doing — be concise, skip explanations, just execute. Someone who says "how do I make my bot remember things" needs more context about what's happening under the hood.

Signals of technical comfort: mentions specific endpoints, HTTP methods, JSON fields, talks about tokens/auth, uses CLI flags fluently, references config files directly.

Signals of less familiarity: asks "what does X do", uses casual language about the bot/agent, describes goals rather than mechanisms ("I want it to check something every morning").

Default to a middle ground — brief explanation of what you're about to do, then do it. Dial up or down from there based on cues.

## Discovery — Before You Act

Before running any ZeroClaw operation, make sure you know where things are:

1. **Find the binaries.** Probe both PATH installs before choosing a session binary, so collisions are visible up front:
   - `command -v zrc` (PATH)
   - `command -v zeroclaw` (PATH)
   - The current project's alias build output: `./target/release/zrc` or `./target/debug/zrc`
   - The current project's build output: `./target/release/zeroclaw` or `./target/debug/zeroclaw` — this is the right choice when the user is working inside the ZeroClaw source tree and may have local changes
   - Common install locations: `~/.cargo/bin/zrc`, `~/.cargo/bin/zeroclaw`, `~/Downloads/zeroclaw-bin/zrc`, `~/Downloads/zeroclaw-bin/zeroclaw`

   Use the selected binary consistently for later commands in the same session. Store it as `ZC=/path/to/zrc` or `ZC=/path/to/zeroclaw` in examples and replace every `$ZC` below with that same resolved path. If `zeroclaw` and `zrc` both exist but point to different installs, prefer `zrc` for the EugOT fork and report the collision before running stateful commands. If no binary is found anywhere, offer to build from source (see "Building from Source" below). If the user is a developer working on ZeroClaw itself, they'll likely want the local build — watch for cues like them editing source files, mentioning PRs, or being in the project directory.

   In the normal EugOT fork path this means examples resolve to `zrc`, for example `ZC="$(command -v zrc)"` followed by `"$ZC" status` or `"$ZC" quickstart`. Keep `zeroclaw` as the fallback only when `zrc` is absent, and do not switch back to bare `zeroclaw` after `zrc` has been selected.

2. **Check if the gateway is running** (only needed for REST/WebSocket operations). A quick `curl -sf http://127.0.0.1:42617/health` tells you. If it's not running and the user wants REST access, let them know and offer to start it (`$ZC gateway` or `$ZC daemon`).

3. **Check auth status.** If the gateway requires pairing (`require_pairing = true` is the default), REST calls need a bearer token. Run `$ZC status` to see the current state, or check `~/.zeroclaw/config.toml` for a stored token under `[gateway]`.

4. **Discover the agent alias.** Every `$ZC agent` invocation requires `-a <alias>` — there is no default agent. Read `~/.zeroclaw/config.toml` and find the `[agents.<alias>]` headers; that `<alias>` is what the user means when they say "my agent." If multiple agents exist, ask the user which one to target before invoking. The examples below use `<alias>` as a placeholder for the alias you discovered.

Cache these findings for the conversation — don't re-discover every time.

## Important: REPL Limitation

`$ZC agent -a <alias>` (interactive REPL) requires interactive stdin, which doesn't work through the Bash tool. When the user wants to chat with their agent, use single-message mode instead:

```bash
"$ZC" agent -a <alias> -m "the message"
```

Each `-m` invocation is independent (no conversation history between calls). If the user needs multi-turn conversation, let them know they can run `$ZC agent -a <alias>` directly in their terminal, or use the WebSocket endpoint for programmatic streaming.

## First-Time Setup

If the user hasn't set up ZeroClaw yet (no `~/.zeroclaw/config.toml` exists), guide them through quickstart:

```bash
"$ZC" quickstart                                              # Interactive — picks a provider + agent
"$ZC" quickstart --model-provider ollama --model qwen2.5:7b   # Non-interactive
"$ZC" config set channels.<name>.<field>=<value>              # Configure a channel after quickstart
```

After quickstart, verify everything works:
```bash
"$ZC" status
"$ZC" doctor
```

If they already have a config but a channel is broken, edit just that channel's fields with `$ZC config set channels.<name>.<field>=<value>` rather than re-running quickstart (quickstart leaves an existing config alone).

## Building from Source

If the user wants to build ZeroClaw (or no binary is installed):

```bash
cargo build --release
```

This produces `target/release/zeroclaw` and, in the EugOT fork, `target/release/zrc`. For faster iteration during development, `cargo build` (debug mode) is quicker but produces slower binaries at `target/debug/zeroclaw` and `target/debug/zrc`.

You can also run directly without a separate build step:
```bash
cargo run --release --bin zeroclaw -- <subcommand> [args]
```

Before building, `cargo check` gives a quick compile validation without the full build.

## Choosing CLI vs REST

Both surfaces can do most things. Rules of thumb:

- **CLI is simpler** for one-off operations from the terminal. It handles auth internally and formats output nicely. Prefer CLI when the user is working locally.
- **REST is needed** when the user is building an integration, scripting from another language, or accessing a remote ZeroClaw instance. Also needed for streaming (WebSocket, SSE).
- If unclear, **default to CLI** — it's less setup.

## Core Operations

### Sending Messages

**CLI:** `$ZC agent -a <alias> -m "your message here"` — always use `-m` mode (not bare `$ZC agent -a <alias>`) so it returns instead of blocking on the REPL.

**REST:**
```bash
curl -X POST http://127.0.0.1:42617/webhook \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"message": "your message here"}'
```
Response: `{"response": "...", "model": "..."}`

**WebSocket** (for streaming): connect to `ws://127.0.0.1:42617/ws/chat?token=<token>`, send `{"type": "message", "content": "..."}`, receive `{"type": "done", "full_response": "..."}`.

### System Status

Run `$ZC status` to see provider, model, uptime, channels, memory backend. For deeper diagnostics: `$ZC doctor`.

**REST:** `GET /api/status` (same info as JSON), `GET /health` (no auth, quick ok/not-ok).

### Memory

The CLI can list, get, and clear memories but **cannot store** them directly. To store a memory:
- Via agent: `$ZC agent -a <alias> -m "remember that my favorite color is blue"`
- Via REST: `POST /api/memory` with `{"key": "...", "content": "...", "category": "core"}`

**CLI (read/delete):**
- `$ZC memory list` — list all entries
- `$ZC memory list --category core --limit 10` — filtered
- `$ZC memory get "key-name"` — get specific entry
- `$ZC memory stats` — usage statistics
- `$ZC memory clear --key "prefix" --yes` — delete entries (confirm with user first)

**REST (full CRUD):**
- `GET /api/memory` — list all (optional: `?query=search+text&category=core`)
- `POST /api/memory` — store: `{"key": "...", "content": "...", "category": "core"}`
- `DELETE /api/memory/{key}` — delete entry

Categories: `core`, `daily`, `conversation`, or any custom string.

### Cron / Scheduling

**CLI:**
- `$ZC cron list` — show all jobs
- `$ZC cron add '0 9 * * 1-5' 'Good morning' --tz America/New_York` — recurring
- `$ZC cron add-at '2026-03-11T10:00:00Z' 'Remind me'` — one-time at specific time
- `$ZC cron add-every 3600000 'Check health'` — interval in ms
- `$ZC cron once 30m 'Follow up'` — delay from now
- `$ZC cron pause <id>` / `$ZC cron resume <id>` / `$ZC cron remove <id>`

**REST:**
- `GET /api/cron` — list jobs
- `POST /api/cron` — add: `{"name": "...", "schedule": "0 9 * * *", "command": "..."}`
- `DELETE /api/cron/{id}` — remove job

### Tools

Tools are used automatically by the agent during conversations (shell, file ops, memory, browser, HTTP, web search, git, etc. — 30+ tools gated by security policy).

To see what's available: `GET /api/tools` (REST) lists all registered tools with descriptions and parameter schemas.

### Configuration

Edit `~/.zeroclaw/config.toml` directly, use `$ZC config set <key>=<value>` for one field at a time, or delete `~/.zeroclaw/` and re-run `$ZC quickstart` for a fresh setup.

**REST:**
- `GET /api/config`: compatibility whole-config read (secrets masked as `***MASKED***`)
- `PATCH /api/config`: atomically update config with an RFC 6902 JSON Patch document
- `OPTIONS /api/config`: get the whole-config JSON Schema; its current `Allow` header still lists legacy `PUT`, which the router does not register

### Providers & Models

- `$ZC providers` — list all supported providers
- `$ZC models list` — cached model catalog
- `$ZC models refresh --all` — refresh from providers
- `$ZC models set anthropic/claude-sonnet-4-6` — set default model

Override per-message: `$ZC agent -a <alias> -p anthropic --model claude-sonnet-4-6 -m "hello"`

### Real-Time Events (SSE)

REST only — useful for building dashboards or monitoring:
```bash
curl -N -H "Authorization: Bearer <token>" http://127.0.0.1:42617/api/events
```
Streams JSON events: `llm_request`, `tool_call_start`, `tool_call`, `agent_start`, `agent_end`, `error`.

### Cost Tracking

`GET /api/cost` — returns session/daily/monthly costs, token counts, per-model breakdown.

### Emergency Stop

Confirm with the user before running any estop command — these are disruptive.

- `$ZC estop --level kill-all` — stop everything
- `$ZC estop --level network-kill` — block all network
- `$ZC estop --level tool-freeze --tool shell` — freeze specific tool
- `$ZC estop status` — check current estop state
- `$ZC estop resume --network` — resume

### Gateway Lifecycle

- `$ZC gateway` — start HTTP gateway (foreground)
- `$ZC gateway -p 8080 --host 127.0.0.1` — custom bind
- `$ZC daemon` — start gateway + channels + scheduler + heartbeat
- `$ZC service install/start/stop/status/uninstall` — OS service management

### Channels

Channel availability depends on the installed build's feature set. Use `$ZC channels list` to inspect configured channels and `$ZC channels doctor` to check their health. Consult the current channel guide and generated config reference before editing `~/.zeroclaw/config.toml`; do not infer config keys from a fixed channel inventory.

For the full list of channels and their config fields, read `references/cli-reference.md` (Channels section).

### Pairing (Authentication Setup)

When `require_pairing = true` (default), REST clients need a bearer token:
```bash
curl -X POST http://127.0.0.1:42617/pair -H "X-Pairing-Code: <code>"
```
Response includes `{"token": "..."}` — save this for subsequent requests.

## Common Workflows

Here are multi-step sequences you're likely to need:

**"Is my agent healthy?"**
1. Run `$ZC status` — check provider, model, channels
2. Run `$ZC doctor` — check connectivity, diagnose issues
3. If gateway needed: `curl -sf http://127.0.0.1:42617/health`

**"Set up a new channel"**
1. Read the current config: `cat ~/.zeroclaw/config.toml`
2. Add the channel config (edit the TOML)
3. Restart: `$ZC service restart` (or restart daemon manually)
4. Verify: `$ZC channels doctor`

**"Switch to a different model"**
1. Check available: `$ZC models list`
2. Set it: `$ZC models set <provider/model>`
3. Verify: `$ZC status`
4. Test: `$ZC agent -a <alias> -m "hello, what model are you?"`

## Gateway Defaults

- **Port:** 42617
- **Host:** 127.0.0.1
- **Auth:** Pairing required (bearer token)
- **Rate limits:** 60 webhook requests/min, 10 pairing attempts/min
- **Body limit:** 64KB
- **Timeout:** 30 seconds
- **Idempotency:** Optional `X-Idempotency-Key` header on `/webhook` (300s TTL)
- **Config location:** `~/.zeroclaw/config.toml`

## Reference Files

For a curated operational API reference, read `references/rest-api.md`. For precision-sensitive route work, verify the current gateway router and the documented subset exposed at `/api/openapi.json`.

For common CLI commands and options, read `references/cli-reference.md`. Use `zeroclaw --help` and `zeroclaw <command> --help` as the current command-tree authority.

Only load these when you need precise details beyond what's in this file — for most operations, the quick references above are sufficient.

## Troubleshooting

**"zrc/zeroclaw: command not found"** — Binary not in PATH. Check `./target/release/zrc`, `./target/release/zeroclaw`, `~/.cargo/bin/zrc`, `~/.cargo/bin/zeroclaw`, or build from source with `cargo build --release`.

**"Connection refused" on REST calls** — Gateway isn't running. Start it with `$ZC gateway` or `$ZC daemon`.

**"Unauthorized" (401/403)** — Bearer token is missing or invalid. Re-pair via `POST /pair` with the pairing code, or check `~/.zeroclaw/config.toml` for the stored token.

**"LLM request failed" (500)** — Provider issue. Run `$ZC doctor` to check connectivity. Common causes: expired API key, provider outage, rate limiting on the provider side.

**"Too many requests" (429)** — You're hitting ZeroClaw's rate limit. Back off — the response includes `retry_after` with the number of seconds to wait.

**Agent not using tools / acting limited** — Check autonomy settings in config.toml under `[autonomy]`. `level = "read_only"` disables most tools. Try `level = "supervised"` or `level = "full"`.

**Memory not persisting** — Check `[memory]` config. If `backend = "none"`, nothing is stored. Switch to `"sqlite"` or `"markdown"`. Also verify `auto_save = true`.

**Channel not responding** — Run `$ZC channels doctor` for the specific channel. Common issues: expired bot token, wrong allowed_users list, channel not enabled in `[channels]`.

Report errors to the user with context appropriate to their expertise level. For beginners, explain what went wrong and suggest the fix. For experts, just show the error and the fix.
