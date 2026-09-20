# How an `openai` backend is served

anthroxy speaks the Anthropic Messages API to Claude Code. A backend with `kind = "openai"` speaks OpenAI Chat Completions instead, so every request and response crosses a translation layer. This page describes that layer; ADR-0010 records the decisions behind it.

## The rule

Everything a non-Anthropic backend sends or receives crosses the IR. That covers the chat request and its answer, but also the model list, the error body, and anything a probe or a CLI subcommand reads from such a backend. Each side has one codec — `anthropic/` knows the Messages API and the IR, `openai/` knows Chat Completions and the IR — and `translate/` is the only module that holds both ends.

A shortcut is a defect even when it is short: no reading a field out of one format to write it into the other, no second parser for a shape a codec already decodes. When a new thing has to cross, it gets a place in `ir/` first.

## The shape

```
Claude Code                    anthroxy                                    OpenAI server
───────────                    ────────                                    ─────────────
POST /v1/messages ──► anthropic::decode ──► ir::Request ──► openai::encode ──► POST /v1/chat/completions
(Messages JSON)        Messages → IR                        IR → Chat Completions

SSE events        ◄── anthropic::StreamEncoder ◄── ir::Event… ◄── openai::ChunkDecoder ◄── SSE chunks
(message_start…)      IR → Messages SSE                      Chat chunk → IR       ◄── sse::Parser ◄── bytes

JSON document     ◄── anthropic::encode_message ◄── ir::Message ◄── Message::from_events ◄── openai::decode_response
```

The middle column is the intermediate representation (IR, `src/ir/`). It names no wire format. The Anthropic codecs (`src/anthropic/`) know only the Messages API and the IR; the OpenAI codecs (`src/openai/`) know only Chat Completions and the IR. The one place that holds both ends is `src/translate/`, and the axum-facing glue is `src/server/handlers/openai.rs`. A backend without a kind never enters any of this: its bytes are relayed as before.

A probe (`anthroxy check`, the console) reads a backend the same way: `translate::models` for the list, `translate::failure` for an error body. There is no second parser anywhere.

`GET /v1/models` from a live origin is the same split: `openai::decode_models` or `anthropic::decode_models` → `ir::Model` → `anthropic::encode_models` (Claude Code catalog fields: `context_window`, `runtime`, `thinking`). `translate::models` is the only function that chooses a catalog decoder by backend kind.

## Request

1. `proxy.rs` peeks `model` and `stream`, routes, applies `drop_fields` to the Anthropic body, and hands the body to `handlers/openai.rs` when the backend is an `openai` one. `count_tokens` stops here with a 404: Chat Completions has no such call.
2. `anthropic::decode` turns the Messages request into `ir::Request`. This is where Anthropic-only things are settled: `system` (string or blocks) becomes one text; content blocks become typed parts (text, image, tool use, tool result with its images kept apart); `thinking` blocks and `cache_control` are dropped; a `document` becomes its text or a note saying it was omitted; a block a role cannot carry, a field of the wrong type, or a block type the IR has no place for is a 400 rather than a silent loss.
3. `openai::encode_request` writes `ir::Request` as a Chat Completions body: the system text first, `tool_result` parts as `tool` messages (their images ride in the user message that follows), `input_schema` as `function.parameters` (an object schema always gets `properties`), `metadata.user_id` as `user`, `output_config.effort` as `reasoning_effort`, and `stream_options.include_usage` on every stream.
4. The request goes to `<url>/v1/chat/completions` with the credential and without `anthropic-version` / `anthropic-beta`.

## Response

1. `Relay` forwards the backend's bytes unchanged and records them in the body log; the translator sits on top of it, so the log always holds what the backend actually sent.
2. `sse::Parser` cuts bytes into frames. A line split across two chunks is kept until it completes; frames without data are keep-alives.
3. `openai::ChunkDecoder` turns one frame into IR events: the first chunk opens the message, `reasoning_content` is a thinking delta, `content` a text delta, each `tool_calls[i]` a tool call start plus argument deltas (servers that omit `index` or send the name late are handled), `finish_reason` and `usage` become their events, `[DONE]` ends the message, an error frame is an error event.
4. `anthropic::StreamEncoder` writes Messages SSE from those events. It owns the state machine: block indexes in arrival order, `content_block_start/stop` when the kind of content changes, `message_delta` with the stop reason (`tool_use` whenever a tool block went out) and the usage, and one `error` event followed by silence when anything fails.
5. A non-streaming answer takes the same events through `ir::Message::from_events` into one document. Both paths follow the same block rules, and a test holds them to it. A backend that answers a streaming request with a document is replayed as the events it stands for.

## Errors

A failure crosses the IR like anything else: each side's codec decodes its own error body into `ir::Failure` — a `FailureKind` the client can act on, plus what the origin said — and `anthropic::encode_error` writes the Messages error document from it. `translate::failure` is the only place an error decoder is chosen by backend kind.

The kind comes from the HTTP status where the status says something, and from the names the document uses (`error.type`, `error.code`, spelled loosely across servers) where it does not: a 503 that calls itself `overloaded_error` reaches the client as `overloaded_error`, not `api_error`. Inside an event stream there is no status at all, so the document's own names are all there is.

A 4xx/5xx from the backend keeps its status — that is what the transport said and what a retry policy reads — so a server that answers `500` with `invalid_api_key` reaches the client as HTTP 500 carrying `authentication_error`. The type and the status may disagree that way; the type is the more honest of the two. The message starts with `[backend <name>, HTTP <status>]`. A failure inside the stream (connection lost, malformed frame, an error frame) is one `error` event carrying the same kind.

## What the IR buys

- One-sided changes. Supporting `role: "system"` mid-conversation touched the two codecs a few lines each; carrying tool-result images added one IR field; the `document` note lives in the decoder alone.
- Meaning, not shape. `ir::Usage` is defined the Anthropic way (input excludes cache reads), so the OpenAI decoder does the arithmetic once and the encoder only writes.
- One response path. Stream and document are the same event sequence, which is also why a document answered to a streaming request needs no special code.
- No buffering. Events flow per chunk; the first token reaches Claude Code as soon as the backend produces it.
- Quiet gaps are filled. While the backend produces nothing, a `ping` goes out every 15 seconds, before `message_start` too, since Claude Code abandons a stream that stays silent for three minutes (ADR-0014).
- A place for the next API. Another wire format is another pair of codecs; the Anthropic side stays as it is.

## What it costs

- Every request to an `openai` backend is parsed and re-serialized; the byte-for-byte relay of ADR-0003 is kept only for `anthropic` backends.
- The router lags the Anthropic API for this kind: a new block type or parameter is a 400 until a mapping exists.
- The thinking blocks the router emits carry no signature. They are stripped again before any `anthropic` backend sees them, so switching models back costs no failed request.
- Tool call ids are the backend's own and reach Claude Code unchanged. Some servers use characters the Anthropic API rejects (vLLM names a Kimi model's calls `functions.<name>:<n>`), so a request to an `anthropic` backend has those characters replaced with `_` (ADR-0013).
