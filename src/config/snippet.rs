//! Fragments of a configuration file for the operator to paste: keys and
//! values quoted the way TOML reads them back.

/// A table key: bare when TOML allows it, a quoted string otherwise.
pub fn key(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare { name.to_owned() } else { string(name) }
}

/// A basic string with every character TOML needs escaped.
pub fn string(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

/// `["a", "b"]`.
pub fn string_array<S: AsRef<str>>(values: &[S]) -> String {
    let items: Vec<String> = values.iter().map(|v| string(v.as_ref())).collect();
    format!("[{}]", items.join(", "))
}

/// A `[[models]]` entry serving `upstream_model` from `backend`, with the
/// last path segment of the upstream name as its id.
pub fn model_block(backend: &str, upstream_model: &str) -> String {
    let id = upstream_model.rsplit('/').next().unwrap_or(upstream_model);
    format!(
        "[[models]]\nid = {}\nbackend = {}\nupstream_model = {}\n",
        string(id),
        string(backend),
        string(upstream_model)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_block_parses_back() {
        let block = model_block("vllm", "Qwen/Qwen3.5-32B");
        let value: toml::Value = toml::from_str(&block).unwrap();
        let model = &value["models"].as_array().unwrap()[0];
        assert_eq!(model["id"].as_str(), Some("Qwen3.5-32B"));
        assert_eq!(model["backend"].as_str(), Some("vllm"));
        assert_eq!(model["upstream_model"].as_str(), Some("Qwen/Qwen3.5-32B"));
    }

    #[test]
    fn keys_are_bare_only_when_toml_allows() {
        assert_eq!(key("vllm-2_b"), "vllm-2_b");
        assert_eq!(key("my.backend"), "\"my.backend\"");
        assert_eq!(key(""), "\"\"");
    }

    #[test]
    fn snippets_parse_back_to_the_same_values() {
        let tricky = "a\"b\\c\nd'é";
        let text = format!(
            "[backends.{}]\nurl = {}\ndrop_fields = {}\n",
            key("my.backend"),
            string(tricky),
            string_array(&["x", tricky])
        );
        let value: toml::Value = toml::from_str(&text).unwrap();
        let backend = &value["backends"]["my.backend"];
        assert_eq!(backend["url"].as_str(), Some(tricky));
        assert_eq!(
            backend["drop_fields"].as_array().unwrap()[1].as_str(),
            Some(tricky)
        );
    }
}
