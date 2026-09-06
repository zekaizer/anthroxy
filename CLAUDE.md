# anthroxy

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

- `src/lib.rs` — the library. All behaviour lives here; integration tests target this crate.
- `src/main.rs` — the `anthroxy` binary. Thin entry point: parse the command line, call the library. No logic.
- One module per responsibility, one file per concern; add a file rather than growing one:
  - `config/` — TOML schema, `${ENV}` expansion, validation, the `init` example.
  - `anthropic/` — wire types the router emits or inspects (errors, model list, `model` peek/rewrite).
  - `routing/` — model id/alias → backend + upstream model.
  - `credential/` — `CredentialSource` trait; fixed and command-backed sources.
  - `upstream/` — backend registry, header translation, retry policy, HTTP client, probe.
  - `server/` — axum app: request id span, client auth, handlers (`health`, `models`, `proxy`), relay stream, error mapping.
  - `observability/` — tracing subscriber, per-request body capture (fed by the relay stream).
  - `service/` — systemd user unit.
  - `cli/` — clap grammar and one file per subcommand.
- `tests/` — black-box tests: `proxy.rs`/`body_log.rs` against a mock backend in `tests/support/`, `cli.rs` against the binary.
- `docs/adr/` — architecture decision records.
- `.local/` — gitignored personal notes. Never cite them from code or committed docs.

## Versioning and releases

- `0.x.y` is the development cycle: breaking changes to configuration, CLI and HTTP behaviour are allowed outright, with no deprecation paths or compatibility shims. Code stays in its final form at every commit; no leftovers, no history in comments.
- A release tag `vX.Y.Z` must equal the `Cargo.toml` version; the release workflow refuses otherwise.
- The release body is a short summary of what changed since the previous release tag (`git log <prev>..<tag>`), written by user-visible effect, not a raw commit list.

## Docs on demand

This file stays short. Details live under `docs/` and are read only when a task needs them. When adding guidance, put it in a doc and list it here with its read-when condition.

- `docs/adr/README.md` — read before writing or changing an ADR.
- `docs/adr/NNNN-*.md` — read the ADR covering an area before changing that area.
