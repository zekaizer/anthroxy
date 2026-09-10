//! Reading an upstream body whole: error documents and the responses an
//! `openai` backend answers as one document.

use bytes::Bytes;

use crate::observability::Recorder;
use crate::server::RouterError;
use crate::upstream::{UpstreamError, UpstreamResponse};

/// The whole body, recorded as received. The caller logs the outcome.
pub async fn read_all(
    upstream: UpstreamResponse,
    backend: &str,
    recorder: Option<Recorder>,
) -> Result<Bytes, RouterError> {
    let raw = upstream
        .response
        .bytes()
        .await
        .map_err(|source| UpstreamError::Body {
            backend: backend.to_owned(),
            source,
        })?;
    if let Some(recorder) = recorder {
        recorder.finish_with_body(&raw);
    }
    Ok(raw)
}
