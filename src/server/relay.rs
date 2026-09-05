//! Relays an upstream body to the client chunk by chunk, observing it on the
//! way through.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Bytes;
use futures_util::Stream;
use tracing::Span;

/// How a relayed body ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayOutcome {
    /// Upstream finished the body.
    Complete,
    /// Upstream failed mid-body.
    UpstreamError(String),
    /// The client went away before the body finished.
    ClientDisconnected,
}

/// Sees every chunk and the final outcome. Implementations must not block.
pub trait RelayObserver: Send + 'static {
    fn on_chunk(&mut self, chunk: &Bytes);
    fn on_end(&mut self, outcome: &RelayOutcome);
}

/// Body stream that fans chunks out to observers and reports its end exactly
/// once, including when dropped early.
pub struct Relay<S> {
    inner: S,
    observers: Vec<Box<dyn RelayObserver>>,
    span: Span,
    finished: bool,
}

impl<S> Relay<S> {
    pub fn new(inner: S, span: Span) -> Self {
        Self {
            inner,
            observers: Vec::new(),
            span,
            finished: false,
        }
    }

    pub fn observe(mut self, observer: impl RelayObserver) -> Self {
        self.observers.push(Box::new(observer));
        self
    }

    fn end(&mut self, outcome: RelayOutcome) {
        if self.finished {
            return;
        }
        self.finished = true;
        let _guard = self.span.enter();
        for observer in &mut self.observers {
            observer.on_end(&outcome);
        }
    }
}

impl<S> Stream for Relay<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(chunk))) => {
                let _guard = this.span.enter();
                for observer in &mut this.observers {
                    observer.on_chunk(&chunk);
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(error))) => {
                let text = error.to_string();
                this.end(RelayOutcome::UpstreamError(text.clone()));
                Poll::Ready(Some(Err(std::io::Error::other(text))))
            }
            Poll::Ready(None) => {
                this.end(RelayOutcome::Complete);
                Poll::Ready(None)
            }
        }
    }
}

impl<S> Drop for Relay<S> {
    fn drop(&mut self) {
        self.end(RelayOutcome::ClientDisconnected);
    }
}

/// Emits one summary line per body: bytes, chunks, time to first byte and
/// total duration.
pub struct TracingObserver {
    started: Instant,
    first_chunk: Option<Instant>,
    bytes: usize,
    chunks: usize,
}

impl TracingObserver {
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            first_chunk: None,
            bytes: 0,
            chunks: 0,
        }
    }
}

impl RelayObserver for TracingObserver {
    fn on_chunk(&mut self, chunk: &Bytes) {
        if self.first_chunk.is_none() {
            self.first_chunk = Some(Instant::now());
            tracing::debug!(
                ttfb_ms = self.started.elapsed().as_millis() as u64,
                "first upstream body chunk"
            );
        }
        self.bytes += chunk.len();
        self.chunks += 1;
        tracing::trace!(bytes = chunk.len(), "relayed chunk");
    }

    fn on_end(&mut self, outcome: &RelayOutcome) {
        let ttfb_ms = self
            .first_chunk
            .map(|t| t.duration_since(self.started).as_millis() as u64);
        let duration_ms = self.started.elapsed().as_millis() as u64;
        match outcome {
            RelayOutcome::Complete => tracing::info!(
                bytes = self.bytes,
                chunks = self.chunks,
                ttfb_ms,
                duration_ms,
                "response body complete"
            ),
            RelayOutcome::UpstreamError(error) => tracing::warn!(
                bytes = self.bytes,
                chunks = self.chunks,
                duration_ms,
                %error,
                "upstream body failed mid-stream"
            ),
            RelayOutcome::ClientDisconnected => tracing::warn!(
                bytes = self.bytes,
                chunks = self.chunks,
                duration_ms,
                "client disconnected before the response body finished"
            ),
        }
    }
}
