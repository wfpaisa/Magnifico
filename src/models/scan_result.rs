use std::collections::HashSet;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
    pub is_directory: bool,
    pub is_symlink: bool,
}

impl FileEntry {
    pub fn new(path: PathBuf, size: u64, is_directory: bool, is_symlink: bool) -> Self {
        Self {
            path,
            size,
            is_directory,
            is_symlink,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub files: Vec<FileEntry>,
    pub directories: Vec<FileEntry>,
    pub total_size: u64,
    pub file_count: usize,
    pub dir_count: usize,
    /// Folders whose whole contents are in `files`/`directories`, with
    /// nothing left out. The cleaner may remove each as one unit.
    pub whole_dirs: Vec<PathBuf>,
    pub skipped: Vec<(PathBuf, String)>,
    pub security_violations: Vec<String>,
    pub errors: Vec<String>,
    pub cancelled: bool,
}

impl ScanResult {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, entry: FileEntry) {
        self.total_size += entry.size;
        self.file_count += 1;
        self.files.push(entry);
    }

    pub fn add_directory(&mut self, entry: FileEntry) {
        self.dir_count += 1;
        self.directories.push(entry);
    }

    pub fn add_skipped(&mut self, path: PathBuf, reason: impl Into<String>) {
        self.skipped.push((path, reason.into()));
    }

    pub fn add_security_violation(&mut self, violation: impl Into<String>) {
        self.security_violations.push(violation.into());
    }

    pub fn add_error(&mut self, error: impl Into<String>) {
        self.errors.push(error.into());
    }

    pub fn merge(&mut self, other: ScanResult) {
        let mut seen_files: std::collections::HashSet<PathBuf> =
            self.files.iter().map(|entry| entry.path.clone()).collect();
        for file in other.files {
            if seen_files.insert(file.path.clone()) {
                self.add_file(file);
            }
        }

        let mut seen_directories: std::collections::HashSet<PathBuf> = self
            .directories
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        for directory in other.directories {
            if seen_directories.insert(directory.path.clone()) {
                self.add_directory(directory);
            }
        }
        for dir in other.whole_dirs {
            if !self.whole_dirs.contains(&dir) {
                self.whole_dirs.push(dir);
            }
        }
        self.skipped.extend(other.skipped);
        self.security_violations.extend(other.security_violations);
        self.errors.extend(other.errors);
        self.cancelled |= other.cancelled;
    }

    pub fn has_security_violations(&self) -> bool {
        !self.security_violations.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.directories.is_empty()
    }

    /// Forget cleaned paths without reading the disk again.
    pub fn remove_cleaned(&mut self, cleaned_paths: &HashSet<PathBuf>) {
        let remains = |entry: &FileEntry| {
            !entry
                .path
                .ancestors()
                .any(|path| cleaned_paths.contains(path))
        };
        self.files.retain(remains);
        self.directories.retain(remains);
        self.whole_dirs
            .retain(|dir| !dir.ancestors().any(|path| cleaned_paths.contains(path)));
        self.file_count = self.files.len();
        self.dir_count = self.directories.len();
        self.total_size = self.files.iter().map(|entry| entry.size).sum();
    }

    pub fn formatted_size(&self) -> String {
        bytesize::ByteSize(self.total_size).to_string()
    }
}

#[derive(Debug, Error)]
pub enum CleanError {
    #[error("Permission denied: {0}")]
    PermissionDenied(PathBuf),

    #[error("Security violation: {0}")]
    SecurityViolation(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Directory not empty: {0}")]
    DirectoryNotEmpty(PathBuf),

    #[error("Already removed: {0}")]
    AlreadyRemoved(PathBuf),

    #[error("In use by a running program: {0}")]
    InUse(PathBuf),
}

/// Identity captured before a successful move to the trash.
#[derive(Debug, Clone)]
pub struct TrashedFile {
    pub path: PathBuf,
    pub size: u64,
    pub id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CleanResult {
    pub deleted_files: Vec<PathBuf>,
    pub deleted_directories: Vec<PathBuf>,
    pub bytes_freed: u64,
    /// Sizes of successful file removals, reused for local inventory updates.
    pub removed_sizes: Vec<(PathBuf, u64)>,
    pub failed: Vec<(PathBuf, String)>,
    pub skipped: Vec<(PathBuf, String)>,
    pub cancelled: bool,
    pub blocked_reason: Option<String>,
    /// Files whose contents were overwritten before deletion.
    pub files_overwritten: usize,
    /// Files moved to the trash instead of being deleted.
    pub files_trashed: usize,
    /// Original paths and sizes of successful moves, for precise trash deletion.
    pub trashed_files: Vec<TrashedFile>,
    /// Files deleted for good because there were too many for the trash.
    pub files_too_many_for_trash: usize,
}

impl CleanResult {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_deleted_file(&mut self, path: PathBuf, size: u64) {
        self.bytes_freed += size;
        self.removed_sizes.push((path.clone(), size));
        self.deleted_files.push(path);
    }

    pub fn add_deleted_directory(&mut self, path: PathBuf) {
        self.deleted_directories.push(path);
    }

    pub fn add_failed(&mut self, path: PathBuf, reason: impl Into<String>) {
        self.failed.push((path, reason.into()));
    }

    pub fn add_skipped(&mut self, path: PathBuf, reason: impl Into<String>) {
        self.skipped.push((path, reason.into()));
    }

    pub fn files_deleted_count(&self) -> usize {
        self.deleted_files.len()
    }

    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }

    pub fn blocked(&mut self, reason: impl Into<String>) {
        self.blocked_reason = Some(reason.into());
    }

    #[cfg(test)]
    pub fn is_blocked(&self) -> bool {
        self.blocked_reason.is_some()
    }

    /// Add a second cleanup's outcome to this one.
    pub fn merge(&mut self, other: CleanResult) {
        self.deleted_files.extend(other.deleted_files);
        self.removed_sizes.extend(other.removed_sizes);
        self.deleted_directories.extend(other.deleted_directories);
        self.bytes_freed += other.bytes_freed;
        self.failed.extend(other.failed);
        self.skipped.extend(other.skipped);
        self.cancelled |= other.cancelled;
        if self.blocked_reason.is_none() {
            self.blocked_reason = other.blocked_reason;
        }
        self.files_overwritten += other.files_overwritten;
        self.files_trashed += other.files_trashed;
        self.trashed_files.extend(other.trashed_files);
        self.files_too_many_for_trash += other.files_too_many_for_trash;
    }

    pub fn formatted_bytes_freed(&self) -> String {
        bytesize::ByteSize(self.bytes_freed).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_deduplicates_paths_and_totals() {
        let path = PathBuf::from("/tmp/magnifico-test/cache.bin");
        let entry = FileEntry::new(path, 1_000, false, false);
        let mut first = ScanResult::new();
        first.add_file(entry.clone());
        let mut second = ScanResult::new();
        second.add_file(entry);

        first.merge(second);

        assert_eq!(first.file_count, 1);
        assert_eq!(first.total_size, 1_000);
        assert_eq!(first.files.len(), 1);
    }

    #[test]
    fn merge_propagates_cancellation() {
        let mut first = ScanResult::new();
        let mut second = ScanResult::new();
        second.cancelled = true;
        first.merge(second);
        assert!(first.cancelled);
    }
}
