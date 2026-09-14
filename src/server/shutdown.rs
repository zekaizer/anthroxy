//! Cutting what a stop's grace period left in flight, while the runtime still
//! runs, so each exchange ends through its own bookkeeping.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{Body, BodyDataStream, Bytes, HttpBody};
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use futures_util::Stream;

use super::{AppState, RequestId, RouterError};

/// Races the handler against [`AppState::cut`] and makes a streamed body end
/// with an error when the cut comes. A body of known length is already in
/// memory and is left alone.
pub async fn cut_on_stop(
    State(app): State<AppState>,
    request_id: RequestId,
    request: Request,
    next: Next,
) -> Response {
    let response = tokio::select! {
        response = next.run(request) => response,
        () = app.cut() => return RouterError::Stopping.into_response(&request_id),
    };
    if response.body().size_hint().exact().is_some() {
        return response;
    }
    response.map(|body| {
        Body::from_stream(Cut {
            inner: body.into_data_stream(),
            cut: Some(Box::pin(app.cut())),
        })
    })
}

struct Cut {
    inner: BodyDataStream,
    /// `None` once the cut came; the body has ended.
    cut: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl Stream for Cut {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(cut) = &mut self.cut else {
            return Poll::Ready(None);
        };
        if cut.as_mut().poll(cx).is_ready() {
            self.cut = None;
            return Poll::Ready(Some(Err(axum::Error::new(RouterError::Stopping))));
        }
        Pin::new(&mut self.inner).poll_next(cx)
    }
}
