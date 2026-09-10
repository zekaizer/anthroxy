/// Largest pending frame; a backend that never sends a blank line is cut off
/// here instead of growing memory without bound.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// One event's payload: its `data:` lines joined with `\n`. Frames without
/// data (comments, keep-alives, a bare `event:` line, an empty `data:`)
/// are not reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub data: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SseError {
    #[error("event stream is not valid UTF-8")]
    Utf8,
    #[error("one event exceeds {MAX_FRAME_BYTES} bytes")]
    TooLarge,
}

/// Splits a byte stream into frames as it arrives. Bytes after the last
/// complete frame are kept for the next call; an error discards them.
#[derive(Debug, Default)]
pub struct Parser {
    buffer: Vec<u8>,
    /// Bytes of `buffer` already known to hold no frame end.
    scanned: usize,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Frame>, SseError> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > MAX_FRAME_BYTES {
            self.reset();
            return Err(SseError::TooLarge);
        }
        let mut frames = Vec::new();
        let mut consumed = 0;
        while let Some((end, next)) = frame_end(&self.buffer[consumed..], self.scanned) {
            let raw = &self.buffer[consumed..consumed + end];
            consumed += next;
            self.scanned = 0;
            match parse_frame(raw) {
                Ok(Some(frame)) => frames.push(frame),
                Ok(None) => {}
                Err(error) => {
                    self.reset();
                    return Err(error);
                }
            }
        }
        self.buffer.drain(..consumed);
        // Everything up to the last newline is known not to end a frame.
        self.scanned = self
            .buffer
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |at| at + 1);
        Ok(frames)
    }

    /// The frames left in the buffer at end of stream: one when the last
    /// frame had data but no terminating blank line, none otherwise.
    pub fn finish(&mut self) -> Result<Vec<Frame>, SseError> {
        let raw = std::mem::take(&mut self.buffer);
        self.scanned = 0;
        Ok(parse_frame(&raw)?.into_iter().collect())
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.scanned = 0;
    }
}

/// Offset where the first frame ends (before its terminating blank line) and
/// the offset just past that blank line, scanning from `from`, which must
/// sit at a line start.
fn frame_end(buf: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut start = from.min(buf.len());
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

/// `None` for a frame without data (no `data:` line, or an empty one).
fn parse_frame(raw: &[u8]) -> Result<Option<Frame>, SseError> {
    let text = std::str::from_utf8(raw).map_err(|_| SseError::Utf8)?;
    let mut data: Option<Vec<&str>> = None;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(value) = line.strip_prefix("data:") {
            data.get_or_insert_with(Vec::new)
                .push(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    Ok(data
        .map(|lines| lines.join("\n"))
        .filter(|data| !data.is_empty())
        .map(|data| Frame { data }))
}
