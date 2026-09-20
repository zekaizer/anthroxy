# anthroxy

Rust binary. A single-endpoint gateway in front of several Anthropic-API-compatible backends (vLLM among them), so Claude Code points at one URL and can switch models mid-session.

## Fixed constraints

- Language: Rust, edition 2024.
- Ingress is always Claude Code speaking the Anthropic Messages API. Other clients are out of scope.
- Runtime target: Windows 11 WSL2 (Ubuntu). Claude Code on the Windows host must reach it as well.
- A backend that does not speak the Anthropic Messages API is reached only through the IR. Everything crossing it — chat requests, response documents, event streams, model lists, error bodies, and whatever a probe or the CLI reads from it — is decoded into `ir/` by that format's codec and encoded out by the other. A shortcut that maps one wire format onto the other directly is not allowed, and `translate/` stays the only module naming both.

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
  - `config/` — TOML schema, `${ENV}` expansion, validation, the `init` example, client environment rendering, redacted view, TOML fragments.
  - `anthropic/` — wire types the router emits or inspects (errors, model list, `model` peek/rewrite) and the Messages API ↔ IR codecs.
  - `ir/` — the intermediate representation between chat APIs: request document, response events, folded message. Names no wire format.
  - `openai/` — IR ↔ OpenAI Chat Completions codecs (request, chunk, completion, error body).
  - `sse/` — incremental server-sent events parser.
  - `translate/` — composes the codecs; the only module that knows both wire formats. `Translator` adapts an upstream byte stream.
  - `routing/` — model id/alias → backend + upstream model.
  - `credential/` — `CredentialSource` trait; fixed and command-backed sources.
  - `upstream/` — backend registry, header translation, retry policy, HTTP client, probe.
  - `server/` — axum app: accepting connections, request id span, client auth, handlers (`health`, `models`, `proxy`, `openai` for `kind = "openai"` backends, `console/` for the web console's `/api/` routes with its page under `console/assets/`), relay stream, pings on quiet event streams, error mapping, cutting what a stop leaves in flight.
  - `activity/` — in-memory record of exchanges in flight and recently finished, the unmatched model-name tally, hints read from upstream error bodies.
  - `stats/` — persistent per-exchange JSONL statistics: line format, daily files, aggregation.
  - `observability/` — tracing subscriber, per-request body capture (fed by the relay stream), recording listing and deletion.
  - `service/` — systemd user unit.
  - `cli/` — clap grammar and one file per subcommand.
  - `text.rs` — escaping and cutting for anything the router did not choose that reaches a message or a log line.
  - `private_fs.rs` — owner-only directory creation, writes and appends for files that hold what a user sent or did; the count of writes still queued, which a stop waits for.
- `scripts/` — helpers that are not the binary: `console-check` drives the web console and says where to look.
- `tests/` — black-box tests: `proxy.rs`/`body_log.rs`/`openai.rs`/`activity.rs`/`console.rs`/`reload.rs` against a mock backend in `tests/support/`, `cli.rs` against the binary.
- `docs/adr/` — architecture decision records.
- `.local/` — gitignored personal notes. Never cite them from code or committed docs.

## Versioning and releases

- `0.x.y` is the development cycle: breaking changes to configuration, CLI and HTTP behaviour are allowed outright, with no deprecation paths or compatibility shims. Code stays in its final form at every commit; no leftovers, no history in comments.
- A release tag `vX.Y.Z` must equal the `Cargo.toml` version; the release workflow refuses otherwise.
- The release body is a short summary of what changed since the previous release tag (`git log <prev>..<tag>`), written by user-visible effect, not a raw commit list.

## Docs on demand

This file stays short. Details live under `docs/` and are read only when a task needs them. When adding guidance, put it in a doc and list it here with its read-when condition.

The README is for someone running the router, not for someone changing it: it stays short and sends detail here.

- `docs/adr/README.md` — read before writing or changing an ADR.
- `docs/adr/NNNN-*.md` — read the ADR covering an area before changing that area.
- `docs/configuration.md` — read before changing `config/`, the CLI's configuration handling, or anything a configuration key controls; it is the user-facing reference for every key.
- `docs/console.md` — read before changing what the console shows or what `server/console/`'s `/api/` routes answer.
- `docs/http-api.md` — read before adding, removing or changing a route, a response header or an error body.
- `docs/operating.md` — read before changing `service/`, the logs a request emits, or body capture.
- `docs/claude-code.md` — read when a question is about what Claude Code does before the router sees a request: which models reach its picker, the context window it assumes, whether MCP tools are deferred.
- `docs/console-design.md` — read before changing `server/console/assets/`: the token scales, the badge taxonomy, what colour is allowed to mean, and how a change is checked (`scripts/console-check`).
- `docs/translation.md` — read before changing `ir/`, `openai/`, `translate/` or the Anthropic codecs: how an `openai` backend is served and what the IR is for.
