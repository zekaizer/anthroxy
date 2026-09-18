# 0016. Passthrough backend and unauthenticated `/v1`

## Status

accepted

## Context

Claude Code's subscription path uses `/login` OAuth (`Authorization: Bearer sk-ant-oat01-…`). That path is off as soon as `ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_API_KEY` is set. The operator still wants one `ANTHROPIC_BASE_URL` that can `/model` between Anthropic's live catalogue and local backends.

ADR-0003's `/v1` always requires `server.token`, and ADR-0004 replaces the client's `Authorization` with a backend credential. Both make the subscription Bearer unusable: the router either 401s it or strips it. Putting a credential on a `passthrough` backend would do the same strip, so that backend cannot carry a credential of its own.

`GET /v1/models` today is only the `[[models]]` table. Anthropic's current ids are not in that file.

## Decision

- `server.v1_auth` is `"token"` (default, ADR-0003) or `"none"`. `"none"` makes `/v1/*` unauthenticated and requires `server.listen` on a loopback address. The console still uses `server.token`. `anthroxy env` then emits `ANTHROPIC_BASE_URL` without `ANTHROPIC_AUTH_TOKEN`.
- A backend may have `kind = "passthrough"`. At most one. It has a `url` and no `[[models]]` pointing at it. Its model list is `GET {url}{models_path}` (default `/v1/models`), cached 30 seconds per client `Authorization`. A fetch failure omits the live entries; configured models remain.
- `GET /v1/models` returns the live list first, then `[[models]]`. An id that exists in both is the configured route only.
- A request whose `model` is only on the live list goes to that backend. Configured id/alias still wins.
- A passthrough backend has no credential, `headers`, `anthropic_beta`, or `drop_fields`. The client's `Authorization` / `x-api-key` are forwarded. Request and response bytes are not rewritten: no model rename, no tool-id normalisation, no error annotate, no `event: ping`, no `x-anthroxy-*`. Path and query are appended as for `kind = "anthropic"`.
- Other backends keep ADR-0003/0004/0005/0013/0014.

## Consequences

- One Claude Code session with only `ANTHROPIC_BASE_URL` can name a live Anthropic id (subscription oat goes upstream) or a configured local id (backend credential, oat not forwarded).
- Loopback is the only `/v1` gate when `v1_auth = "none"`. Binding `0.0.0.0` in that mode is a configuration error.
- Refresh and other OAuth side calls stay on Anthropic's hosts; the router does not proxy them.
- The picker can show models the configuration file does not name. A stale cache (30s) can hide a newly enabled id until it expires.
