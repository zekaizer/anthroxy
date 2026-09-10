//! The commented configuration written by `anthroxy init`. Valid as-is.

pub const EXAMPLE: &str = r##"# anthroxy configuration
#
# One endpoint for Claude Code in front of several Anthropic-compatible
# backends. Point Claude Code at this router (see `anthroxy env`) and pick
# any model below with /model — switching takes effect on the next request.
#
# `${NAME}` is replaced with the environment variable NAME when the file is
# loaded. Write `$${NAME}` to keep a literal `${NAME}` for a shell command.

[server]
# Listen on every interface so Claude Code on the Windows host can reach a
# router running inside WSL2. Use "127.0.0.1:8787" to stay local.
listen = "0.0.0.0:8787"
# The one token Claude Code presents (as ANTHROPIC_AUTH_TOKEN). Generated at
# init; replace it with anything you like.
token = "__TOKEN__"
# Largest accepted request body. Claude Code sends whole conversations.
max_body_bytes = "64MiB"

[logging]
# Log filter: error | warn | info | debug | trace, or a tracing directive such
# as "anthroxy=debug,info". `--log-level` and RUST_LOG override this.
level = "info"
# "text" for humans, "json" for log shippers.
format = "text"
# Uncomment to write every request/response body plus routing metadata to
# disk, one directory per request. Handy when a backend misbehaves.
# body_dir = "~/.local/state/anthroxy/bodies"
# Recorded exchanges older than this are deleted (checked every 10 minutes).
# "0s" keeps everything.
body_retention = "7d"

[upstream]
# Connection establishment limit per attempt.
connect_timeout = "10s"
# Longest silence tolerated while waiting for the next response chunk. Local
# models can take minutes to process a large prompt before the first token.
read_timeout = "5m"
# Extra attempts after a connection failure (never after a timeout, never once
# the response has started).
retries = 2
retry_backoff = "200ms"
# Upstream status codes retried like a connection failure, e.g. [502, 503].
retry_on_status = []
# HTTPS backends are verified against the built-in Mozilla roots plus the OS
# certificate store (or SSL_CERT_FILE when set). Uncomment to trust a private
# CA from a PEM file as well, e.g. behind a corporate TLS proxy.
# ca_certificate = "~/.config/anthroxy/corp-root.pem"

# ---------------------------------------------------------------------------
# Backends: one table per server. `url` is the origin; the request path
# (/v1/messages) is appended unchanged. `kind` is "anthropic" (default) for
# a server that speaks the Messages API, "openai" for one that speaks Chat
# Completions.
# ---------------------------------------------------------------------------

[backends.local]
url = "http://127.0.0.1:1234"
# credential kinds:
#   { kind = "none" }                                   (default)
#   { kind = "static",  value = "sk-...", header = "x_api_key" }
#   { kind = "env",     name = "VLLM_API_KEY" }
#   { kind = "command", command = "cat ~/.token", refresh = "5m", timeout = "10s" }
# `header` is "bearer" (Authorization: Bearer ...), "x_api_key", or any header
# as { name = "api-key" } / { name = "authorization", scheme = "Token" }.
# A command may print `{"token": "...", "expires_at": ...}` instead, with
# output = "json"; it is then also re-run two minutes before `expires_at`.
credential = { kind = "none" }

# [backends.vllm]
# url = "http://10.0.0.5:8000"
# credential = { kind = "static", value = "${VLLM_API_KEY}" }
# # Request body fields this backend rejects as unknown, removed before
# # forwarding. Dot-separated paths reach into objects (not arrays).
# drop_fields = ["context_management", "metadata.user_id"]

# A server that speaks the OpenAI Chat Completions API. Requests are
# translated to /v1/chat/completions and answers back to Messages events;
# `count_tokens` is answered 404 by the router. `anthropic_beta` does not
# apply here.
# [backends.inhouse]
# kind = "openai"
# url = "https://llm.example.corp"
# credential = { kind = "static", value = "${INHOUSE_API_KEY}" }

# A backend whose token another program keeps fresh; the command is re-run
# every `refresh`, and once more immediately if the backend answers 401.
# [backends.claude]
# url = "https://api.anthropic.com"
# credential = { kind = "command", command = "cat ~/.claude-oauth-token", refresh = "5m" }
# # Beta flags merged into the client's anthropic-beta header.
# anthropic_beta = ["oauth-2025-04-20"]
# # Headers forced on every request to this backend.
# [backends.claude.headers]
# "anthropic-version" = "2023-06-01"

# ---------------------------------------------------------------------------
# Models: what Claude Code sees, in picker order.
#   id             name shown to and sent by Claude Code
#   backend        which [backends.*] serves it
#   upstream_model what that backend calls it (defaults to id)
#   display_name   label in the /model picker (defaults to id)
#   aliases        extra ids routed here, e.g. Claude Code's built-in model
#                  names so its background requests land on your backend
# ---------------------------------------------------------------------------

[[models]]
id = "local-default"
backend = "local"
upstream_model = "gemma-4-e2b-it-qat"
display_name = "Local (LM Studio)"
aliases = ["claude-haiku-4-5", "claude-haiku-4-5-20251001"]

# [[models]]
# id = "qwen"
# backend = "vllm"
# upstream_model = "Qwen/Qwen3.5-32B"

[routing]
# Where requests for an unlisted model go. Remove to reject them with 404.
default_model = "local-default"
"##;

/// The example with a freshly generated token in place.
pub fn render(token: &str) -> String {
    EXAMPLE.replace("__TOKEN__", token)
}

/// 32 URL-safe random characters.
pub fn generate_token() -> String {
    use rand::RngExt;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    (0..32)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn example_is_a_valid_configuration() {
        let text = render("test-token");
        let config = Config::parse(&text, |_| None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(config.server.token, "test-token");
        assert_eq!(config.models.len(), 1);
        assert_eq!(
            config.routing.default_model.as_deref(),
            Some("local-default")
        );
    }

    #[test]
    fn generated_tokens_are_long_and_distinct() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
