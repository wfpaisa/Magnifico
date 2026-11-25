use crate::models::{
    BrowserType, CleanError, CleanResult, FileEntry, RemovalMethod, ScanResult, TrashedFile,
};
use crate::services::open_files::open_file_ids;
use crate::services::secure_delete::{secure_delete_file, SecureOutcome};
use crate::services::SecurityAuditor;
use gio::prelude::FileExt;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};
use walkdir::WalkDir;

const ALREADY_REMOVED_REASON: &str = "Already removed before cleanup reached it";
/// Above this many files one cleanup does not use the trash: every moved
/// file gets its own entry, and a trash with tens of thousands of entries
/// stalls the desktop's trash service (and every app that talks to it).
pub const TRASH_MAX_FILES: usize = 5_000;

/// The same identity GIO reports as `id::file` for a local file.
pub fn file_id(metadata: &fs::Metadata) -> String {
    format!("l{}:{}", metadata.dev(), metadata.ino())
}

const IN_USE_REASON: &str = "In use by a running program — close it and clean again";

#[derive(Debug, Clone, Copy)]
pub struct CleanOptions {
    pub max_files_per_operation: usize,
    pub max_size_per_operation: u64,
    pub removal_method: RemovalMethod,
    /// Overwrite browser profile data (cookies, history, sessions) before
    /// deleting it, whatever `removal_method` says.
    pub secure_browser_data: bool,
}

impl Default for CleanOptions {
    fn default() -> Self {
        Self {
            max_files_per_operation: usize::MAX,
            max_size_per_operation: u64::MAX,
            removal_method: RemovalMethod::Permanent,
            secure_browser_data: false,
        }
    }
}

/// Live counters a UI can poll while a cleanup runs.
#[derive(Debug, Default)]
pub struct CleanProgress {
    /// Bytes successfully removed so far.
    pub bytes_done: AtomicU64,
    pub files_done: AtomicUsize,
}

/// What happened to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileOutcome {
    Deleted,
    Overwritten,
    Trashed(Option<String>),
}

/// Per-cleanup facts computed once, not per file.
struct CleanContext {
    open_files: HashSet<(u64, u64)>,
    trash_root: Option<PathBuf>,
    browser_profile_roots: Vec<PathBuf>,
    /// The trash was asked for, but this cleanup has too many files for it.
    too_many_for_trash: bool,
}

pub struct Cleaner {
    security: SecurityAuditor,
    cancelled: Arc<AtomicBool>,
    options: CleanOptions,
    progress: Option<Arc<CleanProgress>>,
}

impl Cleaner {
    pub fn new() -> Self {
        Self::with_options(CleanOptions::default())
    }

    pub fn with_options(options: CleanOptions) -> Self {
        Self::with_options_and_cancellation(options, Arc::new(AtomicBool::new(false)))
    }

    pub fn with_options_and_cancellation(
        options: CleanOptions,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        // Directory symlinks are deliberately never traversed. Keeping this
        // field false also protects users with an older config that enabled
        // the removed unsafe option.
        let security = SecurityAuditor::new();

        Self {
            security,
            cancelled,
            options,
            progress: None,
        }
    }

    pub fn with_progress(mut self, progress: Arc<CleanProgress>) -> Self {
        self.progress = Some(progress);
        self
    }

    fn report(&self, size: u64) {
        if let Some(progress) = &self.progress {
            progress.bytes_done.fetch_add(size, Ordering::Relaxed);
            progress.files_done.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn validate_operation(&self, scan_result: &ScanResult) -> Result<(), String> {
        if scan_result.file_count > self.options.max_files_per_operation {
            return Err(crate::i18n::tr_args(
                "Cleanup blocked by safety limit: {count} files found, limit is {limit}.",
                &[
                    (
                        "{count}",
                        &crate::i18n::format_count(scan_result.file_count),
                    ),
                    (
                        "{limit}",
                        &crate::i18n::format_count(self.options.max_files_per_operation),
                    ),
                ],
            ));
        }

        if scan_result.total_size > self.options.max_size_per_operation {
            return Err(crate::i18n::tr_args(
                "Cleanup blocked by safety limit: {size} selected, limit is {limit} per operation. Raise the limit in Settings → Safety.",
                &[
                    ("{size}", &scan_result.formatted_size()),
                    (
                        "{limit}",
                        &bytesize::ByteSize(self.options.max_size_per_operation).to_string(),
                    ),
                ],
            ));
        }

        Ok(())
    }

    #[cfg(test)]
    pub fn dry_run(&self, scan_result: &ScanResult) -> CleanResult {
        let mut result = CleanResult::new();

        if let Err(reason) = self.validate_operation(scan_result) {
            result.blocked(reason);
            return result;
        }

        for file in &scan_result.files {
            if self.is_cancelled() {
                result.cancelled = true;
                break;
            }

            let audit = self.security.audit_for_deletion(&file.path);
            if audit.is_safe {
                result.add_deleted_file(file.path.clone(), file.size);
            } else {
                for v in audit.violations {
                    result.add_skipped(file.path.clone(), v.to_string());
                }
            }
        }

        for dir in &scan_result.directories {
            if self.is_cancelled() {
                result.cancelled = true;
                break;
            }

            let audit = self.security.audit_for_deletion(&dir.path);
            if audit.is_safe {
                result.add_deleted_directory(dir.path.clone());
            } else {
                for v in audit.violations {
                    result.add_skipped(dir.path.clone(), v.to_string());
                }
            }
        }

        result
    }

    /// Build the (browser, base-path) lookup table once. Each entry's path
    /// comes from `cache_dir()`/`home_dir()` which both perform syscalls,
    /// so this is intentionally computed once per cleanup rather than per
    /// file.
    fn browser_path_table() -> Vec<(BrowserType, PathBuf)> {
        let mut paths = Vec::new();
        for browser in BrowserType::all() {
            for root in browser
                .cache_roots()
                .into_iter()
                .chain(browser.config_roots())
            {
                paths.push((*browser, root));
            }
        }
        paths
    }

    /// Check if a path belongs to a browser directory using a precomputed
    /// (browser, base-path) lookup table.
    fn browser_for_path(table: &[(BrowserType, PathBuf)], path: &Path) -> Option<BrowserType> {
        table
            .iter()
            .find(|(_, base)| path.starts_with(base))
            .map(|(browser, _)| *browser)
    }

    pub fn clean(&self, scan_result: &ScanResult) -> CleanResult {
        let mut result = CleanResult::new();

        if let Err(reason) = self.validate_operation(scan_result) {
            result.blocked(reason);
            warn!(
                "{}",
                result
                    .blocked_reason
                    .as_deref()
                    .unwrap_or("Cleanup blocked")
            );
            return result;
        }

        if self.is_cancelled() {
            result.cancelled = true;
            return result;
        }

        info!(
            "Starting cleanup: {} files, {} directories, {} total",
            scan_result.file_count,
            scan_result.dir_count,
            scan_result.formatted_size()
        );

        // Precompute browser detection state once instead of rescanning /proc
        // and rebuilding base paths for every file. Without this, a cleanup of
        // N files runs N * 9 directory walks of /proc, which freezes the UI
        // for large scan results.
        let browser_table = Self::browser_path_table();
        let running_browsers: HashSet<BrowserType> = BrowserType::running_browsers();
        let mut context = CleanContext {
            open_files: open_file_ids(),
            trash_root: dirs::data_local_dir().map(|d| d.join("Trash")),
            browser_profile_roots: BrowserType::all()
                .iter()
                .flat_map(|b| b.config_roots())
                .collect(),
            too_many_for_trash: false,
        };

        let handled = self.clean_whole_dirs(scan_result, &context, &browser_table, &mut result);
        let remaining = scan_result
            .files
            .iter()
            .filter(|file| !handled.contains(&file.path))
            .count();
        context.too_many_for_trash =
            self.options.removal_method == RemovalMethod::Trash && remaining > TRASH_MAX_FILES;
        if context.too_many_for_trash {
            warn!(
                "{} files are too many for the trash (limit {}); deleting them permanently",
                remaining, TRASH_MAX_FILES
            );
        }

        for file in &scan_result.files {
            if handled.contains(&file.path) {
                continue;
            }
            if self.is_cancelled() {
                result.cancelled = true;
                warn!("Cleanup cancelled by user");
                break;
            }
            // Re-check if the browser is running to prevent SQLite corruption (TOCTOU mitigation)
            if let Some(browser) = Self::browser_for_path(&browser_table, &file.path) {
                if running_browsers.contains(&browser) {
                    result.add_skipped(
                        file.path.clone(),
                        format!(
                            "{} is currently running — close it before cleaning to avoid data corruption",
                            browser.display_name()
                        ),
                    );
                    continue;
                }
            }

            let method = self.method_for(&file.path, &context);
            match self.delete_file(&file.path, method, &context) {
                Ok((size, outcome)) => {
                    self.report(size);
                    result.add_deleted_file(file.path.clone(), size);
                    match &outcome {
                        FileOutcome::Overwritten => result.files_overwritten += 1,
                        FileOutcome::Trashed(id) => {
                            result.files_trashed += 1;
                            result.trashed_files.push(TrashedFile {
                                path: file.path.clone(),
                                size,
                                id: id.clone(),
                            });
                        }
                        FileOutcome::Deleted => {
                            if context.too_many_for_trash {
                                result.files_too_many_for_trash += 1;
                            }
                        }
                    }
                    debug!("Removed file ({:?}): {}", outcome, file.path.display());
                }
                Err(CleanError::AlreadyRemoved(_)) => {
                    result.add_skipped(file.path.clone(), ALREADY_REMOVED_REASON);
                    debug!("Already removed: {}", file.path.display());
                }
                Err(CleanError::InUse(_)) => {
                    result.add_skipped(file.path.clone(), IN_USE_REASON);
                    debug!("In use, kept: {}", file.path.display());
                }
                Err(e) => {
                    result.add_failed(file.path.clone(), e.to_string());
                    warn!("Failed to delete {}: {}", file.path.display(), e);
                }
            }
        }

        let mut dirs: Vec<&FileEntry> = scan_result.directories.iter().collect();
        dirs.sort_by_key(|entry| std::cmp::Reverse(entry.path.components().count()));

        for dir in dirs {
            if handled.contains(&dir.path) {
                continue;
            }
            if self.is_cancelled() {
                result.cancelled = true;
                break;
            }

            match self.delete_directory(&dir.path) {
                Ok(()) => {
                    result.add_deleted_directory(dir.path.clone());
                    debug!("Deleted directory: {}", dir.path.display());
                }
                Err(e) => {
                    if matches!(e, CleanError::DirectoryNotEmpty(_)) {
                        result.add_skipped(
                            dir.path.clone(),
                            "Directory was preserved because it is not empty",
                        );
                    } else if matches!(e, CleanError::AlreadyRemoved(_)) {
                        result.add_skipped(dir.path.clone(), ALREADY_REMOVED_REASON);
                    } else {
                        result.add_failed(dir.path.clone(), e.to_string());
                        warn!("Failed to delete directory {}: {}", dir.path.display(), e);
                    }
                }
            }
        }

        info!(
            "Cleanup complete: {} files deleted, {} freed, {} failed",
            result.files_deleted_count(),
            result.formatted_bytes_freed(),
            result.failed_count()
        );

        result
    }

    /// Remove each folder the scan took completely as one unit: one move to
    /// the trash or one recursive delete, instead of one per file. A folder
    /// is only taken whole when, right now, it is a real folder we own,
    /// passes the security audit, holds no file a program has open, no
    /// socket or device, nothing owned by someone else, and no browser
    /// data. Anything else falls back to the file-by-file path. Returns
    /// the scanned paths already handled.
    fn clean_whole_dirs(
        &self,
        scan_result: &ScanResult,
        context: &CleanContext,
        browser_table: &[(BrowserType, PathBuf)],
        result: &mut CleanResult,
    ) -> HashSet<PathBuf> {
        let mut handled = HashSet::new();
        if scan_result.whole_dirs.is_empty() {
            return handled;
        }

        // Outermost folders only: a nested one goes with its parent.
        let mut roots: Vec<&PathBuf> = scan_result.whole_dirs.iter().collect();
        roots.sort_by_key(|dir| dir.components().count());
        let mut units: HashSet<&Path> = HashSet::new();
        for root in roots {
            if !root.ancestors().any(|dir| units.contains(dir)) {
                units.insert(root.as_path());
            }
        }

        let mut members: HashMap<&Path, (Vec<&FileEntry>, Vec<&FileEntry>)> = HashMap::new();
        let unit_of = |path: &Path| path.ancestors().find_map(|dir| units.get(dir).copied());
        for file in &scan_result.files {
            if let Some(unit) = unit_of(&file.path) {
                members.entry(unit).or_default().0.push(file);
            }
        }
        for dir in &scan_result.directories {
            if let Some(unit) = unit_of(&dir.path) {
                members.entry(unit).or_default().1.push(dir);
            }
        }

        for (root, (files, dirs)) in members {
            if self.is_cancelled() {
                break;
            }
            let method = self.method_for(root, context);
            if method == RemovalMethod::Secure
                || browser_table
                    .iter()
                    .any(|(_, base)| root.starts_with(base) || base.starts_with(root))
            {
                continue;
            }
            let Some(metadata) = self.whole_dir_ready(root, context) else {
                continue;
            };
            let size: u64 = files.iter().map(|file| file.size).sum();

            let removed = match method {
                RemovalMethod::Trash => gio::File::for_path(root)
                    .trash(None::<&gio::Cancellable>)
                    .map_err(|e| e.message().to_string()),
                _ => self.remove_tree(root).map_err(|e| e.to_string()),
            };
            // The recursive delete reports its progress as it goes; a move
            // to the trash is one step, counted here.
            let report = method == RemovalMethod::Trash;
            if let Err(error) = removed {
                warn!(
                    "Could not remove {} as a whole ({error}); going file by file",
                    root.display()
                );
                // A failed recursive delete may have removed part of it.
                for entry in files.iter().chain(dirs.iter()) {
                    if fs::symlink_metadata(&entry.path)
                        .is_err_and(|e| e.kind() == ErrorKind::NotFound)
                    {
                        self.mark_removed(entry, result, report);
                        handled.insert(entry.path.clone());
                    }
                }
                continue;
            }

            info!(
                "Removed {} as one unit ({} files, {:?})",
                root.display(),
                files.len(),
                method
            );
            for entry in files.iter().chain(dirs.iter()) {
                self.mark_removed(entry, result, report);
                handled.insert(entry.path.clone());
            }
            if !dirs.iter().any(|dir| dir.path == root) {
                result.add_deleted_directory(root.to_path_buf());
            }
            if method == RemovalMethod::Trash {
                result.files_trashed += files.len();
                result.trashed_files.push(TrashedFile {
                    path: root.to_path_buf(),
                    size,
                    id: Some(file_id(&metadata)),
                });
            }
        }
        handled
    }

    /// Delete a folder tree, contents first, reporting each file to the
    /// progress counters so the screen keeps moving on a large folder.
    fn remove_tree(&self, root: &Path) -> std::io::Result<()> {
        for entry in WalkDir::new(root).follow_links(false).contents_first(true) {
            if self.is_cancelled() {
                return Err(std::io::Error::new(ErrorKind::Interrupted, "cancelled"));
            }
            let entry = entry.map_err(std::io::Error::from)?;
            if entry.file_type().is_dir() {
                fs::remove_dir(entry.path())?;
            } else {
                let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                fs::remove_file(entry.path())?;
                self.report(size);
            }
        }
        Ok(())
    }

    fn mark_removed(&self, entry: &FileEntry, result: &mut CleanResult, report: bool) {
        if entry.is_directory {
            result.add_deleted_directory(entry.path.clone());
        } else {
            if report {
                self.report(entry.size);
            }
            result.add_deleted_file(entry.path.clone(), entry.size);
        }
    }

    /// The folder's metadata when it can be removed as one unit right now.
    fn whole_dir_ready(&self, root: &Path, context: &CleanContext) -> Option<fs::Metadata> {
        if !self.security.audit_for_deletion(root).is_safe {
            return None;
        }
        let metadata = self.security.verify_before_deletion(root).ok()?;
        if !metadata.is_dir() {
            return None;
        }
        for entry in WalkDir::new(root).follow_links(false) {
            let entry = entry.ok()?;
            // Not following links, this is the entry itself.
            let meta = entry.metadata().ok()?;
            let file_type = meta.file_type();
            if !(file_type.is_file() || file_type.is_dir() || file_type.is_symlink())
                || meta.uid() != metadata.uid()
                || (file_type.is_file() && context.open_files.contains(&(meta.dev(), meta.ino())))
            {
                debug!(
                    "{} cannot go as a whole: {}",
                    root.display(),
                    entry.path().display()
                );
                return None;
            }
        }
        Some(metadata)
    }

    /// A scanned entry can disappear before cleanup reaches it, for example
    /// when a running application prunes its own cache. That is not a
    /// failure, so it is reported separately from real errors.
    fn ensure_present(path: &Path) -> Result<(), CleanError> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == ErrorKind::NotFound => {
                Err(CleanError::AlreadyRemoved(path.to_path_buf()))
            }
            _ => Ok(()),
        }
    }

    fn removal_error(path: &Path, error: std::io::Error) -> CleanError {
        match error.kind() {
            ErrorKind::NotFound => CleanError::AlreadyRemoved(path.to_path_buf()),
            ErrorKind::PermissionDenied => CleanError::PermissionDenied(path.to_path_buf()),
            _ => CleanError::IoError(error),
        }
    }

    /// Pick how one file is removed. Files already in the trash are never
    /// moved back into it, and browser profile data is overwritten when the
    /// user asked for that, whatever the global method.
    fn method_for(&self, path: &Path, context: &CleanContext) -> RemovalMethod {
        let in_trash = context
            .trash_root
            .as_ref()
            .is_some_and(|root| path.starts_with(root));
        let browser_profile = context
            .browser_profile_roots
            .iter()
            .any(|root| path.starts_with(root));

        match self.options.removal_method {
            RemovalMethod::Secure => RemovalMethod::Secure,
            _ if self.options.secure_browser_data && browser_profile => RemovalMethod::Secure,
            RemovalMethod::Trash if !in_trash && !context.too_many_for_trash => {
                RemovalMethod::Trash
            }
            _ => RemovalMethod::Permanent,
        }
    }

    fn delete_file(
        &self,
        path: &Path,
        method: RemovalMethod,
        context: &CleanContext,
    ) -> Result<(u64, FileOutcome), CleanError> {
        Self::ensure_present(path)?;

        let audit = self.security.audit_for_deletion(path);
        if !audit.is_safe {
            let violation = audit
                .violations
                .into_iter()
                .next()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "Unknown security violation".to_string());
            return Err(CleanError::SecurityViolation(violation));
        }

        let metadata = self.security.verify_before_deletion(path).map_err(|e| {
            // The entry may have vanished after the presence check above.
            Self::ensure_present(path)
                .err()
                .unwrap_or_else(|| CleanError::SecurityViolation(e.to_string()))
        })?;

        let actual_size = metadata.len();

        if context
            .open_files
            .contains(&(metadata.dev(), metadata.ino()))
        {
            return Err(CleanError::InUse(path.to_path_buf()));
        }

        let outcome = match method {
            RemovalMethod::Permanent => {
                fs::remove_file(path).map_err(|e| Self::removal_error(path, e))?;
                FileOutcome::Deleted
            }
            RemovalMethod::Trash => {
                let id = Some(file_id(&metadata));
                gio::File::for_path(path)
                    .trash(None::<&gio::Cancellable>)
                    .map_err(|e| {
                        Self::ensure_present(path).err().unwrap_or_else(|| {
                            CleanError::IoError(std::io::Error::other(format!(
                                "Could not move to trash: {}",
                                e.message()
                            )))
                        })
                    })?;
                FileOutcome::Trashed(id)
            }
            RemovalMethod::Secure => {
                if !metadata.file_type().is_file() {
                    // Sockets, FIFOs and the like hold no data to overwrite.
                    fs::remove_file(path).map_err(|e| Self::removal_error(path, e))?;
                    FileOutcome::Deleted
                } else {
                    match secure_delete_file(path, &metadata, &self.cancelled) {
                        Ok(SecureOutcome::Overwritten) => FileOutcome::Overwritten,
                        Ok(SecureOutcome::UnlinkedOnlyHardLinked) => FileOutcome::Deleted,
                        Err(e) => return Err(Self::removal_error(path, e)),
                    }
                }
            }
        };

        Ok((actual_size, outcome))
    }

    fn delete_directory(&self, path: &Path) -> Result<(), CleanError> {
        Self::ensure_present(path)?;

        let audit = self.security.audit_for_deletion(path);
        if !audit.is_safe {
            let violation = audit
                .violations
                .into_iter()
                .next()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "Unknown security violation".to_string());
            return Err(CleanError::SecurityViolation(violation));
        }

        self.security.verify_before_deletion(path).map_err(|e| {
            // The entry may have vanished after the presence check above.
            Self::ensure_present(path)
                .err()
                .unwrap_or_else(|| CleanError::SecurityViolation(e.to_string()))
        })?;

        let is_empty = path
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);

        if !is_empty {
            return Err(CleanError::DirectoryNotEmpty(path.to_path_buf()));
        }

        fs::remove_dir(path).map_err(|e| Self::removal_error(path, e))?;

        Ok(())
    }
}

impl Default for Cleaner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();

        let file1 = dir.path().join("test1.txt");
        let mut f = File::create(&file1).unwrap();
        f.write_all(b"test content 1").unwrap();

        let file2 = dir.path().join("test2.log");
        let mut f = File::create(&file2).unwrap();
        f.write_all(b"test content 2 longer").unwrap();

        let subdir = dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();

        let file3 = subdir.join("nested.txt");
        let mut f = File::create(&file3).unwrap();
        f.write_all(b"nested content").unwrap();

        dir
    }

    #[test]
    fn progress_counts_only_successfully_removed_bytes() {
        let dir = setup_test_dir();
        let progress = Arc::new(CleanProgress::default());
        let cleaner = Cleaner::new().with_progress(progress.clone());
        let mut scan = ScanResult::new();
        scan.add_file(FileEntry::new(
            dir.path().join("missing"),
            1000,
            false,
            false,
        ));
        let path = dir.path().join("test1.txt");
        let actual = fs::metadata(&path).unwrap().len();
        scan.add_file(FileEntry::new(path, 999, false, false));
        let result = cleaner.clean(&scan);
        assert_eq!(progress.bytes_done.load(Ordering::Relaxed), actual);
        assert_eq!(result.bytes_freed, actual);
        assert_eq!(progress.files_done.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_dry_run_no_deletion() {
        let dir = setup_test_dir();
        let cleaner = Cleaner::new();

        let mut scan = ScanResult::new();
        let file = dir.path().join("test1.txt");
        scan.add_file(FileEntry::new(file.clone(), 14, false, false));

        let result = cleaner.dry_run(&scan);

        assert!(file.exists());
        assert_eq!(result.files_deleted_count(), 1);
    }

    #[test]
    fn secure_mode_overwrites_files() {
        let dir = setup_test_dir();
        let cleaner = Cleaner::with_options(CleanOptions {
            removal_method: RemovalMethod::Secure,
            ..CleanOptions::default()
        });
        let mut scan = ScanResult::new();
        let file = dir.path().join("test1.txt");
        scan.add_file(FileEntry::new(file.clone(), 14, false, false));

        let result = cleaner.clean(&scan);
        assert!(!file.exists());
        assert_eq!(result.files_deleted_count(), 1);
        assert_eq!(result.files_overwritten, 1);
    }

    #[test]
    fn files_open_in_another_program_are_kept() {
        let dir = setup_test_dir();
        let file = dir.path().join("test1.txt");
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .stdin(File::open(&file).unwrap())
            .spawn()
            .unwrap();

        let mut scan = ScanResult::new();
        scan.add_file(FileEntry::new(file.clone(), 14, false, false));
        let result = Cleaner::new().clean(&scan);
        child.kill().ok();
        child.wait().ok();

        assert!(file.exists());
        assert_eq!(result.files_deleted_count(), 0);
        assert_eq!(result.skipped.len(), 1);
    }

    #[test]
    fn removal_method_never_trashes_the_trash_and_secures_browser_data() {
        let trash = PathBuf::from("/home/u/.local/share/Trash");
        let profile = PathBuf::from("/home/u/.mozilla/firefox");
        let context = CleanContext {
            open_files: HashSet::new(),
            trash_root: Some(trash.clone()),
            browser_profile_roots: vec![profile.clone()],
            too_many_for_trash: false,
        };
        let with = |removal_method, secure_browser_data| {
            Cleaner::with_options(CleanOptions {
                removal_method,
                secure_browser_data,
                ..CleanOptions::default()
            })
        };

        let trash_mode = with(RemovalMethod::Trash, false);
        assert_eq!(
            trash_mode.method_for(&trash.join("files/a"), &context),
            RemovalMethod::Permanent
        );
        assert_eq!(
            trash_mode.method_for(Path::new("/home/u/.cache/x"), &context),
            RemovalMethod::Trash
        );
        let crowded = CleanContext {
            too_many_for_trash: true,
            ..context
        };
        assert_eq!(
            trash_mode.method_for(Path::new("/home/u/.cache/x"), &crowded),
            RemovalMethod::Permanent
        );
        let context = CleanContext {
            too_many_for_trash: false,
            ..crowded
        };

        let secure_browser = with(RemovalMethod::Trash, true);
        assert_eq!(
            secure_browser.method_for(&profile.join("cookies.sqlite"), &context),
            RemovalMethod::Secure
        );
        assert_eq!(
            with(RemovalMethod::Permanent, false).method_for(&profile.join("c"), &context),
            RemovalMethod::Permanent
        );
    }

    #[test]
    fn test_clean_deletes_files() {
        let dir = setup_test_dir();
        let cleaner = Cleaner::new();

        let mut scan = ScanResult::new();
        let file = dir.path().join("test1.txt");
        scan.add_file(FileEntry::new(file.clone(), 14, false, false));

        let result = cleaner.clean(&scan);
        assert_eq!(result.files_deleted_count(), 1);
        assert!(!file.exists());
    }

    #[test]
    fn test_cancellation() {
        let cleaner = Cleaner::new();
        cleaner.cancel();

        let scan = ScanResult::new();
        let result = cleaner.clean(&scan);

        assert!(result.cancelled);
    }

    #[test]
    fn files_removed_after_the_scan_are_skipped_not_failed() {
        let dir = setup_test_dir();
        let cleaner = Cleaner::new();

        let mut scan = ScanResult::new();
        let vanished = dir.path().join("test1.txt");
        scan.add_file(FileEntry::new(vanished.clone(), 14, false, false));
        scan.add_file(FileEntry::new(
            dir.path().join("test2.log"),
            21,
            false,
            false,
        ));
        fs::remove_file(&vanished).unwrap();

        let result = cleaner.clean(&scan);
        assert!(
            result.failed.is_empty(),
            "unexpected failures: {:?}",
            result.failed
        );
        assert_eq!(result.files_deleted_count(), 1);
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].0, vanished);
        assert_eq!(result.skipped[0].1, ALREADY_REMOVED_REASON);
    }

    #[test]
    fn directories_removed_after_the_scan_are_skipped_not_failed() {
        let dir = setup_test_dir();
        let cleaner = Cleaner::new();

        let gone = dir.path().join("gone");
        fs::create_dir(&gone).unwrap();
        let mut scan = ScanResult::new();
        scan.add_directory(FileEntry::new(gone.clone(), 0, true, false));
        fs::remove_dir(&gone).unwrap();

        let result = cleaner.clean(&scan);
        assert!(
            result.failed.is_empty(),
            "unexpected failures: {:?}",
            result.failed
        );
        assert_eq!(
            result.skipped,
            vec![(gone, ALREADY_REMOVED_REASON.to_string())]
        );
    }

    #[test]
    fn cleanup_is_blocked_when_file_limit_is_exceeded() {
        let dir = setup_test_dir();
        let cleaner = Cleaner::with_options(CleanOptions {
            max_files_per_operation: 1,
            ..CleanOptions::default()
        });

        let mut scan = ScanResult::new();
        scan.add_file(FileEntry::new(
            dir.path().join("test1.txt"),
            14,
            false,
            false,
        ));
        scan.add_file(FileEntry::new(
            dir.path().join("test2.log"),
            21,
            false,
            false,
        ));

        let result = cleaner.clean(&scan);
        assert!(result.is_blocked());
        assert!(dir.path().join("test1.txt").exists());
    }

    /// A folder holding many files, scanned the way a cache rule scans it.
    fn whole_folder(dir: &TempDir) -> (PathBuf, ScanResult) {
        let root = dir.path().join("store");
        for n in 0..50 {
            let sub = root.join(format!("pkg{}", n % 5));
            fs::create_dir_all(&sub).unwrap();
            fs::write(sub.join(format!("f{n}")), b"data").unwrap();
        }
        let scan = crate::services::Scanner::new().scan_paths(std::slice::from_ref(&root));
        (root, scan)
    }

    #[test]
    fn a_whole_folder_goes_in_one_operation() {
        let dir = TempDir::new().unwrap();
        let (root, scan) = whole_folder(&dir);
        assert_eq!(scan.whole_dirs, vec![root.clone()]);

        let result = Cleaner::new().clean(&scan);
        assert!(!root.exists());
        assert_eq!(result.files_deleted_count(), 50);
        assert_eq!(result.bytes_freed, 200);
        assert!(result.failed.is_empty() && result.skipped.is_empty());
    }

    #[test]
    fn a_folder_with_an_open_file_falls_back_to_file_by_file() {
        let dir = TempDir::new().unwrap();
        let (root, scan) = whole_folder(&dir);
        let held = root.join("pkg0/f0");
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .stdin(File::open(&held).unwrap())
            .spawn()
            .unwrap();
        let result = Cleaner::new().clean(&scan);
        child.kill().ok();
        child.wait().ok();

        assert!(held.exists());
        assert_eq!(result.files_deleted_count(), 49);
        assert_eq!(result.skipped.len(), 3, "{:?}", result.skipped);
    }

    #[test]
    fn filtered_scans_are_never_whole() {
        let dir = TempDir::new().unwrap();
        let (root, _) = whole_folder(&dir);
        let scanner = crate::services::Scanner::new();
        let filtered = scanner.scan_directory_with_options(&root, true, Some("f1*"), 0);
        assert!(filtered.whole_dirs.is_empty());
        // Custom "files only" rules keep the folder structure.
        let files_only = scanner.scan_directory_with_options(&root, false, None, 0);
        assert!(files_only.whole_dirs.is_empty());
        // Cache rules empty the folder by its direct subfolders.
        let cache = scanner.scan_directory(&root, false);
        assert_eq!(cache.whole_dirs.len(), 5);
    }
}
