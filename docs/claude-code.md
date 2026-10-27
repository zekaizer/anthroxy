# What Claude Code decides before the router sees anything

anthroxy serves whatever Claude Code sends it, but several things a user blames
on the router are settled inside Claude Code, from the model name and
`ANTHROPIC_BASE_URL` alone, before a request exists. This page records those
decisions, so that a surprise here is not chased through `src/`.

Everything below was read out of the Claude Code 2.1.278 bundle, except where a
claim says it was measured, and on which version. Claude Code changes often —
treat a detail that no longer matches as out of date, not as a bug in the
router.

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
`GET /v1/models` does not change it; the field never left discovery. Claude Code
has no per-model window setting anywhere else either: a `modelPicker` row takes
only `model`, `label`, `description` and `behavesAs`, and `behavesAs` or
`modelOverrides` makes an id borrow a known Claude model's window and
capabilities wholesale.

`CLAUDE_CODE_MAX_CONTEXT_TOKENS` overrides the assumed window, and whether it
applies depends on the id. An id that starts with `claude-`, in any casing, and
carries no suffix Claude Code strips (such as `@YYYYMMDD`) is treated like a
Claude model name: the variable applies to it only together with
`DISABLE_COMPACT`, which turns off all compaction. Measured on Claude Code
2.1.311 with the variable at 32768, reading `modelUsage.<id>.contextWindow` from
`claude -p --output-format json`:

| Model id | Window |
| --- | --- |
| `claude-local` | 200000 (variable ignored) |
| `claude-local` with `DISABLE_COMPACT=1` | 32768 |
| `local-claude`, `anthropic-local`, `qwen3-coder` | 32768 |
| `claude-local[1m]` | 1000000 |

So a model that is not Claude should not get an id starting with `claude-`;
`local-claude` passes the picker filter just as well. An earlier measurement on
2.1.278 agrees: with the variable at 500000, an unknown model's gauge read 500k
while `claude-opus-5` and `claude-sonnet-4-6` stayed at 200k.

Appending `[1m]` to a model name claims exactly 1M instead, which is right only
for a model that truly has one.

A gauge above the model's real window ends in the backend refusing the
request, and Claude Code compacts the conversation when that 400 says
`prompt is too long`. The Anthropic API says so itself; the router words that
way an `openai` backend's overflow and an `anthropic` one's worded otherwise,
such as vLLM's ([translation](translation.md#errors)).

`CLAUDE_CODE_MAX_CONTEXT_TOKENS` is one value for every id it applies to, so a
session mixing a 32k local model with a 500k hosted one would have to pick the
32k. `CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT=1` (Claude Code
2.1.223 and later) is the way each backend gets its own limit: for an id Claude
Code does not recognise it skips proactive compaction and compacts only after
the backend rejects the conversation with a too-long error it recognises, and
the router's wording above makes every backend's overflow such an error. The
cost is one rejected request before each such compaction. Measured on Claude
Code 2.1.311 with the variable set, against an `anthropic` backend refusing
with vLLM's wording any request over a fixed size, compaction requests
included: after the refusal Claude Code sent a compaction request; when that
was refused too, it sent it again with about half the input, and the turn then
succeeded. Through a router from before this wording, the same refusal was
retried unchanged and the turn failed. The variable does not change the window
Claude Code assumes, measured on the same version: `local-claude` stayed at
200000 with it, so `/context` and a status line's percentage are still of 200k
unless `CLAUDE_CODE_MAX_CONTEXT_TOKENS` is set as well. `anthroxy env` sets the
variable: it changes nothing for an id Claude Code recognises.

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
always. The same session then reported 804 tokens of MCP tools. Nothing needs
configuring on the anthroxy side: an Anthropic backend gets the blocks as they
came, and for an `openai` backend the router spells the referenced definitions
out in the tool result ([Translation](translation.md)).

What is on the wire, measured: the first request carries every deferred tool's
*name* in a system reminder and, in `tools`, only a `DeferredToolPlaceholder`
marked `defer_loading`. Claude Code runs `ToolSearch` itself and answers it
with `tool_reference` blocks; the referenced tools' full definitions appear in
`tools` from the next request on, still marked `defer_loading`. A deferred
tool that was never referenced is never sent.

Two model families are excluded whatever the setting: `claude-3-5-haiku` and
`claude-3-haiku`.

## Other environment variables worth knowing

- `ANTHROPIC_CUSTOM_MODEL_OPTION` (with `_NAME`, `_DESCRIPTION`) puts a model in
  the picker without discovery — the way to offer an id the filter drops. Its
  `_SUPPORTED_CAPABILITIES` cannot declare a context window; it accepts only
  `thinking`, `adaptive_thinking`, `interleaved_thinking`,
  `mid_conversation_system`, `temperature`, `effort`, `max_effort` and
  `xhigh_effort`.
- `ANTHROPIC_DEFAULT_SONNET_MODEL`, `_OPUS_` and `_HAIKU_` name the models
  Claude Code reaches for on its own, for session titles and small tasks
  (`ANTHROPIC_SMALL_FAST_MODEL` is the deprecated name of the Haiku one).
  Setting them is the alternative to giving a model those names as `aliases`.
- `DISABLE_PROMPT_CACHING` for a backend that rejects `cache_control`,
  `DISABLE_INTERLEAVED_THINKING` for one that rejects that beta.
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` stops telemetry and other traffic
  that is not the conversation, for a closed network.
