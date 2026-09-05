//! `check`: load the file, then acquire every credential and call
//! `GET /v1/models` on every backend. Exit status is non-zero when anything
//! the router needs at runtime is broken.

use std::time::Duration;

use clap::Args;

use super::style::table;
use super::{Cli, Style, display_path};
use crate::config::Config;
use crate::routing::Registry;
use crate::upstream::probe::{ModelsProbe, Probe, probe};
use crate::upstream::{Backend, Backends};

#[derive(Debug, Args)]
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

    let backends = match Backends::from_config(&config) {
        Ok(b) => b,
        Err(error) => {
            println!("{} {error}", style.err_mark());
            anyhow::bail!("configuration is not usable");
        }
    };
    let registry = Registry::from_config(&config);

    let mut problems = 0usize;
    let mut listed: std::collections::HashMap<String, Vec<String>> = Default::default();
    println!();
    println!("{}", style.bold("Backends"));
    if args.no_probe {
        for backend in backends.iter() {
            println!(
                "  {}  {}  {}",
                style.dim("-"),
                style.bold(&backend.name),
                style.dim(&backend.url)
            );
            println!("       credential: {}", backend.credential.describe());
        }
        println!("  {}", style.dim("(probing skipped: --no-probe)"));
    } else {
        let http = reqwest::Client::builder().timeout(args.timeout).build()?;
        for backend in backends.iter() {
            let result = probe(&http, backend).await;
            let (ok, ids) = report_backend(backend, &result, style);
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
    let mut rows = vec![
        ["", "id", "backend", "upstream model", "picker label"]
            .iter()
            .map(|h| style.dim(h))
            .collect::<Vec<_>>(),
    ];
    for route in registry.routes() {
        let mark = match listed.get(&route.backend) {
            Some(ids) if ids.is_empty() => style.dim("-"),
            Some(ids) if ids.contains(&route.upstream_model) => style.ok_mark(),
            Some(_) => style.warn_mark(),
            None => style.dim("-"),
        };
        rows.push(vec![
            format!("  {mark}"),
            style.bold(&route.id),
            route.backend.clone(),
            route.upstream_model.clone(),
            route.display_name.clone(),
        ]);
    }
    print!("{}", table(&rows));
    if listed.values().any(|ids| !ids.is_empty()) {
        println!(
            "  {}",
            style.dim("✓ upstream model listed by the backend, ! not listed (check the name), - backend gave no list")
        );
    }
    if let Some(route) = registry.default_route() {
        println!("  unknown model ids → {}", style.bold(&route.id));
    }

    println!();
    if problems == 0 {
        println!(
            "{} ready. Next: `claude-router serve`, then `claude-router env` for Claude Code.",
            style.ok_mark()
        );
        Ok(())
    } else {
        println!("{} {problems} backend problem(s) above.", style.err_mark());
        anyhow::bail!("{problems} backend problem(s)")
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
        Some(ModelsProbe::Unreachable(detail)) => {
            ok = false;
            (style.err(&format!("unreachable: {detail}")), None)
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
        "  {mark}  {}  {}",
        style.bold(&backend.name),
        style.dim(&backend.url)
    );
    println!("       {credential_line}");
    println!("       {models_line}");
    (ok, ids)
}
