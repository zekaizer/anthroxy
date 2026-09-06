//! `tracing` subscriber configuration.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::LogFormat;

/// Picks the filter: explicit command-line value, then `RUST_LOG`, then the
/// configuration file. A bare `debug` or `trace` applies to the router only;
/// dependencies stay at `info` unless named explicitly (`h2=debug`).
pub fn resolve_directives(cli: Option<&str>, env: Option<&str>, config: &str) -> String {
    let chosen = cli.or(env).unwrap_or(config).trim();
    match chosen.to_ascii_lowercase().as_str() {
        "debug" | "trace" => format!("anthroxy={chosen},info"),
        _ => chosen.to_owned(),
    }
}

/// Installs the global subscriber writing to stderr. Returns an error when
/// `directives` is not a valid filter or a subscriber is already set.
pub fn init(
    directives: &str,
    format: LogFormat,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let filter = EnvFilter::try_new(directives)?;
    let registry = tracing_subscriber::registry().with(filter);
    match format {
        LogFormat::Text => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_ansi(std::io::stderr().is_terminal())
                    .with_target(false),
            )
            .try_init()?,
        LogFormat::Json => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_current_span(true)
                    .with_span_list(false)
                    .with_writer(std::io::stderr),
            )
            .try_init()?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::resolve_directives;

    #[test]
    fn precedence_is_cli_then_env_then_file() {
        assert_eq!(
            resolve_directives(Some("warn"), Some("info"), "error"),
            "warn"
        );
        assert_eq!(resolve_directives(None, Some("info"), "error"), "info");
        assert_eq!(resolve_directives(None, None, "error"), "error");
    }

    #[test]
    fn verbose_bare_levels_scope_to_the_router() {
        assert_eq!(
            resolve_directives(None, None, "debug"),
            "anthroxy=debug,info"
        );
        assert_eq!(
            resolve_directives(Some("trace"), None, "info"),
            "anthroxy=trace,info"
        );
        assert_eq!(
            resolve_directives(None, None, "h2=debug,info"),
            "h2=debug,info"
        );
    }
}
