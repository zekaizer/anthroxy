# 0019. Values a backend is sent that come from the request

## Status

accepted

## Context

Everything a backend's configuration adds to a request was fixed when the file
loaded: `headers` set a constant, and `${NAME}` is resolved once. Some gateways
want a value that differs per request and that the client already sends under
another name. The case that prompted this is a session identifier: Claude Code
sends it as `x-claude-code-session-id`, and a gateway built around another
client reads it from a header of its own naming, sometimes with a prefix.

Teaching the router each gateway's names would put a list of vendors in the
code, and every new gateway would need a release.

## Decision

- A value in `backends.<name>.headers` may contain `{header:<name>}`. It is
  replaced, per request, with the value the client sent under that header.
  Literal text around it and several placeholders in one value are allowed, so a
  prefix or a combination needs no further syntax. Nothing else is computed:
  no slicing, replacing, hashing or case change.
- The placeholder reads the client's request as it arrived, before
  `drop_headers` and before `headers` itself, so a header can be dropped under
  its own name and sent under another.
- A value whose placeholder names a header the client did not send, or sent
  empty, is not sent at all; a value filled in part would be wrong in a way the
  backend cannot tell. The header still belongs to the backend: the client's
  own value under that name does not pass either.
- `{header:authorization}`, `{header:x-api-key}` and
  `{header:proxy-authorization}` are configuration errors. They carry the
  client's credential; with `server.v1_auth = "token"` that is the router's
  token, which no backend may see.
- A placeholder that is not closed, or whose name is not a header name, is a
  configuration error, so a typo is not sent as literal text.
- `${NAME}` keeps its meaning and its time: the environment, at load. The two
  forms do not overlap, and one value may carry both.

## Consequences

- A gateway's naming is a configuration change; no header name is known to the
  code beyond the three that are refused.
- The literal text `{header:…}` cannot be sent in a header value. There is no
  escape for it: `{{` is common in the chat templates and JSON a value may
  hold.
- A request the router originates has no client behind it (the probe of
  `check` and the console), so a value with a placeholder is absent from it. A
  gateway that refuses such a request without that header fails the probe
  while `serve` works.
- A forced header value is still shown as a secret wherever headers are
  reported, whether or not it came from a placeholder.
