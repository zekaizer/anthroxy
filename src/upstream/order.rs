//! The order of the headers of an HTTP/1.1 request (ADR-0022).

use http::header::{ACCEPT, CONTENT_LENGTH, HOST, HeaderMap, HeaderName, HeaderValue};

/// A preset's `header_order`: the names sent first, in this order. Empty
/// changes nothing. Not empty only on a route kept on HTTP/1.1, the one
/// version whose headers have an order.
#[derive(Debug, Clone, Default)]
pub struct HeaderOrder(Vec<HeaderName>);

impl HeaderOrder {
    pub fn new(names: Vec<HeaderName>) -> Self {
        Self(names)
    }

    /// Puts the headers of `request` in the order they are sent in: the
    /// listed names first, in list order, and the others after them in the
    /// order the HTTP client would write them. A listed name the request
    /// lacks is skipped.
    ///
    /// `request` is what the request builder made, so the basic auth of a
    /// URL with userinfo is among its headers. What the HTTP client adds
    /// after that is added here instead: a header it wrote itself would
    /// stand outside the order.
    pub(super) fn arrange(&self, request: &mut reqwest::Request) {
        if self.0.is_empty() {
            return;
        }
        let late = added_late(request);
        let headers = request.headers_mut();
        for (name, value) in late {
            if let Some(value) = value {
                headers.entry(name).or_insert(value);
            }
        }
        // A fresh map: removing from one moves its last name into the gap.
        let mut arranged = HeaderMap::with_capacity(headers.len());
        for name in &self.0 {
            for value in headers.get_all(name) {
                arranged.append(name.clone(), value.clone());
            }
        }
        for (name, value) in headers.iter() {
            if !self.0.contains(name) {
                arranged.append(name.clone(), value.clone());
            }
        }
        *headers = arranged;
    }
}

/// The headers the HTTP client adds to `request` where it lacks them, in the
/// order it writes them, which is after every other, each with the value it
/// would write. `host` is read from the URL as parsed, which is in lower
/// case; a request without a body gets no `content-length`.
fn added_late(request: &reqwest::Request) -> [(HeaderName, Option<HeaderValue>); 3] {
    let host = super::headers::host(request.url().as_str());
    let body_len = request
        .body()
        .and_then(|body| body.as_bytes())
        .map_or(0, <[u8]>::len);
    [
        (ACCEPT, Some(HeaderValue::from_static("*/*"))),
        (
            HOST,
            host.and_then(|host| HeaderValue::from_str(&host).ok()),
        ),
        (
            CONTENT_LENGTH,
            (body_len > 0).then(|| HeaderValue::from(body_len)),
        ),
    ]
}
