/// Largest pending frame; a backend that never sends a blank line is cut off
/// here instead of growing memory without bound.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// One event: the `event:` name, if any, and the `data:` lines joined with
/// `\n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SseError {
    #[error("event stream is not valid UTF-8")]
    Utf8,
    #[error("one event exceeds {0} bytes")]
    TooLarge(usize),
}

/// Splits a byte stream into frames as it arrives. Bytes after the last
/// complete frame are kept for the next call.
#[derive(Debug, Default)]
pub struct Parser {
    buffer: Vec<u8>,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Frame>, SseError> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > MAX_FRAME_BYTES {
            self.buffer.clear();
            return Err(SseError::TooLarge(MAX_FRAME_BYTES));
        }
        let mut frames = Vec::new();
        let mut consumed = 0;
        while let Some((end, next)) = frame_end(&self.buffer[consumed..]) {
            let raw = &self.buffer[consumed..consumed + end];
            consumed += next;
            match parse_frame(raw) {
                Ok(Some(frame)) => frames.push(frame),
                Ok(None) => {}
                Err(error) => {
                    self.buffer.clear();
                    return Err(error);
                }
            }
        }
        self.buffer.drain(..consumed);
        Ok(frames)
    }

    /// The frame left in the buffer at end of stream, if it has content.
    pub fn finish(&mut self) -> Result<Option<Frame>, SseError> {
        let raw = std::mem::take(&mut self.buffer);
        parse_frame(&raw)
    }
}

/// Offset where the first frame ends (before its terminating blank line) and
/// the offset just past that blank line.
fn frame_end(buf: &[u8]) -> Option<(usize, usize)> {
    let mut start = 0;
    while let Some(len) = buf[start..].iter().position(|&b| b == b'\n') {
        let line = &buf[start..start + len];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return Some((start, start + len + 1));
        }
        start += len + 1;
    }
    None
}

/// `None` for a frame with neither `event:` nor `data:` (comments only).
fn parse_frame(raw: &[u8]) -> Result<Option<Frame>, SseError> {
    let text = std::str::from_utf8(raw).map_err(|_| SseError::Utf8)?;
    let mut event = None;
    let mut data: Option<Vec<&str>> = None;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "" => {}
            "data" => data.get_or_insert_with(Vec::new).push(value),
            "event" => event = Some(value.to_owned()),
            _ => {}
        }
    }
    if event.is_none() && data.is_none() {
        return Ok(None);
    }
    Ok(Some(Frame {
        event,
        data: data.unwrap_or_default().join("\n"),
    }))
}
