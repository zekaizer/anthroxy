# 0011. Web console and persistent request statistics

## Status

accepted

## Context

A running router is observable only through its journal and through CLI commands run inside WSL2, while Claude Code usually runs on the Windows host where neither is at hand. The questions an operator asks of a running router are the same every time: is each credential fresh, did the last reload take, which request failed and why, which model names Claude Code sends that nothing serves, what the router is costing in tokens and whether prompt caching survives the trip. Logs answer them only after a search, and every number is lost on restart. Checking a backend end to end (`/v1/messages`, not only `/v1/models`) needs a Claude Code session.

ADR-0003 makes `/healthz` the only unauthenticated route, answers every other path 404, and relays successful bodies without parsing them. ADR-0002 names `SIGHUP` as the reload trigger. A console served by the router itself contradicts all three.

## Decision

**One listener.** The console is served by the router on `server.listen`. `GET /` redirects to `/ui/`. `/ui/` serves a page, a script and a stylesheet embedded in the binary, without authentication: they carry no data. They are sent with a Content-Security-Policy that allows only same-origin scripts, styles and requests.

**Authentication.** The console's data and actions live under `/api/`, behind `server.token`, accepted exactly as for `/v1/*`. The page keeps the token in browser storage and sends it as a header from script, never as a cookie, so a cross-site request cannot carry it. Holding the client token is therefore holding control of this router: reloading it, probing and calling its backends, and deleting body recordings. There is one operator (ADR-0002).

**Reload.** `SIGHUP` and `POST /api/reload` run the same reload, and the outcome of each (time, trigger, success or the rejection message, settings that need a restart) is kept in memory for the console.

**Observing bodies.** Successful bodies are still relayed chunk by chunk and byte for byte (ADR-0003, ADR-0010). The router additionally scans the Messages response it hands the client, event stream or document, for `usage` and `error`, to record token counts and failures that arrive with status 200.

**In-memory activity.** The most recent exchanges (bounded), the exchanges in flight, a bounded tally of model names that matched no route or fell to `routing.default_model`, and credential refresh history are kept in memory and lost on restart. An upstream error body is kept with its exchange, cut to a bounded excerpt, together with configuration hints derived from it.

**Persistent statistics.** Every `POST /v1/messages` exchange appends one JSON line to `requests-YYYY-MM-DD.jsonl` (UTC date) under `stats.dir`, default `$XDG_STATE_HOME/anthroxy/stats`. A line carries routing, status, attempts, timings, bytes, outcome and token usage, and a schema version `v`; it never carries message content. Statistics are on unless `stats.enabled = false`. Files are owner-only, appended without fsync, and deleted after `stats.retention` (default 90 days) by the sweep that prunes body recordings. The console aggregates the lines on demand; there is no database.

## Consequences

- Opening `http://<router>/` from the Windows host shows the router's state without a shell in WSL2.
- ADR-0003's single unauthenticated route and its "no SSE parsing" rule, and ADR-0002's single reload trigger, no longer hold as written.
- A leaked client token grants more than model access; the token is already the router's only secret gate, and the configuration file that holds it can run commands (ADR-0004).
- Scanning the client-facing body costs a JSON parse of the frames that mention `usage` or `error`, not of every frame.
- Losing the last lines of a statistics file in a crash is accepted. Requests that never reach `/v1/messages` (`count_tokens`, `/v1/models`) are not in the statistics.
- A request the console sends to test a backend is a real request: it reaches the backend, costs tokens and appears in activity and statistics, marked as coming from the console.
- Aggregation reads every line in the chosen range; at a single operator's volume that is fast, and a precomputed summary can be added later without changing the line format.
