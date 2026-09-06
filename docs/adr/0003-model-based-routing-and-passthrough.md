# 0003. Model-based routing with verbatim passthrough

## Status

accepted

## Context

Claude Code talks to exactly one `ANTHROPIC_BASE_URL`. Since v2.1.129 it can list a gateway's models through `GET /v1/models` (opt-in via `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`) and the user can pick one with `/model` at any time; the choice arrives as the `model` field of each `POST /v1/messages`. Claude Code also sends background requests (title generation, small tasks) naming its own default small model, and it includes fields and `anthropic-beta` flags the router has no knowledge of. Backends differ in what they call the same model. The router must never lag behind the API: unknown request fields, headers and response shapes have to survive the trip untouched.

## Decision

**Discovery.** `GET /v1/models` returns every configured model, in configuration order, in the Anthropic list shape (`data[]` of `{id, type:"model", display_name, created_at}`, `first_id`, `last_id`, `has_more:false`). `GET /v1/models/{id}` returns one. Nothing is fetched from backends; the table is the configuration.

**Routing key.** The `model` field of the JSON request body is the only routing input. It is matched against each model's `id` and `aliases`. An unknown model is a 404 (`not_found_error`) listing the configured names, unless `routing.default_model` names a fallback; then the request goes there and the match is logged as `Default`.

**Model rename.** When the matched route has an `upstream_model` different from what the client sent, the body is parsed as JSON, `model` is replaced, and the document is re-serialized with key order preserved. Otherwise the body bytes are forwarded exactly as received. Nothing else in the body is ever read or changed.

**Paths.** `POST /v1/messages` and `POST /v1/messages/count_tokens` are proxied with path and query string unchanged; `<backend.url>` is the only prefix added. Other paths are 404.

**Headers.** Every client header is forwarded except hop-by-hop headers, `host`, `content-length`, the client's `authorization`/`x-api-key` (replaced by the backend credential, ADR-0004) and `accept-encoding` (so bodies are relayed and logged uncompressed). A backend's `headers` override and its `anthropic_beta` flags are merged into the client's list. Response headers are forwarded except framing headers; the router adds `x-request-id`, `x-anthroxy-backend`, `x-anthroxy-model` and `x-anthroxy-upstream-model`.

**Bodies.** Successful responses are streamed to the client chunk by chunk as the backend produces them; no buffering, no SSE parsing. Error responses (4xx/5xx) are buffered, see ADR-0005.

**Client authentication.** One static token from `server.token`, accepted as `x-api-key: <token>` or `Authorization: Bearer <token>`. `GET /healthz` is the only unauthenticated route.

## Consequences

- Adding a model or renaming its upstream name is a configuration change; no code knows model names.
- Claude Code's `/model` picker shows configured `display_name`s and switching takes effect on the next request, without restarting the session.
- Because the body is rewritten only when a rename is needed, exotic JSON (huge integers, duplicate keys) round-trips bit-for-bit in the common case; with a rename it round-trips semantically through `serde_json` (order preserved).
- Every request body is buffered in memory once to read `model`; `server.max_body_bytes` (default 64 MiB) bounds that.
- A model can be reached under several names (aliases), which lets Claude Code's hard-coded fallback model ids land on a configured backend.
- `count_tokens` is routed to the same backend; a backend that lacks it returns its own error, relayed as-is.
- `thinking` blocks pass through untouched as well. When a session switches backends, the history carries thinking blocks whose signatures the new backend cannot verify; recovering from that is the client's job, and Claude Code ≥ 2.1.152 does it by retrying without those blocks. One 400 followed by a 200 right after a switch is therefore normal. Should the client stop recovering, the answer is a new ADR superseding this one with a rewriting rule, not an ad-hoc filter.
