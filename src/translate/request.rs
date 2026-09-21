use crate::anthropic::DecodeError;
use crate::openai::SystemPlacement;

/// Messages request body → Chat Completions request body naming
/// `upstream_model`, mid-conversation system messages placed per `placement`.
pub fn request(
    anthropic_body: &[u8],
    upstream_model: &str,
    placement: SystemPlacement,
) -> Result<Vec<u8>, DecodeError> {
    let mut ir = crate::anthropic::decode(anthropic_body)?;
    ir.model = upstream_model.to_owned();
    Ok(crate::openai::encode_request(&ir, placement))
}
