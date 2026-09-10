//! Reading an upstream body whole: error documents and the responses an
//! `openai` backend answers as one document.

use bytes::Bytes;

use crate::observability::Recorder;
use crate::server::RouterError;
use crate::server::relay::RelayOutcome;
use crate::upstream::{UpstreamError, UpstreamResponse};

/// The whole body, recorded as received; a body that breaks off is
/// recorded as the upstream failure it is. The caller logs the outcome.
pub async fn read_all(
    upstream: UpstreamResponse,
    backend: &str,
    recorder: Option<Recorder>,
) -> Result<Bytes, RouterError> {
    match upstream.response.bytes().await {
        Ok(raw) => {
            if let Some(recorder) = recorder {
                recorder.finish_with_body(&raw);
            }
            Ok(raw)
        }
        Err(source) => {
            let error = UpstreamError::Body {
                backend: backend.to_owned(),
                source,
            };
            if let Some(recorder) = recorder {
                recorder.finish(&RelayOutcome::UpstreamError(error.to_string()));
            }
            Err(error.into())
        }
    }
}
