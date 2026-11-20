# 0022. Client emulation presets for `openai` backends

## Status

accepted

Supersedes one clause of ADR-0010 for a backend whose preset names a reasoning
field: "`thinking` and `redacted_thinking` blocks in the history are not sent
either". Extends ADR-0019: a preset's values take `{<variable>}` beside
`{header:<name>}`.

## Context

An OpenAI Chat Completions server is often built around one client. The
in-house service is used through opencode, and anthroxy reaches it today with a
`user-agent` header and a credential. Nothing fails. The request anthroxy sends
is still told apart from opencode's at every level that was measured (opencode
1.19.6 and Claude Code 2.1.328 against one recording server, 2026-11-20):

- Headers: other names, another order, lower-case names where opencode writes
  the standard ones in title case.
- Protocol: against a server that offers both, the router speaks HTTP/2 and
  opencode HTTP/1.1.
- Body: `user` and `reasoning_effort` that opencode does not send, no
  `tool_choice` where opencode sends `"auto"`, a second `system` message after
  the user's turn, another order of the top-level fields.
- Messages: opencode sends an earlier turn's reasoning back as
  `reasoning_content` and writes `content: ""` on a turn that only calls
  tools. The router drops the reasoning and leaves `content` out.

`headers`, `drop_headers`, `drop_fields`, `set_fields` and
`mid_conversation_system` close most of the header and field differences when
set by hand. They do not reach the order of anything, the protocol, the replay
of reasoning, or a field that depends on whether the request has tools.

opencode changes what it sends between releases. Its unreleased encoder writes
`max_completion_tokens`, adds `strict: false` to tool definitions and sends
`content: null`. A client's wire format written into the router would need a
release for each of those.

Claude Code returns a `thinking` block it received from the router in its next
request, with the text intact and an empty signature. The reasoning to replay
is in the request; the router has nothing to remember.

## Decision

- A backend of `kind = "openai"` may name a preset in `emulate`. A `[[models]]`
  entry may name one in place of its backend's. Without `emulate` a backend
  receives what it received before.
- A preset is data. It lists headers to send and to drop, body fields to omit
  and to set, the order of headers and of top-level fields, and a value for
  each of a fixed set of behaviours the encoder implements: where a later
  system message goes, the field earlier reasoning is sent under, the
  `tool_choice` of a request with tools, the `content` of a turn that only
  calls tools, `strict` on tool definitions, the name of the output-token
  limit, the case of header names, and whether the connection is HTTP/1.1.
  `docs/emulation.md` is the reference for every key.
- A preset has no conditions, no expressions and no functions on values. A
  behaviour that depends on the request is a named behaviour in the code, and
  the preset picks its value. A header that must be computed stays the job of
  `header_command` (ADR-0021).
- A preset declares variables with defaults, and `emulate` may give them other
  values. A value uses one as `{<name>}`. This form exists in a preset only:
  in a backend's own `headers` and `set_fields` the same characters remain
  literal text, as ADR-0019 left them.
- A key written on the backend wins over the preset's for the item both name.
  A preset is a set of defaults for a backend, not a second configuration.
- One preset, `opencode`, is built in. Others are files beside the
  configuration file. A preset with an unknown key or a value outside its
  key's set is a configuration error.
- The router does not build presets and has no route or command that records
  another client's requests. A preset is written, by a person or a language
  model, from a log the client itself prints.
- Reasoning sent back to the backend crosses the IR like everything else: the
  Anthropic codec decodes a `thinking` block into the IR and the OpenAI codec
  writes it under the field the preset names. A backend without that key gets
  no reasoning, as ADR-0010 decided.
- The router keeps an HTTP client for each combination of protocol and
  header-name case, since both are settings of a client and one client served
  every backend. Routes with the same settings share a client.

## Consequences

- A new release of an emulated client is a preset file, as long as what
  changed is a value of something the preset vocabulary holds. A new kind of
  behaviour, such as opencode's unreleased `<system-update>` wrapping of a
  later system message, still needs code and then a new allowed value.
- The request stays different where no preset reaches: the system prompt, the
  tool names and schemas and the text of tool results are Claude Code's; the
  TLS handshake is the router's HTTP stack's. A server that reads those can
  tell the router from the client it emulates.
- Header names are all lower case or all title case. opencode mixes them, and
  writing a name in a case of its own is not something the HTTP library
  exposes. Matching it would mean another library for emulated backends, which
  this decision does not take.
- A preset does not send `Accept-Encoding`. The router relays and records
  bodies as they arrive and has no decompression, so it does not invite a
  compressed answer.
- The preset vocabulary repeats four backend keys. Two places can now name a
  header or a body field, and `check` has to show which one a backend ends up
  with.
- Order and case of headers exist over HTTP/1.1 only, so a preset that sets
  either must also set the protocol. A preset that left it open would change
  nothing against a server that offers HTTP/2.
- A preset holds only what its log showed. A trait the log does not show, such
  as a turn that only calls tools, keeps the router's own behaviour, and the
  request differs there without any error.
- The `thinking` blocks the router makes still carry no signature and are
  still removed before an `anthropic` backend sees them (ADR-0010).
  After Claude Code compacts a conversation, the turns before it are gone from
  the request and so is their reasoning.
- Connections are pooled per combination of those settings. A route that
  sets one shares no connection with a route that sets none.
