//! Relays an upstream body to the client chunk by chunk, summarising it in
//! the log and, when a body log is on, recording it.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Bytes;
use futures_util::Stream;
use tokio::sync::watch;
use tracing::Span;

use crate::observability::Recorder;

/// How a relayed body ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayOutcome {
    /// Upstream finished the body.
    Complete,
    /// Upstream failed mid-body.
    UpstreamError(String),
    /// The client went away before the body finished.
    ClientDisconnected,
    /// A stop cut the body before it finished.
    Stopped,
}

impl RelayOutcome {
    /// How a body dropped unfinished ended, given the stop's cut signal.
    pub fn dropped(cut: &watch::Receiver<bool>) -> Self {
        if *cut.borrow() {
            RelayOutcome::Stopped
        } else {
            RelayOutcome::ClientDisconnected
        }
    }
}

/// Body stream that reports its end exactly once, including when dropped
/// early.
pub struct Relay<S> {
    inner: S,
    span: Span,
    recorder: Option<Recorder>,
    cut: watch::Receiver<bool>,
    started: Instant,
    first_chunk: Option<Instant>,
    bytes: usize,
    chunks: usize,
    finished: bool,
}

impl<S> Relay<S> {
    /// `started` is when the request arrived, for time-to-first-byte; `cut`
    /// tells a stop's cut from a client that left.
    pub fn new(
        inner: S,
        span: Span,
        started: Instant,
        recorder: Option<Recorder>,
        cut: watch::Receiver<bool>,
    ) -> Self {
        Self {
            inner,
            span,
            recorder,
            cut,
            started,
            first_chunk: None,
            bytes: 0,
            chunks: 0,
            finished: false,
        }
    }

    fn chunk(&mut self, chunk: &Bytes) {
        let _guard = self.span.enter();
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
        if let Some(recorder) = &mut self.recorder {
            recorder.chunk(chunk);
        }
    }

    fn end(&mut self, outcome: RelayOutcome) {
        if self.finished {
            return;
        }
        self.finished = true;
        let _guard = self.span.enter();
        let ttfb_ms = self
            .first_chunk
            .map(|t| t.duration_since(self.started).as_millis() as u64);
        let duration_ms = self.started.elapsed().as_millis() as u64;
        match &outcome {
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
            RelayOutcome::Stopped => tracing::warn!(
                bytes = self.bytes,
                chunks = self.chunks,
                duration_ms,
                "stop cut the response body before it finished"
            ),
        }
        if let Some(recorder) = self.recorder.take() {
            recorder.finish(&outcome);
        }
    }
}

impl<S, E> Stream for Relay<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
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
        let outcome = RelayOutcome::dropped(&self.cut);
        self.end(outcome);
    }
}
