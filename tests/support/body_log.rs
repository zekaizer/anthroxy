//! Reading a body log entry while the router is still writing it.

use std::path::Path;

/// Bytes of whichever response file the entry holds, zero when it holds none.
pub fn response_size(entry: &Path) -> usize {
    for name in ["response.sse", "response.json", "response.bin"] {
        if let Ok(meta) = std::fs::metadata(entry.join(name)) {
            return meta.len() as usize;
        }
    }
    0
}
