# 0021. Headers a command computes for each attempt

## Status

accepted

## Context

Some gateways authenticate each request with a value that cannot be written
down ahead of time: a token carrying an issue time and a nonce, encrypted or
signed with a key the gateway shares with its clients, often with a hash of the
body. A replayed value is refused.

Nothing a backend's configuration offers fits. `headers` are fixed at load
apart from `{header:<name>}` (ADR-0019). A `command` credential (ADR-0004) is
cached for `refresh`, served past a failed refresh (ADR-0015), produces one
header, and runs one command at a time; a refresh of zero would make it run per
request, but serialised, still replayed on a failure, and with no way to say
which request the value is for.

Each gateway's scheme is its own: which claims, which cipher, which headers.
Building one into the router would make every new gateway a release.

## Decision

- `backends.<name>.header_command` names a command run through `sh -c` once for
  every attempt the router sends to that backend, retries included. Its
  `timeout` defaults to five seconds; zero is refused.
- Nothing about a run is kept for the next one: no cache, no value served after
  a failure, no lock. Concurrent requests run the command concurrently. A run
  is a bounded child as in ADR-0018: its own process group, killed whole on
  timeout, 64 KiB per output stream.
- The command learns about the attempt through its environment, which is the
  router's own plus:
  `ANTHROXY_BACKEND`, `ANTHROXY_METHOD`, `ANTHROXY_URL` (the full URL sent to,
  without the userinfo that travels as basic auth),
  `ANTHROXY_PATH`, `ANTHROXY_MODEL` (the upstream model, empty when the request
  names none), `ANTHROXY_REQUEST_ID` (empty for a request the router makes on
  its own), `ANTHROXY_ATTEMPT` (from 1) and `ANTHROXY_BODY_SHA256` (hex digest
  of the exact bytes sent). `header_command.env` adds variables of the
  operator's: `${NAME}` in a value is the environment at load as elsewhere, and
  `{header:<name>}` reads the client's request as in ADR-0019; a variable whose
  placeholder cannot be filled is not set. A name beginning `ANTHROXY_` is
  refused there. The command line itself keeps `${NAME}` for the shell, as a
  credential command does.
- The command computes issue times, nonces, signatures and ciphertext itself.
  The router does no cryptography beyond the body digest.
- Success is exit status 0 and a stdout holding one JSON object of at least one
  member, every value a string. Each member is a header set on that attempt,
  replacing the client's value and the backend's `headers` under that name. The
  backend credential is set after it and cannot be named: its header is
  refused, as are framing and hop-by-hop headers, `host`, `content-type` and
  `x-anthroxy-*`. With `credential = { kind = "none" }` the command may set
  `authorization`.
- Any other outcome fails the attempt before anything is sent: the client gets
  a 502 naming the backend and the cause, with the command's stderr as a
  credential failure carries it. A failed run is not retried.
- A request the router makes itself (the probe of `check` and the console's,
  a live model list) runs the command too.
- `kind = "passthrough"` takes no `header_command`, as it takes no `headers`.

## Consequences

- A gateway's signing scheme is a script the operator owns, in whatever
  language it is written; a new scheme is not a router release.
- Every attempt pays a process start before it is sent. A shell script costs
  milliseconds, an interpreter with a cold start tens of them.
- Values the command prints never reach a log, the body log or the console.
  What is reported is each run's time, duration, outcome and the header names.
- The body log's list of request headers is written before the first attempt
  and does not include these headers; the console's run history names them.
- `ring`, already in the build through rustls, becomes a direct dependency for
  the body digest.
