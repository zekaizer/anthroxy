# 0020. A backend's error wording reaches the client unchanged

## Status

accepted

Supersedes ADR-0005's rule that an upstream Anthropic error message is prefixed
with the backend and status and has `request_id` filled, and its consequence
that Claude Code does not pattern-match on upstream messages. Supersedes, for
error documents, ADR-0010's message format
`[backend <name>, HTTP <status>] <message>`. The rest of both stands.

## Context

ADR-0005 put `[backend <name>, HTTP <status>]` in front of every upstream error
message on the premise that Claude Code renders the message and matches nothing
in it. That premise no longer holds. Claude Code recovers from several
rejections by reading the upstream's wording: it drops `output_config.effort`
when a 400 starts with `Unexpected value(s)`, drops `context_management` when
the message says it is not permitted, switches `thinking.type`, and compacts the
conversation when the message says `prompt is too long`. Its gateway guide asks
a gateway to forward error bodies unmodified, and says an envelope of the
gateway's own breaks recovery unless the message carries wording Claude Code
recognises (llm-gateway-protocol, "Automatic retry and error forwarding").
A prefix breaks every match anchored at the start of the message.

An `openai` backend's error body has to be rebuilt as an Anthropic document, so
there is no original wording to keep. What Claude Code would act on there is a
context overflow, which each server words its own way: OpenAI's
`context_length_exceeded`, vLLM's `maximum context length is …`, llama.cpp and
LM Studio's `exceed_context_size_error` and `exceeds the available context
size`. Claude Code recognises none of these, so a session on such a backend
stops at the overflow instead of compacting. vLLM's own Messages endpoint, a
`kind = "anthropic"` backend, sends the same vLLM wording inside an Anthropic
error document, with the same result.

## Decision

- An error body from a `kind = "anthropic"` backend is relayed byte-for-byte,
  with one exception below. The router adds nothing else to it.
- An error from a `kind = "openai"` backend is encoded with the backend's own
  message and no prefix, in the HTTP response and in an `error` event alike.
- The IR gains a failure kind for a request that exceeds the model's context
  window, and one table of the names and words servers use for it. Both codecs
  decode a failure as that kind when its names or message match the table.
- The Anthropic codec spells that kind the way the Anthropic API does:
  `invalid_request_error` with a message starting `prompt is too long: `,
  followed by the backend's own message. An Anthropic error document from a
  `kind = "anthropic"` backend whose message matches the table, and does not
  already start that way, gets that start and nothing else changed. The
  Anthropic API's own `prompt is too long` is not in the table, so its
  documents pass untouched.
- Failures the router finds itself (an unreachable backend, a malformed event
  stream, a credential command that failed) keep naming the backend in their
  message. Those are the router's words, and no client matches on them.
- Response headers stay relayed as the backend sent them, `retry-after` and
  `x-should-retry` included, for every backend kind.

## Consequences

- Claude Code's capability retries and automatic compaction work behind the
  router as they do against the backend directly.
- Claude Code's error line no longer names the backend or its status. The
  `x-anthroxy-backend` header, the request id and the console still do.
- A relayed Anthropic error carries the backend's `request_id`, or none. The
  router's id is in `x-request-id` on every response.
- A context overflow worded in a way the table does not list reaches the
  client in the backend's words, which Claude Code does not compact on.
  Recognising a new server is one table row.
