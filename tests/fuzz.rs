//! Deterministic fuzz over everything that reads bytes the router did not
//! write: a client body, a backend's answer, a configuration file. None of it
//! may panic, whatever it is handed; what it returns is the business of the
//! tests that know the format.

use anthroxy::anthropic;
use anthroxy::openai;
use anthroxy::sse;
use anthroxy::translate;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() % xs.len() as u64) as usize]
    }
    fn upto(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const ATOMS: [&str; 43] = [
    "null",
    "true",
    "false",
    "0",
    "-1",
    "1e400",
    "18446744073709551616",
    "4294967296",
    "\"\"",
    "\"x\"",
    "\"\\u0000\"",
    "\"🙂\"",
    "[]",
    "{}",
    "[[]]",
    "{\"a\":1}",
    "\"model\"",
    "\"stream\"",
    "\"content\"",
    "\"tool_calls\"",
    "\"index\"",
    "\"function\"",
    "\"arguments\"",
    "\"name\"",
    "\"id\"",
    "\"usage\"",
    "\"choices\"",
    "\"delta\"",
    "\"finish_reason\"",
    "\"error\"",
    "\"message\"",
    "\"type\"",
    "\"text\"",
    "\"role\"",
    "\"tools\"",
    "\"tool_result\"",
    "\"tool_reference\"",
    "\"tool_name\"",
    "\"tool_use_id\"",
    "\"defer_loading\"",
    "\"input_schema\"",
    "\"user\"",
    "\"assistant\"",
];

const KEYS: [&str; 29] = [
    "model",
    "stream",
    "messages",
    "system",
    "content",
    "role",
    "type",
    "text",
    "tool_calls",
    "index",
    "function",
    "arguments",
    "name",
    "id",
    "usage",
    "choices",
    "delta",
    "finish_reason",
    "error",
    "prompt_tokens",
    "data",
    "object",
    "tools",
    "tool_result",
    "tool_reference",
    "tool_name",
    "tool_use_id",
    "defer_loading",
    "input_schema",
];

fn value(rng: &mut Rng, depth: usize) -> String {
    if depth == 0 || rng.upto(3) == 0 {
        return (*rng.pick(&ATOMS)).to_owned();
    }
    match rng.upto(3) {
        0 => {
            let n = rng.upto(4);
            let items: Vec<String> = (0..n).map(|_| value(rng, depth - 1)).collect();
            format!("[{}]", items.join(","))
        }
        _ => {
            let n = rng.upto(5);
            let items: Vec<String> = (0..n)
                .map(|_| format!("\"{}\":{}", rng.pick(&KEYS), value(rng, depth - 1)))
                .collect();
            format!("{{{}}}", items.join(","))
        }
    }
}

/// A Messages request whose tool definitions and tool-result content are
/// random, so the paths behind a valid envelope are reached: tool_reference
/// expansion, deferred-tool filtering, tool-result images and documents.
#[test]
fn the_request_decoder_survives_arbitrary_tools_and_tool_results() {
    let mut rng = Rng(0x2026_0921);
    for case in 0..5_000u32 {
        let text = format!(
            concat!(
                "{{\"model\":\"m\",\"messages\":[",
                "{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"t\",\"content\":{}}}]}},",
                "{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"u\",\"content\":[{{\"type\":\"tool_reference\",\"tool_name\":{}}}]}}]}}",
                "],\"tools\":[{{\"name\":{},\"description\":{},\"input_schema\":{},\"defer_loading\":{}}},{}]}}"
            ),
            value(&mut rng, 3),
            rng.pick(&ATOMS),
            rng.pick(&ATOMS),
            value(&mut rng, 2),
            value(&mut rng, 3),
            rng.pick(&ATOMS),
            value(&mut rng, 3),
        );
        let bytes = text.as_bytes();
        let _ = anthropic::decode(bytes);
        let _ = anthropic::summarize(bytes);
        let _ = translate::request(bytes, "m", Default::default());
        let _ = case;
    }
}

#[test]
fn decoders_survive_arbitrary_documents() {
    let mut rng = Rng(0x2026_0921);
    for case in 0..20_000u32 {
        let text = value(&mut rng, 4);
        let bytes = text.as_bytes();
        let _ = anthropic::peek(bytes);
        let _ = anthropic::rewrite(bytes, Some("m"), &["a".into(), "a.b".into()], true);
        let _ = anthropic::rewrite(bytes, None, &[], false);
        let _ = anthropic::summarize(bytes);
        let _ = anthropic::decode(bytes);
        let _ = anthropic::decode_models(bytes);
        let _ = openai::decode_response(bytes);
        let _ = openai::decode_models(bytes);
        let _ = openai::decode_error(Some(500), bytes);
        let _ = translate::request(bytes, "m", Default::default());
        let _ = translate::response(bytes, "m");
        let _ = translate::document_events(bytes, "m");
        let _ = translate::models(anthroxy::config::BackendKind::OpenAi, bytes);
        let _ = translate::failure(anthroxy::config::BackendKind::OpenAi, Some(400), bytes);
        let mut decoder = openai::ChunkDecoder::new();
        let _ = decoder.decode(&text);
        let _ = decoder.finish();
        let mut parser = sse::Parser::new();
        let _ = parser.feed(format!("data: {text}\n\n").as_bytes());
        let _ = parser.finish();
        assert!(case < 20_000);
    }
}

#[test]
fn a_stream_of_arbitrary_frames_translates() {
    let mut rng = Rng(0x1234_5678_9abc_def0);
    for _ in 0..3_000u32 {
        let mut decoder = openai::ChunkDecoder::new();
        let mut encoder = anthropic::StreamEncoder::new("fallback");
        let mut out = String::new();
        for _ in 0..rng.upto(8) + 1 {
            let text = value(&mut rng, 3);
            if let Ok(events) = decoder.decode(&text) {
                encoder.encode_all(events, &mut out);
            }
        }
        encoder.encode_all(decoder.finish(), &mut out);
        encoder.finish(&mut out);
    }
}

#[test]
fn the_sse_parser_survives_arbitrary_bytes() {
    let mut rng = Rng(0xfeed_face);
    let alphabet: Vec<u8> = b"data:\r\n {}\"\\:,[]\xff\xfe abc".to_vec();
    for _ in 0..8_000u32 {
        let mut parser = sse::Parser::new();
        for _ in 0..rng.upto(6) + 1 {
            let len = rng.upto(40);
            let chunk: Vec<u8> = (0..len).map(|_| *rng.pick(&alphabet)).collect();
            let _ = parser.feed(&chunk);
        }
        let _ = parser.finish();
    }
}

const TOML_LINES: [&str; 26] = [
    "[server]",
    "token = \"${T}\"",
    "token = \"\"",
    "listen = \"${HOST}:${PORT}\"",
    "listen = \"1.2.3.4:99999\"",
    "max_body_bytes = 0",
    "v1_auth = \"none\"",
    "[upstream]",
    "retries = 4294967295",
    "retry_backoff = \"${D}\"",
    "retry_backoff = \"1y\"",
    "connect_timeout = \"0s\"",
    "retry_on_status = [0, 999]",
    "[backends.a]",
    "url = \"${URL}\"",
    "url = \"\"",
    "url = \"http://u:p@h/x?y#z\"",
    "proxy = \"${P}\"",
    "headers = { \"x\" = \"${V}\" }",
    "drop_headers = [\"*\", \"\", \"a*b*c\"]",
    "drop_fields = [\"\", \".\", \"a..b\", \"model\"]",
    "[[models]]",
    "id = \"${M}\"",
    "backend = \"a\"",
    "aliases = [\"\", \"${A}\"]",
    "[logging]",
];

#[test]
fn configuration_parsing_survives_arbitrary_files() {
    let mut rng = Rng(0x00c0_ffee);
    let env = |name: &str| match name {
        "T" => Some("t".to_owned()),
        "D" => Some("${D}".to_owned()),
        "URL" => Some("http://${URL}".to_owned()),
        "P" => Some(String::new()),
        _ => None,
    };
    for _ in 0..8_000u32 {
        let lines: Vec<&str> = (0..rng.upto(12) + 1)
            .map(|_| *rng.pick(&TOML_LINES))
            .collect();
        let text = lines.join("\n");
        let _ = anthroxy::config::Config::parse(&text, env);
    }
}
