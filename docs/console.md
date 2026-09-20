# The web console

A page served by the router itself, for watching it from any browser that
reaches it — the Windows host included, when the router runs in WSL2. Nothing
needs installing.

`http://<router>/` redirects to `/ui/`, a page embedded in the binary. It signs
in with `server.token` (kept in the tab, or in the browser when "Remember" is
ticked) and sends it as a header on every call; there is no cookie. Anyone
holding the token can use everything on the page, including reloading the router
and deleting recordings.

| Tab | What it shows and does |
| --- | --- |
| Overview | Version, uptime, configuration file; every reload with its trigger, result and settings that need a restart, plus a Reload button; each backend's credential source, masked value, fetch time, reported expiry, next re-run and recent runs with errors; the model table; model names Claude Code sent that nothing serves (404, or served by `routing.default_model`) with an `aliases` fragment; the configuration in force with secrets redacted. |
| Requests | Requests in flight (elapsed time, first byte yet or not, bytes) and the last 200 finished (status, attempts, time to first byte, duration, tokens, output speed, outcome), filterable. A request's detail keeps the upstream error body with hints: a `drop_fields` fragment for a field the backend rejects, `CLAUDE_CODE_MAX_CONTEXT_TOKENS` for a context-window error, where to look for an unknown upstream model or a rejected credential. |
| Statistics | Charts over 24 hours (per hour), 7 days (per 6 hours), 30 days or everything kept (per UTC day): requests with errors, tokens (prompt, prompt from cache, output), time to first byte p50/p95 and output speed, each with a hover readout and a table view. Below them, per model and per UTC day: requests, errors, retries, client disconnects, first-byte and duration p50/p95, input, output, cache read and cache write tokens, cache hit rate, output tokens per second; fallback and credential re-send counts (a re-send after a 401/403 is not counted as a retry), and the model names no route serves with the default model that took them or none for a 404. |
| Tools | A test request (model, prompt, stream) through the full route, answered with status, timings, tokens and text; a probe of every backend from inside the running process (credential, route — direct or through its proxy — the request headers the router set with backend-forced values and the credential masked, the response headers, and model list, with context lengths from `max_model_len`, `loaded_context_length` and similar fields, or LM Studio's `/api/v0/models`) and a `[[models]]` fragment per listed model; Claude Code's variables for the address the browser used, with `CLAUDE_CODE_MAX_CONTEXT_TOKENS` once a probe found the smallest context window among configured models. |
| Recordings | The body log newest first, in the order the router recorded it, a request Claude Code sent again unchanged folded under the one it repeats. Each row says what its request sends: the prompt for the one that opens a turn, what it answers instead for every request after it — the tool results it returns with a hint from each call, or the notice that woke the model. A second rail, neutral beside the session's coloured one, runs down the turn a row belongs to and is notched where the turn begins; only that row repeats the turn's prompt, and the rows under it carry the time since it. With message count, whether it asked for a stream, and Claude Code session (a session filters the list to its requests), model, status, outcome and size, filterable; delete one or all. An entry opens with what it sends, the prompt of its turn, its route, result and token usage — the tokens of a large recorded stream once asked for — and steps to the newer or older entry of the list, saying when that list is filtered, and each of its files reads either as recorded (Raw) or in sections. A request's sections: the turn, as its prompt with what followed it, every message (system reminders and Claude Code's own notices folded, each tool call linked to its result), system blocks, tools grouped into built-in and one group per MCP server, the remaining parameters, each with its size, and, on request, a comparison with an earlier request of the same Claude Code session in the order a prompt cache reads them (tools, system, messages): how much of the prefix they share, where it first differs, and the messages after it; Find narrows every section to the items holding a text and marks it. A response is replayed into its final content, stop reason, usage and any error; the meta file lists its fields and headers. Both Messages and Chat Completions exchanges read this way. |

The page is served with a Content-Security-Policy that allows only its own
script, style and API calls. Requests the console sends are real: they reach the
backend, cost tokens and appear in the activity and statistics marked as coming
from the console. The in-memory lists (requests, reloads, credential runs, model
names) start empty on each restart; statistics and recordings are on disk.

API routes, all behind the token: `GET /api/status`, `GET /api/requests`,
`GET /api/requests/{id}`, `GET /api/stats?range=1d|7d|30d|all`, `GET /api/env`,
`POST /api/reload`, `POST /api/probe`, `POST /api/smoke`
(`{"model", "stream", "prompt"}`), `GET /api/recordings`,
`GET /api/recordings/{entry}/{file}`, `DELETE /api/recordings/{entry}`,
`DELETE /api/recordings`.

The page's own design system is documented separately in
[console-design.md](console-design.md); that page is for changing the console,
not for using it.
