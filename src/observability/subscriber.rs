//! `tracing` subscriber configuration.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::LogFormat;

/// Picks the filter: explicit command-line value, then `RUST_LOG`, then the
/// configuration file.
pub fn resolve_directives(cli: Option<&str>, env: Option<&str>, config: &str) -> String {
    cli.or(env).unwrap_or(config).to_owned()
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
