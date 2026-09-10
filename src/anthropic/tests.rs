use super::*;
use http::StatusCode;

#[test]
fn error_response_serializes_to_anthropic_shape() {
    let e = ErrorResponse::new(ErrorType::NotFoundError, "no such model").with_request_id("req_1");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "type": "error",
            "error": {"type": "not_found_error", "message": "no such model"},
            "request_id": "req_1"
        })
    );
    assert_eq!(e.status(), StatusCode::NOT_FOUND);
    assert_eq!(ErrorType::OverloadedError.status().as_u16(), 529);
    assert_eq!(
        ErrorType::AuthenticationError.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn error_response_parses_upstream_body() {
    let body = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
    let e: ErrorResponse = serde_json::from_str(body).unwrap();
    assert_eq!(e.error.kind, ErrorType::OverloadedError);
    assert_eq!(e.request_id, None);
}

#[test]
fn model_list_fills_first_and_last() {
    let list = ModelList::all(vec![
        ModelObject::new("a", "A", "2026-01-01T00:00:00Z"),
        ModelObject::new("b", "B", "2026-01-01T00:00:00Z"),
    ]);
    assert_eq!(list.first_id.as_deref(), Some("a"));
    assert_eq!(list.last_id.as_deref(), Some("b"));
    assert!(!list.has_more);
    let json = serde_json::to_value(&list).unwrap();
    assert_eq!(json["data"][0]["type"], "model");
    assert_eq!(json["data"][1]["display_name"], "B");

    let empty = ModelList::all(vec![]);
    assert_eq!(empty.first_id, None);
}

#[test]
fn peek_reads_model_and_stream_only() {
    let body = br#"{"model":"m1","max_tokens":5,"stream":true,"messages":[],"unknown":{"x":1}}"#;
    let p = peek(body).unwrap();
    assert_eq!(p.model.as_deref(), Some("m1"));
    assert!(p.stream);

    let p = peek(br#"{"model":"m2","messages":[]}"#).unwrap();
    assert!(!p.stream);
}

#[test]
fn peek_rejects_missing_model_and_non_json() {
    assert!(matches!(
        peek(br#"{"messages":[]}"#),
        Err(PeekError::NoModel)
    ));
    assert!(matches!(peek(b"not json"), Err(PeekError::NotJson(_))));
    assert!(matches!(peek(b"[1,2]"), Err(PeekError::NotJson(_))));
}

#[test]
fn rewrite_of_model_preserves_everything_else_in_order() {
    let body = br#"{"model":"exposed","max_tokens":1024,"system":[{"type":"text","text":"hi","cache_control":{"type":"ephemeral"}}],"messages":[{"role":"user","content":"x"}],"metadata":{"user_id":"u"},"temperature":1.0,"big":12345678901234567890}"#;
    let out = rewrite(body, Some("upstream-name"), &[]).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(
        text,
        r#"{"model":"upstream-name","max_tokens":1024,"system":[{"type":"text","text":"hi","cache_control":{"type":"ephemeral"}}],"messages":[{"role":"user","content":"x"}],"metadata":{"user_id":"u"},"temperature":1.0,"big":12345678901234567890}"#
    );
}

#[test]
fn rewrite_drops_paths_and_keeps_order() {
    let body = br#"{"model":"m","context_management":{"edits":[]},"metadata":{"user_id":"u","keep":1},"messages":[]}"#;
    let drop = [
        "context_management".to_owned(),
        "metadata.user_id".to_owned(),
    ];
    let out = rewrite(body, None, &drop).expect("something was dropped");
    assert_eq!(
        std::str::from_utf8(&out).unwrap(),
        r#"{"model":"m","metadata":{"keep":1},"messages":[]}"#
    );

    let out = rewrite(body, Some("up"), &drop).unwrap();
    assert_eq!(
        std::str::from_utf8(&out).unwrap(),
        r#"{"model":"up","metadata":{"keep":1},"messages":[]}"#
    );
}

#[test]
fn rewrite_is_a_no_op_when_nothing_matches() {
    let body = br#"{"model":"m","messages":[{"a":1}],"metadata":{"k":1}}"#;
    let drop = [
        "absent".to_owned(),
        "metadata.absent".to_owned(),
        "messages.a".to_owned(),
        "model.x".to_owned(),
    ];
    assert_eq!(rewrite(body, None, &drop), None);
    assert_eq!(rewrite(body, None, &[]), None);
    let out = rewrite(body, Some("up"), &drop).unwrap();
    assert_eq!(
        std::str::from_utf8(&out).unwrap(),
        r#"{"model":"up","messages":[{"a":1}],"metadata":{"k":1}}"#
    );
}

#[test]
fn error_type_from_status_follows_the_api_table() {
    use http::StatusCode;
    for (status, expected) in [
        (400, ErrorType::InvalidRequestError),
        (401, ErrorType::AuthenticationError),
        (403, ErrorType::PermissionError),
        (404, ErrorType::NotFoundError),
        (413, ErrorType::RequestTooLarge),
        (429, ErrorType::RateLimitError),
        (422, ErrorType::ApiError),
        (500, ErrorType::ApiError),
        (503, ErrorType::ApiError),
        (529, ErrorType::ApiError),
    ] {
        assert_eq!(
            ErrorType::from_status(StatusCode::from_u16(status).unwrap()),
            expected,
            "{status}"
        );
    }
}
