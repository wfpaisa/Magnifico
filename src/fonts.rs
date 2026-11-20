//! Bundled typeface (National Park, SIL Open Font License, see data/fonts/OFL.txt).

use std::ffi::{c_char, c_int, c_void, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub const FAMILY: &str = "National Park";
const NATIONAL_PARK: &[u8] = include_bytes!("../data/fonts/NationalPark.ttf");

#[link(name = "fontconfig")]
extern "C" {
    fn FcConfigAppFontAddFile(config: *mut c_void, file: *const c_char) -> c_int;
}

/// Fontconfig needs a file, so the embedded font is kept in the user's
/// data folder and rewritten only when the bundled copy changes.
fn stable_copy(dir: &Path) -> Option<PathBuf> {
    let path = dir.join("NationalPark.ttf");
    if std::fs::read(&path).ok().as_deref() != Some(NATIONAL_PARK) {
        std::fs::create_dir_all(dir).ok()?;
        let temp = dir.join(".NationalPark.ttf.tmp");
        std::fs::write(&temp, NATIONAL_PARK).ok()?;
        std::fs::rename(&temp, &path).ok()?;
    }
    Some(path)
}

/// Make National Park available to this process. Call before GTK creates its font
/// map; if it fails the theme's font is used instead.
pub fn register() -> bool {
    let dir = glib::user_data_dir().join("magnifico").join("fonts");
    let Some(path) = stable_copy(&dir) else {
        tracing::warn!("Could not store the bundled font");
        return false;
    };
    let Ok(file) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a null config means fontconfig's current one; `file` is a
    // valid NUL-terminated path that outlives the call.
    let added = unsafe { FcConfigAppFontAddFile(std::ptr::null_mut(), file.as_ptr()) } != 0;
    if !added {
        tracing::warn!("Could not register the bundled font");
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_copy_writes_once_and_matches_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let path = stable_copy(dir.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), NATIONAL_PARK);
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(stable_copy(dir.path()).unwrap(), path);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
    }
}
