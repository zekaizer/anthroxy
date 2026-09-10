use crate::anthropic::DecodeError;

/// Messages request body → Chat Completions request body naming
/// `upstream_model`.
pub fn request(anthropic_body: &[u8], upstream_model: &str) -> Result<Vec<u8>, DecodeError> {
    let mut ir = crate::anthropic::decode(anthropic_body)?;
    ir.model = upstream_model.to_owned();
    Ok(crate::openai::encode_request(&ir))
}
