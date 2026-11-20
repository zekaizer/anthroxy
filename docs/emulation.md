# Client emulation presets

A backend with `kind = "openai"` receives a Chat Completions request that
anthroxy writes from what Claude Code sent. Another client of the same server
writes its requests another way: other headers, other body fields, another
order. A preset describes one such client. A backend that names a preset in
`emulate` receives requests written the way that client writes them.

A preset changes the request only. The answer is read as it is without one.
ADR-0022 records the decisions behind this page.

## What a preset reaches

A preset decides these parts of a request:

- Which headers are sent, their values, their order and the case of their
  names.
- Whether the connection is HTTP/1.1.
- Which top-level body fields are sent, their order, and the name of the
  output-token limit.
- Where a system message that is not the first message goes.
- Whether the reasoning of earlier turns is sent back, and under which name.
- The `tool_choice` of a request that has tools, the `content` of an assistant
  message that only calls tools, and `strict` on tool definitions.

A preset does not reach these, so a request stays different from the client's
there:

- The system prompt, the tool names and schemas, and the text of tool results.
  Claude Code writes them.
- The TLS handshake.
- A mix of title-case and lower-case header names. The names are all one or
  all the other.
- `Accept-Encoding` and `Connection`. The router reads answers uncompressed.

## Turn a preset on

```toml
[backends.inhouse]
kind = "openai"
url = "https://llm.example.corp"
emulate = "opencode"

[[models]]
id = "qwen"
backend = "inhouse"
emulate = { preset = "opencode", version = "1.20.0" }
```

- `emulate` is the name of a preset, or a table with `preset` and a value for
  any of that preset's variables.
- On a `[[models]]` entry, `emulate` replaces the backend's value for requests
  that entry routes.
- `emulate` on a backend of another kind is a configuration error.

A key written on the backend wins over the preset's:

| Key | When both name it |
|---|---|
| `headers` | The backend's value is sent for that header. |
| `drop_headers` | Both lists apply. |
| `set_fields` | The backend's value is set at that path. |
| `mid_conversation_system` | The backend's value, or the model's, is used. |

The header keys of a preset (`headers`, `drop_headers`, `header_order`,
`header_case`, `http_version`) apply to every request the router sends to the
backend, the model list included. The other keys apply to chat requests.

`anthroxy check` prints the preset in effect for each backend and model, with
the values of its variables.

## Where presets come from

- `opencode` is built in. [The `opencode` preset](#the-opencode-preset) shows
  its content.
- A file `presets/<name>.toml` beside the configuration file is the preset
  `<name>`. A file named like a built-in preset is a configuration error. Every
  such file is checked, whether a backend names it or not.
- Preset files are read when the configuration is, a reload included.

## The preset file

Every key is optional. Where a key is absent, the router does what it does
without a preset.

### `variables`

A table of names and default values, each a string. A value in `headers` or
`set_fields` uses a variable as `{<name>}`. `emulate` on a backend or a model
may give another value, also a string. A name is letters, digits and `_`, and
is not `preset`. In a preset value, braces around such a name always mean a
variable, so that text cannot be sent as it is. `check` and the console show a
variable's value, so a variable is no place for a secret.

### `headers`

A table of header names and values, sent on every request. A value may hold
literal text, `{header:<name>}` and `{<variable>}` in any mix.
`{header:<name>}` follows the rules of the backend key of the same name in
[Configuration](configuration.md#backends): it is the value the client sent
under that header, read before `drop_headers` applies, and a header whose
placeholder cannot be filled is not sent. The credential headers cannot be
named.

### `drop_headers`

A list of client headers the backend does not receive, written as the backend
key of the same name: a header name, a `*` pattern, or `@claude-code`.

### `header_order`

A list of lower-case header names, each named once. The listed headers are
sent in that order, and the others after them in the order the router would
send them without a preset. A name the request does not carry is skipped. The
list may name the credential header, `host` and `content-length`. A preset
that sets this key sets `http_version` too.

### `header_case`

`"lower"` (default) or `"title"`. `"title"` writes every header name with a
capital after each hyphen: `Content-Type`, `X-Opencode-Session-Id`. A preset
that sets this key sets `http_version` too.

### `http_version`

`"1.1"` keeps the router from using HTTP/2 with the backend. Absent, the
version is the one the server and the router agree on. Header order and the
case of header names exist in HTTP/1.1 only, which is why `header_order` and
`header_case` need this key.

### `omit_fields`

A list of Chat Completions body fields the backend does not receive, as
dot-separated paths. `model`, `stream` and `messages` cannot be named.

### `set_fields`

A table of Chat Completions body fields and values, written as the backend key
of the same name. A string may hold `{header:<name>}` and `{<variable>}`.
`model`, `stream` and `messages` cannot be set.

### `field_order`

A list of top-level body field names, each named once. The listed fields are
written in that order, and the others after them.

### `max_tokens_field`

`"max_tokens"` (default) or `"max_completion_tokens"`: the name the output-token
limit is sent under.

### `mid_conversation_system`

`"keep"`, `"merge"` or `"user"`, as the backend key of the same name.

### `reasoning_field`

The name of an assistant message field. The text of the `thinking` blocks of
each earlier turn is sent under it, in the order Claude Code returned them. A
turn without such a block, or whose blocks hold no text, gets no field.
Absent, earlier reasoning is not sent. The name is not `role`, `content` or
`tool_calls`.

### `default_tool_choice`

`"auto"`, `"required"` or `"none"`: the `tool_choice` of a request that has
tools and for which Claude Code named none. A request without tools gets no
default.

### `tool_only_content`

The `content` of an assistant message that calls tools and has no text:
`"omitted"` (default, no `content` key), `"empty"` (`""`) or `"null"`.

### `tool_strict`

`true` or `false`: written as `strict` on every tool definition. Absent, no
`strict` is written.

## Order on the body

1. The Messages request is decoded and the Chat Completions body is written,
   following `mid_conversation_system`, `reasoning_field`,
   `default_tool_choice`, `tool_only_content`, `tool_strict` and
   `max_tokens_field`.
2. The preset's `omit_fields` are removed.
3. The preset's `set_fields` are set.
4. The backend's `set_fields` are set.
5. The fields are put in `field_order`.

`drop_fields` on the backend still names fields of the Messages request and
runs before step 1.

## Errors

Each of these is a configuration error. `check` names the preset and the key,
`serve` does not start, and a reload keeps the configuration it had.

- A key this page does not list.
- A value outside the set its key allows.
- A placeholder for a variable the preset does not declare.
- `header_order` or `header_case` without `http_version = "1.1"`.
- `emulate` naming a preset that does not exist.
- `emulate` giving a variable the preset does not declare.
- `emulate` on a backend whose kind is not `openai`.

A preset adds no error to a request.

## The `opencode` preset

Measured from opencode 1.19.6 on 2026-11-20.

```toml
http_version = "1.1"
header_case = "title"
header_order = [
	"authorization", "content-type", "user-agent",
	"x-opencode-session-id", "x-session-affinity", "x-session-id",
	"accept", "host", "content-length",
]
drop_headers = ["@claude-code", "anthropic-client-*"]
omit_fields = ["user", "reasoning_effort"]
field_order = [
	"model", "max_tokens", "messages", "tools", "tool_choice",
	"stream", "stream_options",
]
max_tokens_field = "max_tokens"
mid_conversation_system = "merge"
reasoning_field = "reasoning_content"
default_tool_choice = "auto"
tool_only_content = "empty"

[variables]
version = "1.19.6"

[headers]
user-agent = "opencode/{version}"
accept = "*/*"
x-opencode-session-id = "{header:x-claude-code-session-id}"
x-session-affinity = "{header:x-claude-code-session-id}"
x-session-id = "{header:x-claude-code-session-id}"
```

Against opencode 1.19.6 itself, these differences remain besides the ones
[no preset reaches](#what-a-preset-reaches):

- opencode's `user-agent` continues after the version
  (`ai-sdk/provider-utils/4.0.23 runtime/bun/1.4.4`).
- opencode's three session headers hold a value of its own making (`ses_…`).
  The preset sends Claude Code's session id.
- The three session headers arrive as `X-Opencode-Session-Id`,
  `X-Session-Affinity` and `X-Session-Id`. opencode writes them in lower case.

## Write a preset from a request log

A preset for another client, or for another version of one, is written from a
log of what that client sends.

The log holds the API key and every prompt. Remove the `Authorization` value
before you give the log to anyone, a language model included.

1. Record the client's requests. A client that runs on Bun, as opencode does,
   prints each request with its headers and body when it is started with
   `BUN_CONFIG_VERBOSE_FETCH=curl`. Use the client so that the log holds a
   tool call with reasoning before it, a turn that only calls a tool, and a
   second session. Without those, `reasoning_field`, `tool_only_content` and
   the headers that change per session cannot be read from it.
2. Write `presets/<name>.toml` beside the configuration file. For each key on
   this page, read its value from the log:

   | Key | Read from the log |
   |---|---|
   | `headers` | Each header and its value. One that changes per session takes `{header:x-claude-code-session-id}`. |
   | `header_order`, `header_case` | The order and the spelling of the header names. |
   | `http_version` | The protocol on the request line. |
   | `omit_fields` | Fields anthroxy sends that the log never shows. |
   | `set_fields` | Fields the log always shows with one fixed value. |
   | `field_order` | The order of the top-level body fields. |
   | `max_tokens_field` | The name the output-token limit has. |
   | `mid_conversation_system` | Where `system` messages stand. |
   | `reasoning_field` | The extra field on a replayed assistant message. |
   | `default_tool_choice` | `tool_choice` on a request with tools. |
   | `tool_only_content` | `content` on an assistant message with `tool_calls` and no text. |
   | `tool_strict` | `strict` on a tool definition. |

3. Run `anthroxy check`. It names every key and value the preset gets wrong.
4. Name the preset in `emulate`, turn on
   [body capture](operating.md), and send one request through the router.
5. Compare the capture with the log. `request.json` is the body as the backend
   received it, field order included. `meta.json` names the headers. It shows
   a value the configuration set as `<redacted>` and records neither the
   order nor the case of the names. To see those, point a backend at a
   listener that prints what it receives, such as `nc -l 8099`.
6. Correct each key whose part of the request still differs and is in the list
   of [what a preset reaches](#what-a-preset-reaches).
