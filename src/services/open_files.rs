//! Find files that a running program has open, so cleanup can leave them
//! alone. Deleting a file under a program that is writing it (a compiler, a
//! database, a download) can break that program or lose its work.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;

/// (device, inode) of every regular file open in a process we can inspect.
/// Only our own processes are readable without root, which are exactly the
/// ones whose files cleanup may delete.
pub fn open_file_ids() -> HashSet<(u64, u64)> {
    let own_pid = std::process::id().to_string();
    let mut ids = HashSet::new();

    let Ok(processes) = fs::read_dir("/proc") else {
        return ids;
    };
    for process in processes.flatten() {
        let name = process.file_name();
        let Some(pid) = name.to_str() else { continue };
        if !pid.bytes().all(|b| b.is_ascii_digit()) || pid == own_pid {
            continue;
        }
        let Ok(fds) = fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            // metadata() follows the fd link to the open file itself.
            if let Ok(meta) = fs::metadata(fd.path()) {
                if meta.file_type().is_file() {
                    ids.insert((meta.dev(), meta.ino()));
                }
            }
        }
    }
    ids
}

/// Working folders and open files of every process we can inspect, so a
/// project someone is building or editing is never touched.
pub fn busy_paths() -> Vec<std::path::PathBuf> {
    collect_busy_paths(false)
}

/// Like `busy_paths`, for deciding whether someone is working in a project:
/// a file manager showing a folder keeps it open without working in it.
pub fn working_paths() -> Vec<std::path::PathBuf> {
    collect_busy_paths(true)
}

const FILE_MANAGERS: [&str; 7] = [
    "nautilus",
    "nemo",
    "caja",
    "dolphin",
    "thunar",
    "pcmanfm",
    "pcmanfm-qt",
];

fn collect_busy_paths(skip_file_managers: bool) -> Vec<std::path::PathBuf> {
    let own_pid = std::process::id().to_string();
    let mut paths = Vec::new();
    let Ok(processes) = fs::read_dir("/proc") else {
        return paths;
    };
    for process in processes.flatten() {
        let name = process.file_name();
        let Some(pid) = name.to_str() else { continue };
        if !pid.bytes().all(|b| b.is_ascii_digit()) || pid == own_pid {
            continue;
        }
        if let Ok(cwd) = fs::read_link(process.path().join("cwd")) {
            paths.push(cwd);
        }
        // Only a file manager's open folders are ignored, never its files.
        let file_manager = skip_file_managers
            && fs::read_to_string(process.path().join("comm"))
                .is_ok_and(|comm| FILE_MANAGERS.contains(&comm.trim()));
        let Ok(fds) = fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(target) = fs::read_link(fd.path()) {
                if target.is_absolute() {
                    if file_manager && target.is_dir() {
                        continue;
                    }
                    paths.push(target);
                }
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn finds_a_file_held_open_by_another_process() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("held");
        fs::write(&path, b"x").unwrap();
        let meta = fs::metadata(&path).unwrap();

        // `sleep` inherits the file as its stdin and keeps it open.
        let mut child = Command::new("sleep")
            .arg("5")
            .stdin(File::open(&path).unwrap())
            .spawn()
            .unwrap();
        let ids = open_file_ids();
        child.kill().ok();
        child.wait().ok();

        assert!(ids.contains(&(meta.dev(), meta.ino())));
    }
}
