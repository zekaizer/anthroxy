//! How the requests along one route are written.

use super::{Backend, ForcedHeaders};
use crate::config::{BackendConfig, DropHeaders, ModelConfig, SetFields};
use crate::openai::EncodeOptions;

/// How requests along one route are written: the backend's `headers`,
/// `drop_headers`, `set_fields` and `mid_conversation_system`, with what the
/// route's `[[models]]` entry sets in their place. Built once per snapshot.
#[derive(Debug)]
pub struct Dialect {
    pub headers: HeaderRules,
    pub body: BodyRules,
}

/// The headers of every request to the backend, the model list included.
#[derive(Debug, Default)]
pub struct HeaderRules {
    /// Headers forced onto the request.
    pub forced: ForcedHeaders,
    /// Client headers the backend never sees.
    pub drop: DropHeaders,
}

/// The body of a chat request.
#[derive(Debug, Default)]
pub struct BodyRules {
    /// Fields set in what the backend receives.
    pub set_fields: SetFields,
    /// `kind = "openai"`: how the translated body is written.
    pub encode: EncodeOptions,
}

impl Dialect {
    /// The dialect of the route `model` makes on `backend`; without a model,
    /// of a request no route makes (the model list, a probe). A key the entry
    /// sets replaces the backend's. Assumes both passed validation.
    pub fn new(backend: &BackendConfig, model: Option<&ModelConfig>) -> Self {
        Self {
            headers: HeaderRules {
                forced: ForcedHeaders::new(&backend.headers),
                drop: DropHeaders::new(&backend.drop_headers),
            },
            body: BodyRules {
                set_fields: SetFields::new(&backend.set_fields),
                encode: EncodeOptions {
                    placement: model
                        .and_then(|model| model.mid_conversation_system)
                        .unwrap_or(backend.mid_conversation_system),
                },
            },
        }
    }
}

/// A backend and the header rules of the route a request travels.
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    backend: &'a Backend,
    headers: &'a HeaderRules,
}

impl<'a> Target<'a> {
    /// `headers` must be rules built for `backend`: only
    /// [`Backend::target`] and [`crate::routing::Route::target`] call this.
    pub(crate) fn new(backend: &'a Backend, headers: &'a HeaderRules) -> Self {
        Self { backend, headers }
    }

    pub(super) fn backend(self) -> &'a Backend {
        self.backend
    }

    pub(super) fn headers(self) -> &'a HeaderRules {
        self.headers
    }
}
