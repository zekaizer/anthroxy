# 0007. CI and release pipeline on GitHub Actions

## Status

accepted

## Context

The code is developed on macOS and deployed to WSL2 (Ubuntu, x86_64). Every commit must keep `cargo fmt`, `cargo clippy -D warnings` and `cargo test` green on both platforms, and the operator needs a binary they can copy into WSL2 without installing a Rust toolchain or matching a glibc version. The repository is public on GitHub, so its hosted runners are the obvious executor.

## Decision

- **CI** (`.github/workflows/ci.yml`): on every push to `main` and every pull request, a matrix of `ubuntu-latest` and `macos-latest` runs format check, clippy with warnings denied, and the full test suite, all with `--locked` so the committed `Cargo.lock` is authoritative. Toolchain is `stable` via `dtolnay/rust-toolchain`; build artifacts are cached with `Swatinem/rust-cache`.
- **Release** (`.github/workflows/release.yml`): triggered when a GitHub release is *published*. It refuses to build when the tag (`vX.Y.Z`) does not equal the `Cargo.toml` version, builds `x86_64-unknown-linux-musl` in release profile, verifies with `file` and `ldd` that the executable is statically linked and that `--version` runs, then uploads `anthroxy-x86_64-unknown-linux-musl.tar.gz` and its `.sha256` to the release.
- Static linking needs no code change: TLS is `rustls` (ADR-0001), so the musl target has no C library dependency beyond musl itself.
- Asset names carry no version. The release is the versioned object; `--version` inside the binary reports the version and the exact commit (via `build.rs`).
- `[profile.release] strip = true` keeps the shipped binary small; debugging uses the tracing output, not symbols.

## Consequences

- One tag push plus one click (or `gh release create`) produces the deployable binary; `curl …/releases/latest/download/anthroxy-x86_64-unknown-linux-musl.tar.gz` always fetches the newest one.
- Only x86_64 Linux is shipped. macOS users build from source; an aarch64 Linux asset would be one more matrix entry plus a cross linker, and is deliberately left out until needed.
- A release whose tag disagrees with `Cargo.toml` fails loudly instead of shipping a binary that reports the wrong version.
- The pipeline depends on hosted-runner availability and on the pinned major versions of three third-party actions; major bumps are routine dependency maintenance.
