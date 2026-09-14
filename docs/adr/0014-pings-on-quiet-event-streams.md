# 0014. Pings on quiet event streams

## Status

accepted

Supersedes, for the ping events described here, ADR-0003's rule that a successful body is relayed as the backend produced it and ADR-0010's "no `ping` is emitted"; the rest of both stands.

## Context

Claude Code gives up on a streaming response that sends no bytes for too long. Measured with Claude Code 2.1.270 behind the router, against a backend that sent its response headers and then stayed silent for 330 seconds, with the router's `read_timeout` raised above that:

1. Claude Code abandoned the stream after 180 seconds and sent the request again.
2. It abandoned the retry after another 180 seconds and fell back to a non-streaming request.

The same backend sending `event: ping` every 30 seconds during the silence, before any `message_start`, got one request that Claude Code waited on for the full 330 seconds and finished normally.

A local or shared backend can take that long before its first token: a long prompt's prefill, or a wait in the server's queue. Each retry starts the work again. The Anthropic API sends pings itself. Claude Code's guidance for gateways asks them to send their own `event: ping` during silent gaps, so that long pauses do not trip client or proxy idle timeouts.

The router sends nothing of its own on such a stream:
- For an `anthropic` backend it relays the backend's bytes (ADR-0003), pings included when the backend sends them.
- For an `openai` backend the Chat Completions stream has no ping. The router opens the message only at the first chunk and emits no ping (ADR-0010).

## Decision

- When a successful response to the client is `text/event-stream` and nothing has gone out for 15 seconds, the router sends `event: ping` with data `{"type": "ping"}`. It repeats every 15 seconds of silence. This applies to both backend kinds, before `message_start` as well as after it.
- A ping goes out only at an event boundary: before the first byte, or right after a blank line. A backend that stops in the middle of an event gets no ping, since one would corrupt that event.
- Pings are the router's own bytes, not the backend's:
  - The body log records the backend's bytes, so it has no pings.
  - An exchange's byte count, time to first byte and statistics count only the response itself.
- Nothing is sent before the backend's response headers arrive, since the status the client gets is not known until then. `upstream.read_timeout` still ends a backend that stays silent longer than it allows.

## Consequences

- A backend that needs minutes before its first token is waited on once, not requested three times.
- Relayed streams from an `anthropic` backend are no longer the backend's bytes alone when that backend goes quiet. A client that reads the stream as Anthropic events ignores pings, as it does the API's own.
- A backend that is slow to send even its response headers still trips the client's timeout; that has to be solved at the backend or with Claude Code's `CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS`.
