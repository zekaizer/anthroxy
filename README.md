# claude-router

One endpoint for Claude Code in front of several Anthropic-API-compatible backends (vLLM, LM Studio, api.anthropic.com, …). Claude Code points at the router once; every configured model shows up in its `/model` picker, and switching between them takes effect on the next request without restarting the session.

```
Claude Code ──► claude-router ──┬──► vLLM            (Qwen, …)
 one URL,         routes by     ├──► LM Studio        (local models)
 one token        `model`       └──► api.anthropic.com (rotating OAuth token)
```

What the router does:

- **Model discovery.** `GET /v1/models` lists the configured models so Claude Code can offer them in `/model` (needs `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`).
- **Routing by `model`.** Each request goes to the backend that serves the named model; the model name is rewritten to what that backend calls it. Aliases let Claude Code's built-in model ids land on your backends too.
- **Per-backend credentials.** None, a static key, an environment variable, or a shell command that is re-run periodically (for tokens another program keeps fresh). Claude Code itself only ever sees one static router token.
- **Verbatim passthrough.** Unknown request fields, beta headers and response bodies are relayed as-is; streaming responses are forwarded chunk by chunk.
- **Retries and clear errors.** Connection failures are retried with backoff; every failure names the backend and cause in the Anthropic error format, so Claude Code shows it.
- **Tracing.** Every request has an id (`x-request-id`) that ties together the log lines, the error body and, when enabled, an on-disk record of the exact request and response.

## Install

```sh
cargo install --path .        # or: cargo build --release && cp target/release/claude-router ~/.local/bin/
```

Requires a Rust toolchain with edition 2024 support (1.85+). Linux and macOS.

Claude Code **2.1.152 or newer** on the client side: gateway model discovery arrived in 2.1.129, and 2.1.152 added the recovery that lets a session switch from a non-Anthropic backend back to Anthropic (older versions fail with a 400 on the `thinking` blocks the other model left in the history).

## Quick start

```sh
claude-router init            # writes ~/.config/claude-router/config.toml with a fresh token
$EDITOR ~/.config/claude-router/config.toml
claude-router check           # validates the file and probes every backend
claude-router serve           # starts the router (Ctrl-C to stop)
claude-router env             # prints the variables Claude Code needs
```

`check` output looks like this:

```
✓ ~/.config/claude-router/config.toml  valid: 2 backend(s), 3 model(s), listening on 0.0.0.0:8787

Backends
  ✓  claude  https://api.anthropic.com
       credential: command `cat ~/.claude-token` (refresh 5m, timeout 10s) (sk-a…ZwAA)
       GET /v1/models → HTTP 200 in 612 ms, 11 model(s)
  ✓  lmstudio  http://127.0.0.1:1234
       credential: none
       GET /v1/models → HTTP 200 in 4 ms, 5 model(s)

Models
     id            backend   upstream model      picker label
  ✓  gemma-local   lmstudio  gemma-4-e2b-it-qat  Gemma 4 E2B (LM Studio)
  !  typo-model    lmstudio  does-not-exist      typo-model
  ✓  sonnet        claude    claude-sonnet-5     Sonnet 5
  ✓ upstream model listed by the backend, ! not listed (check the name), - backend gave no list
```

Then, in the shell that runs Claude Code:

```sh
export ANTHROPIC_BASE_URL="http://localhost:8787"
export ANTHROPIC_AUTH_TOKEN="<token from the config>"
export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY="1"
export ANTHROPIC_MODEL="gemma-local"      # the model Claude Code starts with
claude
```

`claude-router env --format powershell` prints the same for the Windows host; `--format json` prints an `"env"` block for `~/.claude/settings.json`.

## Configuration

`~/.config/claude-router/config.toml` (override with `--config` or `CLAUDE_ROUTER_CONFIG`). `claude-router init --stdout` prints a fully commented example; the essentials:

```toml
[server]
listen = "0.0.0.0:8787"          # 0.0.0.0 so the Windows host reaches a WSL2 router
token = "…"                      # what Claude Code sends as ANTHROPIC_AUTH_TOKEN

[logging]
level = "info"                   # error|warn|info|debug|trace or a tracing directive
format = "text"                  # or "json"
# body_dir = "/var/tmp/claude-router"   # record every request/response (see Debugging)

[upstream]
connect_timeout = "10s"
read_timeout = "5m"              # silence tolerated between response chunks
retries = 2                      # after connection failures only
retry_backoff = "200ms"
retry_on_status = []             # e.g. [502, 503]

[backends.vllm]
url = "http://10.0.0.5:8000"
credential = { kind = "static", value = "${VLLM_API_KEY}", header = "bearer" }

[backends.claude]
url = "https://api.anthropic.com"
credential = { kind = "command", command = "cat ~/.claude-token", refresh = "5m" }
anthropic_beta = ["oauth-2025-04-20"]        # merged into the client's anthropic-beta

[[models]]
id = "qwen"                      # what Claude Code sees and sends
backend = "vllm"
upstream_model = "Qwen/Qwen3.5-32B"          # what vLLM calls it (default: id)
display_name = "Qwen 3.5 32B"                # /model picker label (default: id)
aliases = ["claude-haiku-4-5", "claude-haiku-4-5-20251001"]

[[models]]
id = "sonnet"
backend = "claude"
upstream_model = "claude-sonnet-5"

[routing]
default_model = "qwen"           # unknown model ids go here; omit to reject them with 404
```

Notes:

- `${NAME}` in any string value is replaced with the environment variable `NAME` at load time; `$${NAME}` keeps a literal `${NAME}` for shell commands.
- Credential kinds: `none` (default), `static`, `env` (`name = "VAR"`), `command`. `header` is `bearer` (default) or `x_api_key`.
- A `command` credential runs through `sh -c`; trimmed stdout is the token. It is re-run after `refresh`, and once more immediately if the backend answers 401/403.
- Unknown keys are errors. `check` reports every problem at once with its TOML path.
- Claude Code sends background requests (session titles, small tasks) naming its own default models. Give one of your models those names as `aliases`, or set `routing.default_model`, so they are served instead of failing.

## Commands

| Command | Purpose |
| --- | --- |
| `serve [--listen ADDR] [--body-dir DIR]` | Run the router. Prints a banner with the listen URL, backends and models. |
| `check [--no-probe] [--timeout 10s]` | Validate the file; acquire each credential and call `GET /v1/models` on each backend; flag upstream model names the backend does not list. Exit 1 on any problem. |
| `models` | The model table: id, backend, upstream name, picker label, aliases. |
| `init [--force] [--stdout]` | Write (or print) a commented configuration with a fresh random token. |
| `env [--host H] [--format sh\|powershell\|json]` | Variables for Claude Code. |
| `service install [--print] \| uninstall \| status` | systemd user service on Linux (see below). `status` prints a ✓/✗ list: systemd reachable, unit file present and pointing at this binary and config, enabled, active, linger, `/healthz` answering. |

Global options: `--config PATH`, `--log-level FILTER`, `--log-format text|json`.

## HTTP interface

| Route | Auth | Behaviour |
| --- | --- | --- |
| `GET /healthz` | no | `{"status":"ok"}` |
| `GET /v1/models`, `GET /v1/models/{id}` | yes | The configured models, Anthropic list format. |
| `POST /v1/messages`, `POST /v1/messages/count_tokens` | yes | Routed by the body's `model`; path and query forwarded unchanged. |

Auth is `x-api-key: <token>` or `Authorization: Bearer <token>`; failures are 401 in the Anthropic error format. Every response carries `x-request-id`; proxied ones also carry `x-claude-router-backend`, `x-claude-router-model` and `x-claude-router-upstream-model`.

Errors the router produces are `{"type":"error","error":{"type":…,"message":…},"request_id":…}`: 400 for a body without `model`, 404 for an unknown model (listing the configured ones), 413 over `server.max_body_bytes`, 502 when a backend is unreachable or its credential cannot be obtained. Backend errors are relayed with their status; if the body is an Anthropic error, its message is prefixed with `[backend <name>, HTTP <status>]`.

## Running on WSL2 as a service

```sh
claude-router service install     # writes ~/.config/systemd/user/claude-router.service, enables it, enables linger
claude-router service status      # ✓/✗ per check; exit 1 if anything is wrong
journalctl --user -u claude-router -f
claude-router service uninstall
```

Requirements: systemd enabled in the distribution (`[boot] systemd=true` in `/etc/wsl.conf`, default on recent Ubuntu). WSL still shuts the VM down when idle unless `%USERPROFILE%\.wslconfig` sets `[wsl2] vmIdleTimeout=-1`.

From Claude Code on the Windows host, use the distribution's address (`hostname -I` inside WSL) in `ANTHROPIC_BASE_URL`, or `localhost` when WSL networking is set to `mirrored`.

## Debugging

- **Logs.** Text by default, `--log-format json` for shippers. Each request runs in a span `request{id=… method=… path=… model=… backend=…}`; the lines you will look for are `routed`, `upstream responded` (status, attempts, latency), `response body complete` (bytes, chunks, time to first byte) and the `WARN`s: retries, credential refreshes, client disconnects, upstream errors.
- **Body capture.** Set `logging.body_dir` or pass `--body-dir DIR` to `serve`. Each request gets `<DIR>/<time>-<request id>/` with `request.json` (exactly what went upstream), `response.json|sse|bin` (exactly what came back) and `meta.json` (routing, headers with credentials redacted, status, timings, outcome). Nothing rotates these files; clean the directory yourself.
- **Levels.** `--log-level debug` (or `trace`) applies to the router only. To see the HTTP client internals, name them: `--log-level "claude_router=debug,hyper=debug,h2=debug"`.

## Development

```sh
cargo build
cargo test                                   # unit + integration (mock backends) + CLI tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Design decisions are recorded in `docs/adr/`. Source layout: `src/lib.rs` is the library (config, routing, credential, upstream, server, observability, service, cli); `src/main.rs` only parses the command line and calls it.
