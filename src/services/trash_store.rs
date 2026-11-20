// Inspired by Purge TrashStore.swift (MIT, © 2026 Jithin Sabu).
// See THIRD_PARTY_LICENSES.md. GIO owns trash metadata and mounted-volume trash.
use gio::prelude::*;
use std::sync::atomic::Ordering;

#[derive(Clone)]
pub struct Entry {
    pub uri: String,
    pub name: String,
    pub bytes: u64,
    identity: String,
    local: Option<super::cleanup_trash::TrashEntry>,
}

const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::size,id::file,etag::value,trash::orig-path,trash::deletion-date";

fn identity(info: &gio::FileInfo) -> String {
    [
        "id::file",
        "etag::value",
        "trash::orig-path",
        "trash::deletion-date",
    ]
    .iter()
    .map(|key| {
        info.attribute_as_string(key)
            .map(|s| s.to_string())
            .unwrap_or_default()
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn children(file: &gio::File) -> Result<Vec<(gio::File, gio::FileInfo)>, glib::Error> {
    let list = file.enumerate_children(
        ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        None::<&gio::Cancellable>,
    )?;
    let mut entries = Vec::new();
    while let Some(info) = list.next_file(None::<&gio::Cancellable>)? {
        entries.push((list.child(&info), info));
    }
    Ok(entries)
}

fn bytes(file: &gio::File, info: &gio::FileInfo) -> Result<u64, glib::Error> {
    if info.file_type() == gio::FileType::Directory {
        children(file)?.iter().try_fold(0u64, |sum, (file, info)| {
            Ok(sum.saturating_add(bytes(file, info)?))
        })
    } else {
        Ok(info.size().max(0) as u64)
    }
}

pub fn entries() -> Result<Vec<Entry>, glib::Error> {
    let local = super::cleanup_trash::entries().unwrap_or_default();
    children(&gio::File::for_uri("trash:///"))?
        .into_iter()
        .map(|(file, info)| {
            Ok(Entry {
                uri: file.uri().into(),
                name: info.display_name().into(),
                bytes: bytes(&file, &info)?,
                identity: identity(&info),
                local: local
                    .iter()
                    .find(|entry| {
                        file.basename().is_some_and(|name| {
                            entry.matches(
                                &name,
                                info.attribute_byte_string("trash::orig-path")
                                    .as_deref()
                                    .unwrap_or_default(),
                                info.attribute_string("trash::deletion-date")
                                    .as_deref()
                                    .unwrap_or_default(),
                            )
                        })
                    })
                    .cloned(),
            })
        })
        .collect()
}

fn remove(file: &gio::File) -> Result<(), glib::Error> {
    let kind = file.query_file_type(
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        None::<&gio::Cancellable>,
    );
    if kind == gio::FileType::Directory {
        for (child, _) in children(file)? {
            remove(&child)?;
        }
    }
    file.delete(None::<&gio::Cancellable>)
}

/// Delete only the entries reviewed in the sheet, never new arrivals or the root.
pub fn empty(entries: &[Entry], progress: &super::CleanProgress) -> Vec<String> {
    let mut errors = Vec::new();
    for entry in entries {
        let file = gio::File::for_uri(&entry.uri);
        if file.parent().as_ref().map(|p| p.uri()).as_deref() != Some("trash:///") {
            continue;
        }
        let unchanged = file
            .query_info(
                ATTRIBUTES,
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                None::<&gio::Cancellable>,
            )
            .is_ok_and(|info| {
                !entry.identity.trim().is_empty() && identity(&info) == entry.identity
            });
        if !unchanged {
            errors.push(format!(
                "{}: {}",
                entry.name,
                crate::i18n::tr("The trash item changed. Review it again.")
            ));
            continue;
        }
        // Home-trash entries retain the existing inode and metadata revalidation.
        let removed = if let Some(local) = &entry.local {
            if super::cleanup_trash::delete(vec![local.clone()]) == 0 {
                Ok(())
            } else {
                Err(glib::Error::new(
                    gio::IOErrorEnum::Failed,
                    &crate::i18n::tr("The trash item changed. Review it again."),
                ))
            }
        } else {
            remove(&file)
        };
        match removed {
            Ok(()) => {
                progress
                    .bytes_done
                    .fetch_add(entry.bytes, Ordering::Relaxed);
                progress.files_done.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => errors.push(format!("{}: {error}", entry.name)),
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_does_not_follow_links_out_of_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("keep");
        std::fs::write(&outside, b"keep").unwrap();
        let trash = dir.path().join("discard");
        std::fs::create_dir(&trash).unwrap();
        std::os::unix::fs::symlink(&outside, trash.join("link")).unwrap();
        remove(&gio::File::for_path(&trash)).unwrap();
        assert!(outside.exists());
        assert!(!trash.exists());
    }
}
