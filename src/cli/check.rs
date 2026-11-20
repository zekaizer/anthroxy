//! `check`: load the file, then acquire every credential and call
//! `GET /v1/models` on every backend. Exit status is non-zero when anything
//! the router needs at runtime is broken.

use std::sync::Arc;
use std::time::Duration;

use clap::Args;

use super::models::{default_route_line, table_rows};
use super::style::table;
use super::{Cli, Style, display_path};
use crate::config::{self, BackendKind, Config};
use crate::routing::{Registry, Route};
use crate::upstream::probe::{ModelsProbe, Probe, probe_all};
use crate::upstream::{Backend, HttpClients, RetryPolicy, Transport, UpstreamClient};

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

    if let Some(mode) = config::open_to_other_accounts(&path) {
        println!(
            "{} {}",
            style.warn_mark(),
            style.warn(&format!(
                "mode {mode:o}: any router token or static credential in this file is readable by other users; `chmod 600 {}`",
                display_path(&path)
            ))
        );
    }

    let env_file = config::env_file(&path);
    if let Some(mode) = config::open_to_other_accounts(&env_file) {
        println!(
            "{} {}",
            style.warn_mark(),
            style.warn(&format!(
                "mode {mode:o}: the variables in this file are readable by other users; `chmod 600 {}`",
                display_path(&env_file)
            ))
        );
    }

    let mut problems = 0usize;
    if let Err(error) = config.server.token.resolve() {
        println!("{} server.token: {error}", style.err_mark());
        problems += 1;
    }

    let registry = match Registry::from_config(&config) {
        Ok(registry) => registry,
        Err(error) => {
            println!("{} {error}", style.err_mark());
            anyhow::bail!("configuration is not usable");
        }
    };

    // Without a probe nothing is sent, and `ca_certificate` is still read.
    let transports: Vec<Transport> = match args.no_probe {
        true => Vec::new(),
        false => registry
            .backends()
            .map(|backend| backend.dialect.headers.transport)
            .collect(),
    };
    let clients = match HttpClients::build(
        &config.upstream,
        &config.backends,
        Some(args.timeout),
        transports,
    ) {
        Ok(clients) => clients,
        Err(error) => {
            println!("{} {error}", style.err_mark());
            anyhow::bail!("configuration is not usable");
        }
    };

    let mut listed: std::collections::HashMap<String, Vec<String>> = Default::default();
    println!();
    println!("{}", style.bold("Backends"));
    if args.no_probe {
        for backend in registry.backends() {
            println!(
                "  {}  {}  {}{}",
                style.dim("-"),
                style.bold(&backend.name),
                style.dim(&backend.shown_url()),
                kind_label(backend, style)
            );
            println!("       credential: {}", backend.credential.describe());
            print_emulation(backend);
            print_header_command(backend);
        }
        println!("  {}", style.dim("(probing skipped: --no-probe)"));
    } else {
        let client = UpstreamClient::new(clients, RetryPolicy::never());
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
        println!("{} {problems} problem(s) above.", style.err_mark());
        anyhow::bail!("{problems} problem(s)")
    }
}

/// The preset the backend's own `emulate` selects.
fn print_emulation(backend: &Backend) {
    if let Some(emulation) = &backend.dialect.emulation {
        println!("       emulate: {}", emulation.describe());
    }
}

fn print_header_command(backend: &Backend) {
    if let Some(command) = &backend.header_command {
        println!("       header command: {}", command.describe());
    }
}

/// ` (openai)` after the URL of a backend that is not the default kind.
fn kind_label(backend: &Backend, style: &Style) -> String {
    match backend.kind {
        BackendKind::Anthropic => String::new(),
        BackendKind::OpenAi => format!("  {}", style.dim("(openai)")),
        BackendKind::Passthrough => format!("  {}", style.dim("(passthrough)")),
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
        None => (
            style.dim(&format!(
                "GET {} skipped (no credential)",
                backend.models_path
            )),
            None,
        ),
        Some(ModelsProbe::Failed(error)) => {
            ok = false;
            (style.err(&error.to_string()), None)
        }
        Some(ModelsProbe::Skipped { reason }) => (
            style.dim(&format!("GET {} skipped ({reason})", backend.models_path)),
            None,
        ),
        Some(ModelsProbe::Answered {
            status,
            latency,
            models,
            detail,
            ..
        }) => {
            let ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
            let count = if ids.is_empty() {
                String::new()
            } else {
                format!(", {} model(s)", ids.len())
            };
            let text = format!(
                "GET {} → HTTP {status} in {} ms{count}",
                backend.models_path,
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
        style.dim(&backend.shown_url()),
        kind_label(backend, style)
    );
    println!("       {credential_line}");
    print_emulation(backend);
    print_header_command(backend);
    if probe.credential.is_err() {
        println!(
            "       {}",
            style.dim(&format!(
                "`anthroxy credential {}` runs the command and shows its output",
                backend.name
            ))
        );
    }
    println!("       {models_line}");
    (ok, ids)
}
