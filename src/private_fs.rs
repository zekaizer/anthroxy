//! Files the router creates that hold what a user sent or did: owner-only
//! whatever the umask says.

use std::path::Path;

/// Creates `dir` and its missing parents as `0700`. A directory that already
/// exists keeps the mode it has; the operator owns that one.
#[cfg(unix)]
pub fn create_dir_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
pub fn create_dir_private(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Writes owner-only. The mode is in place before the body is, so the file is
/// never briefly world-readable.
#[cfg(unix)]
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)
}

#[cfg(not(unix))]
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}
