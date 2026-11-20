//! Small persisted totals. The worker owns all writes and coalesces updates.
use crate::storage::Storage;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    path::Path,
    sync::{mpsc, Mutex},
    time::Duration,
};

const VERSION: u32 = 1;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanSummary {
    pub version: u32,
    /// caches, dev, system, large, apps, leftovers, models, games
    pub bytes: [u64; 8],
    /// large, apps, leftovers, models, games
    pub counts: [usize; 5],
    pub trash_bytes: u64,
    pub saved_at: i64,
    // Preserve the overview chart and safe amounts until fresh readings arrive.
    pub total: u64,
    pub free: u64,
    pub safe: [u64; 3],
}
impl Default for ScanSummary {
    fn default() -> Self {
        Self {
            version: VERSION,
            bytes: [0; 8],
            counts: [0; 5],
            trash_bytes: 0,
            saved_at: 0,
            total: 0,
            free: 0,
            safe: [0; 3],
        }
    }
}

pub fn load() -> Option<ScanSummary> {
    load_from(&Storage::data_dir().join("cache/summary.json"))
}
fn load_from(path: &Path) -> Option<ScanSummary> {
    // A summary is tiny; reject unexpectedly large files before parsing.
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(65_537)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 65_536 {
        return None;
    }
    let summary: ScanSummary = serde_json::from_slice(&bytes).ok()?;
    (summary.version == VERSION).then_some(summary)
}
fn save(path: &Path, summary: &ScanSummary) -> io::Result<()> {
    let bytes = serde_json::to_vec(summary)?;
    fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = fs::write(&temporary, bytes).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

enum Message {
    Save(ScanSummary),
    Flush(mpsc::Sender<io::Result<()>>),
}
struct Cache {
    latest: Mutex<Option<ScanSummary>>,
    sender: mpsc::Sender<Message>,
}
static CACHE: Lazy<Cache> = Lazy::new(|| {
    let latest = Mutex::new(load());
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let path = Storage::data_dir().join("cache/summary.json");
        let mut pending = None;
        loop {
            let message = if pending.is_some() {
                receiver.recv_timeout(Duration::from_secs(1))
            } else {
                receiver
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            };
            match message {
                Ok(Message::Save(summary)) => pending = Some(summary),
                Ok(Message::Flush(reply)) => {
                    let result = pending.as_ref().map_or(Ok(()), |s| save(&path, s));
                    if result.is_ok() {
                        pending = None;
                    }
                    let _ = reply.send(result);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(summary) = pending.take() {
                        if let Err(error) = save(&path, &summary) {
                            tracing::warn!("Could not save scan summary: {error}");
                            pending = Some(summary);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    Cache { latest, sender }
});

/// Called before GTK initialization so even the initial read stays off GTK.
pub fn init() {
    Lazy::force(&CACHE);
}
pub fn snapshot() -> Option<ScanSummary> {
    CACHE.latest.lock().unwrap().clone()
}
pub fn update(mut summary: ScanSummary) {
    let mut latest = CACHE.latest.lock().unwrap();
    summary.saved_at = latest.as_ref().map_or(0, |s| s.saved_at);
    if latest.as_ref() == Some(&summary) {
        return;
    }
    summary.saved_at = chrono::Utc::now().timestamp();
    *latest = Some(summary.clone());
    let _ = CACHE.sender.send(Message::Save(summary));
}
/// Wait for the worker to flush the latest small summary before closing.
pub fn flush() -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    CACHE
        .sender
        .send(Message::Flush(tx))
        .map_err(io::Error::other)?;
    rx.recv().map_err(io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache/summary.json");
        let summary = ScanSummary {
            bytes: [1, 2, 3, 4, 5, 6, 7, 8],
            counts: [9, 10, 11, 12, 13],
            trash_bytes: 14,
            saved_at: 15,
            ..Default::default()
        };
        save(&path, &summary).unwrap();
        assert_eq!(load_from(&path), Some(summary));
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
    #[test]
    fn missing_and_corrupt_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("summary.json");
        assert!(load_from(&path).is_none());
        fs::write(&path, b"{broken").unwrap();
        assert!(load_from(&path).is_none());
    }
    #[test]
    fn wrong_version_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("summary.json");
        save(
            &path,
            &ScanSummary {
                version: VERSION + 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(load_from(&path).is_none());
    }
}
