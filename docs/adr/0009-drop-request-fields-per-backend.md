# 0009. Dropping request fields per backend

## Status

accepted

Supersedes the "nothing else in the body is ever read or changed" rule of ADR-0003; the rest of ADR-0003 stands.

## Context

Claude Code adds request fields as the Anthropic API grows (`context_management` is the current example, sent with its `anthropic-beta` flag). Anthropic ignores nothing it knows, but a backend that implements the Messages API from a fixed schema, vLLM among them, rejects an unknown parameter with a 400 and the request never reaches the model.

Claude Code's own switch, `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1`, is global to the session: it stops the field and its beta flag for every backend, including the ones that support the feature. A session on this router moves between backends, so the decision belongs to the backend, not the client.

ADR-0003 chose verbatim passthrough so the router never lags behind the API, and it anticipated this case: the answer is a rewriting rule recorded in an ADR, not an ad-hoc filter.

## Decision

- `backends.<name>.drop_fields` lists paths of the JSON request body removed before the request is forwarded to that backend. It applies to every proxied body (`/v1/messages` and `count_tokens`).
- A path is dot-separated object keys: `context_management` removes a top-level field, `metadata.user_id` removes `user_id` inside `metadata`. Every segment descends into an object; a path whose prefix is missing or is not an object removes nothing. Array elements are not addressed; a rule that reaches into arrays (`messages[].content[]…`) needs its own syntax and its own decision.
- `model` cannot be listed: it is the routing key and every backend needs it. A path with an empty segment is a configuration error.
- The body is re-serialized only when a listed path removed something or a rename (ADR-0003) is needed; otherwise the bytes are forwarded as received. Key order is preserved either way.
- Headers are not touched. The `anthropic-beta` flag that accompanies a dropped field stays; a backend that also rejects unknown flags needs `headers` to override the value.

## Consequences

- Working around a backend's schema is a per-backend configuration change; no field name is known to the code.
- A dropped field silently disables its feature on that backend. For `context_management` that means no server-side context editing; the client keeps working because the field is optional.
- The body log (`logging.body_dir`) records the request as sent, so a dropped field is absent from `request.json`; the original is not kept.
- Backends with a non-empty `drop_fields` pay a JSON parse per request; ADR-0003's bit-for-bit round-trip is kept only for backends without it.
- A key that itself contains a dot cannot be named. The Messages API has none.
