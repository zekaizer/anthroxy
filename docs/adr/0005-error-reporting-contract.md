# 0005. Error reporting contract

## Status

accepted

## Context

With one endpoint in front of several backends, "the request failed" is not actionable. The operator needs to know which backend was involved and whether the router, the network or the backend failed. Claude Code renders the Anthropic error body (`error.message`) to the user, so that message is the most visible place to put the answer. Retrying blindly can duplicate work, but a connection that was never established is safe to retry.

## Decision

**Shape.** Every error the router produces is an Anthropic error document, `{"type":"error","error":{"type":<anthropic type>,"message":<text>},"request_id":<router id>}`, with the matching status code:

| Situation | Status | `error.type` |
| --- | --- | --- |
| Client token missing or wrong | 401 | `authentication_error` |
| Body not JSON / no `model` | 400 | `invalid_request_error` |
| Body over `server.max_body_bytes` | 413 | `request_too_large` |
| Unknown model, unknown path | 404 | `not_found_error` |
| Credential command failed, backend unreachable | 502 | `api_error` |

502 is deliberate: it distinguishes "the router could not get an answer" from a backend's own 500.

**Identification.** Router-originated failures name the backend and cause in `error.message` (e.g. ``backend `vllm` unreachable after 3 attempt(s): ... connection refused``). Every response carries `x-request-id`; the same id appears in every log line of that request and in the error body. Responses that involved a backend carry `x-anthroxy-backend`.

**Upstream errors.** A 4xx/5xx from a backend is relayed with its own status and headers. If its body is an Anthropic error document, `error.message` is prefixed with `[backend <name>, HTTP <status>]` and `request_id` is filled with the router id when the backend supplied none. Any other body (plain text, another vendor's JSON) is relayed byte-for-byte. Errors inside an SSE stream are never touched.

**Retries.** Only failures that occur before any response byte has been relayed are retried: connection failures (refused, reset, closed before a status line) and statuses listed in `upstream.retry_on_status`. Timeouts are not retried; the backend may still be working. Default: 2 retries, 200 ms exponential backoff. Each retry is logged with attempt number and delay. A 401/403 against a refreshable credential triggers one credential refresh and re-send, independent of the retry budget (ADR-0004).

## Consequences

- Claude Code shows the backend name and status in its error output, so the operator does not need the router log for the common cases.
- Prefixing an upstream error message alters one string of the backend's response; clients that pattern-match on exact upstream messages would notice. Claude Code does not.
- Error bodies are buffered (they are small); success bodies are never buffered.
- Retrying connection failures can, in rare cases, send a request the backend actually received; for inference requests the cost is duplicated compute, never a changed outcome.
