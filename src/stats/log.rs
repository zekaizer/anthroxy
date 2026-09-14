//! `requests-YYYY-MM-DD.jsonl` files under one directory, by the UTC date a
//! request arrived.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::civil::Date;

use super::StatsRecord;
use crate::private_fs::{PendingWrite, append_private, create_dir_private};

#[derive(Clone)]
pub struct StatsLog {
    inner: Arc<Inner>,
}

struct Inner {
    dir: PathBuf,
    /// `None` keeps files forever.
    retention: Option<Duration>,
    /// Serializes appends within the process.
    append: Mutex<()>,
}

impl StatsLog {
    /// Creates `dir` owner-only when missing. `retention` of zero disables
    /// pruning.
    pub fn open(dir: &Path, retention: Duration) -> std::io::Result<Self> {
        create_dir_private(dir)?;
        Ok(Self {
            inner: Arc::new(Inner {
                dir: dir.to_path_buf(),
                retention: (!retention.is_zero()).then_some(retention),
                append: Mutex::new(()),
            }),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    /// Appends off the request path; a failure is logged, never propagated.
    pub fn append(&self, record: StatsRecord) {
        let log = self.clone();
        let pending = PendingWrite::begin();
        let write = move || {
            let _pending = pending;
            if let Err(error) = log.append_blocking(&record) {
                tracing::error!(dir = %log.dir().display(), %error, "cannot write statistics record");
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(write)),
            Err(_) => write(),
        }
    }

    /// Appends one line to the file of the record's date. The directory is
    /// created again if something removed it.
    pub fn append_blocking(&self, record: &StatsRecord) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(record).expect("records serialize");
        line.push(b'\n');
        let _one_writer = self
            .inner
            .append
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        create_dir_private(&self.inner.dir)?;
        append_private(&self.file(utc_date(record.ts)), &line)
    }

    /// Records that arrived at or after `since`, oldest file first. Lines
    /// that do not parse are skipped.
    pub fn read(&self, since: Option<jiff::Timestamp>) -> Vec<StatsRecord> {
        let first_day = since.map(utc_date);
        let mut files: Vec<(Date, PathBuf)> = self
            .files()
            .into_iter()
            .filter(|(date, _)| first_day.is_none_or(|first| *date >= first))
            .collect();
        files.sort();
        let mut records = Vec::new();
        for (_, path) in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            records.extend(
                text.lines()
                    .filter_map(|line| serde_json::from_str::<StatsRecord>(line).ok())
                    .filter(|record| since.is_none_or(|since| record.ts >= since)),
            );
        }
        records
    }

    /// Deletes files whose whole day is older than `now - retention`.
    /// Returns how many were removed; other files are left alone.
    pub fn prune(&self, now: jiff::Timestamp) -> usize {
        let Some(retention) = self.inner.retention else {
            return 0;
        };
        let Ok(cutoff) = jiff::SignedDuration::try_from(retention)
            .map_err(|_| ())
            .and_then(|retention| now.checked_sub(retention).map_err(|_| ()))
        else {
            return 0;
        };
        let cutoff = utc_date(cutoff);
        let mut removed = 0;
        for (date, path) in self.files() {
            if date >= cutoff {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "cannot prune statistics file");
                }
            }
        }
        removed
    }

    fn file(&self, date: Date) -> PathBuf {
        self.inner.dir.join(format!("requests-{date}.jsonl"))
    }

    /// Statistics files in the directory with their dates, unordered.
    fn files(&self) -> Vec<(Date, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(&self.inner.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let date = name
                    .to_str()?
                    .strip_prefix("requests-")?
                    .strip_suffix(".jsonl")?
                    .parse::<Date>()
                    .ok()?;
                Some((date, entry.path()))
            })
            .collect()
    }
}

fn utc_date(at: jiff::Timestamp) -> Date {
    at.to_zoned(jiff::tz::TimeZone::UTC).date()
}
