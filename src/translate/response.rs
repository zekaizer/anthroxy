use crate::ir::Message;
use crate::openai::ResponseError;

/// Completed Chat Completions body → Messages response document.
/// `fallback_model` is reported when the backend names none.
pub fn response(openai_body: &[u8], fallback_model: &str) -> Result<Vec<u8>, ResponseError> {
    let events = crate::openai::decode_response(openai_body)?;
    let mut message = Message::from_events(events).map_err(ResponseError::Backend)?;
    if message.model.is_empty() {
        message.model = fallback_model.to_owned();
    }
    if message.id.is_empty() {
        message.id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    }
    Ok(crate::anthropic::encode_message(&message))
}
