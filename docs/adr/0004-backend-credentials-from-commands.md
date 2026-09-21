# 0004. Backend credentials from external commands

## Status

accepted; how the command is run and what it may print superseded by ADR-0018; never using a cached value past its refresh window superseded by
ADR-0015 for values with a reported `expires_at`

## Context

Some backends need a rotating credential (an OAuth access token that another
program refreshes on disk or in a keychain), others a fixed API key, others
none. Claude Code's own `apiKeyHelper` mechanism cannot be used per backend: it
authenticates Claude Code to *one* endpoint, is ignored by gateway model
discovery, and is shadowed by `ANTHROPIC_AUTH_TOKEN`. Claude Code therefore
presents a single static token to the router, and the router must obtain each
backend's credential itself.

A long streaming response can outlive the credential that started it. Whether
the backend tolerates that is a backend property; the router can only make sure
each *new* request carries a fresh credential.

## Decision

- Each backend declares a `credential` of kind `none`, `static`, `env` or
  `command`, plus the header it is presented in (`Authorization: Bearer` or
  `x-api-key`).
- `command` runs `sh -c <command>` with a timeout (default 10 s). Trimmed stdout
  is the credential. Non-zero exit, empty stdout, a timeout or header-unsafe
  bytes are errors; stderr is included in the error text.
- The value is cached and re-run after `refresh` (default 5 min). The cache lock
  is held across the run, so concurrent requests trigger one process, not many.
- A failed run is not cached; the next request tries again. A cached value is
  never used past its refresh window even if the new run fails, because a stale
  token turns into confusing upstream 401s rather than an actionable error.
- The proxy layer may call `invalidate()` when the backend answers 401/403, then
  retry the request once with a freshly acquired credential. Applies only before
  any response byte has been relayed.
- `static` and `env` values are resolved at startup so a missing variable fails
  `config check`, not the first request.
- Secrets are never logged in full; `Debug` and status output show the first and
  last four characters.

## Consequences

- Any rotation scheme expressible as a shell one-liner works without router
  changes (keychain lookups, `cat` of a token file, cloud CLI token commands).
- The router depends on `sh` being present; it targets WSL2/Ubuntu and macOS,
  where that holds.
- Refresh is lazy: a credential fetched at most `refresh` ago is trusted. A
  backend that rotates faster than that produces one 401-retry per rotation.
- The command runs with the router's environment and privileges. The
  configuration file is therefore as sensitive as the credentials it can reach.
