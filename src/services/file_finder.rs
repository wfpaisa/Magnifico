//! Find large files and duplicate files in a folder. Both searches only read;
//! removing what they find goes through `move_to_trash`, so it can be undone.
//!
//! The duplicate search follows Purge's approach (MIT): group by size, then
//! by a hash of the start and end of each file, and only then hash whole
//! files, so most files are never read in full.

use super::storage_analyzer::{StorageNodeKind, TrashTarget};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::SystemTime;
use walkdir::WalkDir;

/// At most this many large files are listed, largest first.
const MAX_LARGE_FILES: usize = 1000;
/// Bytes read from each end of a file for the quick duplicate check.
const EDGE_BYTES: u64 = 16 * 1024;

/// Rough kind of a file, from its name, for the filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    Video,
    Image,
    Audio,
    DiskImage,
    VirtualMachine,
    AiModel,
    Archive,
    Incomplete,
    Other,
}

impl FileKind {
    #[allow(dead_code)] // Retained for file-kind consumers outside the review page.
    pub const FILTERS: [FileKind; 8] = [
        Self::Video,
        Self::Image,
        Self::Audio,
        Self::DiskImage,
        Self::VirtualMachine,
        Self::AiModel,
        Self::Archive,
        Self::Incomplete,
    ];

    #[allow(dead_code)]
    pub fn title(self) -> &'static str {
        match self {
            Self::Video => "Videos",
            Self::Image => "Images",
            Self::Audio => "Audio",
            Self::DiskImage => "Disk Images",
            Self::VirtualMachine => "Virtual Machines",
            Self::AiModel => "AI Models",
            Self::Archive => "Archives",
            Self::Incomplete => "Unfinished Downloads",
            Self::Other => "Other",
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Video => "pc-video-x-generic-symbolic",
            Self::Image => "pc-image-x-generic-symbolic",
            Self::Audio => "pc-audio-x-generic-symbolic",
            Self::DiskImage => "pc-media-optical-symbolic",
            Self::VirtualMachine => "pc-computer-symbolic",
            Self::AiModel => "pc-applications-science-symbolic",
            Self::Archive => "pc-package-x-generic-symbolic",
            Self::Incomplete => "pc-folder-download-symbolic",
            Self::Other => "pc-text-x-generic-symbolic",
        }
    }

    pub fn of(path: &Path) -> FileKind {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let ext = name.rsplit('.').next().unwrap_or("");
        match ext {
            "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg"
            | "ts" | "m2ts" => Self::Video,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "tif" | "tiff" | "bmp" | "heic" | "avif"
            | "raw" | "cr2" | "nef" | "arw" | "dng" | "psd" | "xcf" | "kra" => Self::Image,
            "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" | "wma" => Self::Audio,
            "iso" | "img" | "dmg" => Self::DiskImage,
            "qcow2" | "qcow" | "vdi" | "vmdk" | "vhd" | "vhdx" | "ova" | "ovf" => {
                Self::VirtualMachine
            }
            "gguf" | "ggml" | "safetensors" | "ckpt" | "pt" | "pth" | "onnx" => Self::AiModel,
            "bin"
                if ["model", "pytorch", "weights"]
                    .iter()
                    .any(|w| name.contains(w)) =>
            {
                Self::AiModel
            }
            "data" if name.contains(".onnx.") => Self::AiModel,
            "zip" | "tar" | "gz" | "tgz" | "xz" | "txz" | "bz2" | "zst" | "7z" | "rar" | "deb"
            | "rpm" | "appimage" | "flatpak" => Self::Archive,
            "part" | "crdownload" | "partial" | "download" | "!qb" => Self::Incomplete,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FoundFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Last opened, as far as the file system records it (relatime).
    pub accessed: Option<SystemTime>,
    pub kind: FileKind,
    device: u64,
    inode: u64,
}

impl FoundFile {
    fn from_meta(path: PathBuf, meta: &fs::Metadata) -> Self {
        Self {
            kind: FileKind::of(&path),
            size: meta.len(),
            modified: meta.modified().ok(),
            accessed: meta.accessed().ok(),
            device: meta.dev(),
            inode: meta.ino(),
            path,
        }
    }

    pub fn trash_target(&self) -> TrashTarget {
        TrashTarget {
            path: self.path.clone(),
            kind: StorageNodeKind::File,
            size: self.size,
            device: self.device,
            inode: self.inode,
        }
    }
}

/// Regular files under `root` (same disk, symlinks not followed), skipping
/// the trash and any folder `skip_dir` rejects.
fn regular_files<'a>(
    root: &'a Path,
    skip_dir: impl Fn(&Path) -> bool + 'a,
    cancelled: &'a AtomicBool,
) -> impl Iterator<Item = (PathBuf, fs::Metadata)> + 'a {
    let trash = dirs::data_local_dir().map(|d| d.join("Trash"));
    WalkDir::new(root)
        .follow_links(false)
        .same_file_system(true)
        .into_iter()
        .filter_entry(move |entry| {
            if cancelled.load(Ordering::Relaxed) {
                return false;
            }
            if entry.depth() == 0 || !entry.file_type().is_dir() {
                return true;
            }
            Some(entry.path()) != trash.as_deref() && !skip_dir(entry.path())
        })
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some((entry.into_path(), meta))
        })
}

/// Files of at least `min_size` bytes, largest first.
/// `seen` counts every file looked at, for a progress display.
pub fn find_large_files(
    root: &Path,
    min_size: u64,
    seen: &AtomicUsize,
    cancelled: &AtomicBool,
) -> Vec<FoundFile> {
    let mut found = Vec::new();
    // The smallest retained file is evicted first. For ties, evict the latest.
    let mut ranking = BinaryHeap::new();
    for (order, (path, meta)) in regular_files(root, |_| false, cancelled).enumerate() {
        seen.fetch_add(1, Ordering::Relaxed);
        let size = meta.len();
        if size < min_size {
            continue;
        }
        let slot = if found.len() < MAX_LARGE_FILES {
            found.push(FoundFile::from_meta(path, &meta));
            found.len() - 1
        } else {
            let mut smallest = ranking.peek_mut().expect("full result set has a ranking");
            let (Reverse(cutoff), _, slot) = *smallest;
            if size <= cutoff {
                continue;
            }
            found[slot] = FoundFile::from_meta(path, &meta);
            *smallest = (Reverse(size), order, slot);
            continue;
        };
        ranking.push((Reverse(size), order, slot));
    }

    let ranked = ranking.into_sorted_vec().into_iter();
    // Reuse the ranked slots to move each result into display order.
    let mut slots: Vec<_> = found.into_iter().map(Some).collect();
    ranked
        .map(|(_, _, slot)| slots[slot].take().unwrap())
        .collect()
}

#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    /// Same content in every file. `files[0]` is the suggested one to keep.
    pub files: Vec<FoundFile>,
}

impl DuplicateGroup {
    pub fn size(&self) -> u64 {
        self.files.first().map(|f| f.size).unwrap_or(0)
    }

    /// Space freed by keeping one copy.
    pub fn wasted(&self) -> u64 {
        self.size() * (self.files.len().saturating_sub(1) as u64)
    }
}

/// Progress of a duplicate search, readable from the UI thread.
#[derive(Default)]
pub struct DuplicateProgress {
    pub files_seen: AtomicUsize,
    pub files_to_hash: AtomicUsize,
    pub files_hashed: AtomicUsize,
}

/// Files with identical content under `root`, biggest waste first. Hidden
/// folders and regenerable build folders are skipped: copies there are
/// expected and not the user's to tidy.
pub fn find_duplicates(
    root: &Path,
    min_size: u64,
    progress: &DuplicateProgress,
    cancelled: &AtomicBool,
) -> Vec<DuplicateGroup> {
    const SKIP: &[&str] = &["node_modules", "target", "venv", "__pycache__", "build"];
    let skip_dir = |path: &Path| {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        name.starts_with('.') || SKIP.contains(&name.as_ref())
    };

    // 1. Same size. Hard links share their data, so each inode counts once.
    let mut by_size: HashMap<u64, Vec<FoundFile>> = HashMap::new();
    let mut inodes = HashSet::new();
    for (path, meta) in regular_files(root, skip_dir, cancelled) {
        progress.files_seen.fetch_add(1, Ordering::Relaxed);
        if meta.len() < min_size.max(1) || !inodes.insert((meta.dev(), meta.ino())) {
            continue;
        }
        by_size
            .entry(meta.len())
            .or_default()
            .push(FoundFile::from_meta(path, &meta));
    }
    let candidates: Vec<Vec<FoundFile>> = by_size
        .into_values()
        .filter(|files| files.len() > 1)
        .collect();
    progress
        .files_to_hash
        .store(candidates.iter().map(Vec::len).sum(), Ordering::Relaxed);

    // 2. Same start and end, 3. same full hash.
    let mut groups: Vec<DuplicateGroup> = candidates
        .into_par_iter()
        .flat_map_iter(|files| {
            let quick = split_by(files, |file| edge_hash(&file.path, file.size));
            quick
                .into_iter()
                .flat_map(|files| split_by(files, |file| full_hash(&file.path, cancelled)))
                .inspect(|files| {
                    progress
                        .files_hashed
                        .fetch_add(files.len(), Ordering::Relaxed);
                })
                .collect::<Vec<_>>()
        })
        .map(|mut files| {
            files.sort_by(keep_first);
            DuplicateGroup { files }
        })
        .collect();
    if cancelled.load(Ordering::Relaxed) {
        return Vec::new();
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.wasted()));
    groups
}

/// Groups of two or more files with the same key; unreadable files drop out.
fn split_by<K: Eq + std::hash::Hash>(
    files: Vec<FoundFile>,
    key: impl Fn(&FoundFile) -> Option<K>,
) -> Vec<Vec<FoundFile>> {
    let mut groups: HashMap<K, Vec<FoundFile>> = HashMap::new();
    for file in files {
        if let Some(k) = key(&file) {
            groups.entry(k).or_default().push(file);
        }
    }
    groups.into_values().filter(|g| g.len() > 1).collect()
}

fn edge_hash(path: &Path, size: u64) -> Option<[u8; 32]> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; EDGE_BYTES.min(size) as usize];
    file.read_exact(&mut buffer).ok()?;
    hasher.update(&buffer);
    if size > EDGE_BYTES {
        file.seek(SeekFrom::Start(size - buffer.len() as u64))
            .ok()?;
        file.read_exact(&mut buffer).ok()?;
        hasher.update(&buffer);
    }
    Some(*hasher.finalize().as_bytes())
}

fn full_hash(path: &Path, cancelled: &AtomicBool) -> Option<[u8; 32]> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(*hasher.finalize().as_bytes())
}

/// Order copies so the one most worth keeping comes first: not in a
/// downloads or temporary folder, then the oldest, then the shortest path.
fn keep_first(a: &FoundFile, b: &FoundFile) -> std::cmp::Ordering {
    let transient = |file: &FoundFile| {
        file.path.components().any(|c| {
            let name = c.as_os_str().to_string_lossy().to_lowercase();
            matches!(
                name.as_str(),
                "downloads" | "descargas" | "tmp" | "temp" | "copy" | "backup"
            )
        })
    };
    transient(a)
        .cmp(&transient(b))
        .then_with(|| a.modified.cmp(&b.modified))
        .then_with(|| a.path.as_os_str().len().cmp(&b.path.as_os_str().len()))
        .then_with(|| a.path.cmp(&b.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn kinds_come_from_the_file_name() {
        assert_eq!(FileKind::of(Path::new("/a/movie.MKV")), FileKind::Video);
        assert_eq!(FileKind::of(Path::new("/a/arch.iso")), FileKind::DiskImage);
        assert_eq!(FileKind::of(Path::new("/a/llama.gguf")), FileKind::AiModel);
        assert_eq!(FileKind::of(Path::new("/a/firmware.bin")), FileKind::Other);
        assert_eq!(FileKind::of(Path::new("/a/weights.bin")), FileKind::AiModel);
        assert_eq!(
            FileKind::of(Path::new("/a/enc.onnx.data")),
            FileKind::AiModel
        );
        assert_eq!(
            FileKind::of(Path::new("/a/x.zip.part")),
            FileKind::Incomplete
        );
    }

    #[test]
    fn large_files_are_listed_largest_first() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("small"), vec![0; 10]).unwrap();
        fs::write(dir.path().join("big"), vec![0; 300]).unwrap();
        fs::write(dir.path().join("bigger"), vec![0; 500]).unwrap();
        let found = find_large_files(
            dir.path(),
            100,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        let names: Vec<_> = found
            .iter()
            .map(|f| f.path.file_name().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["bigger", "big"]);
    }

    #[test]
    fn capped_large_files_match_stable_full_sort() {
        let dir = TempDir::new().unwrap();
        for i in 0..MAX_LARGE_FILES + 80 {
            fs::write(dir.path().join(i.to_string()), vec![0; i % 13]).unwrap();
        }
        let cancelled = AtomicBool::new(false);
        let mut expected: Vec<_> = regular_files(dir.path(), |_| false, &cancelled)
            .map(|(path, meta)| (path, meta.len()))
            .collect();
        expected.sort_by_key(|entry| Reverse(entry.1));
        let seen = AtomicUsize::new(0);
        let found = find_large_files(dir.path(), 0, &seen, &cancelled);
        assert_eq!(seen.load(Ordering::Relaxed), expected.len());
        assert!(found.into_iter().map(|file| file.path).eq(expected
            .into_iter()
            .take(MAX_LARGE_FILES)
            .map(|entry| entry.0)));
    }

    #[test]
    fn finds_copies_but_not_same_sized_or_hard_linked_files() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = vec![7u8; 40_000];
        let mut other = content.clone();
        other[20_000] = 8; // same size, same edges, different middle
        fs::create_dir_all(root.join("Downloads")).unwrap();
        fs::write(root.join("photo.jpg"), &content).unwrap();
        fs::write(root.join("Downloads/photo.jpg"), &content).unwrap();
        fs::write(root.join("other.jpg"), &other).unwrap();
        fs::hard_link(root.join("photo.jpg"), root.join("link.jpg")).unwrap();

        let groups = find_duplicates(
            root,
            1,
            &DuplicateProgress::default(),
            &AtomicBool::new(false),
        );
        assert_eq!(groups.len(), 1);
        let files = &groups[0].files;
        assert_eq!(files.len(), 2);
        // The copy outside Downloads is suggested to keep.
        assert!(!files[0].path.starts_with(root.join("Downloads")));
        assert_eq!(groups[0].wasted(), 40_000);
    }
}
