use super::*;

fn problems(text: &str) -> Vec<String> {
    Preset::parse("client", text).expect_err("a preset with problems")
}

#[test]
fn an_empty_file_is_a_preset_that_changes_nothing() {
    let preset = Preset::parse("client", "").unwrap();
    assert_eq!(preset.name, "client");
    assert!(preset.variables.is_empty());
    assert!(preset.headers.is_empty());
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
