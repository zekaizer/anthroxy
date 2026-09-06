use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Args;

use super::{Cli, Style, display_path};
use crate::config::Config;
use crate::routing::Registry;
use crate::server::Server;
use crate::upstream::Backends;

#[derive(Debug, Clone, Args)]
pub struct ServeArgs {
    /// Listen address; overrides server.listen. A reload cannot change it.
    #[arg(long, value_name = "ADDR")]
    pub listen: Option<SocketAddr>,
    /// Record every request and response body under this directory;
    /// overrides logging.body_dir
    #[arg(long, value_name = "DIR")]
    pub body_dir: Option<PathBuf>,
}

/// The file plus command-line overrides; used at startup and on every reload.
fn load_effective(cli: &Cli, args: &ServeArgs) -> anyhow::Result<Config> {
    let mut config = cli.load_config()?;
    if let Some(listen) = args.listen {
        config.server.listen = listen;
    }
    if let Some(dir) = &args.body_dir {
        config.logging.body_dir = Some(dir.clone());
    }
    Ok(config)
}

pub async fn run(cli: &Cli, args: &ServeArgs, style: &Style) -> anyhow::Result<()> {
    let path = cli.config_path();
    let config = load_effective(cli, args)?;
    cli.init_tracing(Some(&config), "info")?;

    let server = Server::new(&config)?;
    let bound = server
        .bind(config.server.listen)
        .await
        .map_err(|e| anyhow::anyhow!("cannot listen on {}: {e}", config.server.listen))?;
    let addr = bound.local_addr();
    print_banner(&config, &path, addr, style);
    tracing::info!(%addr, config = %path.display(), "claude-router listening");

    let reloader = tokio::spawn(reload_on_hangup(
        bound.reload_handle(),
        cli.clone(),
        args.clone(),
    ));
    let result = bound.serve(shutdown_signal()).await;
    reloader.abort();
    result?;
    tracing::info!("claude-router stopped");
    Ok(())
}

/// Re-reads the configuration on SIGHUP. A file that fails to load or build
/// leaves the running configuration untouched.
#[cfg(unix)]
async fn reload_on_hangup(handle: crate::server::ReloadHandle, cli: Cli, args: ServeArgs) {
    let Ok(mut hangup) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
    else {
        return;
    };
    let path = cli.config_path();
    while hangup.recv().await.is_some() {
        tracing::info!(config = %path.display(), "SIGHUP received, reloading configuration");
        match load_effective(&cli, &args).and_then(|c| Ok(handle.apply(&c)?)) {
            Ok(report) => {
                tracing::info!(
                    backends = report.backends,
                    models = report.models,
                    "configuration reloaded"
                );
                if report.listen_changed {
                    tracing::warn!(
                        "server.listen changed in the file; restart the router to apply it"
                    );
                }
            }
            Err(error) => {
                tracing::error!(%error, "reload failed; keeping the previous configuration");
            }
        }
    }
}

#[cfg(not(unix))]
async fn reload_on_hangup(_: crate::server::ReloadHandle, _: Cli, _: ServeArgs) {
    std::future::pending::<()>().await
}

fn print_banner(config: &Config, path: &std::path::Path, addr: SocketAddr, style: &Style) {
    let registry = Registry::from_config(config);
    println!(
        "{} {}",
        style.bold(&format!("claude-router {}", crate::build_info::VERSION)),
        style.dim(&format!("({})", display_path(path)))
    );
    let shown = if addr.ip().is_unspecified() {
        format!(
            "http://{addr}  {}",
            style.dim(&format!(
                "(all interfaces; locally http://localhost:{})",
                addr.port()
            ))
        )
    } else {
        format!("http://{addr}")
    };
    println!("  listening on {shown}");
    let backends = Backends::from_config(config).ok();
    for (name, backend) in &config.backends {
        let credential = backends
            .as_ref()
            .and_then(|b| b.get(name))
            .map(|b| b.credential.describe())
            .unwrap_or_default();
        println!(
            "  backend  {}  {}  {}",
            style.bold(name),
            backend.url,
            style.dim(&format!("credential: {credential}"))
        );
    }
    for route in registry.routes() {
        let aliases = if route.aliases.is_empty() {
            String::new()
        } else {
            format!(" (+{} alias)", route.aliases.len())
        };
        println!(
            "  model    {}  → {}/{}{}",
            style.bold(&route.id),
            route.backend,
            route.upstream_model,
            style.dim(&aliases)
        );
    }
    if let Some(route) = registry.default_route() {
        println!("  unknown model ids → {}", route.id);
    }
    match &config.logging.body_dir {
        Some(dir) => println!("  body log {}", display_path(dir)),
        None => println!(
            "  body log {}",
            style.dim("off (logging.body_dir or --body-dir)")
        ),
    }
    println!(
        "  client   {}",
        style.dim("run `claude-router env` for Claude Code's variables")
    );
    println!("{}", style.dim("Press Ctrl-C to stop."));
}

/// Resolves on SIGINT or, on Unix, SIGTERM (what systemd sends).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => tracing::info!("received Ctrl-C, shutting down"),
        () = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
