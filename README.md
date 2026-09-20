# anthroxy

[![CI](https://github.com/zekaizer/anthroxy/actions/workflows/ci.yml/badge.svg)](https://github.com/zekaizer/anthroxy/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/zekaizer/anthroxy?sort=semver)](https://github.com/zekaizer/anthroxy/releases/latest)

One endpoint for Claude Code in front of several backends. Point Claude Code at the router once; every model you configure shows up in its `/model` picker, and switching between them takes effect on the next request without restarting the session.

```
Claude Code ──► anthroxy ──┬──► vLLM              (Qwen, …)
 one URL,      routes by   ├──► LM Studio         (local models)
 one token     `model`     └──► api.anthropic.com (rotating OAuth token)
```

A backend can speak the Anthropic Messages API or the OpenAI Chat Completions API — the router translates the latter in both directions, streaming, tool calls, images and reasoning included, so Claude Code uses it like any other model.

Each backend carries its own credential: none, a static key, an environment variable, or a command that is re-run as the token expires. Claude Code only ever holds one static router token.

Everything that happens is visible: a request id on every response and log line, a browser console showing requests in flight and recently finished with their errors and hints, per-model statistics on disk, and an optional on-disk record of the exact bytes each request and response carried.

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

Claude Code **2.1.152 or newer** on the client side: gateway model discovery arrived in 2.1.129, and 2.1.152 added the recovery that lets a session switch from a non-Anthropic backend back to Anthropic.

## Quick start

```sh
anthroxy init            # writes ~/.config/anthroxy/config.toml with a fresh token
$EDITOR ~/.config/anthroxy/config.toml
anthroxy check           # validates the file and probes every backend
anthroxy serve           # starts the router (Ctrl-C to stop)
anthroxy env             # prints the variables Claude Code needs
```

A configuration with one local backend and one model is enough to start:

```toml
[server]
listen = "0.0.0.0:8787"          # 0.0.0.0 so the Windows host reaches a WSL2 router
token = "…"                      # what Claude Code sends as ANTHROPIC_AUTH_TOKEN

[backends.lmstudio]
url = "http://127.0.0.1:1234"
kind = "openai"                  # omit for a backend that speaks the Anthropic Messages API

[[models]]
id = "claude-local"              # what Claude Code sees and sends
backend = "lmstudio"
upstream_model = "gemma-4-e2b-it-qat"     # what the backend calls it
display_name = "Gemma (local)"            # /model picker label
```

`check` tells you whether each backend answers, each credential works and each upstream model name exists:

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
export ANTHROPIC_BASE_URL='http://localhost:8787'
export ANTHROPIC_AUTH_TOKEN='<token from the config>'
export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY='1'
export ENABLE_TOOL_SEARCH='auto'          # keep MCP tools out of the context
export ANTHROPIC_MODEL='claude-local'     # the model Claude Code starts with
claude
```

`anthroxy env --format powershell` prints the same for the Windows host; `--format json` prints an `"env"` block for `~/.claude/settings.json`.

Two surprises are worth knowing before the first session: **a model id that contains neither `claude` nor `anthropic` never reaches the `/model` picker**, and **Claude Code assumes a 200k context window for any model it does not recognise**, so a smaller local model needs `CLAUDE_CODE_MAX_CONTEXT_TOKENS`. Both are decided inside Claude Code, not by the router — [What Claude Code decides](docs/claude-code.md) explains them and the rest.

## Watching it run

Open `http://localhost:8787/` (or whatever address Claude Code uses) in a browser and sign in with `server.token`. The console shows credential freshness and reload results, requests in flight and recently finished with their errors and configuration hints, charts and per-model statistics, a test request, a probe of every backend from inside the running process, and the body recordings. See [The web console](docs/console.md).

## Commands

| Command | Purpose |
| --- | --- |
| `serve [--listen ADDR] [--body-dir DIR]` | Run the router. `SIGHUP` re-reads the configuration without dropping connections. |
| `check [--no-probe] [--timeout 10s]` | Validate the file, acquire each credential, call `GET /v1/models` on each backend, flag upstream model names the backend does not list. Exit 1 on any problem. |
| `credential [BACKEND…] [--as-service] [--reveal]` | Run each backend's credential command the way the router runs it and report what came back. Exit 1 on any problem. |
| `models` | The model table: id, backend, upstream name, picker label, aliases. |
| `init [--force] [--stdout]` | Write (or print) a commented configuration with a fresh random token. |
| `env [--host H] [--format sh\|powershell\|json]` | Variables for Claude Code. |
| `service install [--print] \| reload \| uninstall \| status` | systemd user service on Linux and WSL2. |

Global options: `--config PATH`, `--log-level FILTER`, `--log-format text|json`.

## Keeping it running

```sh
anthroxy service install     # unit file, enabled, with linger so it survives logout
anthroxy service status      # ✓/✗ per check; exit 1 if anything is wrong
journalctl --user -u anthroxy -f
```

On WSL2 this needs systemd enabled in the distribution, and `%USERPROFILE%\.wslconfig` with `[wsl2] vmIdleTimeout=-1` so Windows does not shut the VM down. See [Running and debugging](docs/operating.md), which also covers logs, credential problems and recording the exact bytes of an exchange.

## Documentation

- [Configuration](docs/configuration.md) — every key, credentials, `drop_fields`, private CAs and proxies, reloading.
- [What Claude Code decides](docs/claude-code.md) — the picker filter, the context window, MCP tool loading, the environment variables that matter.
- [The web console](docs/console.md) — what each tab shows and does.
- [HTTP interface](docs/http-api.md) — routes, authentication, response headers, error bodies.
- [Running and debugging](docs/operating.md) — the service unit, logs, credential debugging, body capture.
- [Translation](docs/translation.md) — how an OpenAI backend is served.
- [Decisions](docs/adr/) — why the router is built the way it is.

## Development

```sh
cargo build
cargo test                                   # unit + integration (mock backends) + CLI tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

`src/lib.rs` is the library and `src/main.rs` only parses the command line; `CLAUDE.md` describes the module layout and the rules a change is held to.
