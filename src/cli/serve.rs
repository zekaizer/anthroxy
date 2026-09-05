use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Args;

use super::{Cli, Style, display_path};
use crate::config::Config;
use crate::routing::Registry;
use crate::server::Server;
use crate::upstream::Backends;

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Listen address; overrides server.listen
    #[arg(long, value_name = "ADDR")]
    pub listen: Option<SocketAddr>,
    /// Record every request and response body under this directory;
    /// overrides logging.body_dir
    #[arg(long, value_name = "DIR")]
    pub body_dir: Option<PathBuf>,
}

pub async fn run(cli: &Cli, args: &ServeArgs, style: &Style) -> anyhow::Result<()> {
    let path = cli.config_path();
    let mut config = cli.load_config()?;
    if let Some(listen) = args.listen {
        config.server.listen = listen;
    }
    if let Some(dir) = &args.body_dir {
        config.logging.body_dir = Some(dir.clone());
    }
    cli.init_tracing(Some(&config), "info")?;

    let server = Server::new(&config)?;
    let bound = server
        .bind(config.server.listen)
        .await
        .map_err(|e| anyhow::anyhow!("cannot listen on {}: {e}", config.server.listen))?;
    let addr = bound.local_addr();
    print_banner(&config, &path, addr, style);
    tracing::info!(%addr, config = %path.display(), "claude-router listening");

    bound.serve(shutdown_signal()).await?;
    tracing::info!("claude-router stopped");
    Ok(())
}

fn print_banner(config: &Config, path: &std::path::Path, addr: SocketAddr, style: &Style) {
    let registry = Registry::from_config(config);
    println!(
        "{} {}",
        style.bold(&format!("claude-router {}", env!("CARGO_PKG_VERSION"))),
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
