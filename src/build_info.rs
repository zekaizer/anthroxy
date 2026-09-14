//! Version and build provenance shown by `--version`, the serve banner and
//! `/healthz`.

/// `0.8.0 (28c942a)`, `0.8.0 (28c942a-dirty)` or `0.8.0 (unknown)` when built
/// outside a git checkout. `ANTHROXY_GIT` is set by `build.rs`.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("ANTHROXY_GIT"), ")");
