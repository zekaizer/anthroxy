# spec: OpenAI Chat Completions backend
Source: intent/openai-chat-completions-backend/intent.md @ aa80fc5. Status: draft. Date: 2026-09-10.

## 1. Problem and goals

- P1. The in-house LLM service cannot be used from Claude Code because anthroxy only attaches backends that speak the Anthropic Messages API. (evidence: E1, E9 / frequency: E2)
- G1. Models of the in-house service can be selected and used in Claude Code the same way as the existing Anthropic models. (resolves: P1 / verification: (a) text conversation with streamed display, (b) a tool-use loop runs to completion, (c) errors are shown on screen with backend name and cause, (d) switching between an Anthropic model and the in-house model with `/model` keeps the conversation history)

## 2. Current structure (per E3)

Claude Code → `POST /v1/messages` → body buffered, `model` and `stream` peeked → routed → rename/drop_fields → forwarded to the backend with the path unchanged → 2xx relayed chunk by chunk, 4xx/5xx buffered and the message prefixed.

## 3. Confirmed facts

### Environment [code observed 2026-09-10, commit aa80fc5]

- E1. Backend configuration has no notion of kind (`src/config/schema.rs:120`).
- E2. One user, every daily session. [user, 2026-09-10]
- E3. Proxy path as in section 2 (`src/server/handlers/proxy.rs`, ADR-0003/0005).
- E4. Only `/v1/messages` and `/v1/messages/count_tokens` are proxied (`src/server/routes.rs:17`).
- E5. The `check` probe reads `data[].id` from `GET /v1/models` and already accepts the OpenAI list shape (`src/upstream/probe.rs:113`).
- E6. The body log records the request as sent and the response as received (`src/observability/body_log.rs`).
- E7. Router error contract: 401/400/413/404/502 as Anthropic error documents, `x-request-id` (ADR-0005).
- E8. The existing vLLM backend is attached through its Anthropic-compatible endpoint (README, ADR-0009).
- E9. OpenCode uses the in-house service via `@ai-sdk/openai-compatible`. [user, 2026-09-10]
- E10. Claude Code ≥ 2.1.152 recovers by itself from the 400 caused by leftover `thinking` blocks after a switch (README, ADR-0003).

### External [fetched 2026-09-10]

- F1. Anthropic streaming events: `message_start` / `content_block_start` / `ping` / `content_block_delta` (text_delta, input_json_delta, thinking_delta, signature_delta) / `content_block_stop` / `message_delta` (stop_reason, usage) / `message_stop` / `error`. [platform.claude.com, "Streaming messages"]
- F2. OpenCode: `@ai-sdk/openai-compatible` → `/v1/chat/completions`, `@ai-sdk/openai` → `/v1/responses`. [opencode.ai/docs/providers]

## 4. Assumptions

- A1. OpenAI Chat Completions streaming format: `chat.completion.chunk` lines with `delta.content`, `delta.tool_calls[{index, id, function{name, arguments}}]`, `finish_reason ∈ {stop, length, tool_calls, content_filter}`, usage in the final chunk when `stream_options.include_usage` is set, terminated by `data: [DONE]`. (basis: OpenAI API reference, re-fetch failed today with 403 / verify: U2 / if wrong: R5–R7 revisited)
- A2. Extent of the in-house service's implementation unknown — tools streaming, `stream_options`, `system` role, `reasoning_content`. (verify: U2)
- A3. The set of fields Claude Code sends: `model`, `max_tokens`, `messages` (text/tool_use/tool_result/image/thinking blocks), `system` (array with cache_control), `tools` (input_schema), `tool_choice`, `metadata.user_id`, `stream`, `temperature`, `thinking`, `context_management` (beta). (basis: general knowledge + ADR-0009 / verify: U1 / if wrong: IC3 and R10 targets change)
- A4. Images are inline base64 png/jpeg. (verify: U1 / if wrong: translated types change)
- A5. DeepSeek-family servers return 400 when `reasoning_content` appears in input messages. (basis: general knowledge / verify: U9 / if wrong: R9 may need reversing)

## 5. Requirements

Common premise: WHERE the backend is configured as the OpenAI kind.

### Functional

- R1. WHEN Claude Code sends `POST /v1/messages` THEN the system SHALL translate it into a Chat Completions request and forward it to the backend. (Must) — Verify: the body received by a mock OpenAI backend matches the IC3 mapping — Basis: P1, E3, C2
- R2. The system SHALL forward `system` (string or block array) as a single system message. (Must) — Verify: a block array arrives as one system message with the texts concatenated in order — Basis: A3, IC3
- R3. The system SHALL translate `tools` and `tool_choice` per IC3. (Must) — Verify: `input_schema` becomes `function.parameters`; each of `auto/any/tool/none` maps to its counterpart — Basis: G1(b), A3
- R4. The system SHALL translate `tool_use` and `tool_result` blocks in the history into assistant `tool_calls` and `tool`-role messages, preserving id correspondence. (Must) — Verify: in a three-turn tool-loop history every `tool_call_id` matches a preceding `tool_calls[].id` — Basis: G1(b)(d)
- R5. WHEN `stream=true` THEN the system SHALL translate backend chunks into the Anthropic SSE events of IC4 and forward them as they arrive. (Must) — Verify: for a text + tool_calls chunk stream the client receives events in `message_start … message_stop` order with IC4 content — Basis: G1(a), F1, A1
- R6. WHILE streaming the system SHALL accumulate `tool_calls` deltas with the same `index` into one `tool_use` block and emit `arguments` as `input_json_delta`. (Must) — Verify: two parallel tool calls arrive as distinct block indexes and concatenating each block's `partial_json` reproduces the original `arguments` — Basis: G1(b), A1
- R7. The system SHALL translate `finish_reason` and `usage` into `stop_reason` and `usage` per IC4. (Must) — Verify: `stop/length/tool_calls` yield `end_turn/max_tokens/tool_use` in `message_delta` — Basis: F1, A1
- R8. WHEN `stream=false` THEN the system SHALL translate the completed response into an Anthropic message document. (Should — promoted to Must depending on U5) — Verify: a non-streaming tool_calls response becomes `content[]` with `text` and `tool_use` blocks — Basis: A3
- R9. The system SHALL NOT forward `thinking` or `redacted_thinking` blocks from the history to the backend. (Must) — Verify: with a history produced by an Anthropic backend, the mock backend receives no such content — Basis: G1(d), E10
- R10. The system SHALL NOT forward fields with no Chat Completions counterpart (`cache_control`, `metadata`, `context_management`, the `thinking` request parameter). (Must) — Verify: the mock backend's body contains none of these keys — Basis: A3, ADR-0009 context (fixed-schema servers answer 400)
- R11. The system SHALL translate `image` blocks (base64) into `image_url` parts with a `data:` URI. (Should) — Verify: one png block arrives as `data:image/png;base64,…` — Basis: user round 1, A4, U6
- R12. IF the backend returns 4xx/5xx THEN the system SHALL translate it into an Anthropic error document per IC5, prefixing the message with backend name and status. (Must) — Verify: an OpenAI-shaped 401 body becomes `authentication_error` and `error.message` starts with `[backend X, HTTP 401]` — Basis: G1(c), E7, D3
- R13. IF the backend connection drops or an error chunk arrives mid-stream THEN the system SHALL send an Anthropic `error` event and close the stream. (Must) — Verify: when the mock backend disconnects after two chunks the client receives `event: error` and the stream ends — Basis: G1(c), F1
- R14. WHEN a backend chunk carries `reasoning_content` THEN the system SHALL forward it as `thinking_delta` of a `thinking` block. (Must) — Verify: three reasoning chunks followed by text chunks yield a `thinking` block at index 0 and a `text` block at index 1, in order — Basis: user round 3, F1

### Non-functional

- NFR1. The system SHALL translate chunk by chunk — no buffering between the first backend chunk and the first client event. (Must) — Verify: with the mock backend inserting 500 ms between chunks, the client receives events at the same spacing — Basis: G1(a), E3
- NFR2. The system SHALL leave the behaviour of existing Anthropic-kind backends unchanged, bit-for-bit passthrough included. (Must) — Verify: all existing `tests/` pass; a backend without a kind never enters the translation code — Basis: E3, E8
- NFR3. The system SHALL record in the body log the OpenAI-shaped request as sent upstream and the response as received. (Should) — Verify: `request.json` is byte-identical to what the mock backend received — Basis: E6
- NFR4. `check` SHALL probe OpenAI backends too and confirm the upstream model exists. (Must) — Verify: ✓/! verdicts from an OpenAI-shaped `/v1/models` response — Basis: E5, U4

## 6. Interface contracts

- IC1. Configuration: a kind field on `backends.<name>`; absent = Anthropic (current behaviour). — Verifies: R1, NFR2 — name and values: D4
- IC2. Path: `/v1/messages` → `<url>/v1/chat/completions`. The query string is discarded. — Verifies: R1
- IC3. Request mapping: `system` → system message / `messages[].content`: text → text part, image → image_url, tool_use → `tool_calls`, tool_result → `tool` message (content flattened to text) / `tools[]` → `{type: function, function: {name, description, parameters = input_schema}}` / `tool_choice`: auto → `auto`, any → `required`, tool → `{type: function, function: {name}}`, none → `none` / `max_tokens` → `max_tokens` / `temperature`, `top_p`, `stop_sequences` (→ `stop`), `stream` unchanged / when `stream=true`, `stream_options.include_usage=true` is always added. — Verifies: R1–R4, R10, R11
- IC4. Response mapping: first chunk → `message_start` (id, model; `usage.input_tokens` is 0 until the usage chunk) / `delta.reasoning_content` → `thinking` block `thinking_delta`, always at a lower index than text and tool_use blocks / `delta.content` → `text` block `text_delta` / `delta.tool_calls[i]` → per-index `tool_use` block + `input_json_delta` / `finish_reason`: stop → `end_turn`, length → `max_tokens`, tool_calls → `tool_use`, content_filter → `end_turn` (D8) / usage: `prompt_tokens` → `input_tokens`, `completion_tokens` → `output_tokens` / `[DONE]` → `message_stop`. No `ping` is sent. — Verifies: R5–R8, R14
- IC5. Error mapping: HTTP status unchanged. `error.type`: 400 → `invalid_request_error`, 401 → `authentication_error`, 403 → `permission_error`, 404 → `not_found_error`, 413 → `request_too_large`, 429 → `rate_limit_error`, anything else → `api_error`. `error.message` = `[backend <name>, HTTP <status>] ` + the OpenAI `error.message` (or the first 200 characters of the body when absent). — Verifies: R12
- IC6. Mid-stream error: `event: error` with `error.type` = `api_error` and the backend name in the message. No events follow. — Verifies: R13

## 7. Constraints

- C1. Rust edition 2024; the only ingress is Claude Code speaking the Anthropic Messages API; runtime is WSL2. [CLAUDE.md]
- C2. Chat Completions (`/v1/chat/completions`) only. [intent]
- C3. Translation goes through an intermediate representation (IR) layer. [intent, author's decision]
- C4. 0.x cycle: breaking changes to configuration, CLI and HTTP allowed, no compatibility shims. [CLAUDE.md]
- C5. Adding a backend kind changes the configuration schema (an external contract) and requires an ADR. [CLAUDE.md, global guidelines]

## 8. Out of scope

- OOS1. Responses API (`/v1/responses`). [intent]
- OOS2. Ingress from clients other than Claude Code (OpenCode, etc.). [intent]
- ~~OOS3. Multimodal input translation~~ (moved into scope, user round 1; see R11)

## 9. Open decisions

- ~~D1. Behaviour when an image block arrives~~ (dependent on the withdrawn OOS3)
- D2. Decided: `reasoning_content` → `thinking` block (user round 3). Consequence: RISK1.
- D3. Decided: backend errors are translated into Anthropic error documents (user round 3; implied by G1(c)).
- D4. Kind field name and values — at implementation start; default `kind = "openai"`.
- D5. `count_tokens` routed to an OpenAI backend — (conditional: when U3 says "called") (a) fixed value (b) character-based estimate (c) 404. Owner: user.
- D6. Whether `drop_fields` applies before or after translation — at implementation start; default "before" (on the Anthropic shape).
- D7. Whether the body log also records the translated result sent to the client — at implementation start.
- D8. `content_filter` mapping — at implementation start; default `end_turn`.
- D9. Whether to emit an empty `signature_delta` when closing a `thinking` block — at implementation start; default "omit".

## 10. Unknowns and next actions

- U1. What Claude Code actually sends — normal, background and `count_tokens` requests. Discriminate: run the corporate anthroxy with `serve --body-dir DIR` for one session and collect three `request.json` files → confirms or refutes A3, A4; closes U3 (is `count_tokens` called), U5 (any `stream=false`), U7 (any PDF blocks). **Top priority.**
- U2. Extent of the in-house service's Chat Completions implementation (bundled: U4, U6, U9). Discriminate: one curl with `stream=true` + `tools` + `stream_options.include_usage=true` + one image part + one assistant message carrying `reasoning_content`:
  - `tool_calls` deltas carry `index` → A1 confirmed; otherwise R5–R7 revisited
  - usage chunk present → IC4 usage mapping holds; absent → `usage` stays 0
  - `reasoning_content` present → R14 exercised; absent → R14 inert
  - image part accepted → R11 feasible (U6); rejected → R11 blocked
  - `GET /v1/models` answers → NFR4 holds (U4)
  - `reasoning_content` in history rejected or ignored → keep R9; required → reverse R9 (U9)
  Second priority.
- U3. Whether Claude Code calls `count_tokens`. (bundled: U1)
- U4. Whether the in-house service serves `GET /v1/models`. (bundled: U2)
- U5. Whether Claude Code ever sends `stream=false`. (bundled: U1)
- U6. Whether the in-house service accepts `image_url` parts with `data:` URIs. (bundled: U2)
- U7. Whether Claude Code sends document (PDF) blocks. (bundled: U1)
- U8. Whether the in-house service accepts file (PDF) parts. (dependent: U7)
- U9. Whether the in-house service rejects, ignores or requires `reasoning_content` in assistant history messages. (bundled: U2)

## 11. Withdrawn premises

- OOS3 and D1 (user round 1: multimodal support is in scope).
- The G1 ↔ OOS3 contradiction ("indistinguishable backend" vs. untranslated image blocks), resolved by withdrawing OOS3.

## 12. Flagged

Policy conflict: ADR-0003's verbatim-passthrough rule cannot coexist with R1–R14; an OpenAI backend must interpret request and response bodies. Resolution per C5: a new ADR supersedes ADR-0003 for the OpenAI kind only. Owner: the user — accepted in round 4.

Risks (accepted in round 4):

- RISK1 (D2). `thinking` blocks produced by the router carry no signature; a history holding them sent back to an Anthropic backend gets a 400 and Claude Code retries once without them (E10). G1(d) holds; the first request after switching back is sent twice.
- RISK2 (U1). Claude Code's real request fields are unverified; if A3/A4 are wrong, the IC3 mapping targets and the R10 removal list change.
- RISK3 (U2, U4, U6, U9). The in-house service's implementation extent is unverified; see U2 for the per-outcome consequences.
- RISK4 (U3 → D5). `count_tokens` behaviour on an OpenAI backend is undefined until U3 closes.
- RISK5 (U5). R8 stays Should until U5 shows whether non-streaming requests occur.
- RISK6 (U7 → U8). PDF parts may need adding to R11.

Goal back-check: P1 → G1 → (a) R1, R2, R5, NFR1 / (b) R3, R4, R6, R7 / (c) R12, R13, IC5, IC6 / (d) R4, R9, R14 + RISK1. Multimodal (user round 1) → R11. No unresolved goal.

intent.md Out of scope was updated accordingly (multimodal item removed).
