# 0013. Tool call ids an Anthropic backend accepts

## Status

accepted

Supersedes the clause of ADR-0010's rule for router-made thinking blocks that a
body without the text `"thinking"` is not parsed at all; the rest of ADR-0010
stands.

## Context

An `openai` backend names its tool calls, and the router hands those ids to
Claude Code unchanged (ADR-0010), which stores them in the session history as
`tool_use.id` and `tool_result.tool_use_id`. Chat Completions sets no format for
them. vLLM serving a Kimi model (`model_type` `kimi_k2`, `kimi_k25`, `kimi_k3`)
names them `functions.<name>:<n>`, where `n` counts the tool calls already in
the history.

The Anthropic API accepts only `^[a-zA-Z0-9_-]+$` for both fields and answers
anything else with a 400 `invalid_request_error`. Claude Code does not recover
from that error the way it does from a rejected thinking signature. Once a
session that used such a backend switches to an Anthropic model, every request
fails until the history is cleared, and switching models mid-session is what the
router is for. This was reproduced end to end: the request fails, and the same
recorded request with its ids rewritten succeeds.

The id could be rewritten on the way to Claude Code instead. That would also
change the ids the `openai` backend sees in its own history, and the Kimi
template renders them into the prompt as the model wrote them.

## Decision

- A request to an `anthropic` backend has every character outside
  `[A-Za-z0-9_-]` in `tool_use.id` and `tool_result.tool_use_id` replaced with
  `_`. This applies to the content blocks of `messages`, on `/v1/messages` and
  `count_tokens` alike.
- The rule is a function of the id alone. A `tool_use` and the `tool_result`
  that answers it therefore still match, within one request and across requests.
- Ids already in the accepted form are not changed. Other block types keep their
  ids: an Anthropic backend produced them.
- Responses are not touched. The client and the `openai` backend keep the ids
  the backend chose.
- A body is parsed for this rule only when it contains the text `"tool_use"`. As
  with every rewrite (ADR-0003, ADR-0009, ADR-0010), a body in which nothing
  changed is forwarded as the bytes received.

## Consequences

- A session can move from a backend with such ids to an Anthropic model without
  a failed request.
- Two ids that differ only in replaced characters (`a.b` and `a:b`) become one.
  Kimi ids cannot collide this way, because the characters replaced sit at fixed
  places and tool names cannot contain them.
- Requests to `anthropic` backends that carry tool calls are parsed even without
  a rename, dropped fields or thinking blocks. Their bytes still reach the
  backend untouched unless an id was rewritten.
- The body log records the ids as sent, rewritten; Claude Code's transcript
  keeps the originals.
