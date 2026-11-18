//! A backend's `header_command`: run for every attempt, its stdout the
//! headers set on that attempt (ADR-0021).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use http::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST};
use http::{HeaderMap, HeaderName, HeaderValue, Method};

use super::headers::is_hop_by_hop;
use crate::config::{HeaderCommandConfig, Template};
use crate::process::{self, RunError};
use crate::text::short;

/// Runs kept for status output.
pub const RUN_HISTORY: usize = 20;

#[derive(Debug)]
pub struct HeaderCommand {
    command: String,
    timeout: Duration,
    env: Vec<(String, Template)>,
    /// Header the backend credential travels in, which the command may not set.
    credential: Option<HeaderName>,
    runs: std::sync::Mutex<VecDeque<HeaderRun>>,
}

/// The attempt a run is for, as its environment tells the command.
pub struct Attempt<'a> {
    pub backend: &'a str,
    pub method: &'a Method,
    pub url: &'a str,
    pub path: &'a str,
    pub model: Option<&'a str>,
    pub request_id: Option<&'a str>,
    /// From 1, counting every send of the request.
    pub number: u32,
    /// Hex SHA-256 of the body sent.
    pub body_sha256: &'a str,
    /// The client's headers as they arrived; `None` for a request the router
    /// makes on its own.
    pub client: Option<&'a HeaderMap>,
}

/// Why a run produced no headers. Never quotes a value the command printed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HeaderCommandError {
    #[error("cannot start: {0}")]
    Spawn(Arc<std::io::Error>),
    #[error("did not finish within {}", humantime::format_duration(*.0))]
    Timeout(Duration),
    #[error("printed more than {0} bytes")]
    OutputTooLarge(usize),
    #[error("{status}{}", crate::credential::stderr_suffix(.stderr))]
    Failed { status: String, stderr: String },
    #[error("stdout is not JSON: {0}")]
    NotJson(String),
    #[error("stdout is not a JSON object")]
    NotAnObject,
    #[error("stdout names no header")]
    Empty,
    #[error("`{0}` is not a string")]
    NotAString(String),
    #[error("`{0}` is not a valid header name")]
    Name(String),
    #[error("`{0}` has a value that cannot be sent in a header")]
    Value(String),
    #[error("`{0}` {1}")]
    Reserved(String, &'static str),
}

/// One run, for status output.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HeaderRun {
    pub at: jiff::Timestamp,
    pub duration_ms: u64,
    /// Names of the headers it set.
    pub headers: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HeaderCommandStatus {
    pub source: String,
    /// Recent runs, oldest first.
    pub runs: Vec<HeaderRun>,
}

impl HeaderCommand {
    /// Assumes `config` passed validation.
    pub fn new(config: &HeaderCommandConfig, credential: Option<HeaderName>) -> Self {
        Self {
            command: config.command.clone(),
            timeout: config.timeout,
            env: config
                .env
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        Template::parse(value).expect("validated env value"),
                    )
                })
                .collect(),
            credential,
            runs: std::sync::Mutex::new(VecDeque::new()),
        }
    }

    /// The headers to set on `attempt`, from a run of its own. The values are
    /// marked sensitive and never logged.
    pub async fn headers(
        &self,
        attempt: &Attempt<'_>,
    ) -> Result<Vec<(HeaderName, HeaderValue)>, HeaderCommandError> {
        let at = jiff::Timestamp::now();
        let started = Instant::now();
        let outcome = self.run(attempt).await;
        let elapsed = started.elapsed();
        match &outcome {
            Ok(headers) => tracing::debug!(
                attempt = attempt.number,
                duration_ms = elapsed.as_millis() as u64,
                headers = %names(headers).join(","),
                "header command ran"
            ),
            Err(error) => tracing::warn!(
                attempt = attempt.number,
                duration_ms = elapsed.as_millis() as u64,
                %error,
                "header command failed"
            ),
        }
        let mut runs = self
            .runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if runs.len() == RUN_HISTORY {
            runs.pop_front();
        }
        runs.push_back(HeaderRun {
            at,
            duration_ms: elapsed.as_millis() as u64,
            headers: outcome
                .as_ref()
                .map(|headers| names(headers))
                .unwrap_or_default(),
            error: outcome.as_ref().err().map(ToString::to_string),
        });
        outcome
    }

    async fn run(
        &self,
        attempt: &Attempt<'_>,
    ) -> Result<Vec<(HeaderName, HeaderValue)>, HeaderCommandError> {
        let mut env = vec![
            ("ANTHROXY_BACKEND".to_owned(), attempt.backend.to_owned()),
            ("ANTHROXY_METHOD".to_owned(), attempt.method.to_string()),
            ("ANTHROXY_URL".to_owned(), attempt.url.to_owned()),
            ("ANTHROXY_PATH".to_owned(), attempt.path.to_owned()),
            (
                "ANTHROXY_MODEL".to_owned(),
                attempt.model.unwrap_or_default().to_owned(),
            ),
            (
                "ANTHROXY_REQUEST_ID".to_owned(),
                attempt.request_id.unwrap_or_default().to_owned(),
            ),
            ("ANTHROXY_ATTEMPT".to_owned(), attempt.number.to_string()),
            (
                "ANTHROXY_BODY_SHA256".to_owned(),
                attempt.body_sha256.to_owned(),
            ),
        ];
        let no_client = HeaderMap::new();
        let client = attempt.client.unwrap_or(&no_client);
        env.extend(
            self.env
                .iter()
                .filter_map(|(name, template)| Some((name.clone(), template.render(client)?))),
        );
        let output = process::run("sh", &["-c", &self.command], &env, self.timeout)
            .await
            .map_err(|error| match error {
                RunError::Spawn(error) | RunError::Read(error) => {
                    HeaderCommandError::Spawn(Arc::new(error))
                }
                RunError::Timeout(timeout) => HeaderCommandError::Timeout(timeout),
                RunError::OutputTooLarge(limit) => HeaderCommandError::OutputTooLarge(limit),
            })?;
        if !output.status.success() {
            return Err(HeaderCommandError::Failed {
                status: match output.status.code() {
                    Some(code) => format!("exit {code}"),
                    None => output.status.to_string(),
                },
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        parse_output(&output.stdout, self.credential.as_ref())
    }

    pub fn status(&self) -> HeaderCommandStatus {
        HeaderCommandStatus {
            source: self.describe(),
            runs: self
                .runs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .cloned()
                .collect(),
        }
    }

    /// Where the headers come from, for status output.
    pub fn describe(&self) -> String {
        format!(
            "command `{}` (timeout {})",
            self.command,
            humantime::format_duration(self.timeout)
        )
    }
}

fn names(headers: &[(HeaderName, HeaderValue)]) -> Vec<String> {
    headers.iter().map(|(name, _)| name.to_string()).collect()
}

/// The headers in what a successful run printed: one JSON object whose
/// members are header names and string values. Parsed as untyped JSON first,
/// since a typed error would quote the value it met.
pub fn parse_output(
    stdout: &[u8],
    credential: Option<&HeaderName>,
) -> Result<Vec<(HeaderName, HeaderValue)>, HeaderCommandError> {
    let document: serde_json::Value = serde_json::from_slice(stdout)
        .map_err(|error| HeaderCommandError::NotJson(error.to_string()))?;
    let serde_json::Value::Object(members) = document else {
        return Err(HeaderCommandError::NotAnObject);
    };
    if members.is_empty() {
        return Err(HeaderCommandError::Empty);
    }
    members
        .into_iter()
        .map(|(written, value)| {
            let shown = short(&written);
            let name = HeaderName::from_bytes(written.as_bytes())
                .map_err(|_| HeaderCommandError::Name(shown.clone()))?;
            if let Some(why) = reserved(&name, credential) {
                return Err(HeaderCommandError::Reserved(name.to_string(), why));
            }
            let serde_json::Value::String(text) = value else {
                return Err(HeaderCommandError::NotAString(name.to_string()));
            };
            let mut value = HeaderValue::from_str(&text)
                .map_err(|_| HeaderCommandError::Value(name.to_string()))?;
            value.set_sensitive(true);
            Ok((name, value))
        })
        .collect()
}

/// Why the command may not set `name`, if it may not.
fn reserved(name: &HeaderName, credential: Option<&HeaderName>) -> Option<&'static str> {
    if credential == Some(name) {
        Some("is the backend credential's header")
    } else if is_hop_by_hop(name) || *name == CONTENT_LENGTH || *name == HOST {
        Some("describes the connection the router makes")
    } else if *name == CONTENT_TYPE {
        Some("describes the body the router sends")
    } else if name.as_str().starts_with("x-anthroxy-") {
        Some("is the router's own")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(stdout: &str) -> Result<Vec<String>, String> {
        parse_output(stdout.as_bytes(), Some(&http::header::AUTHORIZATION))
            .map(|headers| headers.iter().map(|(name, _)| name.to_string()).collect())
            .map_err(|error| error.to_string())
    }

    #[test]
    fn an_object_of_strings_is_the_headers_lowercased_and_sensitive() {
        let headers = parse_output(br#"{"X-Gw-Iat":"1","x-gw-token":"abc"}"#, None).unwrap();
        assert_eq!(
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
                .collect::<Vec<_>>(),
            [("x-gw-iat", "1"), ("x-gw-token", "abc")]
        );
        assert!(headers.iter().all(|(_, value)| value.is_sensitive()));
    }

    #[test]
    fn anything_but_a_non_empty_object_of_strings_is_refused() {
        assert!(names("x-a: 1").unwrap_err().contains("not JSON"));
        assert_eq!(
            names(r#"["a"]"#).unwrap_err(),
            "stdout is not a JSON object"
        );
        assert_eq!(names("{}").unwrap_err(), "stdout names no header");
        assert_eq!(names(r#"{"x-n": 5}"#).unwrap_err(), "`x-n` is not a string");
        assert_eq!(
            names(r#"{"bad name":"v"}"#).unwrap_err(),
            "`bad name` is not a valid header name"
        );
    }

    #[test]
    fn a_problem_with_a_value_quotes_none_of_it() {
        let error = names(r#"{"x-token":"sk-secret\nmore"}"#).unwrap_err();
        assert_eq!(
            error,
            "`x-token` has a value that cannot be sent in a header"
        );
        assert!(!error.contains("sk-secret"));
    }

    #[test]
    fn headers_the_router_or_the_credential_owns_are_refused() {
        for name in [
            "authorization",
            "content-length",
            "content-type",
            "host",
            "connection",
            "transfer-encoding",
            "proxy-authorization",
            "x-anthroxy-backend",
        ] {
            let error = names(&format!(r#"{{"{name}":"v"}}"#)).unwrap_err();
            assert!(error.starts_with(&format!("`{name}` ")), "{name}: {error}");
        }
    }

    #[test]
    fn without_a_credential_the_command_may_set_authorization() {
        let headers = parse_output(br#"{"authorization":"Bearer x"}"#, None).unwrap();
        assert_eq!(headers[0].0, http::header::AUTHORIZATION);
    }
}
