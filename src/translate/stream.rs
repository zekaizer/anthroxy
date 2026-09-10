use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;

use crate::anthropic::StreamEncoder;
use crate::openai::ChunkDecoder;
use crate::sse::{Frame, Parser, SseError};

/// Chat Completions SSE bytes in, Messages SSE bytes out, one output chunk
/// per input chunk at most. After a malformed event or an `error` frame one
/// `error` event is emitted and the rest of the input is consumed in
/// silence, so the upstream body still ends normally for whoever records
/// it. A transport error ends the output at once; the inner stream is not
/// polled again.
pub struct Translator<S> {
    inner: S,
    parser: Parser,
    decoder: ChunkDecoder,
    encoder: StreamEncoder,
    backend: String,
    /// `inner` ended or failed; it is not polled again.
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

    /// Encodes the frames the parser produced; a bad frame ends the message.
    fn frames(&mut self, parsed: Result<Vec<Frame>, SseError>, out: &mut String) {
        let frames = match parsed {
            Ok(frames) => frames,
            Err(error) => return self.fail(&format!("malformed event stream: {error}"), out),
        };
        for frame in frames {
            match self.decoder.decode(&frame.data) {
                Ok(events) => self.encoder.encode_all(events, out),
                Err(error) => return self.fail(&format!("malformed event: {error}"), out),
            }
        }
    }

    /// End of input: the last frame, then what the decoder still holds.
    fn end(&mut self, out: &mut String) {
        let parsed = self.parser.finish();
        self.frames(parsed, out);
        let held = self.decoder.finish();
        self.encoder.encode_all(held, out);
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
                Poll::Ready(Some(Ok(chunk))) => {
                    let parsed = this.parser.feed(&chunk);
                    this.frames(parsed, &mut out);
                }
                Poll::Ready(Some(Err(error))) => {
                    this.done = true;
                    this.fail(&format!("connection lost mid-stream: {error}"), &mut out);
                }
                Poll::Ready(None) => {
                    this.done = true;
                    this.end(&mut out);
                }
            }
            if this.done || !out.is_empty() {
                break;
            }
        }
        Poll::Ready((!out.is_empty()).then(|| Ok(Bytes::from(out))))
    }
}
