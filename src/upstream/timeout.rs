//! Per-attempt clocks on an upstream body: one wall clock for a non-stream
//! response, first-byte then idle for a stream.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::Stream;
use tokio::time::{Instant, Sleep, sleep_until};

use crate::config::UpstreamConfig;

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub non_stream: Duration,
    pub stream_first_byte: Duration,
    pub stream_idle: Duration,
}

impl Timeouts {
    pub fn from_config(config: &UpstreamConfig) -> Self {
        Self {
            non_stream: config.non_stream_timeout,
            stream_first_byte: config.stream_first_byte_timeout,
            stream_idle: config.stream_idle_timeout,
        }
    }
}

impl Default for Timeouts {
    fn default() -> Self {
        Self::from_config(&UpstreamConfig::default())
    }
}

/// Which configured clock ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutClock {
    NonStream,
    StreamFirstByte,
    StreamIdle,
}

impl TimeoutClock {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NonStream => "non_stream_timeout",
            Self::StreamFirstByte => "stream_first_byte_timeout",
            Self::StreamIdle => "stream_idle_timeout",
        }
    }
}

impl std::fmt::Display for TimeoutClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub enum BodyError {
    TimedOut(TimeoutClock),
    Upstream(reqwest::Error),
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut(clock) => {
                write!(f, "upstream.{} elapsed", clock.as_str())
            }
            Self::Upstream(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BodyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TimedOut(_) => None,
            Self::Upstream(error) => Some(error),
        }
    }
}

/// How the rest of this body is timed, once headers have arrived.
#[derive(Debug, Clone, Copy)]
pub struct BodyClock {
    pub stream: bool,
    /// Absolute deadline for a non-stream body, or for a stream's first byte.
    pub deadline: Instant,
    pub idle: Duration,
}

pub struct TimedBody<S> {
    inner: S,
    clock: BodyClock,
    got_first: bool,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl<S> TimedBody<S> {
    pub fn new(inner: S, clock: BodyClock) -> Self {
        Self {
            inner,
            clock,
            got_first: false,
            sleep: None,
        }
    }

    fn waiting(&self) -> TimeoutClock {
        if !self.clock.stream {
            TimeoutClock::NonStream
        } else if !self.got_first {
            TimeoutClock::StreamFirstByte
        } else {
            TimeoutClock::StreamIdle
        }
    }
}

impl<S> Stream for TimedBody<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<Bytes, BodyError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                self.got_first = true;
                self.sleep = None;
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(BodyError::Upstream(error)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => {
                if self.sleep.is_none() {
                    let until = if self.clock.stream && self.got_first {
                        Instant::now() + self.clock.idle
                    } else {
                        self.clock.deadline
                    };
                    self.sleep = Some(Box::pin(sleep_until(until)));
                }
                let clock = self.waiting();
                match self.sleep.as_mut().expect("just set").as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Some(Err(BodyError::TimedOut(clock)))),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use futures_util::StreamExt;

    fn bytes(s: &str) -> Bytes {
        Bytes::from(s.to_owned())
    }

    struct Rx(tokio::sync::mpsc::UnboundedReceiver<Result<Bytes, reqwest::Error>>);

    impl Stream for Rx {
        type Item = Result<Bytes, reqwest::Error>;
        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.0.poll_recv(cx)
        }
    }

    #[tokio::test]
    async fn a_stream_idle_clock_resets_after_a_chunk() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut body = TimedBody::new(
            Rx(rx),
            BodyClock {
                stream: true,
                deadline: Instant::now() + Duration::from_secs(5),
                idle: Duration::from_millis(80),
            },
        );
        tx.send(Ok(bytes("a"))).unwrap();
        assert_eq!(body.next().await.unwrap().unwrap(), bytes("a"));
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(Ok(bytes("b"))).unwrap();
        assert_eq!(body.next().await.unwrap().unwrap(), bytes("b"));
        let started = Instant::now();
        let err = body.next().await.unwrap().unwrap_err();
        assert!(started.elapsed() >= Duration::from_millis(70));
        assert!(matches!(err, BodyError::TimedOut(TimeoutClock::StreamIdle)));
        drop(tx);
    }

    #[tokio::test]
    async fn a_non_stream_clock_does_not_reset() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut body = TimedBody::new(
            Rx(rx),
            BodyClock {
                stream: false,
                deadline: Instant::now() + Duration::from_millis(80),
                idle: Duration::from_secs(10),
            },
        );
        let err = body.next().await.unwrap().unwrap_err();
        assert!(matches!(err, BodyError::TimedOut(TimeoutClock::NonStream)));
        drop(tx);
    }
}
