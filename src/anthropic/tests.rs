use super::*;
use http::StatusCode;
use serde_json::{Value, json};

/// `rewrite` for a body already known to parse.
fn unwrapped_rewrite(body: &[u8], model: Option<&str>, drop_fields: &[String]) -> Option<Vec<u8>> {
    rewrite(body, model, drop_fields, false).expect("a body peek accepted")
}

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
fn peek_rejects_a_body_that_is_not_a_json_object() {
    // serde_json also reads a struct from an array; `rewrite` cannot.
    for body in [
        br#"["m1", true]"#.as_slice(),
        br#"[]"#.as_slice(),
        br#""m1""#.as_slice(),
        br#"42"#.as_slice(),
        br#"null"#.as_slice(),
    ] {
        let err = peek(body).err().unwrap_or_else(|| {
            panic!("accepted {}", String::from_utf8_lossy(body));
        });
        assert!(matches!(err, PeekError::NotJson(_)), "{err}");
    }
}

#[test]
fn rewrite_of_model_preserves_everything_else_in_order() {
    let body = br#"{"model":"exposed","max_tokens":1024,"system":[{"type":"text","text":"hi","cache_control":{"type":"ephemeral"}}],"messages":[{"role":"user","content":"x"}],"metadata":{"user_id":"u"},"temperature":1.0,"big":12345678901234567890}"#;
    let out = unwrapped_rewrite(body, Some("upstream-name"), &[]).unwrap();
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
    let out = unwrapped_rewrite(body, None, &drop).expect("something was dropped");
    assert_eq!(
        std::str::from_utf8(&out).unwrap(),
        r#"{"model":"m","metadata":{"keep":1},"messages":[]}"#
    );

    let out = unwrapped_rewrite(body, Some("up"), &drop).unwrap();
    assert_eq!(
        std::str::from_utf8(&out).unwrap(),
        r#"{"model":"up","metadata":{"keep":1},"messages":[]}"#
    );
}

#[test]
fn rewrite_reports_a_body_it_cannot_parse_as_a_bad_request() {
    // `peek` skips a value it does not read without measuring its depth;
    // parsing the whole document has a recursion limit.
    let deep = format!(
        "{{\"model\": \"m1\", \"x\": {}{}}}",
        "[".repeat(200),
        "]".repeat(200)
    );
    assert!(
        peek(deep.as_bytes()).is_ok(),
        "the body reaches the rewrite"
    );
    let err = rewrite(deep.as_bytes(), Some("m2"), &[], false)
        .err()
        .unwrap();
    assert!(matches!(err, PeekError::NotJson(_)), "{err}");

    assert!(
        rewrite(deep.as_bytes(), None, &[], false)
            .unwrap()
            .is_none(),
        "nothing to rewrite: the body is forwarded unread"
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
    assert_eq!(unwrapped_rewrite(body, None, &drop), None);
    assert_eq!(unwrapped_rewrite(body, None, &[]), None);
    let out = unwrapped_rewrite(body, Some("up"), &drop).unwrap();
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

// ---- unsigned thinking blocks (rewrite)

fn run(body: Value) -> Option<Value> {
    rewrite(&serde_json::to_vec(&body).unwrap(), None, &[], true)
        .expect("a JSON object")
        .map(|b| serde_json::from_slice(&b).unwrap())
}

#[test]
fn unsigned_blocks_go_and_signed_ones_stay() {
    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "router made"},
            {"type": "thinking", "thinking": "anthropic made", "signature": "sig"},
            {"type": "redacted_thinking", "data": "xx"},
            {"type": "text", "text": "hello"}
        ]},
        {"role": "user", "content": [{"type": "text", "text": "more"}]}
    ]});
    assert_eq!(
        run(body).unwrap()["messages"],
        json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "anthropic made", "signature": "sig"},
                {"type": "redacted_thinking", "data": "xx"},
                {"type": "text", "text": "hello"}
            ]},
            {"role": "user", "content": [{"type": "text", "text": "more"}]}
        ])
    );
}

#[test]
fn an_empty_signature_is_what_claude_code_stores_for_router_blocks() {
    let body = json!({"model": "m", "messages": [
        {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "router made", "signature": ""},
            {"type": "text", "text": "hello"}
        ]}
    ]});
    assert_eq!(
        run(body).unwrap()["messages"][0]["content"],
        json!([{"type": "text", "text": "hello"}])
    );
}

#[test]
fn an_assistant_turn_left_empty_is_dropped() {
    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": [{"type": "thinking", "thinking": "only"}]},
        {"role": "user", "content": "again"}
    ]});
    assert_eq!(
        run(body).unwrap()["messages"],
        json!([{"role": "user", "content": "hi"}, {"role": "user", "content": "again"}])
    );
}

#[test]
fn untouched_bodies_are_not_rewritten() {
    assert_eq!(
        run(json!({"model": "m", "messages": [{"role": "user", "content": "hi"}]})),
        None
    );
    assert_eq!(
        run(
            json!({"model": "m", "messages": [{"role": "assistant", "content": [
            {"type": "thinking", "thinking": "t", "signature": "s"}]}]})
        ),
        None
    );
    assert_eq!(
        run(json!({"model": "m", "messages": [{"role": "user", "content": "the word thinking"}]})),
        None
    );
    assert_eq!(rewrite(b"{}", None, &[], true).unwrap(), None);
}

// ---- tool call ids (rewrite, ADR-0013)

#[test]
fn tool_call_ids_outside_the_accepted_form_are_rewritten_in_pairs() {
    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": "read it"},
        {"role": "assistant", "content": [
            {"type": "text", "text": "functions.read:0 stays in text"},
            {"type": "tool_use", "id": "functions.read:0", "name": "read", "input": {}},
            {"type": "tool_use", "id": "call_ok-1", "name": "read", "input": {}}
        ]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "functions.read:0", "content": "a"},
            {"type": "tool_result", "tool_use_id": "call_ok-1", "content": "b"}
        ]},
        {"role": "assistant", "content": [
            {"type": "server_tool_use", "id": "srvtoolu.kept", "name": "web_search", "input": {}}
        ]}
    ]});
    let messages = run(body).unwrap()["messages"].clone();
    assert_eq!(
        messages[1]["content"][0]["text"],
        "functions.read:0 stays in text"
    );
    assert_eq!(messages[1]["content"][1]["id"], "functions_read_0");
    assert_eq!(messages[1]["content"][2]["id"], "call_ok-1");
    assert_eq!(messages[2]["content"][0]["tool_use_id"], "functions_read_0");
    assert_eq!(messages[2]["content"][1]["tool_use_id"], "call_ok-1");
    assert_eq!(messages[3]["content"][0]["id"], "srvtoolu.kept");
}

#[test]
fn accepted_tool_call_ids_leave_the_body_alone() {
    let body = json!({"model": "m", "messages": [
        {"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_01AbC", "name": "read", "input": {}}
        ]},
        {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_01AbC"}]}
    ]});
    assert_eq!(run(body.clone()), None);
    let mut foreign = body;
    foreign["messages"][0]["content"][0]["id"] = json!("functions.read:0");
    assert_eq!(
        rewrite(&serde_json::to_vec(&foreign).unwrap(), None, &[], false).unwrap(),
        None,
        "a body for an openai backend keeps its ids"
    );
}

#[test]
fn summary_names_the_last_prompt_without_reminders_or_tool_results() {
    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": [
            {"type": "text", "text": "<system-reminder>\nContext.\n</system-reminder>"},
            {"type": "text", "text": "What is in\n  hostname.txt?"}
        ]},
        {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {}}]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "fixture-host"},
            {"type": "text", "text": "<system-reminder>Only a reminder.</system-reminder>"}
        ]},
        {"role": "system", "content": "Mid-conversation system text."}
    ]});
    assert_eq!(
        summarize(body.to_string().as_bytes()),
        RequestSummary {
            messages: 4,
            prompt: Some("What is in hostname.txt?".to_owned()),
            step: Some("← Read".to_owned()),
        }
    );
}

#[test]
fn summary_prompt_is_one_cut_line_and_absent_without_user_text() {
    let long = format!("{}\n{}", "a".repeat(150), "b".repeat(150));
    let body = json!({"model": "m", "messages": [{"role": "user", "content": long}]});
    let prompt = summarize(body.to_string().as_bytes()).prompt.unwrap();
    assert_eq!(prompt.chars().count(), 201, "{prompt}");
    assert!(prompt.starts_with(&format!("{} b", "a".repeat(150))) && prompt.ends_with('…'));

    let body = json!({"model": "m", "messages": [{"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "t1", "content": "out"}
    ]}]});
    assert_eq!(
        summarize(body.to_string().as_bytes()),
        RequestSummary {
            messages: 1,
            prompt: None,
            step: Some("← tool".to_owned()),
        }
    );
    assert_eq!(summarize(b"not json"), RequestSummary::default());
    assert_eq!(
        summarize(br#"{"model": "m", "messages": 3}"#),
        RequestSummary::default()
    );
}

#[test]
fn summary_takes_a_message_sent_while_the_model_worked_as_the_prompt() {
    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": "Fix the build."},
        {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "ok"},
            {"type": "text", "text": "<system-reminder>\nThe user sent a new message while you were working:\nAlso run the tests.\n</system-reminder>"}
        ]}
    ]});
    let summary = summarize(body.to_string().as_bytes());
    assert_eq!(summary.prompt.as_deref(), Some("Also run the tests."));
    assert_eq!(summary.step, None, "the request carries its prompt");
}

/// Claude Code sends its interruption notice as a text block of the user
/// message that carries the next prompt.
#[test]
fn summary_leaves_out_the_notice_of_an_interrupted_request() {
    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": "Fix the build."},
        {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "rejected"},
            {"type": "text", "text": "[Request interrupted by user for tool use]"},
            {"type": "text", "text": "Run it on this branch."},
            {"type": "text", "text": "[Request interrupted by user]"},
            {"type": "text", "text": "Then commit."}
        ]}
    ]});
    let summary = summarize(body.to_string().as_bytes());
    assert_eq!(
        summary.prompt.as_deref(),
        Some("Run it on this branch. Then commit.")
    );
    assert_eq!(summary.step, None);

    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": "Fix the build."},
        {"role": "assistant", "content": "Working on it."},
        {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}
    ]});
    let summary = summarize(body.to_string().as_bytes());
    assert_eq!(summary.prompt.as_deref(), Some("Fix the build."));
    assert_eq!(summary.step.as_deref(), Some("interrupted"));
}

/// Every request of a turn after the first answers tool calls; it keeps the
/// turn's prompt and says what it sends instead.
#[test]
fn summary_tells_a_request_that_returns_tool_results_from_the_one_that_asks() {
    let turn = |last: Value| {
        json!({"model": "m", "messages": [
            {"role": "user", "content": "Fix the build."},
            {"role": "assistant", "content": [
                {"type": "text", "text": "Looking."},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {}},
                {"type": "tool_use", "id": "t2", "name": "Bash", "input": {}},
                {"type": "tool_use", "id": "t3", "name": "Read", "input": {}}
            ]},
            last
        ]})
    };
    let summary = summarize(
        turn(json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "a"},
            {"type": "tool_result", "tool_use_id": "t2", "content": "b"},
            {"type": "tool_result", "tool_use_id": "t3", "content": "c"},
            {"type": "tool_result", "tool_use_id": "unknown", "content": "d"},
            {"type": "text", "text": "<system-reminder>A reminder.</system-reminder>"}
        ]}))
        .to_string()
        .as_bytes(),
    );
    assert_eq!(summary.prompt.as_deref(), Some("Fix the build."));
    assert_eq!(summary.step.as_deref(), Some("← Read ×2, Bash, tool"));

    let summary = summarize(
        turn(json!({"role": "assistant", "content": "Partial"}))
            .to_string()
            .as_bytes(),
    );
    assert_eq!(summary.step.as_deref(), Some("assistant prefill"));

    let summary = summarize(
        turn(json!({"role": "user", "content": [
            {"type": "text", "text": "<system-reminder>Only this.</system-reminder>"}
        ]}))
        .to_string()
        .as_bytes(),
    );
    assert_eq!(summary.step.as_deref(), Some("system reminder"));
}

/// A tool result is named by its call and a hint from the call's input, so
/// the requests of one turn read apart.
#[test]
fn summary_names_tool_results_by_what_each_call_did() {
    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": "Fix the build."},
        {"role": "assistant", "content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test --all", "description": "Run the tests"}},
            {"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "/work/anthroxy/src/anthropic/summary.rs"}},
            {"type": "tool_use", "id": "t3", "name": "Grep", "input": {"pattern": "fn summarize"}},
            {"type": "tool_use", "id": "t4", "name": "Bash", "input": {"command": "git status\n--short"}},
            {"type": "tool_use", "id": "t5", "name": "TodoWrite", "input": {"todos": []}}
        ]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "ok"},
            {"type": "tool_result", "tool_use_id": "t2", "content": "ok"},
            {"type": "tool_result", "tool_use_id": "t3", "content": "ok"},
            {"type": "tool_result", "tool_use_id": "t4", "content": "ok"},
            {"type": "tool_result", "tool_use_id": "t5", "content": "ok"}
        ]}
    ]});
    assert_eq!(
        summarize(body.to_string().as_bytes()).step.as_deref(),
        Some("← Bash Run the tests, Read summary.rs, Grep fn summarize +2 more")
    );

    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": "Fix the build."},
        {"role": "assistant", "content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "git status\n--short"}}
        ]},
        {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]}
    ]});
    assert_eq!(
        summarize(body.to_string().as_bytes()).step.as_deref(),
        Some("← Bash git status")
    );
}

/// Claude Code sends slash and shell commands, their output, background task
/// notices, hook feedback and a compaction summary as user text. A command the
/// user typed is a prompt; the rest is not.
#[test]
fn summary_reads_claude_code_notices_as_what_they_are() {
    let body = |messages: Value| {
        summarize(
            json!({"model": "m", "messages": messages})
                .to_string()
                .as_bytes(),
        )
    };

    let summary = body(json!([
        {"role": "user", "content": [
            {"type": "text", "text": "<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>"},
            {"type": "text", "text": "<command-name>/goal</command-name>\n            <command-message>goal</command-message>\n            <command-args>ship the release</command-args>"},
            {"type": "text", "text": "<local-command-stdout>Goal set: ship the release</local-command-stdout>"},
            {"type": "text", "text": "A session-scoped Stop hook is now active with condition: \"ship the release\"."}
        ]}
    ]));
    assert_eq!(summary.prompt.as_deref(), Some("/goal ship the release"));
    assert_eq!(summary.step, None);

    // A prompt command's text follows it in the same message; a local
    // command's output ends the command, and what the user typed next stays.
    let summary = body(json!([
        {"role": "user", "content": [
            {"type": "text", "text": "<command-message>simplify</command-message>\n<command-name>/simplify</command-name>\n<command-args>code size</command-args>"},
            {"type": "text", "text": "Review target: `code size`\n\nYou are improving the quality of the changed code."}
        ]}
    ]));
    assert_eq!(summary.prompt.as_deref(), Some("/simplify code size"));
    assert_eq!(summary.step, None);
    let summary = body(json!([
        {"role": "user", "content": [
            {"type": "text", "text": "<local-command-caveat>Caveat: local commands.</local-command-caveat>"},
            {"type": "text", "text": "<command-name>/effort</command-name>\n<command-message>effort</command-message>\n<command-args>high</command-args>"},
            {"type": "text", "text": "<local-command-stdout>Set effort level to high</local-command-stdout>"},
            {"type": "text", "text": "Now fix it."}
        ]}
    ]));
    assert_eq!(summary.prompt.as_deref(), Some("/effort high Now fix it."));

    let summary = body(json!([
        {"role": "user", "content": "<command-name>/clear</command-name>\n <command-message>clear</command-message>\n <command-args></command-args>"}
    ]));
    assert_eq!(summary.prompt.as_deref(), Some("/clear"));

    let summary = body(json!([
        {"role": "user", "content": [
            {"type": "text", "text": "<bash-input> git status</bash-input>"},
            {"type": "text", "text": "<bash-stdout>clean</bash-stdout><bash-stderr></bash-stderr>"}
        ]}
    ]));
    assert_eq!(summary.prompt.as_deref(), Some("! git status"));
    assert_eq!(summary.step, None);

    for (notice, label) in [
        (
            "Stop hook feedback:\n[ship it]: not done yet",
            "hook feedback",
        ),
        ("Goal check-in: «ship it» is still active", "goal check-in"),
        (
            "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>",
            "task notification",
        ),
        (
            "<local-command-stdout>Set effort level to high</local-command-stdout>",
            "command output",
        ),
        (
            "This session is being continued from a previous conversation that ran out of context.\n\nSummary: ...",
            "compaction summary",
        ),
        (
            "Base directory for this skill: /home/u/.claude/skills/diagnose\n\n# Diagnose",
            "skill",
        ),
        (
            "Another Claude session sent a message:\n<agent-message from=\"a1\">\n[Subagent hand-back] report",
            "agent message",
        ),
        ("Continue from where you left off.", "continue"),
        (
            "[Your previous response had no visible output. Please continue and produce a user-visible response.]",
            "continue",
        ),
        (
            "[Image: original 1400x2175, displayed at 1287x2000. Multiply coordinates by 1.09 to map to original image.]",
            "image note",
        ),
        (
            "<ide_opened_file>The user opened the file /w/CLAUDE.md in the IDE.</ide_opened_file>",
            "ide context",
        ),
        (
            "<ide_selection>The user selected lines 1 to 3.</ide_selection>",
            "ide context",
        ),
    ] {
        let summary = body(json!([
            {"role": "user", "content": "Fix the build."},
            {"role": "assistant", "content": "Done."},
            {"role": "user", "content": notice}
        ]));
        assert_eq!(
            summary.prompt.as_deref(),
            Some("Fix the build."),
            "{notice}"
        );
        assert_eq!(summary.step.as_deref(), Some(label), "{notice}");
    }

    let summary = body(json!([
        {"role": "user", "content": "This session is being continued from a previous conversation that ran out of context."},
        {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Edit", "input": {}}]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "ok"},
            {"type": "text", "text": "<task-notification>\n<status>completed</status>\n</task-notification>"}
        ]}
    ]));
    assert_eq!(summary.prompt, None, "a compaction summary is no prompt");
    assert_eq!(summary.step.as_deref(), Some("← Edit · task notification"));

    let summary = body(json!([
        {"role": "user", "content": "please explain what <task-notification> means"}
    ]));
    assert_eq!(
        summary.prompt.as_deref(),
        Some("please explain what <task-notification> means"),
        "a tag inside the user's own words is text"
    );
}

#[test]
fn summary_keeps_text_after_a_reminder_tag_that_never_closes() {
    let body = json!({"model": "m", "messages": [
        {"role": "user", "content": "why does <system-reminder> show up in my log?"}
    ]});
    assert_eq!(
        summarize(body.to_string().as_bytes()).prompt.as_deref(),
        Some("why does <system-reminder> show up in my log?")
    );
}

#[test]
fn summary_reads_past_messages_and_blocks_it_cannot_make_sense_of() {
    let body = json!({"model": "m", "messages": [
        null,
        {"role": null, "content": "no role"},
        {"role": "user", "content": [
            {"type": null, "text": "no type"},
            {"type": "text", "text": null},
            {"type": "text", "text": "the prompt"}
        ]},
        7
    ]});
    assert_eq!(
        summarize(body.to_string().as_bytes()),
        RequestSummary {
            messages: 4,
            prompt: Some("the prompt".to_owned()),
            step: None,
        }
    );
}
