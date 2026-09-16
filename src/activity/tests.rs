use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;

use super::*;
use crate::routing::Match;

fn begin(activity: &Arc<Activity>, id: &str) -> Exchange {
    activity.begin(
        id,
        Source::Client,
        Some("10.0.0.2:5000".into()),
        "POST",
        "/v1/messages",
        tokio::sync::watch::channel(false).1,
    )
}

fn routed(exchange: &Exchange, matched: Match) {
    exchange.requested("claude-haiku-4-5", true);
    exchange.routed(
        "claude-haiku-4-5",
        matched,
        "fast",
        "mock",
        BackendKind::Anthropic,
        "mock-fast-v1",
    );
}

const SSE: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":9,\"cache_read_input_tokens\":90,\"output_tokens\":1}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\n\n";

#[tokio::test]
async fn a_streamed_exchange_moves_from_in_flight_to_recent_with_its_usage() {
    let activity = Activity::new();
    let exchange = begin(&activity, "rtr_1");
    routed(&exchange, Match::Alias);
    exchange.recording("20260911T000000.000Z-rtr_1".into());
    exchange.responded(200, 2, false, Duration::from_millis(30));

    let in_flight = activity.in_flight();
    assert_eq!(in_flight.len(), 1);
    assert_eq!(in_flight[0].id, "rtr_1");
    assert_eq!(in_flight[0].backend.as_deref(), Some("mock"));
    assert_eq!(in_flight[0].outcome, None);

    let (first, second) = SSE.split_at(40);
    let chunks = vec![
        Ok(Bytes::from(first.to_owned())),
        Ok(Bytes::from(second.to_owned())),
    ];
    let body: Vec<Bytes> = exchange
        .track(
            futures_util::stream::iter(chunks),
            Some("text/event-stream"),
        )
        .map(|chunk| chunk.unwrap())
        .collect()
        .await;
    assert_eq!(
        body.concat(),
        SSE.as_bytes(),
        "bytes pass through untouched"
    );

    assert!(activity.in_flight().is_empty());
    let recent = activity.recent();
    assert_eq!(recent.len(), 1);
    let view = &recent[0];
    assert_eq!(view.outcome, Some(Outcome::Complete));
    assert_eq!(view.status, Some(200));
    assert_eq!(view.attempts, Some(2));
    assert_eq!(view.matched, Some("alias"));
    assert_eq!(view.peer.as_deref(), Some("10.0.0.2:5000"));
    assert_eq!(view.bytes, SSE.len() as u64);
    assert!(view.ttfb_ms.is_some() && view.duration_ms.is_some());
    assert_eq!(
        view.usage,
        Some(TokenUsage {
            input: 9,
            output: 5,
            cache_read: 90,
            cache_creation: 0,
            cache_reported: true,
        })
    );
    assert_eq!(
        view.recording.as_deref(),
        Some("20260911T000000.000Z-rtr_1")
    );
    assert_eq!(activity.find("rtr_1").map(|v| v.id), Some("rtr_1".into()));
    assert!(
        activity.names().is_empty(),
        "an alias is a configured name, not a tally entry"
    );
}

#[tokio::test]
async fn a_stream_the_client_leaves_or_that_fails_is_recorded_as_such() {
    let activity = Activity::new();
    let exchange = begin(&activity, "rtr_left");
    routed(&exchange, Match::Exact);
    exchange.responded(200, 1, false, Duration::ZERO);
    let mut tracked = exchange.track(
        futures_util::stream::iter(vec![Ok(Bytes::from_static(b"data: {}\n\n"))])
            .chain(futures_util::stream::pending()),
        Some("text/event-stream"),
    );
    assert!(tracked.next().await.is_some());
    drop(tracked);
    assert_eq!(
        activity.recent()[0].outcome,
        Some(Outcome::ClientDisconnected)
    );

    let exchange = begin(&activity, "rtr_broken");
    routed(&exchange, Match::Exact);
    exchange.responded(200, 1, false, Duration::ZERO);
    let chunks: Vec<Result<Bytes, std::io::Error>> =
        vec![Err(std::io::Error::other("connection reset"))];
    let _ = exchange
        .track(
            futures_util::stream::iter(chunks),
            Some("text/event-stream"),
        )
        .collect::<Vec<_>>()
        .await;
    let view = activity.find("rtr_broken").unwrap();
    assert_eq!(view.outcome, Some(Outcome::Error));
    assert_eq!(view.error.as_deref(), Some("connection reset"));
}

#[tokio::test]
async fn an_error_event_in_a_200_stream_is_an_error() {
    let activity = Activity::new();
    let exchange = begin(&activity, "rtr_overloaded");
    routed(&exchange, Match::Exact);
    exchange.responded(200, 1, false, Duration::ZERO);
    let body = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
    let _ = exchange
        .track(
            futures_util::stream::iter(vec![Ok(Bytes::from(body))]),
            Some("text/event-stream"),
        )
        .collect::<Vec<_>>()
        .await;
    let view = activity.find("rtr_overloaded").unwrap();
    assert_eq!(view.outcome, Some(Outcome::Error));
    assert_eq!(view.error.as_deref(), Some("overloaded_error: Overloaded"));
}

#[test]
fn an_exchange_dropped_before_its_response_is_a_client_that_left() {
    let activity = Activity::new();
    drop(begin(&activity, "rtr_gone"));
    let view = activity.find("rtr_gone").unwrap();
    assert_eq!(view.outcome, Some(Outcome::ClientDisconnected));
    assert!(activity.in_flight().is_empty());
}

#[test]
fn router_failures_upstream_errors_and_buffered_bodies() {
    let activity = Activity::new();
    let exchange = begin(&activity, "rtr_404");
    exchange.requested("ghost", false);
    exchange.unrouted("ghost");
    exchange.fail(404, "model `ghost` is not served".into());
    let view = activity.find("rtr_404").unwrap();
    assert_eq!(
        (view.status, view.outcome, view.error.as_deref()),
        (
            Some(404),
            Some(Outcome::Error),
            Some("model `ghost` is not served")
        )
    );
    assert!(view.duration_ms.is_some());

    let exchange = begin(&activity, "rtr_400");
    routed(&exchange, Match::Default);
    exchange.responded(400, 1, false, Duration::ZERO);
    let body = format!(
        "{{\"type\":\"error\",\"error\":{{\"type\":\"invalid_request_error\",\"message\":\"bad\"}},\"pad\":\"{}\"}}",
        "x".repeat(ERROR_BODY_BYTES)
    );
    let hint = Hint {
        summary: "drop it".into(),
        snippet: None,
    };
    exchange.upstream_error(body.as_bytes(), vec![hint.clone()]);
    exchange.finish_body(400, body.as_bytes(), Some("application/json"));
    let view = activity.find("rtr_400").unwrap();
    assert_eq!(view.outcome, Some(Outcome::Error));
    assert_eq!(view.error.as_deref(), Some("invalid_request_error: bad"));
    assert!(view.error_body.as_ref().unwrap().len() <= ERROR_BODY_BYTES);
    assert_eq!(view.hints, [hint]);
    assert_eq!(view.bytes, body.len() as u64);

    let exchange = begin(&activity, "rtr_doc");
    routed(&exchange, Match::Exact);
    exchange.responded(200, 1, false, Duration::ZERO);
    exchange.finish_body(
        200,
        br#"{"type":"message","usage":{"input_tokens":1,"output_tokens":2}}"#,
        Some("application/json"),
    );
    let view = activity.find("rtr_doc").unwrap();
    assert_eq!(view.outcome, Some(Outcome::Complete));
    assert_eq!(view.usage.map(|u| u.output), Some(2));
    assert!(view.ttfb_ms.is_some());

    let recent: Vec<String> = activity.recent().iter().map(|v| v.id.clone()).collect();
    assert_eq!(recent, ["rtr_doc", "rtr_400", "rtr_404"], "newest first");
}

#[test]
fn recent_is_bounded() {
    let activity = Activity::new();
    for i in 0..RECENT + 5 {
        begin(&activity, &format!("rtr_{i}")).fail(500, "x".into());
    }
    let recent = activity.recent();
    assert_eq!(recent.len(), RECENT);
    assert_eq!(recent[0].id, format!("rtr_{}", RECENT + 4));
    assert!(activity.find("rtr_0").is_none());
}

#[test]
fn unknown_and_defaulted_names_are_tallied_within_bounds() {
    let activity = Activity::new();
    for _ in 0..3 {
        let exchange = begin(&activity, "rtr_u");
        exchange.unrouted("ghost");
        exchange.fail(404, "x".into());
    }
    let exchange = begin(&activity, "rtr_d");
    routed(&exchange, Match::Default);
    exchange.fail(502, "x".into());

    let names = activity.names();
    assert_eq!(names.len(), 2);
    assert_eq!(names[0].name, "claude-haiku-4-5", "most recent first");
    assert_eq!((names[0].unknown, names[0].defaulted), (0, 1));
    assert_eq!((names[1].name.as_str(), names[1].unknown), ("ghost", 3));

    let mut tally = names::NameTally::default();
    tally.unknown(&"n\n".repeat(500));
    assert!(!tally.list()[0].name.contains('\n'));
    assert!(tally.list()[0].name.chars().count() < 300);
    for i in 0..names::NAMES + 10 {
        tally.unknown(&format!("name-{i}"));
    }
    let list = tally.list();
    assert_eq!(list.len(), names::NAMES);
    assert_eq!(list[0].name, format!("name-{}", names::NAMES + 9));
}
