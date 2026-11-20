use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const AUTOSTART_FILE: &str = "io.github.wfpaisa.Magnifico.desktop";

pub fn is_enabled() -> bool {
    dirs::home_dir().is_some_and(|home| is_enabled_in_home(&home))
}

fn is_enabled_in_home(home: &Path) -> bool {
    autostart_path_in_home(home).is_file()
}

pub fn set_enabled(enabled: bool) -> io::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Could not locate the home directory",
        )
    })?;
    set_enabled_in_home(&home, enabled)
}

fn autostart_path_in_home(home: &Path) -> PathBuf {
    home.join(".config").join("autostart").join(AUTOSTART_FILE)
}

fn set_enabled_in_home(home: &Path, enabled: bool) -> io::Result<()> {
    let path = autostart_path_in_home(home);
    if !enabled {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }

    let parent = path.parent().expect("autostart path always has a parent");
    fs::create_dir_all(parent)?;

    let desktop_entry = "[Desktop Entry]\n\
         Type=Application\n\
         Name=Magnífico\n\
         Comment=Start Magnífico at login for scheduled cleanup\n\
         Exec=magnifico --background\n\
         Icon=io.github.wfpaisa.Magnifico\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n";

    let temporary_path = parent.join(format!(".{AUTOSTART_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary_path, desktop_entry)?;
    fs::rename(&temporary_path, &path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary_path);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_creates_and_refreshes_native_entry() {
        let home = tempfile::tempdir().unwrap();
        set_enabled_in_home(home.path(), true).unwrap();
        set_enabled_in_home(home.path(), true).unwrap();

        let contents = fs::read_to_string(autostart_path_in_home(home.path())).unwrap();
        assert!(contents.contains("Name=Magnífico"));
        assert!(contents.contains("Exec=magnifico --background"));
        assert!(contents.contains("Icon=io.github.wfpaisa.Magnifico"));
        assert!(is_enabled_in_home(home.path()));
    }

    #[test]
    fn disable_removes_the_entry_and_is_idempotent() {
        let home = tempfile::tempdir().unwrap();
        set_enabled_in_home(home.path(), true).unwrap();
        set_enabled_in_home(home.path(), false).unwrap();
        assert!(!autostart_path_in_home(home.path()).exists());
        assert!(!is_enabled_in_home(home.path()));
        set_enabled_in_home(home.path(), false).unwrap();
    }
}
