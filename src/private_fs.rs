//! Files the router creates that hold what a user sent or did: owner-only
//! whatever the umask says. Writes queued off the request path are counted,
//! so a stopping server can wait for them before the runtime goes.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

static PENDING: AtomicUsize = AtomicUsize::new(0);
static SETTLED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// One queued write, counted process-wide until dropped. Move it into the
/// task that does the write.
#[must_use]
pub struct PendingWrite(());

impl PendingWrite {
    pub fn begin() -> Self {
        PENDING.fetch_add(1, Ordering::SeqCst);
        Self(())
    }
}

impl Drop for PendingWrite {
    fn drop(&mut self) {
        if PENDING.fetch_sub(1, Ordering::SeqCst) == 1 {
            SETTLED.notify_waiters();
        }
    }
}

/// Resolves once no [`PendingWrite`] is alive.
pub async fn settled() {
    loop {
        let notified = SETTLED.notified();
        tokio::pin!(notified);
        // Registered before the count is read, so a drop in between wakes it.
        notified.as_mut().enable();
        if PENDING.load(Ordering::SeqCst) == 0 {
            return;
        }
        notified.await;
    }
}

/// Creates `dir` and its missing parents as `0700`. A directory that already
/// exists keeps the mode it has; the operator owns that one. A symbolic link
/// at `dir`, or a directory another account owns, is refused: a shared
/// parent such as `/var/tmp` lets anyone plant one before the router starts.
#[cfg(unix)]
pub fn create_dir_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Err(std::io::Error::other("is a symbolic link"));
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::other("is owned by another account"));
    }
    Ok(())
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
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?
        .write_all(bytes)
}

#[cfg(not(unix))]
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

/// Appends to `path`, creating it owner-only when missing.
#[cfg(unix)]
pub fn append_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?
        .write_all(bytes)
}

#[cfg(not(unix))]
pub fn append_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?
        .write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn a_planted_symbolic_link_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let link = dir.path().join("state");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        assert!(create_dir_private(&link).is_err(), "a linked directory");

        let target = dir.path().join("target.txt");
        std::fs::write(&target, "kept").unwrap();
        let file_link = dir.path().join("file.link");
        std::os::unix::fs::symlink(&target, &file_link).unwrap();
        assert!(write_private(&file_link, b"x").is_err(), "a linked file");
        assert!(append_private(&file_link, b"x").is_err(), "a linked file");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept");

        let real = dir.path().join("real");
        create_dir_private(&real).unwrap();
        write_private(&real.join("a"), b"a").unwrap();
        append_private(&real.join("a"), b"b").unwrap();
        assert_eq!(std::fs::read_to_string(real.join("a")).unwrap(), "ab");
    }
}
