use super::*;
use crate::openai::{MaxTokensField, SystemPlacement, ToolChoiceDefault, ToolOnlyContent};

fn problems(text: &str) -> Vec<String> {
    Preset::parse("client", text).expect_err("a preset with problems")
}

#[test]
fn an_empty_file_is_a_preset_that_changes_nothing() {
    let preset = Preset::parse("client", "").unwrap();
    assert_eq!(preset.name, "client");
    assert!(preset.variables.is_empty());
    assert!(preset.headers.is_empty());
    assert!(preset.drop_headers.is_empty());
    assert!(preset.set_fields.bind(&preset.variables).is_empty());
    assert!(preset.omit_fields.is_empty());
    assert!(preset.field_order.is_empty());
}

#[test]
fn every_problem_of_a_file_is_listed_with_its_key_path() {
    let text = r#"
tool_choice = "auto"

[variables]
version = "1.19.6"
bad-name = "x"
preset = "p"
count = 1
line = "a\nb"

[headers]
user-agent = "opencode/{versoin} {versoin} {os}"
"x name" = "1"
content-length = "3"
content-type = "{header:x-type}"
x-open = "{header:x-a"
x-count = 3
x-line = "{line}"
"#;
    assert_eq!(
        problems(text),
        [
            "variables.bad-name: a name is letters, digits and `_`",
            "variables.count: the default must be a string",
            "variables.preset: `preset` names the preset in `emulate` and cannot be a variable",
            "headers: `x-count`: the value must be a string",
            "headers: `content-length` describes the connection the router makes and cannot be set here",
            "headers: `content-type` says what the body is; a value read from the client's request may be left out",
            "headers: `x name` is not a valid header name",
            "headers: `x-open`: `{header:` is not closed by `}`",
            "headers: `user-agent`: `{versoin}` names no variable in [variables]",
            "headers: `user-agent`: `{os}` names no variable in [variables]",
            "variables.line: the default cannot be sent in a header",
            "`tool_choice` is not a preset key",
        ]
    );
}

#[test]
fn a_default_is_a_header_value_only_where_a_header_sends_it() {
    let unsent = "[variables]\nnote = \"a\\nb\"\n";
    assert_eq!(
        Preset::parse("client", unsent).map(|preset| preset.variables),
        Ok([("note".to_owned(), "a\nb".to_owned())].into())
    );
    assert_eq!(
        problems(&format!("{unsent}\n[headers]\nx-note = \"{{note}}\"\n")),
        ["variables.note: the default cannot be sent in a header"]
    );
}

#[test]
fn a_value_that_names_a_declared_variable_has_no_problem_of_its_own() {
    let text = r#"
[variables]
count = 1
preset = "p"

[headers]
x-a = "{count} {preset}"
"#;
    assert_eq!(
        problems(text),
        [
            "variables.count: the default must be a string",
            "variables.preset: `preset` names the preset in `emulate` and cannot be a variable",
        ]
    );
}

#[test]
fn a_header_with_a_problem_of_its_own_still_answers_for_the_variables_it_names() {
    let text = r#"
[variables]
note = "a\nb"

[headers]
"x name" = "{gone}"
x-open = "{nope} {note} {header:x session}"
"#;
    assert_eq!(
        problems(text),
        [
            "headers: `x name` is not a valid header name",
            "headers: `x-open`: a `{header:…}` placeholder does not name a header",
            "headers: `x name`: `{gone}` names no variable in [variables]",
            "headers: `x-open`: `{nope}` names no variable in [variables]",
            "variables.note: the default cannot be sent in a header",
        ]
    );
}

#[test]
fn a_key_of_the_wrong_shape_is_a_problem_of_that_key() {
    assert_eq!(
        problems("variables = \"x\"\nheaders = [\"a\"]\n"),
        [
            "variables: must be a table of names and default values",
            "headers: must be a table of names and values",
        ]
    );
}

#[test]
fn a_file_that_is_not_toml_is_one_problem_on_one_line() {
    let found = problems("[headers]\nuser-agent = \"a\"\nuser-agent = \n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("line 3: "), "{found:?}");
    assert!(!found[0].contains('\n'), "{found:?}");
}

#[test]
fn a_header_is_named_once_whatever_its_case() {
    assert_eq!(
        problems("[headers]\nUser-Agent = \"a\"\nuser-agent = \"b\"\n"),
        ["headers: `User-Agent` and `user-agent` are the same header"]
    );
}

#[test]
fn a_preset_shown_for_debugging_names_its_headers_and_variables_only() {
    let text = "[variables]\ntenant = \"default-tenant\"\n\n[headers]\nx-client-api-key = \"llm_secret-value {tenant}\"\n";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("client.toml"), text).unwrap();
    let library = PresetLibrary::with_dir(dir.path());
    assert_eq!(library.problems(), [] as [&str; 0]);

    let shown = format!("{library:?}");
    for secret in ["llm_secret-value", "default-tenant"] {
        assert!(!shown.contains(secret), "`{secret}` in {shown}");
    }
    for name in ["x-client-api-key", "client", "tenant"] {
        assert!(shown.contains(name), "`{name}` not in {shown}");
    }
}

#[test]
fn a_placeholder_cannot_read_the_clients_credential() {
    assert_eq!(
        problems("[headers]\nx-key = \"{header:authorization}\"\n"),
        [
            "headers: `x-key`: `{header:authorization}` would send the client's credential to the backend"
        ]
    );
}

#[test]
fn drop_headers_obeys_the_rules_of_the_backend_key() {
    let text = r#"
drop_headers = ["x-stainless-*", "@claude-code", "@other", "USER-agent", "content-*", "a b", "host"]

[headers]
user-agent = "client/1"
"#;
    assert_eq!(
        problems(text),
        [
            "drop_headers: `@other` is not a preset; the only one is `@claude-code`",
            "drop_headers: `USER-agent` is set in `headers` for this preset; a header is forced or dropped, not both",
            "drop_headers: `content-*` never reaches a backend anyway (`content-length`)",
            "drop_headers: `content-*` says what the body is (`content-type`)",
            "drop_headers: `a b` is not a header name or a `*` pattern",
            "drop_headers: `host` never reaches a backend anyway",
        ]
    );
    assert_eq!(
        problems("drop_headers = \"x-app\"\n"),
        ["drop_headers: must be a list of strings"]
    );
    assert_eq!(
        problems("drop_headers = [1, \"host\", {}]\n"),
        [
            "drop_headers: entry 1 must be a string",
            "drop_headers: entry 3 must be a string",
            "drop_headers: `host` never reaches a backend anyway",
        ],
        "an entry of the wrong type does not hide the problem of another"
    );
}

#[test]
fn set_fields_cannot_reach_what_a_preset_leaves_alone() {
    let text = r#"
[variables]
version = "1"

[set_fields]
model = "m"
"stream.x" = true
messages = []
"a..b" = 1
ratio = nan
"extra.agent" = "c/{versoin} {header:x-app"
"extra.note" = ["{os}", { deep = "{os} {version}" }]
user = "u"
"user.id" = "u"
"#;
    assert_eq!(
        problems(text),
        [
            "set_fields: `a..b`: the path has an empty segment",
            "set_fields: `extra.agent`: `{header:` is not closed by `}`",
            "set_fields: `messages`: `messages` is not a preset's to change",
            "set_fields: `model`: `model` is not a preset's to change",
            "set_fields: `ratio`: a number that is not finite has no JSON form",
            "set_fields: `stream.x`: `stream` is not a preset's to change",
            "set_fields: `user` and `user.id` set the same field",
            "set_fields: `extra.note`: `{os}` names no variable in [variables]",
        ]
    );
    assert_eq!(
        problems("set_fields = 1\n[variables]\nversion = \"1\"\n"),
        ["set_fields: must be a table of body fields and values"]
    );
    assert_eq!(
        problems("[set_fields]\n\"extra.note\" = [\"{os}\", { deep = \"{os} {version}\" }]\n"),
        [
            "set_fields: `extra.note`: `{os}` names no variable in [variables]",
            "set_fields: `extra.note`: `{version}` names no variable in [variables]",
        ]
    );
}

#[test]
fn mid_conversation_system_takes_the_values_of_the_backend_key() {
    for (written, placement) in [
        ("keep", SystemPlacement::Keep),
        ("merge", SystemPlacement::Merge),
        ("user", SystemPlacement::User),
    ] {
        let text = format!("mid_conversation_system = \"{written}\"\n");
        let preset = Preset::parse("client", &text).unwrap();
        assert_eq!(preset.mid_conversation_system, Some(placement));
    }
    assert_eq!(
        Preset::parse("client", "").unwrap().mid_conversation_system,
        None
    );
    assert_eq!(
        problems("mid_conversation_system = \"first\"\n"),
        ["mid_conversation_system: `first` is not one of `keep`, `merge`, `user`"]
    );
    assert_eq!(
        problems("mid_conversation_system = 1\n"),
        ["mid_conversation_system: must be a string"]
    );
}

#[test]
fn omit_fields_names_paths_a_preset_may_change() {
    let preset = Preset::parse(
        "client",
        "omit_fields = [\"user\", \"stream_options.include_usage\"]\n",
    )
    .unwrap();
    assert_eq!(preset.omit_fields, ["user", "stream_options.include_usage"]);
    assert_eq!(
        problems(
            "omit_fields = [\"model\", \"messages.0\", \"stream\", \"a..b\", \"\", \"user\"]\n"
        ),
        [
            "omit_fields: `model`: `model` is not a preset's to change",
            "omit_fields: `messages.0`: `messages` is not a preset's to change",
            "omit_fields: `stream`: `stream` is not a preset's to change",
            "omit_fields: `a..b`: the path has an empty segment",
            "omit_fields: ``: the path has an empty segment",
        ]
    );
    assert_eq!(
        problems("omit_fields = [\"model\", true]\n"),
        [
            "omit_fields: entry 2 must be a string",
            "omit_fields: `model`: `model` is not a preset's to change",
        ]
    );
    assert_eq!(
        problems("omit_fields = { user = true }\n"),
        ["omit_fields: must be a list of strings"]
    );
}

#[test]
fn field_order_lists_top_level_fields_once() {
    let preset = Preset::parse(
        "client",
        "field_order = [\"model\", \"max_tokens\", \"messages\", \"stream\"]\n",
    )
    .unwrap();
    assert_eq!(
        preset.field_order,
        ["model", "max_tokens", "messages", "stream"],
        "the fields a preset cannot change can still be placed"
    );
    assert_eq!(
        problems("field_order = [\"model\", \"extra.a\", \"\", \"model\"]\n"),
        [
            "field_order: `extra.a` is not a top-level field; only those are ordered",
            "field_order: a name must not be empty",
            "field_order: `model` is listed twice",
        ]
    );
    assert_eq!(
        problems("field_order = [\"extra.a\", 3]\n"),
        [
            "field_order: entry 2 must be a string",
            "field_order: `extra.a` is not a top-level field; only those are ordered",
        ]
    );
    assert_eq!(
        problems("field_order = \"model\"\n"),
        ["field_order: must be a list of strings"]
    );
}

#[test]
fn max_tokens_field_is_one_of_two_names() {
    for (written, field) in [
        ("max_tokens", MaxTokensField::MaxTokens),
        ("max_completion_tokens", MaxTokensField::MaxCompletionTokens),
    ] {
        let text = format!("max_tokens_field = \"{written}\"\n");
        assert_eq!(
            Preset::parse("client", &text).unwrap().max_tokens_field,
            field
        );
    }
    assert_eq!(
        Preset::parse("client", "").unwrap().max_tokens_field,
        MaxTokensField::MaxTokens
    );
    assert_eq!(
        problems("max_tokens_field = \"max_output_tokens\"\n"),
        ["max_tokens_field: `max_output_tokens` is not `max_tokens` or `max_completion_tokens`"]
    );
}

#[test]
fn a_long_value_does_not_push_the_allowed_ones_out_of_the_message() {
    let text = format!("max_tokens_field = \"{}\"\n", "x".repeat(300));
    let found = problems(&text);
    assert!(
        found[0].ends_with("…` is not `max_tokens` or `max_completion_tokens`"),
        "{found:?}"
    );
}

#[test]
fn default_tool_choice_is_one_of_three_words() {
    for (written, choice) in [
        ("auto", ToolChoiceDefault::Auto),
        ("required", ToolChoiceDefault::Required),
        ("none", ToolChoiceDefault::None),
    ] {
        let text = format!("default_tool_choice = \"{written}\"\n");
        assert_eq!(
            Preset::parse("client", &text).unwrap().default_tool_choice,
            Some(choice)
        );
    }
    assert_eq!(
        Preset::parse("client", "").unwrap().default_tool_choice,
        None
    );
    assert_eq!(
        problems("default_tool_choice = \"any\"\n"),
        ["default_tool_choice: `any` is not one of `auto`, `required`, `none`"]
    );
    assert_eq!(
        problems("default_tool_choice = { type = \"auto\" }\n"),
        ["default_tool_choice: must be a string"]
    );
}

#[test]
fn tool_only_content_is_one_of_three_words() {
    for (written, content) in [
        ("omitted", ToolOnlyContent::Omitted),
        ("empty", ToolOnlyContent::Empty),
        ("null", ToolOnlyContent::Null),
    ] {
        let text = format!("tool_only_content = \"{written}\"\n");
        assert_eq!(
            Preset::parse("client", &text).unwrap().tool_only_content,
            content
        );
    }
    assert_eq!(
        Preset::parse("client", "").unwrap().tool_only_content,
        ToolOnlyContent::Omitted
    );
    assert_eq!(
        problems("tool_only_content = \"\"\n"),
        ["tool_only_content: `` is not one of `omitted`, `empty`, `null`"]
    );
}

#[test]
fn tool_strict_is_a_boolean() {
    let strict = |text: &str| Preset::parse("client", text).unwrap().tool_strict;
    assert_eq!(strict("tool_strict = true\n"), Some(true));
    assert_eq!(strict("tool_strict = false\n"), Some(false));
    assert_eq!(strict(""), None);
    assert_eq!(
        problems("tool_strict = \"false\"\n"),
        ["tool_strict: must be `true` or `false`"]
    );
}

#[test]
fn reasoning_field_names_a_field_of_its_own() {
    let field = |text: &str| Preset::parse("client", text).unwrap().reasoning_field;
    assert_eq!(
        field("reasoning_field = \"reasoning_content\"\n").as_deref(),
        Some("reasoning_content")
    );
    assert_eq!(field(""), None);
    for taken in ["role", "content", "tool_calls"] {
        assert_eq!(
            problems(&format!("reasoning_field = \"{taken}\"\n")),
            [format!(
                "reasoning_field: `{taken}` is a field every assistant message has"
            )]
        );
    }
    assert_eq!(
        problems("reasoning_field = \"\"\n"),
        ["reasoning_field: must not be empty"]
    );
    assert_eq!(
        problems("reasoning_field = true\n"),
        ["reasoning_field: must be a string"]
    );
}
