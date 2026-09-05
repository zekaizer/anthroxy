# 0001. Runtime and HTTP stack

## Status

accepted

## Context

The router is a long-running proxy: it accepts Anthropic Messages API traffic from Claude Code, forwards it to one of several backends, and relays server-sent event streams back without buffering. It runs on WSL2 (Ubuntu) and must also be built and tested on macOS. A single operator runs it, so the priorities are: correct streaming, low operational friction (one static binary, no system TLS dependency), and detailed tracing for debugging.

## Decision

- **Async runtime:** `tokio` (multi-thread). It is the de-facto runtime for the crates below and provides `process` (credential commands), `fs`, timers and signal handling.
- **Ingress HTTP server:** `axum` 0.8 on `hyper` 1.x. Bodies are `http_body` streams, so an upstream response can be relayed chunk-by-chunk with `Body::from_stream` and no intermediate buffering.
- **Egress HTTP client:** `reqwest` 0.12 with `rustls-tls` and `stream`, default features disabled. Rustls removes the OpenSSL runtime dependency so the binary can be copied between machines. Automatic decompression is disabled so bodies are relayed and logged as the backend produced them.
- **Serialization:** `serde` + `serde_json` with `preserve_order`, so a request body that has to be edited (the `model` field) is re-emitted with its keys in the original order.
- **Configuration file parsing:** `toml`. See ADR-0002.
- **Command line:** `clap` (derive). It produces conventional `--help`, subcommands and environment-variable fallbacks without hand-written parsing.
- **Diagnostics:** `tracing` + `tracing-subscriber` (`env-filter`, `json`). Every request runs inside a span carrying a request id, model and backend, and the same events can be emitted as human-readable text or JSON lines.
- **Errors:** `thiserror` for typed library errors, `anyhow` only in the binary entry point.

## Consequences

- One runtime and one HTTP model (`http` 1.x types) across server, client and tests; no adapter layers.
- `axum`'s default 2 MiB body limit is far below what Claude Code sends and must be raised explicitly in the server layer.
- The binary depends on `rustls` roots bundled at build time (`webpki-roots` via reqwest); corporate TLS interception proxies would need the `rustls-tls-native-roots` feature instead.
- `hyper`/`axum` upgrades are tied to the `http` 1.x major line; a future `http` 2.x would require a coordinated bump.
