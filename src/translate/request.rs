use crate::anthropic::DecodeError;
use crate::body_field::{BodyField, set_all};
use crate::openai::SystemPlacement;

/// Messages request body → Chat Completions request body naming
/// `upstream_model`, mid-conversation system messages placed per `placement`,
/// then every one of `set_fields` set in it (ADR-0019).
pub fn request(
    anthropic_body: &[u8],
    upstream_model: &str,
    placement: SystemPlacement,
    set_fields: &[BodyField],
) -> Result<Vec<u8>, DecodeError> {
    let mut ir = crate::anthropic::decode(anthropic_body)?;
    ir.model = upstream_model.to_owned();
    let mut body = crate::openai::encode_request(&ir, placement);
    set_all(&mut body, set_fields);
    Ok(serde_json::to_vec(&body).expect("a JSON tree serializes"))
}
