use std::net::IpAddr;

use clap::{Args, ValueEnum};

use super::{Cli, is_wsl};
use crate::config::Config;

#[derive(Debug, Clone, Args)]
pub struct EnvArgs {
    /// Host name or address clients use to reach the router
    /// [default: derived from server.listen; "localhost" when it binds every interface]
    #[arg(long, value_name = "HOST")]
    pub host: Option<String>,
    /// Output syntax
    #[arg(long, value_enum, default_value_t = EnvFormat::Sh)]
    pub format: EnvFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EnvFormat {
    /// `export NAME=value` lines for bash/zsh
    Sh,
    /// `$env:NAME = "value"` lines for PowerShell on the Windows host
    Powershell,
    /// An `"env"` object to paste into ~/.claude/settings.json
    Json,
}

pub fn run(cli: &Cli, args: &EnvArgs) -> anyhow::Result<()> {
    let config = cli.load_config()?;
    print!("{}", render(&config, args.host.as_deref(), args.format));
    Ok(())
}

pub fn render(config: &Config, host: Option<&str>, format: EnvFormat) -> String {
    let host = host
        .map(str::to_owned)
        .unwrap_or_else(|| default_host(config));
    let base_url = format!("http://{host}:{}", config.server.listen.port());
    let default_model = config
        .routing
        .default_model
        .clone()
        .or_else(|| config.models.first().map(|m| m.id.clone()))
        .unwrap_or_default();
    let vars = [
        ("ANTHROPIC_BASE_URL", base_url),
        ("ANTHROPIC_AUTH_TOKEN", config.server.token.clone()),
        ("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY", "1".to_owned()),
        ("ANTHROPIC_MODEL", default_model),
    ];
    let mut out = String::new();
    match format {
        EnvFormat::Sh => {
            out.push_str("# Point Claude Code at anthroxy (paste into your shell or profile)\n");
            for (name, value) in &vars {
                out.push_str(&format!("export {name}=\"{value}\"\n"));
            }
            out.push_str(
                "# ANTHROPIC_MODEL is the model Claude Code starts with; /model switches later.\n",
            );
            if is_wsl() && host == "localhost" {
                out.push_str("# From the Windows host: use this distribution's address instead of localhost\n");
                out.push_str("#   (hostname -I) unless WSL networking is set to mirrored.\n");
            }
        }
        EnvFormat::Powershell => {
            out.push_str("# Point Claude Code at anthroxy (PowerShell)\n");
            for (name, value) in &vars {
                out.push_str(&format!("$env:{name} = \"{value}\"\n"));
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

fn default_host(config: &Config) -> String {
    match config.server.listen.ip() {
        ip if ip.is_unspecified() => "localhost".to_owned(),
        IpAddr::V6(v6) => format!("[{v6}]"),
        ip => ip.to_string(),
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

    #[test]
    fn sh_output_uses_localhost_for_unspecified_bind() {
        let out = render(&config("0.0.0.0:8787"), None, EnvFormat::Sh);
        assert!(
            out.contains("export ANTHROPIC_BASE_URL=\"http://localhost:8787\"\n"),
            "{out}"
        );
        assert!(out.contains("export ANTHROPIC_AUTH_TOKEN=\"tok\"\n"));
        assert!(out.contains("export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=\"1\"\n"));
        assert!(
            out.contains("export ANTHROPIC_MODEL=\"m2\"\n"),
            "default model wins"
        );
    }

    #[test]
    fn explicit_host_and_bound_ip_are_used() {
        let out = render(&config("192.168.1.5:9000"), None, EnvFormat::Powershell);
        assert!(
            out.contains("$env:ANTHROPIC_BASE_URL = \"http://192.168.1.5:9000\"\n"),
            "{out}"
        );
        let out = render(&config("0.0.0.0:8787"), Some("wsl.local"), EnvFormat::Json);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["env"]["ANTHROPIC_BASE_URL"], "http://wsl.local:8787");
        assert_eq!(value["env"]["ANTHROPIC_MODEL"], "m2");
    }
}
