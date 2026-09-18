//! Tracking one request from arrival to the end of its response body.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::Stream;
use tokio::sync::watch;

use super::{Activity, ERROR_BODY_BYTES, ExchangeView, Hint, Outcome};
use crate::anthropic::UsageScanner;
use crate::config::BackendKind;
use crate::routing::Match;
use crate::text::cut;

/// Runs once with the finished view.
type OnFinish = Box<dyn FnOnce(&ExchangeView) + Send>;

/// Updates one [`ExchangeView`]. Finishing moves it from in flight to recent;
/// dropping it unfinished records a client that went away, or a failure once
/// a stop cut what was in flight.
pub struct Exchange {
    activity: Arc<Activity>,
    view: Arc<Mutex<ExchangeView>>,
    started: Instant,
    finished: bool,
    on_finish: Option<OnFinish>,
    cut: watch::Receiver<bool>,
}

impl Exchange {
    pub(super) fn new(
        activity: Arc<Activity>,
        view: ExchangeView,
        cut: watch::Receiver<bool>,
    ) -> Self {
        let view = Arc::new(Mutex::new(view));
        activity.register(&view);
        Self {
            activity,
            view,
            started: Instant::now(),
            finished: false,
            on_finish: None,
            cut,
        }
    }

    /// The stop's cut signal this exchange was started with, for what else
    /// the request drops unfinished.
    pub fn cut_signal(&self) -> watch::Receiver<bool> {
        self.cut.clone()
    }

    /// Runs once with the finished view, after it joined the recent list.
    pub fn on_finish(&mut self, then: impl FnOnce(&ExchangeView) + Send + 'static) {
        self.on_finish = Some(Box::new(then));
    }

    fn update(&self, change: impl FnOnce(&mut ExchangeView)) {
        change(&mut self.view.lock().unwrap_or_else(|p| p.into_inner()));
    }

    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn requested(&self, model: &str, stream: bool) {
        self.update(|v| {
            v.requested_model = Some(cut(model, 200));
            v.stream = stream;
        });
    }

    /// Claude Code's session id, as the client sent it.
    pub fn session(&self, id: &str) {
        self.update(|v| v.session = Some(cut(id, 200)));
    }

    /// The request named a model no route serves; tallied.
    pub fn unrouted(&self, model: &str) {
        self.activity.names_mut().unknown(model);
    }

    /// `requested` is what the client named; a [`Match::Default`] is tallied.
    pub fn routed(
        &self,
        requested: &str,
        matched: Match,
        model: &str,
        backend: &str,
        kind: BackendKind,
        upstream_model: &str,
    ) {
        if matched == Match::Default {
            self.activity.names_mut().defaulted(requested);
        }
        self.update(|v| {
            v.matched = Some(match matched {
                Match::Exact => "exact",
                Match::Alias => "alias",
                Match::Default => "default",
            });
            v.model = Some(model.to_owned());
            v.backend = Some(backend.to_owned());
            v.kind = Some(kind);
            v.upstream_model = Some(upstream_model.to_owned());
        });
    }

    pub fn recording(&self, entry: String) {
        self.update(|v| v.recording = Some(entry));
    }

    /// Response headers arrived from the backend.
    pub fn responded(
        &self,
        status: u16,
        attempts: u32,
        credential_refreshed: bool,
        latency: Duration,
    ) {
        self.update(|v| {
            v.status = Some(status);
            v.attempts = Some(attempts);
            v.credential_refreshed = credential_refreshed;
            v.latency_ms = Some(latency.as_millis() as u64);
        });
    }

    /// The backend answered an error; its body is kept, cut.
    pub fn upstream_error(&self, body: &[u8], hints: Vec<Hint>) {
        let kept = &body[..body.len().min(ERROR_BODY_BYTES)];
        let mut text = String::from_utf8_lossy(kept).into_owned();
        while text.len() > ERROR_BODY_BYTES {
            text.pop();
        }
        self.update(|v| {
            v.error_body = Some(text);
            v.hints = hints;
        });
    }

    /// The router answered `status` itself.
    pub fn fail(mut self, status: u16, error: String) {
        self.update(|v| {
            v.status = Some(status);
            v.error = Some(error);
        });
        self.finish(Outcome::Error);
    }

    /// The whole body went to the client at once.
    pub fn finish_body(mut self, status: u16, body: &[u8], content_type: Option<&str>) {
        let mut scanner = UsageScanner::for_content_type(content_type);
        scanner.feed(body);
        let scan = scanner.finish();
        let failed = status >= 400 || scan.error.is_some();
        let ttfb = self.elapsed_ms();
        self.update(|v| {
            v.status = Some(status);
            v.ttfb_ms = Some(ttfb);
            v.bytes = body.len() as u64;
            v.usage = scan.usage;
            v.error = scan
                .error
                .or_else(|| failed.then(|| cut(&String::from_utf8_lossy(body), 200)));
        });
        self.finish(if failed {
            Outcome::Error
        } else {
            Outcome::Complete
        });
    }

    /// Wraps the body stream the client reads.
    pub fn track<S>(self, inner: S, content_type: Option<&str>) -> Tracked<S> {
        Tracked {
            inner,
            scanner: Some(UsageScanner::for_content_type(content_type)),
            exchange: Some(self),
        }
    }

    fn finish(&mut self, outcome: Outcome) {
        if self.finished {
            return;
        }
        self.finished = true;
        let duration = self.elapsed_ms();
        self.update(|v| {
            v.outcome = Some(outcome);
            v.duration_ms = Some(duration);
        });
        let done = self.activity.complete(&self.view);
        if let Some(then) = self.on_finish.take() {
            then(&done);
        }
    }
}

impl Drop for Exchange {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if *self.cut.borrow() {
            // A request cut before its response started was answered 503.
            self.update(|v| {
                v.status.get_or_insert(503);
                v.error = Some(STOPPED.to_owned());
            });
            self.finish(Outcome::Error);
        } else {
            self.finish(Outcome::ClientDisconnected);
        }
    }
}

const STOPPED: &str = "a stop cut the request before it finished";

/// Body stream that reports to its [`Exchange`] as the client reads it.
pub struct Tracked<S> {
    inner: S,
    scanner: Option<UsageScanner>,
    /// Taken when the body ends.
    exchange: Option<Exchange>,
}

impl<S> Tracked<S> {
    fn chunk(&mut self, chunk: &Bytes) {
        if let Some(exchange) = &self.exchange {
            let elapsed = exchange.elapsed_ms();
            exchange.update(|v| {
                v.ttfb_ms.get_or_insert(elapsed);
                v.bytes += chunk.len() as u64;
            });
        }
        if let Some(scanner) = &mut self.scanner {
            scanner.feed(chunk);
        }
    }

    /// Ends the exchange; `failure` is a stream error. Without either, a body
    /// still in flight when dropped is a client that went away.
    fn end(&mut self, outcome: Option<Result<(), String>>) {
        let Some(mut exchange) = self.exchange.take() else {
            return;
        };
        let scan = self
            .scanner
            .take()
            .map(UsageScanner::finish)
            .unwrap_or_default();
        let failure = match &outcome {
            Some(Err(error)) => Some(error.clone()),
            _ => scan.error,
        };
        exchange.update(|v| {
            v.usage = scan.usage.or(v.usage);
            if failure.is_some() {
                v.error = failure.clone();
            }
        });
        match (outcome, failure) {
            (None, _) => drop(exchange),
            (Some(_), Some(_)) => exchange.finish(Outcome::Error),
            (Some(_), None) => exchange.finish(Outcome::Complete),
        }
    }
}

impl<S> Stream for Tracked<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(chunk))) => {
                this.chunk(&chunk);
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.end(Some(Err(error.to_string())));
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                this.end(Some(Ok(())));
                Poll::Ready(None)
            }
        }
    }
}

impl<S> Drop for Tracked<S> {
    fn drop(&mut self) {
        self.end(None);
    }
}
