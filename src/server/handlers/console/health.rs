//! Whether anything about the running router needs looking at, for the one
//! line of the console that is on screen whichever tab is open.
//!
//! Only two things raise it, and both are things a reader would otherwise
//! have to go and find: a backend whose credential the router does not hold,
//! and a request that failed a moment ago.

use serde_json::{Value, json};

use crate::activity::{Activity, Outcome};
use crate::config::CredentialConfig;
use crate::server::Snapshot;

/// How recently a request has to have failed to still be worth raising.
const RECENT: jiff::SignedDuration = jiff::SignedDuration::from_secs(5 * 60);

/// `attention` is something to look at, `trouble` something that is already
/// stopping requests.
const OK: &str = "ok";
const ATTENTION: &str = "attention";
const TROUBLE: &str = "trouble";

pub fn report(snapshot: &Snapshot, activity: &Activity, now: jiff::Timestamp) -> Value {
    let mut problems: Vec<Value> = Vec::new();
    problems.extend(credentials(snapshot));
    problems.extend(recent_failures(activity, now));
    let level = if problems.iter().any(|p| p["level"] == TROUBLE) {
        TROUBLE
    } else if problems.is_empty() {
        OK
    } else {
        ATTENTION
    };
    json!({"level": level, "problems": problems})
}

/// A credential the router cannot produce on demand. Only a command source
/// can fail after startup; a static or environment one is resolved when the
/// configuration is built, and `none` has nothing to hold.
fn credentials(snapshot: &Snapshot) -> Vec<Value> {
    snapshot
        .registry
        .backends()
        .filter(|backend| {
            matches!(
                snapshot
                    .config
                    .backends
                    .get(&backend.name)
                    .map(|config| &config.credential),
                Some(CredentialConfig::Command { .. })
            )
        })
        .filter_map(|backend| {
            let status = backend.credential.status();
            // A value in hand answers the question, whatever earlier runs did.
            if status.masked.is_some() {
                return None;
            }
            match status.refreshes.last() {
                Some(run) if run.error.is_some() => Some(json!({
                    "kind": "credential",
                    "level": TROUBLE,
                    "backend": backend.name,
                    "summary": format!("{}: the credential command failed, so its requests cannot be sent", backend.name),
                    "detail": run.error,
                })),
                // Ran and produced a value that has since expired: the next
                // request runs it again, which is the arrangement working.
                Some(_) => None,
                None => Some(json!({
                    "kind": "credential",
                    "level": ATTENTION,
                    "backend": backend.name,
                    "summary": format!("{}: the credential command has not run yet, so nothing has been proved about it", backend.name),
                    "detail": Value::Null,
                })),
            }
        })
        .collect()
}

fn recent_failures(activity: &Activity, now: jiff::Timestamp) -> Option<Value> {
    let since = now.checked_sub(RECENT).unwrap_or(now);
    let failed: Vec<_> = activity
        .recent()
        .into_iter()
        .filter(|view| view.received_at >= since && view.outcome == Some(Outcome::Error))
        .collect();
    if failed.is_empty() {
        return None;
    }
    let mut models: Vec<&str> = failed
        .iter()
        .filter_map(|view| view.model.as_deref().or(view.requested_model.as_deref()))
        .collect();
    models.sort_unstable();
    models.dedup();
    Some(json!({
        "kind": "errors",
        "level": ATTENTION,
        "count": failed.len(),
        "summary": format!(
            "{} request(s) failed in the last 5 minutes{}",
            failed.len(),
            if models.is_empty() { String::new() } else { format!(", on {}", models.join(", ")) }
        ),
        "detail": failed.iter().find_map(|view| view.error.clone()),
    }))
}
