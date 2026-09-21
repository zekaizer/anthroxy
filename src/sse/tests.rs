use super::*;

fn frame(data: &str) -> Frame {
    Frame {
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
        vec![frame("{\"a\": 1}"), frame("[DONE]")]
    );
}

#[test]
fn accepts_crlf_and_mixed_line_endings() {
    let mut p = Parser::new();
    let frames = p
        .feed(b"event: ping\r\ndata: 1\r\n\r\ndata: 2\n\r\n")
        .unwrap();
    assert_eq!(frames, vec![frame("1"), frame("2")]);
}

#[test]
fn joins_multiple_data_lines_and_skips_comments_and_ids() {
    let mut p = Parser::new();
    let frames = p
        .feed(b": keep-alive\nid: 7\nretry: 100\ndata: a\ndata:b\ndata: \n\n")
        .unwrap();
    assert_eq!(frames, vec![frame("a\nb\n")]);
}

#[test]
fn frames_without_data_are_not_reported() {
    let mut p = Parser::new();
    assert_eq!(
        p.feed(b": comment only\n\nevent: ping\n\ndata: y\n\n")
            .unwrap(),
        vec![frame("y")]
    );
    assert_eq!(p.feed(b"event: x\ndata: \n\n").unwrap(), vec![]);
    assert_eq!(p.feed(b"event: x\n").unwrap(), vec![]);
    assert_eq!(p.finish().unwrap(), vec![]);
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
    assert_eq!(p.finish().unwrap(), vec![frame("tail")]);
    assert_eq!(p.finish().unwrap(), vec![]);
}

#[test]
fn caps_the_pending_buffer() {
    let mut p = Parser::new();
    let big = vec![b'x'; MAX_FRAME_BYTES + 1];
    assert_eq!(p.feed(&big), Err(SseError::TooLarge));
    assert_eq!(p.feed(b"data: after\n\n").unwrap(), vec![frame("after")]);
}

#[test]
fn a_long_line_arriving_in_pieces_is_scanned_once() {
    let mut p = Parser::new();
    let piece = vec![b'x'; 4 * 1024];
    let started = std::time::Instant::now();
    for _ in 0..1024 {
        assert_eq!(p.feed(&piece).unwrap(), vec![]);
    }
    assert!(started.elapsed().as_secs() < 2, "{:?}", started.elapsed());
    assert_eq!(p.feed(b"\ndata: ok\n\n").unwrap(), vec![frame("ok")]);
}

#[test]
fn an_error_discards_the_pending_bytes() {
    let mut p = Parser::new();
    assert_eq!(p.feed(b"data: \xff\n\n"), Err(SseError::Utf8));
    assert_eq!(p.feed(b"data: ok\n\n").unwrap(), vec![frame("ok")]);
}

#[test]
fn a_frame_arriving_line_by_line_is_found_once_complete() {
    let mut p = Parser::new();
    for piece in [
        &b"data: one\n"[..],
        b"data: two\n",
        b"\n",
        b"data: three\n\n",
    ] {
        let frames = p.feed(piece).unwrap();
        if piece == b"\n" {
            assert_eq!(frames, vec![frame("one\ntwo")]);
        } else if piece.starts_with(b"data: three") {
            assert_eq!(frames, vec![frame("three")]);
        } else {
            assert_eq!(frames, vec![]);
        }
    }
}
