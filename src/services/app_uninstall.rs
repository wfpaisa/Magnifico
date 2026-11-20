// Linux inventory inspired by Purge AppUninstallScanner.swift (MIT,
// © 2026 Jithin Sabu). See THIRD_PARTY_LICENSES.md.
use super::{
    app_leftovers::{self, AppLeftover, Status},
    storage_analyzer::{StorageNodeKind, TrashTarget},
};
use gio::prelude::*;
use std::{
    collections::HashSet,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlatpakInstallation {
    User,
    System,
}
impl FlatpakInstallation {
    fn flag(self) -> &'static str {
        match self {
            Self::User => "--user",
            Self::System => "--system",
        }
    }
}
#[derive(Clone, Debug)]
pub enum Removal {
    Flatpak {
        installation: FlatpakInstallation,
        id: String,
    },
    Snap(String),
    Native {
        manager: String,
        package: String,
    },
    Trash,
    Unavailable,
}
#[derive(Clone, Debug)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub bytes: u64,
    pub data_bytes: u64,
    pub removal: Removal,
    pub targets: Vec<TrashTarget>,
    /// Caches, settings and saved data; removed only when the user agrees.
    pub data_targets: Vec<TrashTarget>,
}
impl InstalledApp {
    pub fn size(&self) -> u64 {
        self.bytes.saturating_add(self.data_bytes)
    }
    pub fn admin(&self) -> bool {
        matches!(self.removal, Removal::Native { .. } | Removal::Snap(_))
            || matches!(
                &self.removal,
                Removal::Flatpak {
                    installation: FlatpakInstallation::System,
                    ..
                }
            )
    }
    pub fn selectable(&self) -> bool {
        !matches!(self.removal, Removal::Unavailable)
    }
    pub fn source(&self) -> &'static str {
        match self.removal {
            Removal::Flatpak {
                installation: FlatpakInstallation::User,
                ..
            } => "Flatpak (User)",
            Removal::Flatpak {
                installation: FlatpakInstallation::System,
                ..
            } => "Flatpak (System)",
            Removal::Snap(_) => "Snap",
            Removal::Native { .. } => "Native package",
            Removal::Trash => {
                if self.targets.iter().any(|t| is_appimage(&t.path)) {
                    "AppImage"
                } else {
                    "User launcher"
                }
            }
            Removal::Unavailable => "Removal unavailable",
        }
    }
    pub fn command(&self) -> Option<Vec<String>> {
        let args: Vec<String> = match &self.removal {
            Removal::Flatpak { installation, id } => vec![
                "/usr/bin/flatpak".into(),
                installation.flag().into(),
                "uninstall".into(),
                "--noninteractive".into(),
                "-y".into(),
                "--".into(),
                id.clone(),
            ],
            Removal::Snap(id) => vec![
                "/usr/bin/snap".into(),
                "remove".into(),
                "--".into(),
                id.clone(),
            ],
            Removal::Native { manager, package } => {
                let (program, flags) = match manager.as_str() {
                    "pacman" => ("/usr/bin/pacman", vec!["-R", "--noconfirm"]),
                    "dpkg" => ("/usr/bin/apt-get", vec!["remove", "-y"]),
                    "rpm" => {
                        if Path::new("/usr/bin/dnf").exists() {
                            ("/usr/bin/dnf", vec!["remove", "-y"])
                        } else {
                            ("/usr/bin/zypper", vec!["--non-interactive", "remove"])
                        }
                    }
                    _ => return None,
                };
                let mut args = vec![program.into()];
                args.extend(flags.into_iter().map(str::to_owned));
                args.push("--".into());
                args.push(package.clone());
                args
            }
            _ => return None,
        };
        if self.admin() {
            let mut prefixed = vec!["/usr/bin/pkexec".into()];
            prefixed.extend(args);
            Some(prefixed)
        } else {
            Some(args)
        }
    }
}
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(format!("/usr/bin/{program}"))
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn size_tree(path: &Path, cancel: &AtomicBool) -> u64 {
    let mut inodes = HashSet::new();
    walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .take_while(|_| !cancel.load(Ordering::Relaxed))
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file() && inodes.insert((m.dev(), m.ino())))
        .map(|m| m.len())
        .sum()
}
fn target(path: &Path, bytes: u64) -> Option<TrashTarget> {
    let m = fs::symlink_metadata(path).ok()?;
    if m.file_type().is_symlink() {
        return None;
    }
    Some(TrashTarget {
        path: path.into(),
        size: bytes,
        device: m.dev(),
        inode: m.ino(),
        kind: if m.is_dir() {
            StorageNodeKind::Directory
        } else {
            StorageNodeKind::File
        },
    })
}
fn owner(desktop: &Path) -> Option<(String, String, u64)> {
    let path = desktop.to_str()?;
    if let Some(package) = output("pacman", &["-Qoq", "--", path]) {
        let bytes = output("pacman", &["-Qi", "--", &package])
            .and_then(|t| {
                t.lines().find_map(|l| {
                    l.strip_prefix("Installed Size")
                        .and_then(|l| l.split_once(':'))
                        .map(|(_, v)| parse_size(v.trim()))
                })
            })
            .unwrap_or(0);
        return Some(("pacman".into(), package, bytes));
    }
    if let Some(line) = output("dpkg-query", &["-S", path]) {
        let package = line.lines().next()?.split_once(": ")?.0.to_owned();
        let bytes = output("dpkg-query", &["-W", "-f=${Installed-Size}", &package])
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024;
        return Some(("dpkg".into(), package, bytes));
    }
    if let Some(line) = output("rpm", &["-qf", "--qf", "%{NAME}\t%{SIZE}", path]) {
        let (package, size) = line.split_once('\t')?;
        return Some(("rpm".into(), package.into(), size.parse().unwrap_or(0)));
    }
    None
}
fn parse_size(value: &str) -> u64 {
    let value = value.replace(',', ".");
    let split = value
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(value.len());
    let n = value[..split].parse::<f64>().unwrap_or(0.0);
    let factor = match value[split..].trim() {
        "kB" | "KB" => 1e3,
        "KiB" => 1024.,
        "MB" => 1e6,
        "MiB" => 1048576.,
        "GB" => 1e9,
        "GiB" => 1073741824.,
        "TB" => 1e12,
        _ => 1.,
    };
    (n * factor) as u64
}
pub fn scan(home: &Path, data: &[AppLeftover], cancel: &AtomicBool) -> Vec<InstalledApp> {
    let mut apps = Vec::new();
    let mut ids = HashSet::new();
    for installation in [FlatpakInstallation::User, FlatpakInstallation::System] {
        let scope = installation.flag();
        if let Some(text) = output(
            "flatpak",
            &[scope, "list", "--app", "--columns=application,name,size"],
        ) {
            for line in text.lines() {
                let fields: Vec<_> = line.split('\t').collect();
                if fields.len() < 3
                    || !valid_package(fields[0])
                    || fields[0].split('.').any(str::is_empty)
                {
                    continue;
                }
                let id = fields[0].to_owned();
                apps.push(InstalledApp {
                    id: id.clone(),
                    name: fields[1].into(),
                    icon: Some(id.clone()),
                    bytes: parse_size(fields[2]),
                    data_bytes: 0,
                    removal: Removal::Flatpak {
                        installation,
                        id: id.clone(),
                    },
                    targets: vec![],
                    data_targets: vec![],
                });
                ids.insert(id);
            }
        }
    }
    if let Some(text) = output("snap", &["list"]) {
        for line in text.lines().skip(1) {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 3 {
                continue;
            }
            let id = fields[0];
            if !valid_package(id) || !fields[2].chars().all(|c| c.is_ascii_alphanumeric()) {
                continue;
            }
            let bytes = fs::metadata(format!("/var/lib/snapd/snaps/{id}_{}.snap", fields[2]))
                .map(|m| m.len())
                .unwrap_or(0);
            apps.push(InstalledApp {
                id: id.into(),
                name: id.into(),
                icon: Some(id.into()),
                bytes,
                data_bytes: 0,
                removal: Removal::Snap(id.into()),
                targets: vec![],
                data_targets: vec![],
            });
            ids.insert(id.into());
        }
    }
    let mut packages = HashSet::new();
    let mut images = HashSet::new();
    for info in gio::AppInfo::all() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(desktop) = info.downcast_ref::<gio::DesktopAppInfo>() else {
            continue;
        };
        if desktop.is_hidden() || desktop.is_nodisplay() {
            continue;
        }
        let Some(path) = desktop.filename() else {
            continue;
        };
        let id = desktop
            .id()
            .unwrap_or_default()
            .trim_end_matches(".desktop")
            .to_string();
        if ids.contains(&id)
            || desktop
                .string("X-Flatpak")
                .is_some_and(|s| ids.contains(s.as_str()))
            || desktop
                .string("X-SnapInstanceName")
                .is_some_and(|s| ids.contains(s.as_str()))
        {
            continue;
        }
        let mut app = InstalledApp {
            id,
            name: info.name().to_string(),
            icon: info
                .icon()
                .and_then(|i| i.to_string())
                .map(|s| s.to_string()),
            bytes: 0,
            data_bytes: 0,
            removal: Removal::Unavailable,
            targets: vec![],
            data_targets: vec![],
        };
        if path.starts_with(home)
            && path.starts_with(
                dirs::data_local_dir()
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("applications"),
            )
        {
            app.removal = Removal::Trash;
            // Only the launcher and an explicitly named AppImage belong to a
            // desktop entry. Never infer an arbitrary executable or directory.
            if let Some(t) = target(&path, fs::metadata(&path).map(|m| m.len()).unwrap_or(0)) {
                app.bytes += t.size;
                app.targets.push(t);
            }
            if let Some(exec) = desktop
                .string("Exec")
                .and_then(|s| glib::shell_parse_argv(s).ok())
                .and_then(|a| a.first().map(PathBuf::from))
            {
                if is_appimage(&exec)
                    && exec.starts_with(home)
                    && exec.is_file()
                    && images.insert(exec.clone())
                {
                    if let Some(t) =
                        target(&exec, fs::metadata(&exec).map(|m| m.len()).unwrap_or(0))
                    {
                        app.bytes += t.size;
                        app.targets.push(t);
                    }
                }
            }
            if app.targets.is_empty() {
                app.removal = Removal::Unavailable;
            }
        } else if let Some((manager, package, bytes)) = owner(&path) {
            if !valid_package(&package) || !packages.insert((manager.clone(), package.clone())) {
                continue;
            }
            app.bytes = bytes;
            app.removal = Removal::Native { manager, package };
        }
        apps.push(app);
    }
    for root in [
        home.join("Applications"),
        home.join(".local/bin"),
        home.join("Downloads"),
        home.join("AppImages"),
    ] {
        for entry in walkdir::WalkDir::new(root)
            .max_depth(3)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let path = entry.path();
            if !entry.file_type().is_file() || !is_appimage(path) || !images.insert(path.into()) {
                continue;
            }
            if let Some(t) = target(path, entry.metadata().map(|m| m.len()).unwrap_or(0)) {
                apps.push(InstalledApp {
                    id: path.display().to_string(),
                    name: path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    icon: None,
                    bytes: t.size,
                    data_bytes: 0,
                    removal: Removal::Trash,
                    targets: vec![t],
                    data_targets: vec![],
                });
            }
        }
    }
    let mut claimed = HashSet::new();
    for app in &mut apps {
        let keys: HashSet<_> = [
            app_leftovers::normalize(&app.id),
            app_leftovers::normalize(&app.name),
        ]
        .into_iter()
        .collect();
        for leftover in data.iter().filter(|l| {
            l.status == Status::Installed && keys.contains(&app_leftovers::normalize(&l.key))
        }) {
            for location in &leftover.locations {
                if location.complete && claimed.insert(location.target.path.clone()) {
                    app.data_bytes += location.target.size;
                    app.data_targets.push(location.target.clone());
                }
            }
        }
        // Flatpak data may not appear in the conservative leftover index.
        if let Removal::Flatpak { id, .. } = &app.removal {
            let path = home.join(".var/app").join(id);
            if !claimed.contains(&path) && path.is_dir() {
                let bytes = size_tree(&path, cancel);
                if let Some(t) = target(&path, bytes) {
                    claimed.insert(path);
                    app.data_bytes += bytes;
                    app.data_targets.push(t);
                }
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}
fn is_appimage(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("appimage"))
}
fn valid_package(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-._:@".contains(c))
}
/// Commands run without a shell; data moves only after a successful uninstall.
pub fn uninstall_with_progress(
    home: &Path,
    app: &InstalledApp,
    include_data: bool,
    progress: Option<&super::CleanProgress>,
) -> Result<String, String> {
    if let Some(args) = app.command() {
        let out = Command::new(&args[0])
            .args(&args[1..])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into());
        }
    } else if !matches!(app.removal, Removal::Trash) {
        return Err(crate::i18n::tr("Removal unavailable"));
    }
    if let Some(progress) = progress.filter(|_| app.command().is_some()) {
        progress
            .bytes_done
            .fetch_add(app.bytes, std::sync::atomic::Ordering::Relaxed);
    }
    let mut targets = app.targets.clone();
    if include_data {
        targets.extend(app.data_targets.iter().cloned());
    }
    let result = super::move_to_trash_with_progress(home, &targets, progress);
    if result.failed.is_empty() {
        Ok(app.name.clone())
    } else {
        Err(result
            .failed
            .iter()
            .map(|(p, e)| format!("{}: {e}", p.display()))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_inventory_deduplicates_appimages_and_keeps_launcher_scope() {
        if std::env::var_os("MAGNIFICO_UNINSTALL_FIXTURE").is_none() {
            let home = tempfile::tempdir().unwrap();
            let out = Command::new(std::env::current_exe().unwrap())
                .env("MAGNIFICO_UNINSTALL_FIXTURE", "1")
                .env("HOME", home.path())
                .env("XDG_DATA_HOME", home.path().join(".local/share"))
                .env("XDG_DATA_DIRS", home.path().join("empty"))
                .args(["--exact", "services::app_uninstall::tests::user_inventory_deduplicates_appimages_and_keeps_launcher_scope", "--nocapture"])
                .output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let home = dirs::home_dir().unwrap();
        let desktop = home.join(".local/share/applications");
        fs::create_dir_all(&desktop).unwrap();
        fs::create_dir_all(home.join("Applications")).unwrap();
        let image = home.join("Applications/Test App.AppImage");
        fs::write(&image, b"test application").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&image, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            desktop.join("test.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=Test App\nExec=\"{}\"\n",
                image.display()
            ),
        )
        .unwrap();
        fs::write(
            desktop.join("launcher.desktop"),
            "[Desktop Entry]\nType=Application\nName=Launcher Only\nExec=/usr/bin/true\n",
        )
        .unwrap();
        fs::write(
            desktop.join("hidden.desktop"),
            "[Desktop Entry]\nType=Application\nName=Hidden App\nHidden=true\nExec=/usr/bin/true\n",
        )
        .unwrap();
        let apps = scan(&home, &[], &AtomicBool::new(false));
        let images: Vec<_> = apps
            .iter()
            .filter(|a| a.targets.iter().any(|t| t.path == image))
            .collect();
        assert_eq!(
            images.len(),
            1,
            "the AppImage and its launcher share one card"
        );
        assert_eq!(images[0].targets.len(), 2);
        assert_eq!(
            images[0].bytes,
            images[0].targets.iter().map(|t| t.size).sum::<u64>()
        );
        let launcher = apps.iter().find(|a| a.name == "Launcher Only").unwrap();
        assert!(launcher.selectable());
        assert_eq!(launcher.source(), "User launcher");
        assert_eq!(launcher.targets.len(), 1);
        assert_eq!(launcher.targets[0].path, desktop.join("launcher.desktop"));
        assert!(!apps.iter().any(|a| a.name == "Hidden App"));
    }
    #[test]
    fn flatpak_user_and_system_keep_their_installation_and_permissions() {
        for installation in [FlatpakInstallation::User, FlatpakInstallation::System] {
            let app = InstalledApp {
                id: "org.test.App".into(),
                name: "Test".into(),
                icon: None,
                bytes: 1,
                data_bytes: 2,
                removal: Removal::Flatpak {
                    installation,
                    id: "org.test.App".into(),
                },
                targets: vec![],
                data_targets: vec![],
            };
            let command = app.command().unwrap();
            let system = installation == FlatpakInstallation::System;
            assert_eq!(app.admin(), system);
            assert_eq!(command[0] == "/usr/bin/pkexec", system);
            assert!(command.contains(&installation.flag().to_owned()));
            assert!(!command.contains(&if system { "--user" } else { "--system" }.to_owned()));
            assert_eq!(
                app.source(),
                if system {
                    "Flatpak (System)"
                } else {
                    "Flatpak (User)"
                }
            );
        }
    }
    #[test]
    fn sizes_and_package_arguments() {
        assert_eq!(parse_size("1.5 GB"), 1_500_000_000);
        assert_eq!(parse_size("12.0 MiB"), 12 * 1048576);
        assert!(!valid_package("--root=/"));
        assert!(!valid_package("a\nb"));
        assert!(valid_package("libfoo:amd64"));
        let app = InstalledApp {
            id: "x".into(),
            name: "X".into(),
            icon: None,
            bytes: 0,
            data_bytes: 0,
            removal: Removal::Native {
                manager: "dpkg".into(),
                package: "test".into(),
            },
            targets: vec![],
            data_targets: vec![],
        };
        assert_eq!(
            app.command().unwrap(),
            [
                "/usr/bin/pkexec",
                "/usr/bin/apt-get",
                "remove",
                "-y",
                "--",
                "test"
            ]
        );
    }
}
