//! Accepting connections: HTTP/1.1 with a deadline on every request head,
//! closed gracefully on stop.

use std::future::Future;
use std::io;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::{Service, service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::watch;

/// Serves `app` until `shutdown` resolves, then has every connection close
/// after its current response, and resolves once all have closed.
///
/// A connection gets `head_timeout` for each request head, the idle time
/// before it included. HTTP/2 is not offered: telling it from HTTP/1.1 takes
/// a read that no deadline covers.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    head_timeout: Duration,
    shutdown: impl Future<Output = ()>,
) {
    let (stop, stopped) = watch::channel(false);
    // Every connection holds a receiver; the sender sees them all go.
    let (open, open_marker) = watch::channel(());
    tokio::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    accept_failed(error).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let app = TowerToHyperService::new(app.clone());
        let service = service_fn(move |mut request: http::Request<Incoming>| {
            request.extensions_mut().insert(ConnectInfo(peer));
            app.call(request)
        });
        let mut stopped = stopped.clone();
        let marker = open_marker.clone();
        tokio::spawn(async move {
            let _marker = marker;
            let connection = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(head_timeout)
                .serve_connection(TokioIo::new(stream), service);
            tokio::pin!(connection);
            tokio::select! {
                _ = connection.as_mut() => return,
                _ = stopped.wait_for(|stop| *stop) => connection.as_mut().graceful_shutdown(),
            }
            let _ = connection.await;
        });
    }
    drop(listener);
    stop.send_replace(true);
    drop(open_marker);
    open.closed().await;
}

/// A connection the peer abandoned is nothing to wait for; anything else,
/// running out of file descriptors among them, gets a pause before the next
/// accept.
async fn accept_failed(error: io::Error) {
    if matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    ) {
        return;
    }
    tracing::error!(%error, "accept error");
    tokio::time::sleep(Duration::from_secs(1)).await;
}
