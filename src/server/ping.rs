//! Keep-alive `ping` events on an event stream the backend has gone quiet on
//! (ADR-0014).

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::Stream;

/// Silence after which a ping goes out, and between pings.
pub const PING_INTERVAL: Duration = Duration::from_secs(15);

const PING: &[u8] = b"event: ping\ndata: {\"type\": \"ping\"}\n\n";

/// Passes `inner` through and writes a `ping` event whenever nothing went out
/// for the interval, if the bytes so far end at an event boundary.
pub struct Pings<S> {
    inner: S,
    interval: Duration,
    quiet: Pin<Box<tokio::time::Sleep>>,
    /// The last bytes relayed, enough to see a blank line; empty before any.
    tail: Vec<u8>,
}

impl<S> Pings<S> {
    pub fn new(inner: S, interval: Duration) -> Self {
        Self {
            inner,
            interval,
            quiet: Box::pin(tokio::time::sleep(interval)),
            tail: Vec::new(),
        }
    }

    fn restart_quiet(&mut self) {
        let deadline = tokio::time::Instant::now() + self.interval;
        self.quiet.as_mut().reset(deadline);
    }

    fn at_boundary(&self) -> bool {
        self.tail.is_empty() || self.tail.ends_with(b"\n\n") || self.tail.ends_with(b"\r\n\r\n")
    }

    fn relayed(&mut self, chunk: &[u8]) {
        self.tail.extend_from_slice(chunk);
        let excess = self.tail.len().saturating_sub(4);
        self.tail.drain(..excess);
    }
}

impl<S> Stream for Pings<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if !chunk.is_empty() {
                    this.relayed(&chunk);
                    this.restart_quiet();
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(other) => Poll::Ready(other),
            Poll::Pending => {
                while this.quiet.as_mut().poll(cx).is_ready() {
                    this.restart_quiet();
                    if this.at_boundary() {
                        return Poll::Ready(Some(Ok(Bytes::from_static(PING))));
                    }
                }
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use futures_util::StreamExt;
    use futures_util::stream::BoxStream;
    use tokio::sync::mpsc;
    use tokio::time::Instant;

    use super::*;

    type Sender = mpsc::UnboundedSender<Result<Bytes, io::Error>>;

    fn channel() -> (Sender, Pings<BoxStream<'static, Result<Bytes, io::Error>>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let inner = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })
        .boxed();
        (tx, Pings::new(inner, Duration::from_secs(15)))
    }

    /// The next item within a minute of paused time.
    async fn next(pings: &mut Pings<BoxStream<'static, Result<Bytes, io::Error>>>) -> Bytes {
        tokio::time::timeout(Duration::from_secs(60), pings.next())
            .await
            .expect("an item within a minute")
            .expect("an item")
            .expect("no error")
    }

    #[tokio::test(start_paused = true)]
    async fn a_quiet_stream_gets_a_ping_each_interval_until_data_comes() {
        let (tx, mut pings) = channel();
        let started = Instant::now();
        assert_eq!(next(&mut pings).await, PING);
        assert_eq!(started.elapsed(), Duration::from_secs(15));
        assert_eq!(next(&mut pings).await, PING);
        assert_eq!(started.elapsed(), Duration::from_secs(30));

        tx.send(Ok(Bytes::from_static(b"event: a\ndata: {}\n\n")))
            .unwrap();
        assert_eq!(next(&mut pings).await, "event: a\ndata: {}\n\n");
        drop(tx);
        assert!(pings.next().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn data_puts_the_next_ping_off() {
        let (tx, mut pings) = channel();
        let started = Instant::now();
        tokio::time::sleep(Duration::from_secs(10)).await;
        tx.send(Ok(Bytes::from_static(b"event: a\r\ndata: {}\r\n\r\n")))
            .unwrap();
        assert_eq!(next(&mut pings).await, "event: a\r\ndata: {}\r\n\r\n");
        assert_eq!(next(&mut pings).await, PING);
        assert_eq!(started.elapsed(), Duration::from_secs(25));
    }

    #[tokio::test(start_paused = true)]
    async fn no_ping_lands_inside_an_event() {
        let (tx, mut pings) = channel();
        tx.send(Ok(Bytes::from_static(b"event: a\ndata: {\"x\":")))
            .unwrap();
        assert_eq!(next(&mut pings).await, "event: a\ndata: {\"x\":");
        assert!(
            tokio::time::timeout(Duration::from_secs(40), pings.next())
                .await
                .is_err(),
            "a ping went out mid-event"
        );
        tx.send(Ok(Bytes::from_static(b"1}\n"))).unwrap();
        assert_eq!(next(&mut pings).await, "1}\n");
        tx.send(Ok(Bytes::from_static(b"\n"))).unwrap();
        assert_eq!(next(&mut pings).await, "\n");
        let before = Instant::now();
        assert_eq!(next(&mut pings).await, PING);
        assert_eq!(before.elapsed(), Duration::from_secs(15));
    }
}
