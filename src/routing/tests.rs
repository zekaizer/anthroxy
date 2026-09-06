use super::*;
use crate::config::Config;

fn registry(models: &str, routing: &str) -> Registry {
    let text = format!(
        "[server]\ntoken = \"t\"\n[backends.a]\nurl = \"http://a\"\n[backends.b]\nurl = \"http://b\"\n{models}\n{routing}"
    );
    Registry::from_config(&Config::parse(&text, |_| None).unwrap())
}

const TWO_MODELS: &str = r#"
[[models]]
id = "fast"
backend = "a"
upstream_model = "qwen3.5-4b"
aliases = ["claude-haiku-4-5", "claude-haiku-4-5-20251001"]

[[models]]
id = "smart"
backend = "b"
display_name = "Smart One"
"#;

#[test]
fn routes_keep_configuration_order_and_fill_defaults() {
    let r = registry(TWO_MODELS, "");
    let routes = r.routes();
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[0].id, "fast");
    assert_eq!(routes[0].upstream_model, "qwen3.5-4b");
    assert_eq!(
        routes[0].display_name, "fast",
        "display_name falls back to id"
    );
    assert_eq!(
        routes[1].upstream_model, "smart",
        "upstream_model falls back to id"
    );
    assert_eq!(routes[1].display_name, "Smart One");
}

#[test]
fn resolves_exact_and_alias_names() {
    let r = registry(TWO_MODELS, "");
    let exact = r.resolve("fast").unwrap();
    assert_eq!(exact.matched, Match::Exact);
    assert_eq!(exact.route.backend, "a");

    let alias = r.resolve("claude-haiku-4-5-20251001").unwrap();
    assert_eq!(alias.matched, Match::Alias);
    assert_eq!(alias.route.id, "fast");
}

#[test]
fn unknown_model_without_default_is_none() {
    let r = registry(TWO_MODELS, "");
    assert!(r.resolve("claude-opus-5").is_none());
    assert!(r.default_route().is_none());
}

#[test]
fn unknown_model_falls_back_to_default() {
    let r = registry(
        TWO_MODELS,
        "[routing]\ndefault_model = \"claude-haiku-4-5\"",
    );
    let res = r.resolve("claude-opus-5").unwrap();
    assert_eq!(res.matched, Match::Default);
    assert_eq!(res.route.id, "fast");
    assert_eq!(r.default_route().unwrap().id, "fast");
    // A known name still resolves directly, never through the default.
    assert_eq!(r.resolve("smart").unwrap().matched, Match::Exact);
}

#[test]
fn known_names_lists_ids_and_aliases_in_order() {
    let r = registry(TWO_MODELS, "");
    assert_eq!(
        r.known_names(),
        vec![
            "fast",
            "claude-haiku-4-5",
            "claude-haiku-4-5-20251001",
            "smart"
        ]
    );
}
