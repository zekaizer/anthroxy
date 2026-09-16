use super::*;

fn usage(input: u64, output: u64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        cache_reported: false,
        thinking_tokens: 0,
    }
}

#[test]
fn folds_events_into_blocks_in_arrival_order() {
    let message = Message::from_events([
        Event::Start {
            id: "chatcmpl-1".into(),
            model: "m".into(),
        },
        Event::ThinkingDelta("think ".into()),
        Event::ThinkingDelta("more".into()),
        Event::TextDelta("hello".into()),
        Event::ToolCallStart {
            index: 0,
            id: "call_a".into(),
            name: "read".into(),
        },
        Event::ToolCallDelta {
            index: 0,
            arguments: "{\"path\":".into(),
        },
        Event::ToolCallStart {
            index: 1,
            id: "call_b".into(),
            name: "bash".into(),
        },
        Event::ToolCallDelta {
            index: 1,
            arguments: "{\"cmd\":\"ls\"}".into(),
        },
        Event::ToolCallDelta {
            index: 0,
            arguments: "\"a\"}".into(),
        },
        Event::Finish(StopReason::ToolUse),
        Event::Usage(usage(12, 34)),
        Event::Done,
    ])
    .unwrap();
    assert_eq!(message.id, "chatcmpl-1");
    assert_eq!(message.model, "m");
    assert_eq!(
        message.blocks,
        vec![
            Block::Thinking("think more".into()),
            Block::Text("hello".into()),
            Block::ToolUse {
                id: "call_a".into(),
                name: "read".into(),
                arguments: "{\"path\":\"a\"}".into(),
            },
            Block::ToolUse {
                id: "call_b".into(),
                name: "bash".into(),
                arguments: "{\"cmd\":\"ls\"}".into(),
            },
        ]
    );
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    assert_eq!(message.usage, usage(12, 34));
}

#[test]
fn text_after_a_tool_call_opens_a_new_block() {
    let message = Message::from_events([
        Event::Start {
            id: String::new(),
            model: String::new(),
        },
        Event::TextDelta("a".into()),
        Event::ToolCallStart {
            index: 0,
            id: "c".into(),
            name: "t".into(),
        },
        Event::TextDelta("b".into()),
        Event::Done,
    ])
    .unwrap();
    assert_eq!(
        message.blocks,
        vec![
            Block::Text("a".into()),
            Block::ToolUse {
                id: "c".into(),
                name: "t".into(),
                arguments: String::new(),
            },
            Block::Text("b".into()),
        ]
    );
}

#[test]
fn stop_reason_defaults_to_tool_use_when_tools_were_called() {
    let with_tools = Message::from_events([
        Event::ToolCallStart {
            index: 0,
            id: "c".into(),
            name: "t".into(),
        },
        Event::Finish(StopReason::EndTurn),
        Event::Done,
    ])
    .unwrap();
    assert_eq!(with_tools.stop_reason, StopReason::ToolUse);

    let without = Message::from_events([Event::TextDelta("x".into()), Event::Done]).unwrap();
    assert_eq!(without.stop_reason, StopReason::EndTurn);

    let truncated =
        Message::from_events([Event::Finish(StopReason::MaxTokens), Event::Done]).unwrap();
    assert_eq!(truncated.stop_reason, StopReason::MaxTokens);
}

#[test]
fn an_error_event_fails_the_fold() {
    let result = Message::from_events([
        Event::TextDelta("partial".into()),
        Event::Error("overloaded".into()),
    ]);
    assert_eq!(result, Err("overloaded".to_owned()));
}

#[test]
fn content_after_finish_opens_a_new_block_like_the_stream_does() {
    let message = Message::from_events([
        Event::TextDelta("a".into()),
        Event::Finish(StopReason::EndTurn),
        Event::TextDelta("b".into()),
        Event::Done,
    ])
    .unwrap();
    assert_eq!(
        message.blocks,
        vec![Block::Text("a".into()), Block::Text("b".into())]
    );
}

#[test]
fn empty_deltas_and_repeated_starts_are_ignored() {
    let message = Message::from_events([
        Event::Start {
            id: "first".into(),
            model: "m1".into(),
        },
        Event::ThinkingDelta(String::new()),
        Event::TextDelta(String::new()),
        Event::Start {
            id: "second".into(),
            model: "m2".into(),
        },
        Event::TextDelta("x".into()),
        Event::Done,
    ])
    .unwrap();
    assert_eq!(message.id, "first");
    assert_eq!(message.model, "m1");
    assert_eq!(message.blocks, vec![Block::Text("x".into())]);
}
