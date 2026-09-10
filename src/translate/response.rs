use crate::ir::Message;
use crate::openai::ResponseError;

/// Completed Chat Completions body → Messages response document.
/// `fallback_model` is reported when the backend names none.
pub fn response(openai_body: &[u8], fallback_model: &str) -> Result<Vec<u8>, ResponseError> {
    let events = crate::openai::decode_response(openai_body)?;
    let message = Message::from_events(events).map_err(ResponseError::Backend)?;
    Ok(crate::anthropic::encode_message(&message, fallback_model))
}

/// Completed Chat Completions body → the Messages event stream it stands
/// for, for a client that asked for events and got a document.
pub fn document_events(openai_body: &[u8], fallback_model: &str) -> Result<String, ResponseError> {
    let events = crate::openai::decode_response(openai_body)?;
    let mut encoder = crate::anthropic::StreamEncoder::new(fallback_model);
    let mut out = String::new();
    encoder.encode_all(events, &mut out);
    encoder.finish(&mut out);
    Ok(out)
}
