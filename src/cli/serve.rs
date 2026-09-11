use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;

use super::models::default_route_line;
use super::{Cli, Style, display_path};
use crate::config::{Config, Overrides};
use crate::server::{AppState, Loaded, Loader, ReloadTrigger, Server, Snapshot};

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
    if let Some(mode) = crate::config::open_to_other_accounts(&path) {
        tracing::warn!(
            config = %path.display(),
            mode = %format!("{mode:o}"),
            "the configuration holds the router token; `chmod 600` it to keep it to this account"
        );
    }

    let server = Server::bind(&config).await?;
    let addr = server.local_addr();
    let state = server.state();
    state.set_loader(path.clone(), loader(cli.clone(), args.clone(), &config));
    // Before the banner: until this handler exists SIGHUP terminates the
    // process, and a reload signalled the moment the router looks ready would
    // do exactly that.
    let reloader = tokio::spawn(reload_on_hangup(hangup(), state.clone()));
    print_banner(&state.snapshot(), &path, addr, style);
    crate::upstream::network::log(&config);
    tracing::info!(%addr, config = %path.display(), "anthroxy listening");

    let result = server.serve(shutdown_signal()).await;
    reloader.abort();
    result?;
    tracing::info!("anthroxy stopped");
    Ok(())
}

/// Reads the file with this invocation's overrides. The log subscriber is
/// global and installed once, so a changed filter or format is reported as
/// needing a restart rather than silently ignored.
fn loader(cli: Cli, args: ServeArgs, started_with: &Config) -> Loader {
    let installed = cli.logging(Some(started_with), "info");
    Arc::new(move || {
        let config = load_effective(&cli, &args)?;
        let restart_needed = if cli.logging(Some(&config), "info") != installed {
            vec!["logging level or format".to_owned()]
        } else {
            Vec::new()
        };
        Ok(Loaded {
            config,
            restart_needed,
        })
    })
}

/// Re-reads the configuration on SIGHUP. A file that fails to load or build
/// leaves the running configuration untouched.
#[cfg(unix)]
async fn reload_on_hangup(hangup: Option<tokio::signal::unix::Signal>, state: AppState) {
    let Some(mut hangup) = hangup else {
        return;
    };
    while hangup.recv().await.is_some() {
        tracing::info!("SIGHUP received, reloading configuration");
        let state = state.clone();
        let _ = tokio::task::spawn_blocking(move || state.reload(ReloadTrigger::Signal)).await;
    }
}

#[cfg(not(unix))]
async fn reload_on_hangup(_: (), _: AppState) {
    std::future::pending::<()>().await
}

/// Claims SIGHUP for the reload loop, before anything announces the router is
/// up. `None` when the handler could not be installed.
#[cfg(unix)]
fn hangup() -> Option<tokio::signal::unix::Signal> {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()).ok()
}

#[cfg(not(unix))]
fn hangup() {}

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
    match &snapshot.stats {
        Some(stats) => println!("  stats    {}", display_path(stats.dir())),
        None => println!("  stats    {}", style.dim("off (stats.enabled)")),
    }
    let local = if addr.ip().is_unspecified() {
        format!("http://localhost:{}/", addr.port())
    } else {
        format!("http://{addr}/")
    };
    println!(
        "  console  {local}  {}",
        style.dim("(sign in with server.token)")
    );
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
