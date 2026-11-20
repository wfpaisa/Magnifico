//! Overwrite-then-unlink deletion for sensitive files, plus checks that tell
//! the user how much an overwrite can actually guarantee on their disk.
//!
//! An overwrite replaces the file's contents through the same inode, so on a
//! classic in-place filesystem (ext4, xfs) on a spinning disk the old data is
//! gone. On SSDs the drive's wear leveling may keep the old blocks around
//! until TRIM, and on copy-on-write filesystems (btrfs, zfs) the new data goes
//! to new blocks. Full-disk encryption is the only real guarantee there; the
//! UI says so instead of promising "unrecoverable".

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const CHUNK: usize = 1024 * 1024;

/// How a file was removed by [`secure_delete_file`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureOutcome {
    /// Contents overwritten with random data, truncated, renamed, unlinked.
    Overwritten,
    /// The file has other hard links: overwriting would destroy the data
    /// under its other names, so only this name was removed.
    UnlinkedOnlyHardLinked,
}

/// Overwrite `path` with random data, truncate it, give it a random name and
/// unlink it. `expected` is the metadata the caller verified (owner, not a
/// symlink); the file actually opened must be that same inode, so a file
/// swapped in after the check is never touched.
///
/// Cancellation stops the overwrite early, but the file is still truncated
/// and unlinked: a half-overwritten file is useless to keep.
pub fn secure_delete_file(
    path: &Path,
    expected: &fs::Metadata,
    cancelled: &AtomicBool,
) -> io::Result<SecureOutcome> {
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;

    let opened = file.metadata()?;
    if opened.dev() != expected.dev() || opened.ino() != expected.ino() {
        return Err(io::Error::other("file changed after the safety check"));
    }
    if !opened.file_type().is_file() {
        return Err(io::Error::other(
            "not a regular file; secure deletion skipped",
        ));
    }

    if opened.nlink() > 1 {
        drop(file);
        fs::remove_file(path)?;
        return Ok(SecureOutcome::UnlinkedOnlyHardLinked);
    }

    overwrite_with_random(&mut file, opened.len(), cancelled)?;
    file.sync_all()?;
    file.set_len(0)?;
    file.sync_all()?;
    drop(file);

    // Remove the original name from the directory entry as well.
    let renamed = rename_to_random_name(path, opened.ino())?;
    sync_parent(&renamed);
    fs::remove_file(&renamed)?;
    sync_parent(&renamed);

    Ok(SecureOutcome::Overwritten)
}

fn overwrite_with_random(file: &mut File, len: u64, cancelled: &AtomicBool) -> io::Result<()> {
    let mut buffer = vec![0u8; CHUNK];
    let mut remaining = len;
    while remaining > 0 {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let n = remaining.min(CHUNK as u64) as usize;
        fill_random(&mut buffer[..n])?;
        file.write_all(&buffer[..n])?;
        remaining -= n as u64;
    }
    file.flush()
}

fn fill_random(buffer: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        // SAFETY: the pointer and length describe the unfilled tail of `buffer`.
        let n = unsafe {
            libc::getrandom(
                buffer[filled..].as_mut_ptr().cast(),
                buffer.len() - filled,
                0,
            )
        };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        filled += n as usize;
    }
    Ok(())
}

/// Rename to a random name in the same directory without replacing anything
/// (`RENAME_NOREPLACE`), and confirm the renamed entry is still our inode.
fn rename_to_random_name(path: &Path, ino: u64) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let from = CString::new(path.as_os_str().as_bytes())?;

    for _ in 0..8 {
        let mut bytes = [0u8; 12];
        fill_random(&mut bytes)?;
        let name: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        let target = parent.join(format!(".magnifico-{}", name));
        let to = CString::new(target.as_os_str().as_bytes())?;

        // SAFETY: both arguments are valid NUL-terminated paths.
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            let moved = fs::symlink_metadata(&target)?;
            if moved.ino() != ino {
                // Someone swapped the entry between our checks; put it back.
                let _ = fs::rename(&target, path);
                return Err(io::Error::other("file changed during secure deletion"));
            }
            return Ok(target);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EEXIST) => continue,
            // Filesystems without RENAME_NOREPLACE: delete under the
            // original name rather than fail the whole cleanup.
            Some(libc::EINVAL) | Some(libc::ENOSYS) => return Ok(path.to_path_buf()),
            _ => return Err(error),
        }
    }
    Ok(path.to_path_buf())
}

fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
}

/// What an overwrite can guarantee for files under a given path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageFacts {
    /// The underlying disk reports itself as non-rotational (SSD/NVMe).
    pub solid_state: bool,
    /// Copy-on-write filesystem (btrfs, zfs, bcachefs): overwrites go to new blocks.
    pub copy_on_write: bool,
    /// The filesystem sits on a dm-crypt (LUKS) device.
    pub encrypted: bool,
}

pub fn storage_facts(path: &Path) -> StorageFacts {
    let mut facts = StorageFacts::default();

    if let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) {
        // SAFETY: statfs writes into the zeroed struct we own.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } == 0 {
            const BTRFS: i64 = 0x9123_683E;
            const ZFS: i64 = 0x2FC1_2FC1;
            const BCACHEFS: i64 = 0xCA45_1A4E;
            let kind = stat.f_type as i64;
            facts.copy_on_write = kind == BTRFS || kind == ZFS || kind == BCACHEFS;
        }
    }

    let Ok(meta) = fs::metadata(path) else {
        return facts;
    };
    let dev = meta.dev();
    let (major, minor) = (libc::major(dev), libc::minor(dev));
    let block = PathBuf::from(format!("/sys/dev/block/{}:{}", major, minor));
    let Ok(block) = fs::canonicalize(&block) else {
        return facts;
    };

    // Walk this device and everything below it (dm-crypt, LVM, partitions).
    let mut stack = vec![block];
    let mut seen = 0;
    while let Some(device) = stack.pop() {
        seen += 1;
        if seen > 32 {
            break;
        }
        if read_trimmed(&device.join("dm/uuid")).is_some_and(|u| u.starts_with("CRYPT-")) {
            facts.encrypted = true;
        }
        // A partition has no queue of its own; its parent disk does.
        let rotational = read_trimmed(&device.join("queue/rotational"))
            .or_else(|| read_trimmed(&device.parent()?.join("queue/rotational")));
        if rotational.as_deref() == Some("0") {
            facts.solid_state = true;
        }
        if let Ok(slaves) = fs::read_dir(device.join("slaves")) {
            for slave in slaves.flatten() {
                if let Ok(target) = fs::canonicalize(slave.path()) {
                    stack.push(target);
                }
            }
        }
    }

    facts
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::TempDir;

    #[test]
    fn overwrites_contents_before_unlinking() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cookies.sqlite");
        let secret = b"SECRET-SESSION-TOKEN".repeat(10_000);
        fs::write(&path, &secret).unwrap();

        // Keep a second handle open: after unlink it still sees the inode,
        // so we can check what was left on it.
        let mut witness = File::open(&path).unwrap();
        let meta = fs::symlink_metadata(&path).unwrap();
        let outcome = secure_delete_file(&path, &meta, &AtomicBool::new(false)).unwrap();

        assert_eq!(outcome, SecureOutcome::Overwritten);
        assert!(!path.exists());
        let mut left = Vec::new();
        witness.read_to_end(&mut left).unwrap();
        assert!(left.is_empty(), "file must be truncated");
        // No stray renamed file is left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn overwrite_replaces_every_byte() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history");
        let secret = vec![0xAAu8; CHUNK + 4096];
        fs::write(&path, &secret).unwrap();
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        overwrite_with_random(&mut file, secret.len() as u64, &AtomicBool::new(false)).unwrap();
        drop(file);
        let data = fs::read(&path).unwrap();
        assert_eq!(data.len(), secret.len());
        let same = data.iter().filter(|b| **b == 0xAA).count();
        // Random data matches 0xAA in about 1/256 of the bytes.
        assert!(same < data.len() / 100, "{} bytes unchanged", same);
    }

    #[test]
    fn hard_linked_file_keeps_its_other_name_intact() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a");
        let other = dir.path().join("b");
        fs::write(&path, b"shared data").unwrap();
        fs::hard_link(&path, &other).unwrap();

        let meta = fs::symlink_metadata(&path).unwrap();
        let outcome = secure_delete_file(&path, &meta, &AtomicBool::new(false)).unwrap();

        assert_eq!(outcome, SecureOutcome::UnlinkedOnlyHardLinked);
        assert!(!path.exists());
        assert_eq!(fs::read(&other).unwrap(), b"shared data");
    }

    #[test]
    fn refuses_a_file_swapped_after_the_check() {
        let dir = TempDir::new().unwrap();
        let checked = dir.path().join("checked");
        fs::write(&checked, b"checked").unwrap();
        let meta = fs::symlink_metadata(&checked).unwrap();

        // A different file now sits at the path that was checked.
        let path = dir.path().join("swapped");
        fs::write(&path, b"replacement").unwrap();

        assert!(secure_delete_file(&path, &meta, &AtomicBool::new(false)).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert_eq!(fs::read(&checked).unwrap(), b"checked");
    }

    #[test]
    fn refuses_symlinks() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"do not touch").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let meta = fs::metadata(&target).unwrap();

        assert!(secure_delete_file(&link, &meta, &AtomicBool::new(false)).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"do not touch");
    }

    #[test]
    fn cancellation_still_removes_the_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big");
        fs::write(&path, vec![1u8; 3 * CHUNK]).unwrap();
        let meta = fs::symlink_metadata(&path).unwrap();
        secure_delete_file(&path, &meta, &AtomicBool::new(true)).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn storage_facts_does_not_panic() {
        let _ = storage_facts(Path::new("/"));
        let _ = storage_facts(Path::new("/nonexistent/path"));
    }
}
