# 0002. TOML configuration file with environment interpolation

## Status

accepted; `SIGHUP` as the only reload trigger superseded by ADR-0011

## Context

The router needs to know its listen address, the client token, each backend's
origin and credential source, and the model table. A single operator edits this
by hand; there is no control plane. Credentials must not be forced into the
file, because the file may be synced between machines. Claude Code itself is
configured through environment variables and JSON, so operators already deal
with both.

## Decision

- Configuration is one TOML file, by default
  `$XDG_CONFIG_HOME/anthroxy/config.toml`, overridable with `--config` or
  `ANTHROXY_CONFIG`.
- Every table is parsed with `deny_unknown_fields`; an unknown key is a load
  error, not a silent no-op.
- `${NAME}` inside any string value is replaced with the environment variable
  `NAME` after parsing; an unset variable is a load error. Keys and comments are
  not expanded. `$${NAME}` passes `${NAME}` through unchanged so shell commands
  in the file keep their own expansion.
- Durations are humantime strings (`10s`, `5m`); byte sizes accept an integer or
  a unit suffix (`64MiB`).
- Validation is exhaustive: all problems are reported together with their TOML
  path (`backends.vllm.url`, `models[2].backend`).
- Models are an ordered array (`[[models]]`); that order is the order shown in
  Claude Code's model picker.

## Consequences

- TOML has comments and nested tables, so a generated example file can document
  every option in place.
- Interpolation runs on the parsed document, so schema errors (unknown or
  mistyped fields) are reported without line numbers; syntax errors keep them.
  Field names in the message identify the spot.
- Adding a config option requires touching the schema, the validator when
  cross-field rules apply, and the generated example; there is no
  reflection-based fallback.
- Reload is explicit, by `SIGHUP` (`anthroxy service reload` under systemd), and
  atomic: a new snapshot of models, backends, credentials, token and logging
  replaces the old one; requests already in flight finish on the snapshot they
  started with. The listen socket is not re-bound, so `server.listen` needs a
  restart. A file that fails to load or validate is rejected whole and the
  running configuration stays. Model *switching* (choosing among configured
  models) needs no reload.
