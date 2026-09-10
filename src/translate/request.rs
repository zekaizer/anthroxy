use crate::anthropic::DecodeError;

/// Messages request body → Chat Completions request body.
pub fn request(anthropic_body: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let ir = crate::anthropic::decode(anthropic_body)?;
    Ok(crate::openai::encode_request(&ir))
}
