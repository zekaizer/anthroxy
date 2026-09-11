use std::time::Duration;

use super::*;
use crate::activity::{Outcome, Source};
use crate::anthropic::TokenUsage;

fn ts(text: &str) -> jiff::Timestamp {
    text.parse().unwrap()
}

fn record(at: &str, model: Option<&str>) -> StatsRecord {
    StatsRecord {
        v: SCHEMA,
        ts: ts(at),
        id: format!("rtr_{at}"),
        source: Source::Client,
        requested_model: model.map(str::to_owned),
        model: model.map(str::to_owned),
        matched: model.map(|_| "exact".to_owned()),
        backend: model.map(|_| "mock".to_owned()),
        upstream_model: model.map(str::to_owned),
        stream: true,
        status: Some(200),
        attempts: Some(1),
        latency_ms: Some(50),
        ttfb_ms: Some(100),
        duration_ms: Some(1100),
        bytes: 10,
        outcome: Outcome::Complete,
        usage: Some(TokenUsage {
            input: 10,
            output: 50,
            cache_read: 80,
            cache_creation: 10,
        }),
    }
}

#[test]
fn a_line_round_trips_and_tolerates_fields_it_does_not_know() {
    let original = record("2026-09-11T10:00:00Z", Some("fast"));
    let line = serde_json::to_string(&original).unwrap();
    assert!(line.starts_with("{\"v\":1,"), "{line}");
    assert_eq!(
        serde_json::from_str::<StatsRecord>(&line).unwrap(),
        original
    );

    let newer = line.replacen("{\"v\":1,", "{\"v\":1,\"added_later\":[1],", 1);
    assert_eq!(
        serde_json::from_str::<StatsRecord>(&newer).unwrap(),
        original
    );
}

#[test]
fn records_land_in_the_file_of_their_utc_date_and_read_back_by_range() {
    let dir = tempfile::tempdir().unwrap();
    let log = StatsLog::open(&dir.path().join("stats"), Duration::from_secs(86400)).unwrap();
    for at in [
        "2026-09-09T23:59:59Z",
        "2026-09-10T00:00:01Z",
        "2026-09-10T12:00:00Z",
    ] {
        log.append_blocking(&record(at, Some("fast"))).unwrap();
    }
    let mut files: Vec<String> = std::fs::read_dir(log.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["requests-2026-09-09.jsonl", "requests-2026-09-10.jsonl"]
    );

    std::fs::write(
        log.dir().join("requests-2026-09-10.jsonl"),
        format!(
            "{}not json\n",
            std::fs::read_to_string(log.dir().join("requests-2026-09-10.jsonl")).unwrap()
        ),
    )
    .unwrap();
    assert_eq!(log.read(None).len(), 3, "a malformed line is skipped");
    let since: Vec<jiff::Timestamp> = log
        .read(Some(ts("2026-09-10T06:00:00Z")))
        .iter()
        .map(|r| r.ts)
        .collect();
    assert_eq!(since, [ts("2026-09-10T12:00:00Z")]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(log.dir()), 0o700);
        assert_eq!(mode(&log.dir().join("requests-2026-09-09.jsonl")), 0o600);
    }
}

#[test]
fn prune_removes_whole_days_past_retention_only() {
    let dir = tempfile::tempdir().unwrap();
    let log = StatsLog::open(dir.path(), Duration::from_secs(2 * 86400)).unwrap();
    for name in [
        "requests-2026-09-07.jsonl",
        "requests-2026-09-08.jsonl",
        "requests-2026-09-09.jsonl",
        "requests-2026-09-11.jsonl",
        "notes.txt",
        "requests-garbage.jsonl",
    ] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }
    // Cutoff 2026-09-09T12:00Z: the 8th ended before it, the 9th did not.
    assert_eq!(log.prune(ts("2026-09-11T12:00:00Z")), 2);
    let mut left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "notes.txt",
            "requests-2026-09-09.jsonl",
            "requests-2026-09-11.jsonl",
            "requests-garbage.jsonl"
        ]
    );
    let keep = StatsLog::open(dir.path(), Duration::ZERO).unwrap();
    assert_eq!(keep.prune(ts("2030-01-01T00:00:00Z")), 0);
}

#[tokio::test]
async fn append_off_the_request_path_reaches_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let log = StatsLog::open(dir.path(), Duration::ZERO).unwrap();
    log.append(record("2026-09-11T10:00:00Z", Some("fast")));
    for _ in 0..100 {
        if log.read(None).len() == 1 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the appended record never reached the file");
}

#[test]
fn ranges_parse_and_start_where_they_say() {
    let now = ts("2026-09-11T12:00:00Z");
    assert_eq!(Range::parse("1d"), Some(Range::Day));
    assert_eq!(Range::parse("7d"), Some(Range::Week));
    assert_eq!(Range::parse("30d"), Some(Range::Month));
    assert_eq!(Range::parse("all"), Some(Range::All));
    assert_eq!(Range::parse("2d"), None);
    assert_eq!(Range::Day.since(now), Some(ts("2026-09-10T12:00:00Z")));
    assert_eq!(Range::Month.since(now), Some(ts("2026-08-12T12:00:00Z")));
    assert_eq!(Range::All.since(now), None);
}

#[test]
fn aggregation_by_model_and_day() {
    let mut records = Vec::new();
    // fast: 20 successful streams on the 10th with ttfb 1..=20 ms.
    for i in 1..=20u64 {
        let mut r = record("2026-09-10T10:00:00Z", Some("fast"));
        r.ttfb_ms = Some(i);
        r.duration_ms = Some(i + 1000);
        records.push(r);
    }
    // fast: a retried 502 and a disconnect on the 11th.
    let mut failed = record("2026-09-11T09:00:00Z", Some("fast"));
    failed.status = Some(502);
    failed.outcome = Outcome::Error;
    failed.attempts = Some(3);
    failed.usage = None;
    failed.ttfb_ms = Some(1);
    records.push(failed);
    let mut left = record("2026-09-11T09:30:00Z", Some("fast"));
    left.outcome = Outcome::ClientDisconnected;
    left.usage = Some(TokenUsage {
        input: 5,
        ..TokenUsage::default()
    });
    records.push(left);
    // smart: one error event inside a 200 stream.
    let mut overloaded = record("2026-09-11T10:00:00Z", Some("smart"));
    overloaded.outcome = Outcome::Error;
    records.push(overloaded);
    // An unrouted 404.
    let mut unknown = record("2026-09-11T11:00:00Z", None);
    unknown.status = Some(404);
    unknown.outcome = Outcome::Error;
    unknown.usage = None;
    records.push(unknown);

    let report = aggregate(&records, Range::All, ts("2026-09-11T12:00:00Z"));
    assert_eq!(report.range, "all");
    assert_eq!(report.total.requests, 24);
    assert_eq!(report.total.errors, 3);
    assert_eq!(report.total.disconnects, 1);

    let keys: Vec<&str> = report.models.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(
        keys,
        ["fast", "(unrouted)", "smart"],
        "most requests first, then by key"
    );
    let fast = &report.models[0];
    assert_eq!(fast.key, "fast");
    assert_eq!(fast.backend.as_deref(), Some("mock"));
    assert_eq!(
        (fast.requests, fast.errors, fast.retried, fast.disconnects),
        (22, 1, 1, 1)
    );
    assert_eq!((fast.ttfb_p50_ms, fast.ttfb_p95_ms), (Some(10), Some(19)));
    assert_eq!(
        (fast.duration_p50_ms, fast.duration_p95_ms),
        (Some(1010), Some(1019))
    );
    assert_eq!(fast.input_tokens, 20 * 10 + 5);
    assert_eq!(fast.output_tokens, 20 * 50);
    assert_eq!(fast.cache_read_tokens, 20 * 80);
    assert_eq!(fast.cache_creation_tokens, 20 * 10);
    let hit = fast.cache_hit_rate.unwrap();
    assert!(
        (hit - 1600.0 / (205.0 + 1600.0 + 200.0)).abs() < 1e-9,
        "{hit}"
    );
    // 1000 output tokens over sum(duration - ttfb) = 20 * 1000 ms.
    let rate = fast.output_tokens_per_second.unwrap();
    assert!((rate - 50.0).abs() < 1e-9, "{rate}");

    let smart = report.models.iter().find(|r| r.key == "smart").unwrap();
    assert_eq!((smart.requests, smart.errors), (1, 1));
    assert_eq!(
        smart.ttfb_p50_ms, None,
        "a failed exchange has no latency sample"
    );
    let unrouted = report
        .models
        .iter()
        .find(|r| r.key == "(unrouted)")
        .unwrap();
    assert_eq!(
        (unrouted.requests, unrouted.errors, unrouted.backend.clone()),
        (1, 1, None)
    );
    assert_eq!(unrouted.cache_hit_rate, None);
    assert_eq!(unrouted.output_tokens_per_second, None);

    let days: Vec<(&str, u64)> = report
        .days
        .iter()
        .map(|r| (r.key.as_str(), r.requests))
        .collect();
    assert_eq!(days, [("2026-09-10", 20), ("2026-09-11", 4)]);
}

#[test]
fn generation_needs_a_complete_stream_with_time_after_the_first_byte() {
    let sample = generation(true, true, Some(100), Some(2100), Some(50));
    assert_eq!(
        sample,
        Some(Generation {
            tokens: 50,
            millis: 2000
        })
    );
    assert!((sample.unwrap().tokens_per_second() - 25.0).abs() < 1e-9);
    for (stream, complete, ttfb, duration, output) in [
        (false, true, Some(100), Some(2100), Some(50)),
        (true, false, Some(100), Some(2100), Some(50)),
        (true, true, None, Some(2100), Some(50)),
        (true, true, Some(100), None, Some(50)),
        (true, true, Some(100), Some(100), Some(50)),
        (true, true, Some(100), Some(2100), Some(0)),
        (true, true, Some(100), Some(2100), None),
    ] {
        assert_eq!(
            generation(stream, complete, ttfb, duration, output),
            None,
            "{stream} {complete} {ttfb:?} {duration:?} {output:?}"
        );
    }
}

fn at(when: &str) -> StatsRecord {
    record(when, Some("fast"))
}

fn keys_and_requests(report: &Report) -> Vec<(String, u64)> {
    report
        .series
        .iter()
        .map(|row| (row.key.clone(), row.requests))
        .collect()
}

#[test]
fn a_day_is_a_series_of_hours_with_the_quiet_ones_filled() {
    let records = [
        at("2026-09-11T10:05:00Z"),
        at("2026-09-11T10:59:59Z"),
        at("2026-09-11T10:30:00Z"),
        at("2026-09-11T12:10:00Z"),
    ];
    let report = aggregate(&records, Range::Day, ts("2026-09-11T12:30:00Z"));
    assert_eq!(report.bucket, "1h");
    let series = keys_and_requests(&report);
    assert_eq!(series.len(), 25, "12:00 yesterday through 12:00 today");
    assert_eq!(series[0], ("2026-09-10T12:00:00Z".to_owned(), 0));
    assert_eq!(series[22], ("2026-09-11T10:00:00Z".to_owned(), 3));
    assert_eq!(series[23], ("2026-09-11T11:00:00Z".to_owned(), 0));
    assert_eq!(series[24], ("2026-09-11T12:00:00Z".to_owned(), 1));
    let busy = &report.series[22];
    assert_eq!(busy.output_tokens, 150);
    assert_eq!(busy.ttfb_p50_ms, Some(100));
    assert!(busy.output_tokens_per_second.is_some());
    assert_eq!(report.series[23].ttfb_p50_ms, None);
}

#[test]
fn a_week_is_drawn_in_six_hours_and_everything_in_days() {
    let week = aggregate(
        &[at("2026-09-08T07:00:00Z")],
        Range::Week,
        ts("2026-09-11T12:30:00Z"),
    );
    assert_eq!(week.bucket, "6h");
    let series = keys_and_requests(&week);
    assert_eq!(series.len(), 29);
    assert_eq!(series[0].0, "2026-09-04T12:00:00Z");
    assert_eq!(series[28].0, "2026-09-11T12:00:00Z");
    assert!(series.contains(&("2026-09-08T06:00:00Z".to_owned(), 1)));

    let everything = aggregate(
        &[at("2026-09-09T05:00:00Z"), at("2026-09-11T01:00:00Z")],
        Range::All,
        ts("2026-09-11T12:30:00Z"),
    );
    assert_eq!(everything.bucket, "1d");
    assert_eq!(
        keys_and_requests(&everything),
        [
            ("2026-09-09T00:00:00Z".to_owned(), 1),
            ("2026-09-10T00:00:00Z".to_owned(), 0),
            ("2026-09-11T00:00:00Z".to_owned(), 1)
        ]
    );
    assert!(
        aggregate(&[], Range::All, ts("2026-09-11T12:30:00Z"))
            .series
            .is_empty()
    );
}

#[test]
fn fallbacks_are_the_names_the_default_model_or_nothing_served() {
    let mut records = Vec::new();
    for at in ["2026-09-11T08:00:00Z", "2026-09-11T09:00:00Z"] {
        let mut r = record(at, Some("fast"));
        r.requested_model = Some("claude-haiku-4-5-20251001".into());
        r.matched = Some("default".into());
        records.push(r);
    }
    let mut alias = record("2026-09-11T09:30:00Z", Some("fast"));
    alias.requested_model = Some("claude-haiku-4-5".into());
    alias.matched = Some("alias".into());
    records.push(alias);
    let mut unknown = record("2026-09-11T10:00:00Z", None);
    unknown.requested_model = Some("ghost".into());
    unknown.status = Some(404);
    unknown.outcome = Outcome::Error;
    unknown.usage = None;
    records.push(unknown);
    // A body without a model never named one.
    let mut unparsed = record("2026-09-11T10:30:00Z", None);
    unparsed.status = Some(400);
    unparsed.outcome = Outcome::Error;
    records.push(unparsed);

    let report = aggregate(&records, Range::All, ts("2026-09-11T12:00:00Z"));
    assert_eq!(report.total.defaulted, 2);
    let fast = report.models.iter().find(|row| row.key == "fast").unwrap();
    assert_eq!(fast.defaulted, 2);
    assert_eq!(
        report.fallbacks,
        [
            Fallback {
                requested: "claude-haiku-4-5-20251001".into(),
                model: Some("fast".into()),
                requests: 2,
                last_seen: ts("2026-09-11T09:00:00Z"),
            },
            Fallback {
                requested: "ghost".into(),
                model: None,
                requests: 1,
                last_seen: ts("2026-09-11T10:00:00Z"),
            },
        ]
    );
}

#[test]
fn a_line_written_before_matched_existed_reads_as_unknown() {
    let mut line = serde_json::to_value(record("2026-09-11T08:00:00Z", Some("fast"))).unwrap();
    line.as_object_mut().unwrap().remove("matched");
    let read: StatsRecord = serde_json::from_value(line).unwrap();
    assert_eq!(read.matched, None);
}
