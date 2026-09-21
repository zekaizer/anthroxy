//! Reading an upstream body whole: error documents and the responses an
//! `openai` backend answers as one document.

use bytes::{Bytes, BytesMut};

use crate::observability::Recorder;
use crate::server::RouterError;
use crate::server::relay::RelayOutcome;
use crate::upstream::{BodyError, UpstreamError, UpstreamResponse};

/// Largest body read whole; a backend that sends more is failed rather than
/// buffered without bound.
pub const MAX_BUFFERED_BYTES: usize = 16 * 1024 * 1024;

/// The whole body, recorded as received; a body that breaks off or outgrows
/// [`MAX_BUFFERED_BYTES`] is recorded as the upstream failure it is. The
/// caller logs the outcome.
pub async fn read_all(
    upstream: UpstreamResponse,
    backend: &str,
    recorder: Option<Recorder>,
) -> Result<Bytes, RouterError> {
    use futures_util::StreamExt;
    let mut stream = upstream.bytes_stream();
    let mut body = BytesMut::new();
    let read = loop {
        match stream.next().await {
            Some(Ok(chunk)) if body.len() + chunk.len() > MAX_BUFFERED_BYTES => {
                break Err(UpstreamError::BodyTooLarge {
                    backend: backend.to_owned(),
                    limit: MAX_BUFFERED_BYTES,
                });
            }
            Some(Ok(chunk)) => body.extend_from_slice(&chunk),
            None => break Ok(body.freeze()),
            Some(Err(BodyError::Upstream(source))) => {
                break Err(UpstreamError::Body {
                    backend: backend.to_owned(),
                    source,
                });
            }
            Some(Err(BodyError::TimedOut(clock))) => {
                break Err(UpstreamError::TimedOut {
                    backend: backend.to_owned(),
                    clock,
                });
            }
        }
    };
    match read {
        Ok(raw) => {
            if let Some(recorder) = recorder {
                recorder.finish_with_body(&raw);
            }
            Ok(raw)
        }
        Err(error) => {
            if let Some(recorder) = recorder {
                recorder.finish(&RelayOutcome::UpstreamError(error.to_string()));
            }
            Err(error.into())
        }
    }
}
