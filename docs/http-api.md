# HTTP interface

What the router answers, and what it says when something fails. Claude Code needs none of this to work; it matters when you drive the router from a script or debug a response by hand.

| Route | Auth | Behaviour |
| --- | --- | --- |
| `GET /healthz` | no | `{"status":"ok"}` |
| `GET /`, `GET /ui/` | no | The web console page (no data; the page asks for the token). |
| `/api/*` | yes | The console's data and actions; see [The web console](console.md). |
| `GET /v1/models`, `GET /v1/models/{id}` | yes | The configured models, Anthropic list format. |
| `POST /v1/messages`, `POST /v1/messages/count_tokens` | yes | Routed by the body's `model`; path and query forwarded unchanged. On an `openai` backend, `/v1/messages` is translated and sent to `/v1/chat/completions`; `count_tokens` is 404. |

Auth is `x-api-key: <token>` or `Authorization: Bearer <token>`; failures are 401 in the Anthropic error format. Every response carries `x-request-id`; proxied ones also carry `x-anthroxy-backend`, `x-anthroxy-model` and `x-anthroxy-upstream-model`.

Errors the router produces are `{"type":"error","error":{"type":…,"message":…},"request_id":…}`: 400 for a body that is not a JSON object, has no `model`, or nests deeper than the router can rewrite, 404 for an unknown model (listing the configured ones), 413 over `server.max_body_bytes`, 502 when a backend is unreachable, redirects (the router will not follow one, and neither should Claude Code), or its credential cannot be obtained. Backend errors are relayed with their status; if the body is an Anthropic error, its message is prefixed with `[backend <name>, HTTP <status>]`. An `openai` backend's error body is always converted to an Anthropic error document (type from the status, same prefix); a failure in the middle of its stream is one `error` event.

Also see [Configuration](configuration.md) for `server.token` and `server.max_body_bytes`, and [The web console](console.md) for the `/api/*` routes.
