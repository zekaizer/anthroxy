# 0019. Values a backend is sent that come from the request

## Status

accepted

Extends ADR-0009: the body of a request to a backend may now gain fields as
well as lose them.

## Context

Everything a backend's configuration added to a request was a header, fixed
when the file loaded: `headers` set a constant, and `${NAME}` is resolved once.
Gateways ask for more than that in two ways.

Some want a value that differs per request and that the client already sends
under another name. The usual case is a session identifier: Claude Code sends
it as `x-claude-code-session-id`, and a gateway built around another client
reads it under a name of its own, sometimes with a prefix.

Some want it in the body, not in a header: a gateway that refuses a request
whose JSON lacks its own bookkeeping object, or a model server that takes a
switch such as `chat_template_kwargs.enable_thinking` as a body field no client
sends. ADR-0009 lets a backend's configuration remove body fields; nothing let
it add one.

Teaching the router each gateway's names would put a list of vendors in the
code, and every new gateway would need a release.

## Decision

Placeholders:

- A string the configuration sends to a backend may contain `{header:<name>}`.
  It is replaced, per request, with the value the client sent under that
  header. Literal text around it and several placeholders in one value are
  allowed, so a prefix or a combination needs no further syntax. Nothing else
  is computed: no slicing, replacing, hashing or case change.
- The placeholder reads the client's request as it arrived, before
  `drop_headers` and before `headers` itself, so a header can be dropped under
  its own name and sent under another.
- A value whose placeholder names a header the client did not send, or sent
  empty or as bytes that are not UTF-8, is not sent at all; a value filled in
  part would be wrong in a way the backend cannot tell.
- `{header:authorization}`, `{header:x-api-key}` and
  `{header:proxy-authorization}` are configuration errors. They carry the
  client's credential; with `server.v1_auth = "token"` that is the router's
  token, which no backend may see.
- A placeholder that is not closed, or whose name is not a header name, is a
  configuration error, so a typo is not sent as literal text.
- `${NAME}` keeps its meaning and its time: the environment, at load. The two
  forms do not overlap, and one value may carry both.

Headers:

- A value in `backends.<name>.headers` takes placeholders. When it cannot be
  filled, the header still belongs to the backend: the client's own value under
  that name does not pass either. A header the backend cannot read the request
  without (`content-type`) therefore takes none.
- A problem with a header value is reported without quoting any of the value,
  which may be a key.

Body fields:

- `backends.<name>.set_fields` maps a path, in ADR-0009's dot-separated form,
  to a value set in the body that backend receives. It applies to every body
  the router forwards to it, after `drop_fields`, and replaces what is at the
  path. An object missing on the way is created; a step that is not an object
  is replaced by one.
- A table is the paths of its entries. TOML reads an unquoted `a.b = 1` as a
  table inside `a`, so that, `a = { b = 1 }` and `"a.b" = 1` set the same one
  field; were a table a value, the first would replace all of `a` and the last
  would not. An empty table, and a table inside an array, are values.
- A value is whatever else JSON can hold, written in TOML. A string in it, at
  any depth, takes placeholders; when one cannot be filled, that field is left
  out whole and the other fields are still set.
- The body is the one the backend reads. For a `kind = "openai"` backend that
  is the Chat Completions document, written from the IR and then given these
  fields, so its paths name Chat Completions fields; `drop_fields` still names
  Messages fields there, because it runs before translation. The fields are
  the operator's, not a mapping between the two formats, and pass through
  neither codec.
- `model` and `stream` cannot be set: the router routes on one and reads the
  answer by the other. Two paths where one lies inside the other are a
  configuration error, since whichever is set last would undo the other.
- `kind = "passthrough"` takes no `set_fields`, as it takes no `drop_fields`.

## Consequences

- A gateway's naming is a configuration change; no header or field name is
  known to the code beyond the ones that are refused.
- The literal text `{header:…}` cannot be sent. There is no escape for it:
  `{{` is common in the chat templates and JSON a value may hold.
- A request the router originates has no client behind it (the probe of
  `check` and the console), so a header whose value has a placeholder is absent
  from it. A gateway that refuses such a request without that header fails the
  probe while `serve` works. The console's test request is the exception for
  one header: it carries an `x-claude-code-session-id` of its own, since it
  stands in for a Claude Code request and that is the header a gateway reads.
- A forced header value is still shown as a secret wherever headers are
  reported, whether or not it came from a placeholder. A body field is not a
  secret: the body log records the request as sent, so the console shows the
  configured value too.
- A backend with `set_fields` pays a JSON parse per request when it speaks the
  Messages API, as with `drop_fields`; a translated body is written once either
  way.
- A field the gateway requires and whose header the client did not send is
  missing, and the gateway's own refusal is what the client sees.
- `set_fields` has no notion of the route: `count_tokens` gets the same fields
  as `/v1/messages`. A backend that wants a field on one and refuses it on the
  other cannot be served by it.
- A live model list (ADR-0017) is pulled with the headers of the request that
  missed. One pulled without a forced header is kept only for requests that
  lack it too, so a refusal earned by one request is not served to another
  that the backend would have answered.
