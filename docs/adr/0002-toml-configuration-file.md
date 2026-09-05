# 0002. TOML configuration file with environment interpolation

## Status

accepted

## Context

The router needs to know its listen address, the client token, each backend's origin and credential source, and the model table. A single operator edits this by hand; there is no control plane. Credentials must not be forced into the file, because the file may be synced between machines. Claude Code itself is configured through environment variables and JSON, so operators already deal with both.

## Decision

- Configuration is one TOML file, by default `$XDG_CONFIG_HOME/claude-router/config.toml`, overridable with `--config` or `CLAUDE_ROUTER_CONFIG`.
- Every table is parsed with `deny_unknown_fields`; an unknown key is a load error, not a silent no-op.
- `${NAME}` anywhere in the file is replaced with the environment variable `NAME` before parsing; an unset variable is a load error. `$${NAME}` passes `${NAME}` through unchanged so shell commands in the file keep their own expansion.
- Durations are humantime strings (`10s`, `5m`); byte sizes accept an integer or a unit suffix (`64MiB`).
- Validation is exhaustive: all problems are reported together with their TOML path (`backends.vllm.url`, `models[2].backend`).
- Models are an ordered array (`[[models]]`); that order is the order shown in Claude Code's model picker.

## Consequences

- TOML has comments and nested tables, so a generated example file can document every option in place.
- Interpolation is textual and happens before parsing, so a `${...}` inside a quoted string is expanded like any other; the escape is the only way to emit a literal reference.
- Adding a config option requires touching the schema, the validator when cross-field rules apply, and the generated example; there is no reflection-based fallback.
- No hot reload. The model table changes only when the process restarts. Model *switching* (choosing among configured models) needs no reload.
