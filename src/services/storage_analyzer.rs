use gio::prelude::*;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageNodeKind {
    File,
    Directory,
}

#[derive(Debug, Clone)]
pub struct StorageNode {
    pub path: PathBuf,
    pub kind: StorageNodeKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub device: u64,
    pub inode: u64,
    pub children: Vec<StorageNode>,
}

impl StorageNode {
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn is_directory(&self) -> bool {
        self.kind == StorageNodeKind::Directory
    }

    pub fn find(&self, path: &Path) -> Option<&StorageNode> {
        if self.path == path {
            return Some(self);
        }
        if !path.starts_with(&self.path) {
            return None;
        }
        self.children.iter().find_map(|child| child.find(path))
    }
}

#[derive(Debug, Clone)]
pub struct StorageAnalysis {
    pub root: StorageNode,
    pub files_scanned: usize,
    pub directories_scanned: usize,
    pub skipped: Vec<String>,
}

impl StorageAnalysis {
    pub fn item_count(&self) -> usize {
        self.files_scanned + self.directories_scanned
    }

    /// Also discount files omitted from the displayed tree by its size/child limits.
    pub fn apply_removed(&mut self, update: &super::removal_update::RemovalUpdate) -> bool {
        if update.removes(&self.root.path) {
            self.root.children.clear();
            self.root.size = 0;
            self.files_scanned = 0;
            self.directories_scanned = 1;
            return true;
        }
        let visible: Vec<_> = update
            .paths
            .iter()
            .filter(|p| self.root.find(p).is_some())
            .cloned()
            .collect();
        let mut changed = !visible.is_empty();
        fn discount(node: &mut StorageNode, path: &Path, bytes: u64) {
            if !path.starts_with(&node.path) {
                return;
            }
            node.size = node.size.saturating_sub(bytes);
            for child in &mut node.children {
                discount(child, path, bytes);
            }
        }
        for (path, bytes) in &update.sizes {
            if *bytes > 0
                && path.starts_with(&self.root.path)
                && self.root.find(path).is_none()
                && !visible.iter().any(|parent| path.starts_with(parent))
            {
                discount(&mut self.root, path, *bytes);
                changed = true;
            }
        }
        self.remove_paths(&visible);
        changed
    }

    pub fn remove_paths(&mut self, removed: &[PathBuf]) {
        fn remove(node: &mut StorageNode, path: &Path) -> Option<StorageNode> {
            if !path.starts_with(&node.path) {
                return None;
            }
            let index = node
                .children
                .iter()
                .position(|child| path.starts_with(&child.path))?;
            let removed = if node.children[index].path == path {
                node.children.remove(index)
            } else {
                remove(&mut node.children[index], path)?
            };
            node.size = node.size.saturating_sub(removed.size);
            Some(removed)
        }

        fn counts(node: &StorageNode) -> (usize, usize) {
            node.children.iter().fold(
                match node.kind {
                    StorageNodeKind::File => (1, 0),
                    StorageNodeKind::Directory => (0, 1),
                },
                |(files, directories), child| {
                    let (child_files, child_directories) = counts(child);
                    (files + child_files, directories + child_directories)
                },
            )
        }

        // The scan root is protected by trash validation and stays as the anchor.
        for path in removed {
            if let Some(node) = remove(&mut self.root, path) {
                let (files, directories) = counts(&node);
                self.files_scanned = self.files_scanned.saturating_sub(files);
                self.directories_scanned = self.directories_scanned.saturating_sub(directories);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StorageScanOptions {
    pub minimum_size: u64,
    pub stay_on_filesystem: bool,
    pub max_children_per_directory: usize,
}

impl Default for StorageScanOptions {
    fn default() -> Self {
        Self {
            minimum_size: 100 * 1024 * 1024,
            stay_on_filesystem: true,
            max_children_per_directory: 500,
        }
    }
}

#[derive(Debug, Error)]
pub enum StorageAnalysisError {
    #[error("The storage scan was cancelled")]
    Cancelled,
    #[error("The selected location is not a local directory: {0}")]
    InvalidRoot(PathBuf),
    #[error("Could not inspect {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Default)]
struct DirectoryAccumulator {
    size: u64,
    children: Vec<StorageNode>,
}

impl DirectoryAccumulator {
    fn retain_child(&mut self, child: StorageNode, limit: usize) {
        self.children.push(child);
        // Compact in batches so wide directories use O(limit) candidate slots.
        if self.children.len() >= limit.saturating_mul(2) {
            self.rank_children(limit);
        }
    }

    fn rank_children(&mut self, limit: usize) {
        // Stable ordering preserves traversal order among equally sized entries.
        self.children.sort_by_key(|child| Reverse(child.size));
        self.children.truncate(limit);
    }
}

pub fn analyze_storage(
    root: &Path,
    options: StorageScanOptions,
    cancelled: Arc<AtomicBool>,
) -> Result<StorageAnalysis, StorageAnalysisError> {
    let root = fs::canonicalize(root).map_err(|source| StorageAnalysisError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let root_metadata = fs::symlink_metadata(&root).map_err(|source| StorageAnalysisError::Io {
        path: root.clone(),
        source,
    })?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(StorageAnalysisError::InvalidRoot(root));
    }

    let mut accumulators: HashMap<PathBuf, DirectoryAccumulator> = HashMap::new();
    let mut hard_links: HashSet<(u64, u64)> = HashSet::new();
    let mut skipped = Vec::new();
    let mut files_scanned = 0usize;
    let mut directories_scanned = 0usize;
    let mut root_node = None;

    let child_limit = options.max_children_per_directory.max(1);
    let walker = WalkDir::new(&root)
        .follow_links(false)
        .same_file_system(options.stay_on_filesystem)
        .contents_first(true)
        .max_open(32);

    for entry in walker {
        if cancelled.load(Ordering::Relaxed) {
            return Err(StorageAnalysisError::Cancelled);
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                if skipped.len() < 100 {
                    skipped.push(error.to_string());
                }
                continue;
            }
        };
        let path = entry.path();
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                if skipped.len() < 100 {
                    skipped.push(format!("{}: {error}", path.display()));
                }
                continue;
            }
        };

        if metadata.file_type().is_symlink()
            || (options.stay_on_filesystem && metadata.dev() != root_metadata.dev())
        {
            continue;
        }

        if metadata.is_file() {
            let identity = (metadata.dev(), metadata.ino());
            if metadata.nlink() > 1 && !hard_links.insert(identity) {
                continue;
            }

            files_scanned += 1;
            let size = allocated_size(&metadata);
            let Some(parent) = path.parent() else {
                continue;
            };
            let accumulator = accumulators.entry(parent.to_path_buf()).or_default();
            accumulator.size = accumulator.size.saturating_add(size);
            if size >= options.minimum_size {
                accumulator.retain_child(
                    StorageNode {
                        path: path.to_path_buf(),
                        kind: StorageNodeKind::File,
                        size,
                        modified: metadata.modified().ok(),
                        device: metadata.dev(),
                        inode: metadata.ino(),
                        children: Vec::new(),
                    },
                    child_limit,
                );
            }
            continue;
        }

        if !metadata.is_dir() {
            continue;
        }

        directories_scanned += 1;
        let mut accumulator = accumulators.remove(path).unwrap_or_default();
        accumulator.size = accumulator.size.saturating_add(allocated_size(&metadata));
        accumulator.rank_children(child_limit);

        let node = StorageNode {
            path: path.to_path_buf(),
            kind: StorageNodeKind::Directory,
            size: accumulator.size,
            modified: metadata.modified().ok(),
            device: metadata.dev(),
            inode: metadata.ino(),
            children: accumulator.children,
        };

        if path == root {
            root_node = Some(node);
            continue;
        }

        let Some(parent) = path.parent() else {
            continue;
        };
        let parent_accumulator = accumulators.entry(parent.to_path_buf()).or_default();
        parent_accumulator.size = parent_accumulator.size.saturating_add(node.size);
        if node.size >= options.minimum_size {
            parent_accumulator.retain_child(node, child_limit);
        }
    }

    let root = root_node.ok_or_else(|| StorageAnalysisError::InvalidRoot(root.clone()))?;
    Ok(StorageAnalysis {
        root,
        files_scanned,
        directories_scanned,
        skipped,
    })
}

fn allocated_size(metadata: &fs::Metadata) -> u64 {
    let allocated = metadata.blocks().saturating_mul(512);
    if allocated > 0 {
        allocated
    } else {
        metadata.len()
    }
}

#[derive(Debug, Clone)]
pub struct TrashTarget {
    pub path: PathBuf,
    pub kind: StorageNodeKind,
    pub size: u64,
    pub device: u64,
    pub inode: u64,
}

impl From<&StorageNode> for TrashTarget {
    fn from(node: &StorageNode) -> Self {
        Self {
            path: node.path.clone(),
            kind: node.kind,
            size: node.size,
            device: node.device,
            inode: node.inode,
        }
    }
}

#[derive(Debug, Default)]
pub struct TrashResult {
    pub moved: Vec<(PathBuf, u64)>,
    pub failed: Vec<(PathBuf, String)>,
}

impl TrashResult {
    pub fn bytes_moved(&self) -> u64 {
        self.moved.iter().map(|(_, size)| *size).sum()
    }
}

pub fn move_to_trash_with_progress(
    scan_root: &Path,
    targets: &[TrashTarget],
    progress: Option<&super::CleanProgress>,
) -> TrashResult {
    let mut result = TrashResult::default();
    let targets = normalize_targets(targets);

    for target in targets {
        if let Err(reason) = validate_trash_target(scan_root, &target) {
            result.failed.push((target.path.clone(), reason));
            continue;
        }

        let file = gio::File::for_path(&target.path);
        match file.trash(None::<&gio::Cancellable>) {
            Ok(()) => {
                if let Some(progress) = progress {
                    progress
                        .bytes_done
                        .fetch_add(target.size, Ordering::Relaxed);
                    progress.files_done.fetch_add(1, Ordering::Relaxed);
                }
                result.moved.push((target.path, target.size));
            }
            Err(error) => result.failed.push((target.path, error.to_string())),
        }
    }

    result
}

fn normalize_targets(targets: &[TrashTarget]) -> Vec<TrashTarget> {
    let mut sorted: Vec<_> = targets.iter().collect();
    sorted.sort_by_cached_key(|target| (target.path.components().count(), target.path.as_path()));
    sorted.dedup_by(|a, b| a.path == b.path);

    let mut directories = HashSet::new();
    let mut normalized = Vec::new();
    for target in sorted {
        if target
            .path
            .ancestors()
            .skip(1)
            .any(|p| directories.contains(p))
        {
            continue;
        }
        if target.kind == StorageNodeKind::Directory {
            directories.insert(target.path.as_path());
        }
        normalized.push(target.clone());
    }
    normalized
}

fn validate_trash_target(scan_root: &Path, target: &TrashTarget) -> Result<(), String> {
    if target.path == scan_root || !target.path.starts_with(scan_root) {
        return Err("The scan root itself and paths outside it are protected".to_string());
    }
    if target.path == Path::new("/") {
        return Err("The filesystem root is protected".to_string());
    }
    if let Some(home) = dirs::home_dir() {
        if target.path == home || home.starts_with(&target.path) {
            return Err("The home directory and its ancestors are protected".to_string());
        }
    }

    let canonical_root = fs::canonicalize(scan_root)
        .map_err(|error| format!("Could not re-check the analyzed folder: {error}"))?;
    if canonical_root != scan_root {
        return Err("The analyzed folder changed after the scan; scan again first".to_string());
    }

    let canonical_target = fs::canonicalize(&target.path)
        .map_err(|error| format!("Could not re-check the selected item: {error}"))?;
    if canonical_target != target.path || !canonical_target.starts_with(&canonical_root) {
        return Err("The selected path changed after the scan; scan again first".to_string());
    }

    let metadata = fs::symlink_metadata(&target.path)
        .map_err(|error| format!("Could not re-check the selected item: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("Symbolic links cannot be moved by the analyzer".to_string());
    }
    if metadata.uid() != unsafe { libc::getuid() } {
        return Err("The selected item is not owned by the current user".to_string());
    }
    if metadata.dev() != target.device || metadata.ino() != target.inode {
        return Err("The selected item changed after the scan; scan again first".to_string());
    }
    if metadata.is_dir() != (target.kind == StorageNodeKind::Directory) {
        return Err("The selected item type changed after the scan".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn removal_analysis() -> StorageAnalysis {
        fn node(path: &str, size: u64, children: Option<Vec<StorageNode>>) -> StorageNode {
            StorageNode {
                path: path.into(),
                kind: if children.is_some() {
                    StorageNodeKind::Directory
                } else {
                    StorageNodeKind::File
                },
                size,
                modified: None,
                device: 1,
                inode: 1,
                children: children.unwrap_or_default(),
            }
        }
        StorageAnalysis {
            root: node(
                "/scan",
                200,
                Some(vec![
                    node(
                        "/scan/a",
                        140,
                        Some(vec![node(
                            "/scan/a/b",
                            110,
                            Some(vec![
                                node("/scan/a/b/one", 60, None),
                                node("/scan/a/b/two", 30, None),
                            ]),
                        )]),
                    ),
                    node("/scan/keep", 40, None),
                ]),
            ),
            files_scanned: 3,
            directories_scanned: 3,
            skipped: vec![],
        }
    }

    #[test]
    fn external_removals_discount_hidden_files_without_double_counting_directories() {
        let mut analysis = removal_analysis();
        let original = analysis.root.size;
        analysis.root.size += 10;
        let update = super::super::removal_update::RemovalUpdate {
            paths: [
                "/scan/hidden".into(),
                "/scan/a/b".into(),
                "/scan/a/b/one".into(),
            ]
            .into(),
            sizes: vec![("/scan/hidden".into(), 10), ("/scan/a/b/one".into(), 60)],
            ..Default::default()
        };
        assert!(analysis.apply_removed(&update));
        assert_eq!(analysis.root.size, original - 110);
        assert!(analysis.root.find(Path::new("/scan/a/b")).is_none());
        assert!(!analysis.apply_removed(&super::super::removal_update::RemovalUpdate::default()));
    }

    #[test]
    fn remove_file_updates_every_ancestor_and_counts() {
        let mut analysis = removal_analysis();
        analysis.remove_paths(&["/scan/a/b/one".into()]);
        assert!(analysis.root.find(Path::new("/scan/a/b/one")).is_none());
        assert_eq!(analysis.root.size, 140);
        assert_eq!(analysis.root.find(Path::new("/scan/a")).unwrap().size, 80);
        assert_eq!(analysis.root.find(Path::new("/scan/a/b")).unwrap().size, 50);
        assert_eq!(
            analysis.root.find(Path::new("/scan/keep")).unwrap().size,
            40
        );
        assert_eq!(
            (analysis.files_scanned, analysis.directories_scanned),
            (2, 3)
        );
    }

    #[test]
    fn remove_directory_uses_its_total_size_and_counts_descendants() {
        let mut analysis = removal_analysis();
        analysis.remove_paths(&["/scan/a/b".into()]);
        assert!(analysis.root.find(Path::new("/scan/a/b")).is_none());
        assert_eq!(analysis.root.size, 90);
        assert_eq!(analysis.root.find(Path::new("/scan/a")).unwrap().size, 30);
        assert_eq!(
            analysis.root.find(Path::new("/scan/keep")).unwrap().size,
            40
        );
        assert_eq!(
            (analysis.files_scanned, analysis.directories_scanned),
            (1, 2)
        );
    }

    #[test]
    fn remove_nested_and_repeated_paths_only_subtracts_once() {
        for paths in [
            vec!["/scan/a", "/scan/a/b", "/scan/a/b/one", "/scan/a"],
            vec!["/scan/a/b/one", "/scan/a/b", "/scan/a", "/scan/a"],
        ] {
            let mut analysis = removal_analysis();
            analysis.remove_paths(&paths.into_iter().map(PathBuf::from).collect::<Vec<_>>());
            assert!(analysis.root.find(Path::new("/scan/a")).is_none());
            assert_eq!(analysis.root.size, 60);
            assert_eq!(analysis.root.children.len(), 1);
            assert_eq!(
                (analysis.files_scanned, analysis.directories_scanned),
                (1, 1)
            );
        }
    }

    #[test]
    fn remove_missing_paths_preserves_sizes_and_counts() {
        let mut analysis = removal_analysis();
        analysis.remove_paths(&[
            "/scan/a/b/missing".into(),
            "/scan/a-old".into(),
            "/outside".into(),
        ]);
        analysis.remove_paths(&[]);
        assert_eq!(analysis.root.size, 200);
        assert_eq!(analysis.root.find(Path::new("/scan/a")).unwrap().size, 140);
        assert_eq!(
            analysis.root.find(Path::new("/scan/a/b")).unwrap().size,
            110
        );
        assert_eq!(
            (analysis.files_scanned, analysis.directories_scanned),
            (3, 3)
        );
    }

    #[test]
    fn removal_saturates_sizes_and_counts() {
        let mut analysis = removal_analysis();
        analysis.root.size = 1;
        analysis.files_scanned = 0;
        analysis.directories_scanned = 0;
        analysis.remove_paths(&["/scan/a".into()]);
        assert_eq!(analysis.root.size, 0);
        assert_eq!(
            (analysis.files_scanned, analysis.directories_scanned),
            (0, 0)
        );
    }

    #[test]
    fn child_limit_preserves_totals_and_stable_order() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..29 {
            fs::write(dir.path().join(i.to_string()), vec![0; (i % 4) * 8192]).unwrap();
        }
        let scan = |limit| {
            analyze_storage(
                dir.path(),
                StorageScanOptions {
                    minimum_size: 0,
                    max_children_per_directory: limit,
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap()
        };
        let full = scan(usize::MAX);
        for limit in [0usize, 1, 5] {
            let capped = scan(limit);
            assert_eq!(capped.root.size, full.root.size);
            assert_eq!(capped.files_scanned, 29);
            assert!(capped.root.children.iter().map(|n| &n.path).eq(full
                .root
                .children
                .iter()
                .take(limit.max(1))
                .map(|n| &n.path)));
        }
    }

    #[test]
    fn scan_aggregates_directories_and_filters_small_files() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let large = nested.join("large.bin");
        let small = nested.join("small.bin");
        fs::File::create(&large)
            .unwrap()
            .write_all(&vec![0_u8; 16 * 1024])
            .unwrap();
        fs::write(&small, b"small").unwrap();

        let analysis = analyze_storage(
            directory.path(),
            StorageScanOptions {
                minimum_size: 8 * 1024,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        let nested_node = analysis.root.find(&nested).unwrap();
        assert!(nested_node.size >= 16 * 1024);
        assert!(nested_node.find(&large).is_some());
        assert!(nested_node.find(&small).is_none());
        assert_eq!(analysis.files_scanned, 2);
    }

    #[test]
    fn scan_ignores_symlinks_and_duplicate_hard_links() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.bin");
        let hard_link = directory.path().join("hard-link.bin");
        let symlink = directory.path().join("symlink.bin");
        fs::write(&original, vec![1_u8; 8192]).unwrap();
        fs::hard_link(&original, &hard_link).unwrap();
        std::os::unix::fs::symlink(&original, &symlink).unwrap();

        let analysis = analyze_storage(
            directory.path(),
            StorageScanOptions {
                minimum_size: 1,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(analysis.files_scanned, 1);
        assert_eq!(analysis.root.children.len(), 1);
        assert!(
            analysis.root.children[0].path == original
                || analysis.root.children[0].path == hard_link
        );
    }

    #[test]
    fn cancellation_stops_before_scanning() {
        let directory = tempfile::tempdir().unwrap();
        let cancelled = Arc::new(AtomicBool::new(true));
        let error = analyze_storage(directory.path(), StorageScanOptions::default(), cancelled)
            .unwrap_err();
        assert!(matches!(error, StorageAnalysisError::Cancelled));
    }

    #[test]
    fn scan_does_not_follow_directory_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("outside.bin"), b"outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), directory.path().join("link")).unwrap();

        let analysis = analyze_storage(
            directory.path(),
            StorageScanOptions {
                minimum_size: 0,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(analysis.files_scanned, 0);
        assert_eq!(analysis.directories_scanned, 1);
        assert!(analysis.root.children.is_empty());
    }

    #[test]
    fn scan_continues_after_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        // Root bypasses directory permissions, so this case needs an ordinary user.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("hidden.bin"), b"hidden").unwrap();
        let readable = directory.path().join("readable.bin");
        fs::write(&readable, b"readable").unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o000)).unwrap();

        let result = analyze_storage(
            directory.path(),
            StorageScanOptions {
                minimum_size: 0,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        );
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();

        let analysis = result.unwrap();
        assert_eq!(analysis.files_scanned, 1);
        assert!(analysis.root.find(&readable).is_some());
        assert!(!analysis.skipped.is_empty());
    }

    #[test]
    fn target_normalization_removes_descendants_of_selected_folders() {
        let parent = TrashTarget {
            path: PathBuf::from("/tmp/analyzer/parent"),
            kind: StorageNodeKind::Directory,
            size: 100,
            device: 1,
            inode: 1,
        };
        let child = TrashTarget {
            path: parent.path.join("child.bin"),
            kind: StorageNodeKind::File,
            size: 50,
            device: 1,
            inode: 2,
        };

        let normalized = normalize_targets(&[child, parent.clone()]);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].path, parent.path);
    }

    #[test]
    fn normalization_preserves_first_duplicate_and_similarly_named_siblings() {
        let folder = TrashTarget {
            path: PathBuf::from("/root/data"),
            kind: StorageNodeKind::Directory,
            size: 0,
            device: 1,
            inode: 1,
        };
        let file = TrashTarget {
            path: PathBuf::from("/root/data-old/file"),
            kind: StorageNodeKind::File,
            inode: 2,
            ..folder.clone()
        };
        let duplicate = TrashTarget {
            inode: 3,
            ..file.clone()
        };
        let result = normalize_targets(&[file, duplicate, folder]);
        assert_eq!(result.iter().map(|t| t.inode).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn trash_validation_rejects_the_scan_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let metadata = fs::metadata(&root).unwrap();
        let target = TrashTarget {
            path: root.clone(),
            kind: StorageNodeKind::Directory,
            size: allocated_size(&metadata),
            device: metadata.dev(),
            inode: metadata.ino(),
        };

        assert!(validate_trash_target(&root, &target).is_err());
    }

    #[test]
    fn trash_validation_rejects_changed_items() {
        let directory = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let file = root.join("large.bin");
        fs::write(&file, b"original").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let target = TrashTarget {
            path: file.clone(),
            kind: StorageNodeKind::File,
            size: allocated_size(&metadata),
            device: metadata.dev(),
            inode: metadata.ino(),
        };

        fs::rename(&file, root.join("original.bin")).unwrap();
        fs::write(&file, b"replacement").unwrap();

        assert!(validate_trash_target(&root, &target).is_err());
    }
}
