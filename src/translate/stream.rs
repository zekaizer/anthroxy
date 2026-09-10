use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;

use crate::anthropic::StreamEncoder;
use crate::openai::ChunkDecoder;
use crate::sse::Parser;

/// Chat Completions SSE bytes in, Messages SSE bytes out, one output chunk
/// per input chunk at most. After the first failure (transport error,
/// malformed event, an `error` frame) one `error` event is emitted and the
/// rest of the input is consumed in silence, so the upstream body still ends
/// normally for whoever records it.
pub struct Translator<S> {
    inner: S,
    parser: Parser,
    decoder: ChunkDecoder,
    encoder: StreamEncoder,
    backend: String,
    /// `inner` returned `None`; it is not polled again.
    done: bool,
}

impl<S> Translator<S> {
    /// `fallback_model` is reported when the backend names none; `backend`
    /// names the backend in error messages.
    pub fn new(inner: S, fallback_model: &str, backend: &str) -> Self {
        Self {
            inner,
            parser: Parser::new(),
            decoder: ChunkDecoder::new(),
            encoder: StreamEncoder::new(fallback_model),
            backend: backend.to_owned(),
            done: false,
        }
    }
}

impl<S> Translator<S> {
    fn fail(&mut self, detail: &str, out: &mut String) {
        self.encoder
            .error(&format!("[backend {}] {detail}", self.backend), out);
    }

    /// Frames of `chunk` as output text; a bad frame ends the message.
    fn translate(&mut self, chunk: &[u8], out: &mut String) {
        let frames = match self.parser.feed(chunk) {
            Ok(frames) => frames,
            Err(error) => {
                self.fail(&format!("malformed event stream: {error}"), out);
                return;
            }
        };
        for frame in frames {
            match self.decoder.decode(&frame.data) {
                Ok(events) => {
                    for event in events {
                        self.encoder.encode(event, out);
                    }
                }
                Err(error) => {
                    self.fail(&format!("malformed event: {error}"), out);
                    return;
                }
            }
        }
    }

    fn end(&mut self, out: &mut String) {
        match self.parser.finish() {
            Ok(Some(frame)) => match self.decoder.decode(&frame.data) {
                Ok(events) => {
                    for event in events {
                        self.encoder.encode(event, out);
                    }
                }
                Err(error) => self.fail(&format!("malformed event: {error}"), out),
            },
            Ok(None) => {}
            Err(error) => self.fail(&format!("malformed event stream: {error}"), out),
        }
        self.encoder.finish(out);
    }
}

impl<S> Stream for Translator<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.done {
            return Poll::Ready(None);
        }
        let mut out = String::new();
        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(chunk))) => this.translate(&chunk, &mut out),
                Poll::Ready(Some(Err(error))) => {
                    this.fail(&format!("connection lost mid-stream: {error}"), &mut out);
                }
                Poll::Ready(None) => {
                    this.done = true;
                    this.end(&mut out);
                    return if out.is_empty() {
                        Poll::Ready(None)
                    } else {
                        Poll::Ready(Some(Ok(Bytes::from(out))))
                    };
                }
            }
            if !out.is_empty() {
                return Poll::Ready(Some(Ok(Bytes::from(out))));
            }
        }
    }
}
