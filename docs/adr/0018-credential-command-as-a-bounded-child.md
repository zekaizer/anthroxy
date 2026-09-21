# 0018. Credential command: references resolved per run, output bounded

## Status

accepted; supersedes the `${NAME}` clause of ADR-0002 for a credential
`command` and the run rules of ADR-0004

## Context

ADR-0002 expands `${NAME}` in every string value when the file loads, and
ADR-0004 runs a credential `command` through `sh -c` with a timeout. A command
line that names a secret (`vault read -token=${VAULT_TOKEN} …`) therefore held
the secret in the loaded configuration, and the redacted view, the console,
`check`, `credential` and a debug log line all showed it. The child was read
with no output cap until the timeout, and a timeout killed the shell alone,
so a pipeline behind it lived on and was spawned again on every retry of a
failing refresh.

## Decision

- A credential `command` keeps its `${NAME}` references in the loaded
  configuration and resolves them from the router's environment each time it
  runs. Loading still verifies that every referenced variable exists, so
  `check` fails on a missing one as before. `$${NAME}` still yields a literal
  `${NAME}` for the shell. Every other string value is expanded at load as
  ADR-0002 says.
- The command line is never logged.
- A child runs in its own process group, created at spawn, and a timeout kills
  the group. The `libc` crate provides `killpg`; it is the only dependency
  added, and only for this call.
- Each of stdout and stderr is read up to 64 KiB; more is a failed run, a
  fifth failure cause beside those ADR-0004 lists. The same runner starts
  `systemd-run` for `credential --as-service`.

## Consequences

- The console and `check` show `vault read -token=${VAULT_TOKEN} …`, not the
  token.
- A variable set after the router started is picked up on the next run
  without a reload; one unset after start fails the run rather than the
  reload.
- A helper that prints more than 64 KiB, or streams, cannot be a credential
  command; the error names the limit.
- `process_group` and `killpg` are Unix-only, which the runtime target
  already is.
