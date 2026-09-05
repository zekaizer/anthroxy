use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("environment variable `{0}` referenced in configuration is not set")]
    MissingEnv(String),
    #[error("configuration syntax error:\n{0}")]
    Parse(String),
    #[error("configuration is invalid:\n{}", .0.iter().map(|p| format!("  - {p}")).collect::<Vec<_>>().join("\n"))]
    Invalid(Vec<String>),
}
