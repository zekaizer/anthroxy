//! The page, embedded in the binary. It carries no data, so it needs no
//! token; the policy keeps it to its own script, style and API.

use axum::response::{IntoResponse, Redirect, Response};
use http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};

const INDEX: &str = include_str!("assets/index.html");
const SCRIPT: &str = include_str!("assets/app.js");
const STYLE: &str = include_str!("assets/app.css");

const POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (CONTENT_TYPE, content_type),
            (CONTENT_SECURITY_POLICY, POLICY),
            (X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (X_FRAME_OPTIONS, "DENY"),
            (REFERRER_POLICY, "no-referrer"),
            (CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

/// `/` and `/ui` lead to the page.
pub async fn root() -> Redirect {
    Redirect::to("/ui/")
}

pub async fn index() -> Response {
    asset("text/html; charset=utf-8", INDEX)
}

pub async fn script() -> Response {
    asset("text/javascript; charset=utf-8", SCRIPT)
}

pub async fn style() -> Response {
    asset("text/css; charset=utf-8", STYLE)
}
