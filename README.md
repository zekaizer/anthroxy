# anthroxy

[![CI](https://github.com/zekaizer/anthroxy/actions/workflows/ci.yml/badge.svg)](https://github.com/zekaizer/anthroxy/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/zekaizer/anthroxy?sort=semver)](https://github.com/zekaizer/anthroxy/releases/latest)

One endpoint for Claude Code in front of several backends that speak the Anthropic Messages API (vLLM, LM Studio, api.anthropic.com, …) or the OpenAI Chat Completions API. Claude Code points at the router once; every configured model shows up in its `/model` picker, and switching between them takes effect on the next request without restarting the session.

```
Claude Code ──► anthroxy ──┬──► vLLM              (Qwen, …)
 one URL,      routes by   ├──► LM Studio         (local models)
 one token     `model`     └──► api.anthropic.com (rotating OAuth token)
```

What the router does:

- **Model discovery.** `GET /v1/models` lists the configured models so Claude Code can offer them in `/model` (needs `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`).
- **Routing by `model`.** Each request goes to the backend that serves the named model; the model name is rewritten to what that backend calls it. Aliases let Claude Code's built-in model ids land on your backends too.
- **Per-backend credentials.** None, a static key, an environment variable, or a shell command that is re-run periodically (for tokens another program keeps fresh). Claude Code itself only ever sees one static router token.
- **Verbatim passthrough.** Unknown request fields, beta headers and response bodies are relayed as-is; streaming responses are forwarded chunk by chunk.
- **OpenAI backends.** A backend marked `kind = "openai"` gets each request translated to Chat Completions and its answer translated back, streaming, tool calls, images and reasoning included, so Claude Code uses it like any other model.
- **Retries and clear errors.** Connection failures are retried with backoff; every failure names the backend and cause in the Anthropic error format, so Claude Code shows it.
- **Tracing.** Every request has an id (`x-request-id`) that ties together the log lines, the error body and, when enabled, an on-disk record of the exact request and response.

## Install

**Linux x86_64 (WSL2 included), no toolchain needed** — every release ships a fully static executable:

```sh
curl -fsSLO https://github.com/zekaizer/anthroxy/releases/latest/download/anthroxy-x86_64-unknown-linux-musl.tar.gz
tar -xzf anthroxy-x86_64-unknown-linux-musl.tar.gz
install -m 755 anthroxy-x86_64-unknown-linux-musl/anthroxy ~/.local/bin/
anthroxy --version        # prints the version and the commit it was built from
```

**From source** (Linux and macOS, Rust 1.85+ for edition 2024):

```sh
cargo install --path .        # or: cargo build --release && cp target/release/anthroxy ~/.local/bin/
```

Claude Code **2.1.152 or newer** on the client side: gateway model discovery arrived in 2.1.129, and 2.1.152 added the recovery that lets a session switch from a non-Anthropic backend back to Anthropic (older versions fail with a 400 on the `thinking` blocks the other model left in the history).

## Quick start

```sh
anthroxy init            # writes ~/.config/anthroxy/config.toml with a fresh token
$EDITOR ~/.config/anthroxy/config.toml
anthroxy check           # validates the file and probes every backend
anthroxy serve           # starts the router (Ctrl-C to stop)
anthroxy env             # prints the variables Claude Code needs
```

`check` output looks like this:

```
✓ ~/.config/anthroxy/config.toml  valid: 2 backend(s), 3 model(s), listening on 0.0.0.0:8787

Backends
  ✓  claude  https://api.anthropic.com
       credential: command `cat ~/.claude-token` (refresh 5m, timeout 10s) (sk-a…ZwAA)
       GET /v1/models → HTTP 200 in 612 ms, 11 model(s)
  ✓  lmstudio  http://127.0.0.1:1234
       credential: none
       GET /v1/models → HTTP 200 in 4 ms, 5 model(s)

Models
     id           backend   upstream model      picker label             aliases
  ✓  gemma-local  lmstudio  gemma-4-e2b-it-qat  Gemma 4 E2B (LM Studio)  claude-haiku-4-5
  !  typo-model   lmstudio  does-not-exist      typo-model               -
  ✓  sonnet       claude    claude-sonnet-5     Sonnet 5                 -
  ✓ upstream model listed by the backend, ! not listed (check the name), - backend gave no list
  unknown model ids → gemma-local
```

Then, in the shell that runs Claude Code:

```sh
export ANTHROPIC_BASE_URL="http://localhost:8787"
export ANTHROPIC_AUTH_TOKEN="<token from the config>"
export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY="1"
export ANTHROPIC_MODEL="gemma-local"      # the model Claude Code starts with
claude
```

`anthroxy env --format powershell` prints the same for the Windows host; `--format json` prints an `"env"` block for `~/.claude/settings.json`.

## Configuration

`~/.config/anthroxy/config.toml` (override with `--config` or `ANTHROXY_CONFIG`). `anthroxy init --stdout` prints a fully commented example; the essentials:

```toml
[server]
listen = "0.0.0.0:8787"          # 0.0.0.0 so the Windows host reaches a WSL2 router
token = "…"                      # what Claude Code sends as ANTHROPIC_AUTH_TOKEN

[logging]
level = "info"                   # error|warn|info|debug|trace or a tracing directive
format = "text"                  # or "json"
# body_dir = "/var/tmp/anthroxy"   # record every request/response (see Debugging)
body_retention = "7d"            # delete recorded exchanges older than this; "0s" keeps all

[upstream]
connect_timeout = "10s"
read_timeout = "5m"              # silence tolerated between response chunks
retries = 2                      # after connection failures only
retry_backoff = "200ms"
retry_on_status = []             # e.g. [502, 503]
# ca_certificate = "~/.config/anthroxy/corp-root.pem"   # extra trust anchor (see Notes)

[backends.vllm]
url = "http://10.0.0.5:8000"
credential = { kind = "static", value = "${VLLM_API_KEY}", header = "bearer" }
drop_fields = ["context_management"]        # body fields this backend rejects as unknown

[backends.claude]
url = "https://api.anthropic.com"
credential = { kind = "command", command = "cat ~/.claude-token", refresh = "5m" }
anthropic_beta = ["oauth-2025-04-20"]        # merged into the client's anthropic-beta

[backends.cli]                               # a CLI that reports its token's expiry
url = "https://gateway.example.com"
credential = { kind = "command", command = "some-cli token --json | jq '{token: .access_token, expires_at: .expires_at}'", output = "json" }

[backends.inhouse]                           # a server that speaks OpenAI Chat Completions
kind = "openai"
url = "https://llm.example.corp"
credential = { kind = "static", value = "${INHOUSE_API_KEY}" }

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
- Credential kinds: `none` (default), `static`, `env` (`name = "VAR"`), `command`. `header` is `bearer` (default, `Authorization: Bearer <token>`), `x_api_key` (`x-api-key: <token>`), or any header as `{ name = "api-key" }` / `{ name = "authorization", scheme = "Token" }`; a `scheme` is written before the token with one space.
- A `command` credential runs through `sh -c`. With `output = "text"` (default) trimmed stdout is the token; with `output = "json"` stdout is `{"token": "...", "expires_at": ...}`, where the optional `expires_at` is an RFC 3339 timestamp or unix seconds (milliseconds when ≥ 10^11). The command is re-run after `refresh`, two minutes before `expires_at`, and once more immediately if the backend answers 401/403. A token whose `expires_at` has passed is an error, not sent upstream.
- `drop_fields` removes request body fields before forwarding to that backend, for servers that reject parameters they do not know (vLLM and `context_management`, for example). Paths are dot-separated object keys (`metadata.user_id`); arrays cannot be reached and `model` cannot be dropped. The `anthropic-beta` header is left alone. Claude Code's own `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` is the client-side alternative, but it applies to every backend in the session.
- `kind = "openai"` marks a backend that speaks the OpenAI Chat Completions API. `POST /v1/messages` is translated and sent to `<url>/v1/chat/completions`: `system`, messages, tool definitions, tool calls and results, and base64 images are mapped; `cache_control`, `metadata`, `context_management`, `top_k` and the `thinking` parameter are dropped, as are `thinking` blocks in the history. The answer is translated back, streaming included, and `reasoning_content` appears as thinking blocks. `count_tokens` is answered 404 by the router. `anthropic_beta` is not accepted on such a backend, and `anthropic-version`/`anthropic-beta` are not sent to it. `check` still probes `GET /v1/models`, so a server without that endpoint fails `check` while `serve` works. See ADR-0010 for the exact mapping.
- Unknown keys are errors. `check` reports every problem at once with its TOML path.
- HTTPS backends are verified against the Mozilla roots built into the binary plus the OS certificate store (`update-ca-certificates` on Ubuntu, the keychain on macOS). `SSL_CERT_FILE` / `SSL_CERT_DIR` replace the OS store when set; under systemd they go in the unit as `Environment=`. `upstream.ca_certificate` adds every certificate in a PEM file on top, for a private CA that should travel with the configuration file; a file that cannot be read or holds no certificate fails `check` and `serve`. There is no way to skip verification.
- Edits take effect on `SIGHUP` (`kill -HUP <pid>` or `anthroxy service reload`): models, backends, credentials, token, logging and upstream settings swap atomically; in-flight requests finish on the old configuration. Changing `server.listen` still needs a restart. A file that fails to load leaves the running configuration untouched and logs the error.
- Claude Code sends background requests (session titles, small tasks) naming its own default models. Give one of your models those names as `aliases`, or set `routing.default_model`, so they are served instead of failing.

## Commands

| Command | Purpose |
| --- | --- |
| `serve [--listen ADDR] [--body-dir DIR]` | Run the router. Prints a banner with the listen URL, backends and models. `SIGHUP` re-reads the file without dropping connections (everything except `server.listen`). |
| `check [--no-probe] [--timeout 10s]` | Validate the file; acquire each credential and call `GET /v1/models` on each backend; flag upstream model names the backend does not list. Exit 1 on any problem. |
| `models` | The model table: id, backend, upstream name, picker label, aliases. |
| `init [--force] [--stdout]` | Write (or print) a commented configuration with a fresh random token. |
| `env [--host H] [--format sh\|powershell\|json]` | Variables for Claude Code. |
| `service install [--print] \| reload \| uninstall \| status` | systemd user service on Linux (see below). `reload` sends SIGHUP through systemd. `status` prints a ✓/✗ list: systemd reachable, unit file present and pointing at this binary and config, enabled, active, linger, `/healthz` answering. |

Global options: `--config PATH`, `--log-level FILTER`, `--log-format text|json`.

## HTTP interface

| Route | Auth | Behaviour |
| --- | --- | --- |
| `GET /healthz` | no | `{"status":"ok"}` |
| `GET /v1/models`, `GET /v1/models/{id}` | yes | The configured models, Anthropic list format. |
| `POST /v1/messages`, `POST /v1/messages/count_tokens` | yes | Routed by the body's `model`; path and query forwarded unchanged. On an `openai` backend, `/v1/messages` is translated and sent to `/v1/chat/completions`; `count_tokens` is 404. |

Auth is `x-api-key: <token>` or `Authorization: Bearer <token>`; failures are 401 in the Anthropic error format. Every response carries `x-request-id`; proxied ones also carry `x-anthroxy-backend`, `x-anthroxy-model` and `x-anthroxy-upstream-model`.

Errors the router produces are `{"type":"error","error":{"type":…,"message":…},"request_id":…}`: 400 for a body without `model`, 404 for an unknown model (listing the configured ones), 413 over `server.max_body_bytes`, 502 when a backend is unreachable or its credential cannot be obtained. Backend errors are relayed with their status; if the body is an Anthropic error, its message is prefixed with `[backend <name>, HTTP <status>]`. An `openai` backend's error body is always converted to an Anthropic error document (type from the status, same prefix); a failure in the middle of its stream is one `error` event.

## Running on WSL2 as a service

```sh
anthroxy service install     # writes ~/.config/systemd/user/anthroxy.service, enables it, enables linger
anthroxy service status      # ✓/✗ per check; exit 1 if anything is wrong
anthroxy service reload      # re-read the config (systemctl --user reload)
journalctl --user -u anthroxy -f
anthroxy service uninstall
```

Requirements: systemd enabled in the distribution (`[boot] systemd=true` in `/etc/wsl.conf`, default on recent Ubuntu). WSL still shuts the VM down when idle unless `%USERPROFILE%\.wslconfig` sets `[wsl2] vmIdleTimeout=-1`.

From Claude Code on the Windows host, use the distribution's address (`hostname -I` inside WSL) in `ANTHROPIC_BASE_URL`, or `localhost` when WSL networking is set to `mirrored`.

## Debugging

- **Logs.** Text by default, `--log-format json` for shippers. Each request runs in a span `request{id=… method=… path=… model=… backend=…}`; the lines you will look for are `routed`, `upstream responded` (status, attempts, latency), `response body complete` (bytes, chunks, time to first byte) and the `WARN`s: retries, credential refreshes, client disconnects, upstream errors.
- **Body capture.** Set `logging.body_dir` or pass `--body-dir DIR` to `serve`. Each request gets `<DIR>/<time>-<request id>/` with `request.json` (exactly what went upstream), `response.json|sse|bin` (exactly what came back) and `meta.json` (routing, headers with credentials redacted, status, timings, outcome). Entries older than `logging.body_retention` (default 7 days) are deleted at startup and every 10 minutes; set it to `0s` to keep everything.
- **Levels.** `--log-level debug` (or `trace`) applies to the router only. To see the HTTP client internals, name them: `--log-level "anthroxy=debug,hyper=debug,h2=debug"`.

## Development

```sh
cargo build
cargo test                                   # unit + integration (mock backends) + CLI tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Design decisions are recorded in `docs/adr/`. Source layout: `src/lib.rs` is the library (config, routing, credential, upstream, server, observability, service, cli); `src/main.rs` only parses the command line and calls it.
