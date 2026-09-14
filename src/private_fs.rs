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

/// Appends to `path`, creating it owner-only when missing.
#[cfg(unix)]
pub fn append_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
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
