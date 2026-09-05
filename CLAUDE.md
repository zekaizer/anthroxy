# claude-router

Rust binary. A single-endpoint gateway in front of several Anthropic-API-compatible backends (vLLM among them), so Claude Code points at one URL and can switch models mid-session.

## Fixed constraints

- Language: Rust, edition 2024.
- Ingress is always Claude Code speaking the Anthropic Messages API. Other clients are out of scope.
- Runtime target: Windows 11 WSL2 (Ubuntu). Claude Code on the Windows host must reach it as well.

## Commands

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings   # must pass before commit
cargo fmt
```

## Layout

- `src/` — crate source. Module structure is not designed yet; do not invent one ahead of the design step.
- `docs/adr/` — architecture decision records.
- `.local/` — gitignored personal notes. Never cite them from code or committed docs.

## Docs on demand

This file stays short. Details live under `docs/` and are read only when a task needs them. When adding guidance, put it in a doc and list it here with its read-when condition.

- `docs/adr/README.md` — read before writing or changing an ADR.
- `docs/adr/NNNN-*.md` — read the ADR covering an area before changing that area.
