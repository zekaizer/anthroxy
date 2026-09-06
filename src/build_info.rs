//! Version and build provenance shown by `--version`, the serve banner and
//! `/healthz`.

/// `0.1.0 (28c942a)`, `0.1.0 (28c942a-dirty)` or `0.1.0 (unknown)` when built
/// outside a git checkout. `CLAUDE_ROUTER_GIT` is set by `build.rs`.
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CLAUDE_ROUTER_GIT"),
    ")"
);
