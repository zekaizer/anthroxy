use crate::anthropic::DecodeError;
use crate::body_field::BodyEdits;
use crate::openai::EncodeOptions;

/// Messages request body → Chat Completions request body naming
/// `upstream_model`, written per `options`, then `edits` applied to it
/// (ADR-0019, ADR-0022).
pub fn request(
    anthropic_body: &[u8],
    upstream_model: &str,
    options: &EncodeOptions,
    edits: &BodyEdits<'_>,
) -> Result<Vec<u8>, DecodeError> {
    let mut ir = crate::anthropic::decode(anthropic_body)?;
    ir.model = upstream_model.to_owned();
    let mut body = crate::openai::encode_request(&ir, options);
    edits.apply(&mut body);
    Ok(serde_json::to_vec(&body).expect("a JSON tree serializes"))
}
