//! `check`: load the file, then acquire every credential and call
//! `GET /v1/models` on every backend. Exit status is non-zero when anything
//! the router needs at runtime is broken.

use std::sync::Arc;
use std::time::Duration;

use clap::Args;

use super::models::{default_route_line, table_rows};
use super::style::table;
use super::{Cli, Style, display_path};
use crate::config::{BackendKind, Config};
use crate::routing::{Registry, Route};
use crate::upstream::probe::{ModelsProbe, Probe, probe_all};
use crate::upstream::{Backend, RetryPolicy, UpstreamClient, http_client};

#[derive(Debug, Clone, Args)]
pub struct CheckArgs {
    /// Only validate the file; do not contact backends
    #[arg(long)]
    pub no_probe: bool,
    /// Per-backend probe timeout
    #[arg(long, value_name = "DURATION", default_value = "10s", value_parser = humantime::parse_duration)]
    pub timeout: Duration,
}

pub async fn run(cli: &Cli, args: &CheckArgs, style: &Style) -> anyhow::Result<()> {
    cli.init_tracing(None, "warn")?;
    let path = cli.config_path();
    let config = match Config::load(&path) {
        Ok(config) => config,
        Err(error) => {
            println!("{} {}", style.err_mark(), style.bold(&display_path(&path)));
            println!("{error}");
            anyhow::bail!("configuration is not usable");
        }
    };
    println!(
        "{} {}  {}",
        style.ok_mark(),
        style.bold(&display_path(&path)),
        style.dim(&format!(
            "valid: {} backend(s), {} model(s), listening on {}",
            config.backends.len(),
            config.models.len(),
            config.server.listen
        ))
    );

    let registry = match Registry::from_config(&config) {
        Ok(registry) => registry,
        Err(error) => {
            println!("{} {error}", style.err_mark());
            anyhow::bail!("configuration is not usable");
        }
    };

    let client = match http_client(&config.upstream) {
        Ok(client) => client,
        Err(error) => {
            println!("{} {error}", style.err_mark());
            anyhow::bail!("configuration is not usable");
        }
    };

    let mut problems = 0usize;
    let mut listed: std::collections::HashMap<String, Vec<String>> = Default::default();
    println!();
    println!("{}", style.bold("Backends"));
    if args.no_probe {
        for backend in registry.backends() {
            println!(
                "  {}  {}  {}{}",
                style.dim("-"),
                style.bold(&backend.name),
                style.dim(&backend.url),
                kind_label(backend, style)
            );
            println!("       credential: {}", backend.credential.describe());
        }
        println!("  {}", style.dim("(probing skipped: --no-probe)"));
    } else {
        let client =
            UpstreamClient::new(client.timeout(args.timeout).build()?, RetryPolicy::never());
        let results = probe_all(&client, registry.backends().map(Arc::as_ref)).await;
        for (backend, result) in registry.backends().zip(&results) {
            let (ok, ids) = report_backend(backend, result, style);
            if !ok {
                problems += 1;
            }
            if let Some(ids) = ids {
                listed.insert(backend.name.clone(), ids);
            }
        }
    }

    println!();
    println!("{}", style.bold("Models"));
    let mark = |route: &Route| {
        let mark = match listed.get(&route.backend.name) {
            Some(ids) if ids.is_empty() => style.dim("-"),
            Some(ids) if ids.contains(&route.upstream_model) => style.ok_mark(),
            Some(_) => style.warn_mark(),
            None => style.dim("-"),
        };
        format!("  {mark}")
    };
    print!("{}", table(&table_rows(&registry, style, Some(&mark))));
    if listed.values().any(|ids| !ids.is_empty()) {
        println!(
            "  {}",
            style.dim("✓ upstream model listed by the backend, ! not listed (check the name), - backend gave no list")
        );
    }
    println!("  {}", default_route_line(&registry, style));

    println!();
    if problems == 0 {
        println!(
            "{} ready. Next: `anthroxy serve`, then `anthroxy env` for Claude Code.",
            style.ok_mark()
        );
        Ok(())
    } else {
        println!("{} {problems} backend problem(s) above.", style.err_mark());
        anyhow::bail!("{problems} backend problem(s)")
    }
}

/// ` (openai)` after the URL of a backend that is not the default kind.
fn kind_label(backend: &Backend, style: &Style) -> String {
    match backend.kind {
        BackendKind::Anthropic => String::new(),
        BackendKind::OpenAi => format!("  {}", style.dim("(openai)")),
    }
}

/// Prints one backend block; returns (healthy, model ids if the backend listed any).
fn report_backend(backend: &Backend, probe: &Probe, style: &Style) -> (bool, Option<Vec<String>>) {
    let mut ok = true;
    let credential_line = match &probe.credential {
        Ok(text) => format!("credential: {text}"),
        Err(error) => {
            ok = false;
            format!("credential: {}", style.err(&error.to_string()))
        }
    };
    let (models_line, ids) = match &probe.models {
        None => (style.dim("GET /v1/models skipped (no credential)"), None),
        Some(ModelsProbe::Failed(error)) => {
            ok = false;
            (style.err(&error.to_string()), None)
        }
        Some(ModelsProbe::Answered {
            status,
            latency,
            ids,
            detail,
        }) => {
            let count = if ids.is_empty() {
                String::new()
            } else {
                format!(", {} model(s)", ids.len())
            };
            let text = format!(
                "GET /v1/models → HTTP {status} in {} ms{count}",
                latency.as_millis()
            );
            let line = match detail {
                None => style.ok(&text),
                Some(detail) => {
                    ok = false;
                    style.err(&format!("{text}: {detail}"))
                }
            };
            (line, Some(ids.clone()))
        }
    };
    let mark = if ok {
        style.ok_mark()
    } else {
        style.err_mark()
    };
    println!(
        "  {mark}  {}  {}{}",
        style.bold(&backend.name),
        style.dim(&backend.url),
        kind_label(backend, style)
    );
    println!("       {credential_line}");
    println!("       {models_line}");
    (ok, ids)
}
