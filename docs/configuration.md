# Configuration

Every key anthroxy reads, and what each one does. The README covers the few
lines needed to start; this page is the reference. `anthroxy init --stdout`
prints a fully commented example, and `anthroxy check` reports every problem in
the file at once with its TOML path.

The file lives at `~/.config/anthroxy/config.toml`, overridden by `--config` or
`ANTHROXY_CONFIG`.
## The file

```toml
[server]
listen = "0.0.0.0:8787"          # 0.0.0.0 so the Windows host reaches a WSL2 router
token = "…"                      # what Claude Code sends as ANTHROPIC_AUTH_TOKEN

[logging]
level = "info"                   # error|warn|info|debug|trace or a tracing directive
format = "text"                  # or "json"
# body_dir = "/var/tmp/anthroxy"   # record every request/response (see [Operating](operating.md))
body_retention = "7d"            # delete recorded exchanges older than this; "0s" keeps all

[stats]
# enabled = false                # on by default: one line per /v1/messages exchange
# dir = "~/.local/state/anthroxy/stats"   # default: $XDG_STATE_HOME/anthroxy/stats
retention = "90d"                # delete daily files older than this; "0s" keeps all

[upstream]
connect_timeout = "10s"
non_stream_timeout = "15m"       # whole non-stream response after connect
stream_first_byte_timeout = "5m" # stream: until the first body byte
stream_idle_timeout = "60s"      # stream: silence between subsequent chunks
retries = 2                      # after connection failures only
retry_backoff = "200ms"
retry_on_status = []             # e.g. [502, 503]
# ca_certificate = "~/.config/anthroxy/corp-root.pem"   # extra trust anchor (see the reference below)

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
# proxy = "http://proxy.corp:3128"          # reached only through this proxy (see the reference below)

[backends.inhouse]                           # a server that speaks OpenAI Chat Completions
kind = "openai"
url = "https://llm.example.corp"
credential = { kind = "static", value = "${INHOUSE_API_KEY}" }

[backends.grok]                              # xAI Chat Completions + OAuth helper
kind = "openai"
url = "https://api.x.ai"
live_models = true                           # identity from GET /v1/models
credential = { kind = "command", command = "/path/to/anthroxy/scripts/xai-oauth print", output = "json", refresh = "5m" }

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

## Reference

### Values and files

- `${NAME}` in any string value is replaced with the environment variable `NAME`
  at load time; `$${NAME}` keeps a literal `${NAME}` for shell commands.
- Unknown keys are errors. `check` reports every problem at once with its TOML
  path.

### Backends

- A backend `url` is an origin, optionally with a path prefix
  (`http://host/openai`); the request path is appended to it, so it must carry
  no query string or fragment.
- `drop_fields` removes request body fields before forwarding to that backend,
  for servers that reject parameters they do not know (vLLM and
  `context_management`, for example). Paths are dot-separated object keys
  (`metadata.user_id`); arrays cannot be reached and `model` cannot be dropped.
  The `anthropic-beta` header is left alone. Claude Code's own
  `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` is the client-side alternative, but
  it applies to every backend in the session.
- `live_models = true` fetches `GET {url}{models_path}` and publishes those ids
  as Anthropic model identity (`id`, `display_name`, `created_at`). Anthropic
  and OpenAI list JSON both work (ADR-0017). `kind = "passthrough"` always does
  this. Configured `[[models]]` still win on the same id. A backend with
  `live_models` may omit `[[models]]`.
- `kind = "openai"` marks a backend that speaks the OpenAI Chat Completions API.
  `POST /v1/messages` is translated and sent to `<url>/v1/chat/completions`:
  `system`, messages, tool definitions, tool calls and results, and base64
  images are mapped (an image inside a tool result rides in the following user
  message, since tool messages are text-only); `metadata.user_id` and
  `output_config.effort` become `user` and `reasoning_effort`; `cache_control`,
  `context_management`, `top_k` and the `thinking` parameter are dropped, as are
  `thinking` blocks in the history; a PDF or other `document` block is replaced
  by a note saying it was omitted, so the model can say so instead of the turn
  failing. The answer is translated back, streaming included;
  `reasoning_content` appears as thinking blocks (removed again before any
  Anthropic backend sees them, so switching models back costs no failed request;
  likewise, tool call ids with characters the Anthropic API rejects, such as
  vLLM's `functions.<name>:<n>` for Kimi models, are rewritten on the way to an
  Anthropic backend), and cached and reasoning token counts reach Claude Code
  when the server reports them. `count_tokens` is answered 404 by the router.
  `anthropic_beta` is not accepted on such a backend, and
  `anthropic-version`/`anthropic-beta` are not sent to it. `check` still probes
  `GET /v1/models`, so a server without that endpoint fails `check` while
  `serve` works. See [Translation](translation.md) for the exact mapping.

### Credentials

- Credential kinds: `none` (default), `static`, `env` (`name = "VAR"`),
  `command`. `header` is `bearer` (default, `Authorization: Bearer <token>`),
  `x_api_key` (`x-api-key: <token>`), or any header as `{ name = "api-key" }` /
  `{ name = "authorization", scheme = "Token" }`; a `scheme` is written before
  the token with one space.
- A `command` credential runs through `sh -c`. With `output = "text"` (default)
  trimmed stdout is the token; with `output = "json"` stdout is
  `{"token": "...", "expires_at": ...}`, where the optional `expires_at` is an
  RFC 3339 timestamp or a number of unix seconds (milliseconds when ≥ 10^11,
  fractions allowed, up to year 9999). The command is re-run after `refresh`,
  two minutes before `expires_at`, and once more immediately if the backend
  answers 401/403. A token whose `expires_at` has passed is an error, not sent
  upstream. When a refresh fails, a JSON token more than two minutes from its
  `expires_at` keeps being sent, with the command retried at most every 30
  seconds, so a brief token-helper outage does not fail requests; a text token
  is not reused past `refresh` (ADR-0015).

### Reaching a backend

- HTTPS backends are verified against the Mozilla roots built into the binary
  plus the OS certificate store (`update-ca-certificates` on Ubuntu, the
  keychain on macOS). `SSL_CERT_FILE` / `SSL_CERT_DIR` replace the OS store when
  set; under systemd they go in the unit as `Environment=`.
  `upstream.ca_certificate` adds every certificate in a PEM file on top, for a
  private CA that should travel with the configuration file; a file that cannot
  be read or holds no certificate fails `check` and `serve`. There is no way to
  skip verification.
- Backends are reached at the URL the file names: `http_proxy` / `https_proxy` /
  `all_proxy` / `no_proxy` in the environment are ignored, so an ambient proxy
  cannot take a backend request, and the credential on it, somewhere the
  configuration never named. A backend that answers only through a proxy names
  it as `proxy = "http://proxy.corp:3128"` (`https://` as well;
  `http://user:${PROXY_PASSWORD}@proxy.corp:3128` sends basic auth, and the
  userinfo is redacted wherever the configuration is shown). The proxy belongs
  to the backend's origin, so backends sharing a scheme, host and port must name
  the same proxy or none. An `https` backend is tunnelled with `CONNECT` and
  still verified end to end; a TLS-intercepting proxy needs its CA (see the
  previous bullet). See ADR-0012.

- A backend URL may carry userinfo (`https://user:${GATEWAY_PASSWORD}@gw.corp`),
  which is sent as basic auth. It is a secret like any other: wherever the
  router shows a backend URL — the console, `check`, the startup lines, a
  rejected configuration — the userinfo is replaced by `<redacted>`.
- `upstream.retries` sends a request again only when the backend never saw it:
  a connection that was refused or reset, or a status listed in
  `retry_on_status`. The wait before a retry is `retry_backoff`, doubled on each
  further attempt and never longer than 30 seconds, so a generous retry count
  cannot put a request to sleep for hours.

### Statistics on disk

- `[stats]` writes `requests-YYYY-MM-DD.jsonl` (UTC dates) owner-only under
  `stats.dir`: request id, time, requested and routed model and how the name
  matched (`exact`, `alias` or `default`), backend, upstream model, stream flag,
  status, attempts and whether one of them re-sent the request with a
  re-acquired credential, latency, time to first byte, duration, bytes, outcome
  and token usage (input, output, cache read, cache write). Message content,
  error bodies and headers are never written. Daily files older than
  `stats.retention` are deleted with the body log sweep. `count_tokens` and
  `/v1/models` are not counted.

### Reloading

- Edits take effect on `SIGHUP` (`kill -HUP <pid>`, `anthroxy service reload`,
  or the console's Reload button): models, backends, credentials, token, body
  capture and upstream settings swap atomically; in-flight requests finish on
  the old configuration. `server.listen` and the log level and format need a
  restart instead, and a reload that changes one of them says so. A file that
  fails to load leaves the running configuration untouched and logs the error.

## What Claude Code decides for itself

Four things users often try to fix here cannot be fixed here: which models reach
the `/model` picker (ids must contain `claude` or `anthropic`), the context
window the gauge believes (`CLAUDE_CODE_MAX_CONTEXT_TOKENS`), whether MCP tool
definitions are sent up front (`ENABLE_TOOL_SEARCH`), and the model names Claude
Code reaches for on its own for session titles and small tasks (give a model
those names as `aliases`, or set `routing.default_model`). See [What Claude Code
decides](claude-code.md).
