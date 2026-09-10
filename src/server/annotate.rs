//! Marks an upstream error body with the backend it came from (ADR-0005).

use http::StatusCode;
use serde_json::Value;

/// If `raw` is an Anthropic error document, returns it with the message
/// prefixed by backend and status and with `request_id` filled when absent.
/// Anything else yields `None` and is relayed untouched.
///
/// The document is edited in place rather than read into the router's own
/// error type: ADR-0005 alters one string, so a field the backend sent — an
/// `error.type` outside the published set included — comes back as it went.
pub fn annotate_upstream_error(
    raw: &[u8],
    backend: &str,
    status: StatusCode,
    request_id: &str,
) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(raw).ok()?;
    let object = document.as_object_mut()?;
    if object.get("type").and_then(Value::as_str) != Some("error") {
        return None;
    }
    let message = object
        .get_mut("error")?
        .as_object_mut()?
        .get_mut("message")?;
    let text = message.as_str()?.to_owned();
    *message = Value::String(format!(
        "[backend {backend}, HTTP {}] {text}",
        status.as_u16()
    ));
    if object.get("request_id").is_none_or(Value::is_null) {
        object.insert(
            "request_id".to_owned(),
            Value::String(request_id.to_owned()),
        );
    }
    serde_json::to_vec(&document).ok()
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
    fn keeps_every_other_field_the_backend_sent() {
        let raw = br#"{"type":"error","error":{"type":"invalid_request_error","message":"m","param":"max_tokens"},"usage":{"input_tokens":3}}"#;
        let out = annotate_upstream_error(raw, "vllm", StatusCode::BAD_REQUEST, "rtr_1").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["error"]["message"], "[backend vllm, HTTP 400] m");
        assert_eq!(value["error"]["param"], "max_tokens");
        assert_eq!(value["usage"]["input_tokens"], 3);
    }

    #[test]
    fn tags_an_error_type_the_router_does_not_know() {
        let raw = br#"{"type":"error","error":{"type":"BadRequestError","message":"bad"}}"#;
        let out = annotate_upstream_error(raw, "vllm", StatusCode::BAD_REQUEST, "rtr_2").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["error"]["type"], "BadRequestError");
        assert_eq!(value["error"]["message"], "[backend vllm, HTTP 400] bad");
        assert_eq!(value["request_id"], "rtr_2");
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
