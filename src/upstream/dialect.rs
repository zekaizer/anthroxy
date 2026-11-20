//! How the requests along one route are written.

use super::{Backend, ForcedHeaders, Transport};
use crate::body_field::{BodyEdits, BodyField};
use crate::config::{BackendConfig, DropHeaders, ModelConfig, Preset, PresetLibrary, SetFields};
use crate::openai::EncodeOptions;

/// How requests along one route are written: the backend's `headers`,
/// `drop_headers`, `set_fields` and `mid_conversation_system` over the preset
/// in effect, with what the route's `[[models]]` entry sets in their place.
/// Built once per snapshot.
#[derive(Debug)]
pub struct Dialect {
    pub headers: HeaderRules,
    pub body: BodyRules,
}

/// The headers of every request to the backend, the model list included.
#[derive(Debug, Default)]
pub struct HeaderRules {
    /// Headers forced onto the request: the backend's `headers`, and the
    /// preset's for the names it does not set.
    pub forced: ForcedHeaders,
    /// Client headers the backend never sees: the preset's `drop_headers`
    /// and the backend's.
    pub drop: DropHeaders,
    /// Which HTTP client sends the request.
    pub transport: Transport,
}

/// The body of a chat request.
#[derive(Debug, Default)]
pub struct BodyRules {
    /// Fields set in what the backend receives: the preset's, then the
    /// backend's.
    pub set_fields: SetFields,
    /// `kind = "openai"`: how the translated body is written.
    pub encode: EncodeOptions,
    /// `kind = "openai"`: fields of the translated body the backend does not
    /// receive, as dot-separated paths.
    pub omit_fields: Vec<String>,
    /// `kind = "openai"`: top-level fields of the translated body that are
    /// written first, in this order.
    pub field_order: Vec<String>,
}

impl BodyRules {
    /// What is done to the translated body once it is written; `set` is
    /// [`Self::set_fields`] as one request fills them.
    pub fn edits<'a>(&'a self, set: &'a [BodyField]) -> BodyEdits<'a> {
        BodyEdits {
            omit: &self.omit_fields,
            set,
            order: &self.field_order,
        }
    }
}

impl Dialect {
    /// The dialect of the route `model` makes on `backend`; without a model,
    /// of a request no route makes (the model list, a probe). A key the entry
    /// sets replaces the backend's, `emulate` included; a key the backend
    /// writes wins over the preset's (ADR-0022). Assumes both passed
    /// validation against `presets`.
    pub fn new(
        backend: &BackendConfig,
        model: Option<&ModelConfig>,
        presets: &PresetLibrary,
    ) -> Self {
        let emulation = model
            .and_then(|model| model.emulate.as_ref())
            .or(backend.emulate.as_ref())
            .map(|emulate| presets.bind(emulate).expect("validated emulate"));
        let (preset_headers, preset_fields) = match &emulation {
            Some(emulation) => (emulation.headers(), emulation.set_fields()),
            None => Default::default(),
        };
        // Without `emulate`, a preset that says nothing.
        let no_preset = Preset::default();
        let preset = emulation
            .as_ref()
            .map_or(&no_preset, |emulation| &*emulation.preset);
        let drop = [preset.drop_headers.as_slice(), &backend.drop_headers].concat();
        Self {
            headers: HeaderRules {
                forced: ForcedHeaders::new(&backend.headers).over(preset_headers),
                drop: DropHeaders::new(&drop),
                transport: Transport::default(),
            },
            body: BodyRules {
                set_fields: preset_fields.then(SetFields::new(&backend.set_fields)),
                encode: EncodeOptions {
                    placement: model
                        .and_then(|model| model.mid_conversation_system)
                        .or(backend.mid_conversation_system)
                        .or(preset.mid_conversation_system)
                        .unwrap_or_default(),
                    max_tokens_field: preset.max_tokens_field,
                },
                omit_fields: preset.omit_fields.clone(),
                field_order: preset.field_order.clone(),
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
