use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Args;

use super::models::default_route_line;
use super::{Cli, Style, display_path};
use crate::config::{Config, LogFormat, Overrides};
use crate::server::{AppState, Server, Snapshot};

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
    let overrides = Overrides {
        listen: args.listen,
        body_dir: args.body_dir.clone(),
    };
    Ok(cli.load_config()?.with_overrides(&overrides)?)
}

pub async fn run(cli: &Cli, args: &ServeArgs, style: &Style) -> anyhow::Result<()> {
    let path = cli.config_path();
    let config = load_effective(cli, args)?;
    cli.init_tracing(Some(&config), "info")?;

    let server = Server::bind(&config).await?;
    let addr = server.local_addr();
    print_banner(&server.state().snapshot(), &path, addr, style);
    tracing::info!(%addr, config = %path.display(), "anthroxy listening");

    let reloader = tokio::spawn(reload_on_hangup(
        server.state(),
        cli.clone(),
        args.clone(),
        cli.logging(Some(&config), "info"),
    ));
    let result = server.serve(shutdown_signal()).await;
    reloader.abort();
    result?;
    tracing::info!("anthroxy stopped");
    Ok(())
}

/// Re-reads the configuration on SIGHUP. A file that fails to load or build
/// leaves the running configuration untouched. `installed` is the log filter
/// and format this process started with; the subscriber is global and cannot
/// be swapped, so a change to either is reported rather than silently ignored.
#[cfg(unix)]
async fn reload_on_hangup(
    state: AppState,
    cli: Cli,
    args: ServeArgs,
    installed: (String, LogFormat),
) {
    let Ok(mut hangup) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
    else {
        return;
    };
    let path = cli.config_path();
    while hangup.recv().await.is_some() {
        tracing::info!(config = %path.display(), "SIGHUP received, reloading configuration");
        let reloaded = load_effective(&cli, &args)
            .and_then(|config| Ok((state.apply(&config)?, cli.logging(Some(&config), "info"))));
        match reloaded {
            Ok((report, logging)) => {
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
                if logging != installed {
                    tracing::warn!(
                        "logging level or format changed in the file; restart the router to apply it"
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
async fn reload_on_hangup(_: AppState, _: Cli, _: ServeArgs, _: (String, LogFormat)) {
    std::future::pending::<()>().await
}

fn print_banner(snapshot: &Snapshot, path: &std::path::Path, addr: SocketAddr, style: &Style) {
    println!(
        "{} {}",
        style.bold(&format!("anthroxy {}", crate::build_info::VERSION)),
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
    for backend in snapshot.registry.backends() {
        println!(
            "  backend  {}  {}  {}",
            style.bold(&backend.name),
            backend.url,
            style.dim(&format!("credential: {}", backend.credential.describe()))
        );
    }
    for route in snapshot.registry.routes() {
        let aliases = if route.aliases.is_empty() {
            String::new()
        } else {
            format!(" (+{} alias)", route.aliases.len())
        };
        println!(
            "  model    {}  → {}/{}{}",
            style.bold(&route.id),
            route.backend.name,
            route.upstream_model,
            style.dim(&aliases)
        );
    }
    println!("  {}", default_route_line(&snapshot.registry, style));
    match &snapshot.body_log {
        Some(log) => println!("  body log {}", display_path(log.root())),
        None => println!(
            "  body log {}",
            style.dim("off (logging.body_dir or --body-dir)")
        ),
    }
    println!(
        "  client   {}",
        style.dim("run `anthroxy env` for Claude Code's variables")
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
