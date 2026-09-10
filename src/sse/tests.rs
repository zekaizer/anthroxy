use super::*;

fn frame(event: Option<&str>, data: &str) -> Frame {
    Frame {
        event: event.map(str::to_owned),
        data: data.to_owned(),
    }
}

#[test]
fn reassembles_a_frame_split_across_chunks() {
    let mut p = Parser::new();
    assert_eq!(p.feed(b"data: {\"a\":").unwrap(), vec![]);
    assert_eq!(p.feed(b" 1}\n").unwrap(), vec![]);
    assert_eq!(
        p.feed(b"\ndata: [DONE]\n\n").unwrap(),
        vec![frame(None, "{\"a\": 1}"), frame(None, "[DONE]"),]
    );
}

#[test]
fn accepts_crlf_and_mixed_line_endings() {
    let mut p = Parser::new();
    let frames = p
        .feed(b"event: ping\r\ndata: 1\r\n\r\ndata: 2\n\r\n")
        .unwrap();
    assert_eq!(frames, vec![frame(Some("ping"), "1"), frame(None, "2")]);
}

#[test]
fn joins_multiple_data_lines_and_skips_comments_and_ids() {
    let mut p = Parser::new();
    let frames = p
        .feed(b": keep-alive\nid: 7\nretry: 100\ndata: a\ndata:b\ndata: \n\n")
        .unwrap();
    assert_eq!(frames, vec![frame(None, "a\nb\n")]);
}

#[test]
fn skips_comment_only_frames() {
    let mut p = Parser::new();
    assert_eq!(
        p.feed(b": comment only\n\nevent: x\n\ndata: y\n\n")
            .unwrap(),
        vec![frame(Some("x"), ""), frame(None, "y")]
    );
}

#[test]
fn rejects_invalid_utf8() {
    let mut p = Parser::new();
    assert_eq!(p.feed(b"data: \xff\n\n"), Err(SseError::Utf8));
}

#[test]
fn finish_returns_a_trailing_frame_without_blank_line() {
    let mut p = Parser::new();
    assert_eq!(p.feed(b"data: tail").unwrap(), vec![]);
    assert_eq!(p.finish().unwrap(), Some(frame(None, "tail")));
    assert_eq!(p.finish().unwrap(), None);
}

#[test]
fn caps_the_pending_buffer() {
    let mut p = Parser::new();
    let big = vec![b'x'; 16 * 1024 * 1024 + 1];
    assert_eq!(p.feed(&big), Err(SseError::TooLarge(16 * 1024 * 1024)));
}

#[test]
fn an_error_discards_the_pending_bytes() {
    let mut p = Parser::new();
    assert_eq!(p.feed(b"data: \xff\n\n"), Err(SseError::Utf8));
    assert_eq!(p.feed(b"data: ok\n\n").unwrap(), vec![frame(None, "ok")]);
}
