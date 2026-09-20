# What Claude Code decides before the router sees anything

anthroxy serves whatever Claude Code sends it, but several things a user blames
on the router are settled inside Claude Code, from the model name and
`ANTHROPIC_BASE_URL` alone, before a request exists. This page records those
decisions, so that a surprise here is not chased through `src/`.

Everything below was read out of the Claude Code 2.1.278 bundle; the two
measured claims say so. Claude Code changes often — treat a detail that no
longer matches as out of date, not as a bug in the router.

## Discovery: which models reach the `/model` picker

Claude Code fetches the picker's rows itself. All four must hold or it never
asks:

- `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY` is set
- the provider is first-party (not Bedrock, Vertex or Foundry)
- `ANTHROPIC_BASE_URL` is set
- a credential exists — `ANTHROPIC_AUTH_TOKEN`, an `apiKeyHelper`, or an API
  key. Unset all three and discovery is skipped, silently as far as the picker
  is concerned.

The call is `GET {base}/v1/models?limit=1000`, with
`anthropic-version: 2023-06-01`, the credential, and any
`ANTHROPIC_CUSTOM_HEADERS` (which override a default header of the same name).
It times out after 3s (`CLAUDE_CODE_GATEWAY_MODEL_DISCOVERY_TIMEOUT_MS`) and
treats a redirect as a failure.

Of each entry Claude Code keeps **`id`, `display_name` and `description`, and
nothing else**. `context_window` is read and dropped here — this is why a window
served in the catalog never reaches the gauge (see below).

Then the filter: an id must match `/(claude|anthropic)/i`, case-insensitively
and anywhere in the string. **A model named for what the backend calls it —
`grok-4.6`, `gemma-local` — is fetched and dropped without a message the user
sees.** If nothing survives, the previous cache is left as it was.

What survives is cached in `~/.claude/cache/gateway-models.json` against the
`ANTHROPIC_BASE_URL` it came from, and the picker uses it only while that URL
matches exactly, trailing slash and port included.

The filter governs the picker alone. Routing is untouched: an id the picker
never shows still serves every request that names it, whether through
`ANTHROPIC_MODEL`, `ANTHROPIC_CUSTOM_MODEL_OPTION` or `--model`. To be offered
as well as served, a model needs an `id` or `aliases` entry carrying `claude` or
`anthropic`.

## The context window: what the gauge believes

For a model it does not recognise, Claude Code assumes 200k and says so
("default for an unrecognized model" in `/context`). A larger window served in
`GET /v1/models` does not change it; the field never left discovery.

`CLAUDE_CODE_MAX_CONTEXT_TOKENS` is the one lever, and it applies **only to
models whose id is not a Claude model id** — measured: with it set to 500000, an
unknown model's gauge read 500k while `claude-opus-5` and `claude-sonnet-4-6`
stayed at 200k in the same setting. Every unknown model in a session shares that
one value, so it belongs at the smallest window among them; a session mixing a
32k local model with a 500k hosted one has to pick the 32k.

Appending `[1m]` to a model name claims a 1M window instead, which is right only
for a model that truly has one.

## MCP tools: sent up front, or loaded on demand

Claude Code defers MCP tool definitions — sending a small `ToolSearch` tool and
loading definitions as they are needed — only where it believes the endpoint
forwards `tool_reference` content blocks. It does not believe that of an
`ANTHROPIC_BASE_URL` it does not recognise as first-party, and quietly sends
**every** MCP tool definition in every request instead. On a large MCP set that
is the dominant cost in the context: measured on one session, 150 tools at
114.4k tokens, 57% of a 200k window, before the conversation began.

`ENABLE_TOOL_SEARCH` in Claude Code's environment turns deferral back on: `auto`
once the definitions grow large, `auto:N` past N percent of the window, `true`
always. The same session then reported 804 tokens of MCP tools. The router
forwards the blocks either way; nothing here needs configuring on the anthroxy
side.

Two model families are excluded whatever the setting: `claude-3-5-haiku` and
`claude-3-haiku`.

## Other environment variables worth knowing

- `ANTHROPIC_CUSTOM_MODEL_OPTION` (with `_NAME`, `_DESCRIPTION`) puts a model in
  the picker without discovery — the way to offer an id the filter drops. Its
  `_SUPPORTED_CAPABILITIES` cannot declare a context window; it accepts only
  `thinking`, `adaptive_thinking`, `interleaved_thinking`,
  `mid_conversation_system`, `temperature`, `effort`, `max_effort` and
  `xhigh_effort`.
- `ANTHROPIC_DEFAULT_SONNET_MODEL`, `_OPUS_`, `_HAIKU_` and
  `ANTHROPIC_SMALL_FAST_MODEL` name the models Claude Code reaches for on its
  own, for session titles and small tasks. Setting them is the alternative to
  giving a model those names as `aliases`.
- `DISABLE_PROMPT_CACHING` for a backend that rejects `cache_control`,
  `DISABLE_INTERLEAVED_THINKING` for one that rejects that beta.
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` stops telemetry and other traffic
  that is not the conversation, for a closed network.
