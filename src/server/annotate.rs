//! Marks an upstream error body with the backend it came from (ADR-0005).

use http::StatusCode;

use crate::anthropic::ErrorResponse;

/// If `raw` is an Anthropic error document, returns it with the message
/// prefixed by backend and status and with `request_id` filled when absent.
/// Anything else yields `None` and is relayed untouched.
pub fn annotate_upstream_error(
    raw: &[u8],
    backend: &str,
    status: StatusCode,
    request_id: &str,
) -> Option<Vec<u8>> {
    let mut error: ErrorResponse = serde_json::from_slice(raw).ok()?;
    if error.kind != "error" {
        return None;
    }
    error.error.message = format!("{}{}", backend_prefix(backend, status), error.error.message);
    error
        .request_id
        .get_or_insert_with(|| request_id.to_owned());
    serde_json::to_vec(&error).ok()
}

/// `[backend <name>, HTTP <status>] `, the prefix every relayed error
/// message starts with (ADR-0005).
pub fn backend_prefix(backend: &str, status: StatusCode) -> String {
    format!("[backend {backend}, HTTP {}] ", status.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_message_and_fills_request_id() {
        let raw = br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let out =
            annotate_upstream_error(raw, "claude", StatusCode::from_u16(529).unwrap(), "rtr_1")
                .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            value["error"]["message"],
            "[backend claude, HTTP 529] Overloaded"
        );
        assert_eq!(value["error"]["type"], "overloaded_error");
        assert_eq!(value["request_id"], "rtr_1");
    }

    #[test]
    fn keeps_upstream_request_id() {
        let raw =
            br#"{"type":"error","error":{"type":"api_error","message":"x"},"request_id":"req_up"}"#;
        let out =
            annotate_upstream_error(raw, "b", StatusCode::INTERNAL_SERVER_ERROR, "rtr_1").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["request_id"], "req_up");
    }

    #[test]
    fn leaves_other_bodies_alone() {
        assert!(
            annotate_upstream_error(b"plain text", "b", StatusCode::BAD_GATEWAY, "r").is_none()
        );
        assert!(
            annotate_upstream_error(
                br#"{"detail":"fastapi style"}"#,
                "b",
                StatusCode::BAD_GATEWAY,
                "r"
            )
            .is_none()
        );
        assert!(
            annotate_upstream_error(
                br#"{"type":"message","error":{"type":"api_error","message":"m"}}"#,
                "b",
                StatusCode::OK,
                "r"
            )
            .is_none()
        );
    }
}
