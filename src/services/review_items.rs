//! "AI Models" and "Games" review lists: one entry per model or game found by
//! `big_items`, sized and turned into trash targets off the GTK thread.
use super::big_items::{find_ai_models, find_games, BigItem, Launcher};
use super::storage_analyzer::{StorageNodeKind, TrashResult, TrashTarget};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug)]
pub struct ReviewItem {
    pub title: String,
    /// Provider or launcher ("Hugging Face", "Steam", …).
    pub source: String,
    /// Main folder shown under the title.
    pub location: PathBuf,
    pub targets: Vec<TrashTarget>,
    pub bytes: u64,
    pub last_used: Option<SystemTime>,
    /// Local cover or icon image, when the launcher keeps one.
    pub icon: Option<PathBuf>,
    /// Removing it needs Steam closed.
    pub steam: bool,
}

pub fn scan_models(home: &Path, cancel: &AtomicBool) -> Vec<ReviewItem> {
    let mut items: Vec<_> = find_ai_models(home)
        .into_iter()
        .filter_map(|item| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let (source, title) = split_label(&item.label);
            let source = source.to_string();
            build(item, &source, title, None, false, None)
        })
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.bytes));
    items
}

pub fn scan_games(home: &Path, cancel: &AtomicBool) -> Vec<ReviewItem> {
    let covers = steam_covers(home);
    let mut items: Vec<_> = find_games(home)
        .into_iter()
        .filter_map(|(launcher, mut item)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let source = match launcher {
                Launcher::Steam => "Steam",
                Launcher::Heroic => "Heroic",
                Launcher::Folder => "~/Games",
            };
            let mut played = None;
            let mut icon = None;
            if launcher == Launcher::Steam {
                if let Some(id) = item.paths.iter().find_map(|p| app_id(p)) {
                    // The game's own shader cache goes with it.
                    if let Some(steamapps) = item.paths[0].parent().and_then(Path::parent) {
                        let shaders = steamapps.join("shadercache").join(&id);
                        if shaders.is_dir() {
                            item.paths.push(shaders);
                        }
                    }
                    played = item.paths.iter().find_map(|p| last_played(p));
                    icon = covers.iter().find_map(|root| cover(root, &id));
                }
            }
            let title = item.label.clone();
            build(
                item,
                source,
                title,
                played,
                launcher == Launcher::Steam,
                icon,
            )
        })
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.bytes));
    items
}

fn build(
    item: BigItem,
    source: &str,
    title: String,
    played: Option<SystemTime>,
    steam: bool,
    icon: Option<PathBuf>,
) -> Option<ReviewItem> {
    let location = item.paths.first()?.clone();
    let mut targets = Vec::new();
    let mut newest = None;
    for path in &item.paths {
        let Ok(meta) = fs::symlink_metadata(path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let (size, modified) = measure(path, &meta);
        newest = newest.max(modified);
        targets.push(TrashTarget {
            path: path.clone(),
            kind: if meta.is_dir() {
                StorageNodeKind::Directory
            } else {
                StorageNodeKind::File
            },
            size,
            device: meta.dev(),
            inode: meta.ino(),
        });
    }
    let bytes: u64 = targets.iter().map(|t| t.size).sum();
    (bytes > 0).then(|| ReviewItem {
        title,
        source: source.to_string(),
        location,
        targets,
        bytes,
        last_used: played.or(newest),
        icon,
        steam,
    })
}

/// Bytes on disk and the newest modification time below `path`.
fn measure(path: &Path, meta: &fs::Metadata) -> (u64, Option<SystemTime>) {
    if !meta.is_dir() {
        return (meta.blocks() * 512, meta.modified().ok());
    }
    let mut bytes = 0;
    let mut newest = meta.modified().ok();
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .same_file_system(true)
        .into_iter()
        .flatten()
    {
        if let Ok(meta) = entry.metadata() {
            bytes += meta.blocks() * 512;
            if meta.is_file() {
                newest = newest.max(meta.modified().ok());
            }
        }
    }
    (bytes, newest)
}

fn split_label(label: &str) -> (&str, String) {
    match label.split_once(" · ") {
        Some((source, title)) => (source, title.to_string()),
        None => (label, label.to_string()),
    }
}

fn app_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let id = name.strip_prefix("appmanifest_")?.strip_suffix(".acf")?;
    id.chars()
        .all(|c| c.is_ascii_digit())
        .then(|| id.to_string())
}

fn last_played(manifest: &Path) -> Option<SystemTime> {
    app_id(manifest)?;
    let text = fs::read_to_string(manifest).ok()?;
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("\"LastPlayed\""))?;
    let seconds: u64 = line.split('"').nth(3)?.parse().ok()?;
    (seconds > 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
}

fn steam_covers(home: &Path) -> Vec<PathBuf> {
    [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
    .into_iter()
    .map(|root| root.join("appcache/librarycache"))
    .filter(|dir| dir.is_dir())
    .collect()
}

/// Steam keeps artwork as `<id>_icon.jpg` (older) or `<id>/<hash>.jpg` (newer).
fn cover(root: &Path, id: &str) -> Option<PathBuf> {
    for name in [
        format!("{id}_icon.jpg"),
        format!("{id}_library_600x900.jpg"),
        format!("{id}_header.jpg"),
    ] {
        let path = root.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    let folder = root.join(id);
    for name in ["library_600x900.jpg", "header.jpg"] {
        let path = folder.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    // The icon is the small square image with a hash for a name.
    fs::read_dir(&folder)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jpg"))
        .min_by_key(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(u64::MAX))
}

/// True while a Steam client process runs.
pub fn steam_running() -> bool {
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .any(|e| {
            fs::read_to_string(e.path().join("comm"))
                .is_ok_and(|comm| matches!(comm.trim(), "steam" | "steamwebhelper"))
        })
}

/// Move every target to the trash. Each one is checked against its own
/// parent folder, since game libraries may live on other drives.
pub fn trash(items: &[ReviewItem], progress: Option<&super::CleanProgress>) -> TrashResult {
    let mut result = TrashResult::default();
    for target in items.iter().flat_map(|i| i.targets.iter()) {
        let Some(root) = target.path.parent() else {
            continue;
        };
        let part = super::move_to_trash_with_progress(root, std::slice::from_ref(target), progress);
        result.moved.extend(part.moved);
        result.failed.extend(part.failed);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_split_into_provider_and_name() {
        assert_eq!(
            split_label("Hugging Face · org/model"),
            ("Hugging Face", "org/model".to_string())
        );
        assert_eq!(split_label("Ollama"), ("Ollama", "Ollama".to_string()));
    }

    #[test]
    fn steam_manifest_ids_and_last_played() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("appmanifest_620.acf");
        fs::write(
            &manifest,
            "\"AppState\"\n{\n\t\"appid\"\t\t\"620\"\n\t\"LastPlayed\"\t\t\"1700000000\"\n}\n",
        )
        .unwrap();
        assert_eq!(app_id(&manifest).as_deref(), Some("620"));
        assert_eq!(
            last_played(&manifest),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
        assert_eq!(app_id(&dir.path().join("common")), None);
    }

    #[test]
    fn items_are_sized_and_skip_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("models--org--name");
        fs::create_dir_all(model.join("blobs")).unwrap();
        fs::write(model.join("blobs/a"), vec![1u8; 10_000]).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&model, &link).unwrap();
        let item = BigItem {
            label: "Hugging Face · org/name".into(),
            paths: vec![model.clone(), link],
        };
        let built = build(item, "Hugging Face", "org/name".into(), None, false, None).unwrap();
        assert_eq!(built.targets.len(), 1);
        assert!(built.bytes >= 10_000);
        assert_eq!(built.location, model);
    }
}
