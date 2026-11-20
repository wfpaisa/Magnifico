//! Snapshot and revalidate individual home-trash entries before deleting them.
//!
//! The home trash is read straight from disk (`$XDG_DATA_HOME/Trash`), not
//! through `trash:///`: that URI goes through the desktop's shared trash
//! service, which stalls on a large trash and takes other apps with it.

use super::cleaner::file_id;
#[cfg(test)]
use crate::models::TrashedFile;
#[cfg(test)]
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

const INFO_SUFFIX: &str = ".trashinfo";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashEntry {
    /// The trashed file, inside `Trash/files`.
    file: PathBuf,
    /// Its `.trashinfo`, inside `Trash/info`.
    info: PathBuf,
    original: PathBuf,
    deleted: String,
    id: Option<String>,
}

/// Names in `Trash/info` before a cleanup. Only names are read, so this
/// stays cheap however full the trash is.
pub type Snapshot = HashSet<OsString>;

fn trash_root() -> io::Result<PathBuf> {
    dirs::data_local_dir()
        .map(|dir| dir.join("Trash"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no data directory"))
}

fn info_names(root: &Path) -> io::Result<Snapshot> {
    match fs::read_dir(root.join("info")) {
        Ok(entries) => Ok(entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| name.as_bytes().ends_with(INFO_SUFFIX.as_bytes()))
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Snapshot::new()),
        Err(error) => Err(error),
    }
}

pub fn entries() -> io::Result<Vec<TrashEntry>> {
    let root = trash_root()?;
    Ok(info_names(&root)?
        .iter()
        .filter_map(|name| read_entry(&root, name))
        .collect())
}

impl TrashEntry {
    pub fn matches(&self, name: &Path, original: &str, deleted: &str) -> bool {
        self.file.file_name() == Some(name.as_os_str())
            && self.original == Path::new(original)
            && self.deleted == deleted
    }
}

fn read_entry(root: &Path, info_name: &OsStr) -> Option<TrashEntry> {
    let bytes = info_name.as_bytes();
    let name = OsStr::from_bytes(&bytes[..bytes.len().checked_sub(INFO_SUFFIX.len())?]);
    let info = root.join("info").join(info_name);
    let file = root.join("files").join(name);
    let (original, deleted) = parse_info(&fs::read_to_string(&info).ok()?)?;
    let id = fs::symlink_metadata(&file).ok().map(|meta| file_id(&meta));
    Some(TrashEntry {
        file,
        info,
        original,
        deleted,
        id,
    })
}

/// `Path` and `DeletionDate` from a `.trashinfo` file.
fn parse_info(text: &str) -> Option<(PathBuf, String)> {
    let mut path = None;
    let mut date = None;
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[Trash Info]";
        } else if in_section {
            if let Some(value) = line.strip_prefix("Path=") {
                path = Some(PathBuf::from(OsString::from_vec(unescape(value)?)));
            } else if let Some(value) = line.strip_prefix("DeletionDate=") {
                date = Some(value.to_string());
            }
        }
    }
    Some((path?, date?))
}

fn unescape(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Ambiguous matches are deliberately left in the trash. The dialog only
/// enables deletion when every successful move has an unambiguous identity.
#[cfg(test)]
fn identify(
    new: Vec<TrashEntry>,
    paths: &[TrashedFile],
    started: chrono::NaiveDateTime,
    finished: chrono::NaiveDateTime,
) -> Vec<TrashEntry> {
    let originals: HashMap<_, _> = paths.iter().map(|file| (&file.path, &file.id)).collect();
    let mut matches: HashMap<PathBuf, Vec<TrashEntry>> = HashMap::new();
    for entry in new {
        let date = chrono::NaiveDateTime::parse_from_str(&entry.deleted, "%Y-%m-%dT%H:%M:%S");
        if originals
            .get(&entry.original)
            .is_some_and(|id| id.is_some() && **id == entry.id)
            && date.is_ok_and(|date| date >= started && date <= finished)
        {
            matches
                .entry(entry.original.clone())
                .or_default()
                .push(entry);
        }
    }
    matches
        .into_values()
        .filter_map(|mut entries| {
            if entries.len() == 1 {
                entries.pop()
            } else {
                None
            }
        })
        .collect()
}

/// Revalidate each captured entry immediately before deleting it. An entry
/// is a file, or a whole folder the cleaner moved as one unit.
pub fn delete(entries: Vec<TrashEntry>) -> usize {
    let Ok(root) = trash_root() else {
        return entries.len();
    };
    delete_from(&root, entries)
}

fn delete_from(root: &Path, entries: Vec<TrashEntry>) -> usize {
    let mut failed = 0;
    for entry in entries {
        let Some(info_name) = entry.info.file_name() else {
            failed += 1;
            continue;
        };
        match read_entry(root, info_name) {
            Some(current) if current == entry => {
                let is_dir = fs::symlink_metadata(&entry.file).is_ok_and(|meta| meta.is_dir());
                let removed = if is_dir {
                    fs::remove_dir_all(&entry.file)
                } else {
                    fs::remove_file(&entry.file)
                };
                if let Err(error) = removed {
                    if error.kind() != io::ErrorKind::NotFound {
                        tracing::warn!("Could not delete cleanup trash item: {error}");
                        failed += 1;
                        continue;
                    }
                }
                let _ = fs::remove_file(&entry.info);
            }
            None if !entry.info.exists() => {}
            _ => failed += 1,
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, path: &str, deleted: &str) -> TrashEntry {
        TrashEntry {
            file: format!("/trash/files/{name}").into(),
            info: format!("/trash/info/{name}.trashinfo").into(),
            original: path.into(),
            deleted: deleted.into(),
            id: Some(format!("file:{path}")),
        }
    }

    fn source(path: &str) -> TrashedFile {
        TrashedFile {
            path: path.into(),
            size: 1,
            id: Some(format!("file:{path}")),
        }
    }

    #[test]
    fn deletion_rejects_a_replacement_at_the_reviewed_trash_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("files")).unwrap();
        fs::create_dir(root.join("info")).unwrap();
        let file = root.join("files/item");
        fs::write(&file, b"reviewed").unwrap();
        fs::write(
            root.join("info/item.trashinfo"),
            "[Trash Info]\nPath=/home/u/item\nDeletionDate=2026-11-18T12:00:00\n",
        )
        .unwrap();
        let reviewed = read_entry(root, OsStr::new("item.trashinfo")).unwrap();
        fs::rename(&file, root.join("original")).unwrap();
        fs::write(&file, b"replacement").unwrap();
        assert_eq!(delete_from(root, vec![reviewed]), 1);
        assert_eq!(fs::read(&file).unwrap(), b"replacement");
        let current = read_entry(root, OsStr::new("item.trashinfo")).unwrap();
        assert_eq!(delete_from(root, vec![current]), 0);
        assert!(!file.exists());
        assert!(!root.join("info/item.trashinfo").exists());
    }

    #[test]
    fn matches_only_exact_paths_and_dates() {
        let ours = entry("ours", "/tmp/a", "2026-11-17T10:00:02");
        let new = vec![
            ours.clone(),
            entry("other", "/tmp/ab", "2026-11-17T10:00:02"),
            entry("late", "/tmp/b", "2026-11-17T10:01:00"),
        ];
        let start =
            chrono::NaiveDateTime::parse_from_str("2026-11-17T10:00:01", "%Y-%m-%dT%H:%M:%S")
                .unwrap();
        assert_eq!(
            identify(
                new,
                &[source("/tmp/a"), source("/tmp/b")],
                start,
                start + chrono::Duration::seconds(3)
            ),
            vec![ours]
        );
    }

    #[test]
    fn rejects_a_different_file_trashed_from_the_same_path() {
        let start = chrono::Local::now().naive_local();
        let date = start.format("%Y-%m-%dT%H:%M:%S").to_string();
        let mut replacement = entry("replacement", "/tmp/a", &date);
        replacement.id = Some("another-inode".into());
        assert!(identify(
            vec![replacement],
            &[source("/tmp/a")],
            start - chrono::Duration::seconds(1),
            start + chrono::Duration::seconds(1)
        )
        .is_empty());
    }

    #[test]
    fn refuses_ambiguous_or_invalid_trash_metadata() {
        let start = chrono::Local::now().naive_local();
        let date = start.format("%Y-%m-%dT%H:%M:%S").to_string();
        let new = vec![
            entry("a", "/tmp/a", &date),
            entry("b", "/tmp/a", &date),
            entry("c", "/tmp/b", "invalid"),
        ];
        assert!(identify(
            new,
            &[source("/tmp/a"), source("/tmp/b")],
            start - chrono::Duration::seconds(1),
            start + chrono::Duration::seconds(1)
        )
        .is_empty());
    }

    #[test]
    fn parses_escaped_trashinfo() {
        let text =
            "[Trash Info]\nPath=/home/u/caf%C3%A9%20x.txt\nDeletionDate=2026-11-17T10:00:02\n";
        assert_eq!(
            parse_info(text),
            Some((
                PathBuf::from("/home/u/café x.txt"),
                "2026-11-17T10:00:02".to_string()
            ))
        );
        assert_eq!(parse_info("[Other]\nPath=/a\nDeletionDate=x\n"), None);
    }
}
