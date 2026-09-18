//! The environment variables that point Claude Code at the router, rendered
//! for a POSIX shell, PowerShell or `settings.json`.

use std::net::Ipv6Addr;

use super::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum EnvFormat {
    /// `export NAME=value` lines for bash/zsh
    Sh,
    /// `$env:NAME = "value"` lines for PowerShell on the Windows host
    Powershell,
    /// An `"env"` object to paste into ~/.claude/settings.json
    Json,
}

/// The host clients use: `explicit` when given, else the address
/// `server.listen` binds, `localhost` when it binds every interface.
pub fn host(config: &Config, explicit: Option<&str>) -> String {
    explicit
        .map(bracket_ipv6)
        .unwrap_or_else(|| match config.server.listen.ip() {
            ip if ip.is_unspecified() => "localhost".to_owned(),
            ip => bracket_ipv6(&ip.to_string()),
        })
}

/// `http://<host>:<listen port>`.
pub fn base_url(config: &Config, host: &str) -> String {
    format!("http://{host}:{}", config.server.listen.port())
}

/// The variables, in output order.
pub fn vars(config: &Config, base_url: &str) -> Vec<(&'static str, String)> {
    let default_model = config
        .routing
        .default_model
        .clone()
        .or_else(|| config.models.first().map(|m| m.id.clone()))
        .unwrap_or_default();
    let mut vars = vec![("ANTHROPIC_BASE_URL", base_url.to_owned())];
    if config.server.v1_auth != super::V1Auth::None {
        vars.push(("ANTHROPIC_AUTH_TOKEN", config.server.token.clone()));
    }
    vars.push(("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY", "1".to_owned()));
    vars.push(("ANTHROPIC_MODEL", default_model));
    vars
}

/// `wsl_localhost` adds the note that the Windows host needs the
/// distribution's address; only the shell format carries notes.
pub fn render(vars: &[(&str, String)], format: EnvFormat, wsl_localhost: bool) -> String {
    let mut out = String::new();
    match format {
        EnvFormat::Sh => {
            out.push_str("# Point Claude Code at anthroxy (paste into your shell or profile)\n");
            for (name, value) in vars {
                out.push_str(&format!("export {name}={}\n", sh_quote(value)));
            }
            out.push_str(
                "# ANTHROPIC_MODEL is the model Claude Code starts with; /model switches later.\n",
            );
            if wsl_localhost {
                out.push_str("# From the Windows host: use this distribution's address instead of localhost\n");
                out.push_str("#   (hostname -I) unless WSL networking is set to mirrored.\n");
            }
        }
        EnvFormat::Powershell => {
            out.push_str("# Point Claude Code at anthroxy (PowerShell)\n");
            for (name, value) in vars {
                out.push_str(&format!("$env:{name} = {}\n", powershell_quote(value)));
            }
        }
        EnvFormat::Json => {
            let map: serde_json::Map<String, serde_json::Value> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.clone())))
                .collect();
            out.push_str(
                &serde_json::to_string_pretty(&serde_json::json!({ "env": map }))
                    .expect("serializes"),
            );
            out.push('\n');
        }
    }
    out
}

/// Single quotes make every byte literal in a POSIX shell; only the quote
/// itself has to leave and re-enter them.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// PowerShell single quotes are literal too, with a doubled quote for one.
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A bare IPv6 literal only reads as a host once it is bracketed; anything
/// else, brackets included, is already what the user meant.
fn bracket_ipv6(host: &str) -> String {
    match host.parse::<Ipv6Addr>() {
        Ok(v6) => format!("[{v6}]"),
        Err(_) => host.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(listen: &str) -> Config {
        let text = format!(
            "[server]\nlisten = \"{listen}\"\ntoken = \"tok\"\n[backends.a]\nurl = \"http://a\"\n[[models]]\nid = \"m1\"\nbackend = \"a\"\n[[models]]\nid = \"m2\"\nbackend = \"a\"\n[routing]\ndefault_model = \"m2\"\n"
        );
        Config::parse(&text, |_| None).unwrap()
    }

    fn render_for(config: &Config, explicit: Option<&str>, format: EnvFormat) -> String {
        let host = host(config, explicit);
        render(&vars(config, &base_url(config, &host)), format, false)
    }

    #[test]
    fn sh_output_uses_localhost_for_unspecified_bind() {
        let out = render_for(&config("0.0.0.0:8787"), None, EnvFormat::Sh);
        assert!(
            out.contains("export ANTHROPIC_BASE_URL='http://localhost:8787'\n"),
            "{out}"
        );
        assert!(out.contains("export ANTHROPIC_AUTH_TOKEN='tok'\n"));
        assert!(out.contains("export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY='1'\n"));
        assert!(
            out.contains("export ANTHROPIC_MODEL='m2'\n"),
            "default model wins"
        );
    }

    #[test]
    fn v1_auth_none_omits_the_auth_token() {
        let text = "[server]\nlisten = \"127.0.0.1:8787\"\ntoken = \"tok\"\nv1_auth = \"none\"\n[backends.a]\nurl = \"http://a\"\n[[models]]\nid = \"m1\"\nbackend = \"a\"\n";
        let config = Config::parse(text, |_| None).unwrap();
        let out = render_for(&config, None, EnvFormat::Sh);
        assert!(
            out.contains("export ANTHROPIC_BASE_URL='http://127.0.0.1:8787'\n"),
            "{out}"
        );
        assert!(!out.contains("ANTHROPIC_AUTH_TOKEN"), "{out}");
        assert!(out.contains("export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY='1'\n"));
        assert!(out.contains("export ANTHROPIC_MODEL='m1'\n"), "{out}");
    }

    #[test]
    fn values_survive_the_shell_and_powershell_syntax() {
        let text = "[server]\nlisten = \"127.0.0.1:1\"\ntoken = \"a\\\"b$c'd\"\n[backends.a]\nurl = \"http://a\"\n[[models]]\nid = \"m1\"\nbackend = \"a\"\n";
        let config = Config::parse(text, |_| None).unwrap();

        let out = render_for(&config, None, EnvFormat::Sh);
        let line = out
            .lines()
            .find(|l| l.starts_with("export ANTHROPIC_AUTH_TOKEN="))
            .unwrap();
        let echoed = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{line}; printf %s \"$ANTHROPIC_AUTH_TOKEN\""))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&echoed.stdout),
            config.server.token,
            "{line}"
        );

        let out = render_for(&config, None, EnvFormat::Powershell);
        assert!(
            out.contains("$env:ANTHROPIC_AUTH_TOKEN = 'a\"b$c''d'"),
            "{out}"
        );
    }

    #[test]
    fn an_ipv6_host_is_bracketed_wherever_it_came_from() {
        let out = render_for(&config("[::1]:8787"), None, EnvFormat::Sh);
        assert!(
            out.contains("export ANTHROPIC_BASE_URL='http://[::1]:8787'"),
            "{out}"
        );
        let out = render_for(&config("0.0.0.0:8787"), Some("::1"), EnvFormat::Sh);
        assert!(
            out.contains("export ANTHROPIC_BASE_URL='http://[::1]:8787'"),
            "{out}"
        );
        let out = render_for(&config("0.0.0.0:8787"), Some("[::1]"), EnvFormat::Sh);
        assert!(
            out.contains("export ANTHROPIC_BASE_URL='http://[::1]:8787'"),
            "{out}"
        );
    }

    #[test]
    fn explicit_host_and_bound_ip_are_used() {
        let out = render_for(&config("192.168.1.5:9000"), None, EnvFormat::Powershell);
        assert!(
            out.contains("$env:ANTHROPIC_BASE_URL = 'http://192.168.1.5:9000'\n"),
            "{out}"
        );
        let out = render_for(&config("0.0.0.0:8787"), Some("wsl.local"), EnvFormat::Json);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["env"]["ANTHROPIC_BASE_URL"], "http://wsl.local:8787");
        assert_eq!(value["env"]["ANTHROPIC_MODEL"], "m2");
    }

    #[test]
    fn the_windows_host_note_is_for_the_shell_only() {
        let config = config("0.0.0.0:8787");
        let vars = vars(&config, "http://localhost:8787");
        assert!(render(&vars, EnvFormat::Sh, true).contains("From the Windows host"));
        assert!(!render(&vars, EnvFormat::Sh, false).contains("From the Windows host"));
        assert!(!render(&vars, EnvFormat::Powershell, true).contains("From the Windows host"));
    }
}
