//! Conservative, per-application discovery of data left in the home directory.
use super::open_files::busy_paths;
use super::storage_analyzer::{StorageNodeKind, TrashResult, TrashTarget};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::SystemTime;
use walkdir::WalkDir;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Installed,
    NotInstalled,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
}
#[derive(Clone, Debug)]
pub struct Location {
    pub target: TrashTarget,
    pub modified: Option<SystemTime>,
    pub complete: bool,
}
#[derive(Clone, Debug)]
pub struct AppLeftover {
    pub key: String,
    pub name: String,
    pub icon: Option<String>,
    pub status: Status,
    pub confidence: Confidence,
    pub removed_on: Option<String>,
    pub locations: Vec<Location>,
}
impl AppLeftover {
    pub fn size(&self) -> u64 {
        self.locations.iter().map(|l| l.target.size).sum()
    }
    pub fn last_use(&self) -> Option<SystemTime> {
        self.locations.iter().filter_map(|l| l.modified).max()
    }
    pub fn unused_months(&self) -> Option<u64> {
        let months = SystemTime::now()
            .duration_since(self.last_use()?)
            .ok()?
            .as_secs()
            / (30 * 86400);
        (months >= 6).then_some(months)
    }
    pub fn selectable(&self) -> bool {
        self.status == Status::NotInstalled
            && !self.locations.is_empty()
            && self.locations.iter().all(|l| l.complete)
    }
    pub fn preselected(&self) -> bool {
        self.selectable() && self.confidence == Confidence::High
    }
}

#[derive(Default, Debug)]
struct DesktopApp {
    id: String,
    name: String,
    exec: String,
    wm_class: String,
    icon: String,
    flatpak: String,
}
fn desktop_unescape(value: &str) -> String {
    value
        .replace("\\s", " ")
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\r", "\r")
        .replace("\\\\", "\\")
}
fn parse_desktop(id: &str, text: &str) -> Option<DesktopApp> {
    let mut app = DesktopApp {
        id: id.trim_end_matches(".desktop").into(),
        ..Default::default()
    };
    let mut inside = false;
    let mut hidden = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
            continue;
        }
        if !inside || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = desktop_unescape(value);
        match key {
            "Name" => app.name = value,
            "Exec" => {
                // Skip env assignments and field codes; wrappers are not application identities.
                let words = glib::shell_parse_argv(&value).unwrap_or_default();
                app.exec = words
                    .iter()
                    .find(|w| {
                        !w.to_string_lossy().contains('=')
                            && !w.to_string_lossy().starts_with('%')
                            && !w.to_string_lossy().starts_with('-')
                            && !matches!(w.to_str(), Some("env" | "/usr/bin/env"))
                    })
                    .and_then(|w| Path::new(w).file_name())
                    .map(|w| w.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if matches!(app.exec.as_str(), "flatpak" | "sh" | "bash") {
                    app.exec.clear();
                }
            }
            "StartupWMClass" => app.wm_class = value,
            "Icon" => app.icon = value,
            "X-Flatpak" => app.flatpak = value,
            "Hidden" => hidden = value == "true",
            _ => {}
        }
    }
    (!app.name.is_empty() && !hidden).then_some(app)
}

pub fn normalize(name: &str) -> String {
    let mut lower = name.trim().trim_start_matches('.').to_lowercase();
    lower = lower.trim_end_matches(".desktop").to_string();
    for prefix in ["app-", "app_"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            lower = rest.to_string();
        }
    }
    loop {
        let old = lower.clone();
        for suffix in ["-stable", "-bin", "-cache", "_cache", "-config"] {
            if let Some(rest) = lower.strip_suffix(suffix) {
                lower = rest.to_string();
            }
        }
        if lower == old {
            break;
        }
    }
    let full: String = lower.chars().filter(|c| c.is_alphanumeric()).collect();
    if lower.contains("jetbrains")
        || matches!(
            full.as_str(),
            "intellijidea"
                | "intellijideacommunityedition"
                | "pycharm"
                | "pycharmcommunity"
                | "webstorm"
                | "clion"
                | "goland"
                | "rider"
                | "datagrip"
                | "rubymine"
        )
    {
        return "jetbrains".into();
    }
    if let Some(alias) = alias(&full) {
        return alias.into();
    }
    let segment = if lower.starts_with("org.")
        || lower.starts_with("com.")
        || lower.starts_with("io.")
        || lower.starts_with("net.")
    {
        lower
            .rsplit('.')
            .find(|s| !matches!(*s, "desktop" | "client" | "app"))
            .unwrap_or(&lower)
    } else {
        &lower
    };
    let key: String = segment.chars().filter(|c| c.is_alphanumeric()).collect();
    alias(&key).unwrap_or(&key).to_string()
}
fn alias(key: &str) -> Option<&'static str> {
    Some(match key {
        "code"
        | "codeoss"
        | "vscodeoss"
        | "visualstudiocode"
        | "vscodium"
        | "comvisualstudiocode"
        | "comvisualstudiocodeoss"
        | "comvscodiumcodium" => "vscode",
        "mozilla" | "orgmozillafirefox" | "firefoxdeveloperedition" | "firefoxesr" => "firefox",
        "googlechrome" | "comgooglechrome" => "chrome",
        "bravesoftware" | "bravebrowser" | "combravebrowser" => "brave",
        "steam" | "steampath" | "steamroot" | "comvalvesoftwaresteam" => "steam",
        "comdiscordappdiscord" => "discord",
        "comslackslack" => "slack",
        "comspotifyclient" => "spotify",
        _ => return None,
    })
}

fn generic(key: &str) -> bool {
    matches!(
        key,
        "" | "data"
            | "cache"
            | "config"
            | "state"
            | "logs"
            | "backup"
            | "backups"
            | "tmp"
            | "temp"
            | "sessions"
            | "session"
            | "scripts"
            | "bin"
            | "lib"
            | "share"
            | "local"
            | "var"
            | "themes"
            | "wallpapers"
            | "backgrounds"
            | "downloads"
            | "documents"
            | "projects"
            | "electron"
            | "qt"
            | "qt5"
            | "qt6"
            | "mesa"
            | "nv"
            | "nvidia"
            | "radvbuiltinshaders"
            | "mesashadercache"
            | "dxvk"
            | "vkd3d"
            | "shadercache"
            | "fontconfig"
            | "thumbnails"
            | "pip"
            | "npm"
            | "cargo"
            | "rustup"
            | "gradle"
            | "m2"
            | "nuget"
            | "dotnet"
            | "nvm"
            | "pyenv"
            | "sdkman"
            | "bun"
            | "yarn"
            | "go"
            | "vscodeextensions"
    ) || key.starts_with("jetbrains")
}
fn flatpak_id(name: &str) -> bool {
    name.split('.').count() >= 3
        && name.split('.').all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// Accept only direct candidate children; reject shared infrastructure before traversal.
pub fn protected(home: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(home) else {
        return true;
    };
    let parts: Vec<_> = relative
        .components()
        .filter_map(|p| p.as_os_str().to_str())
        .collect();
    let name = match parts.as_slice() {
        [name] if name.starts_with('.') => *name,
        [".config" | ".cache", name] => *name,
        [".local", "share" | "state", name] => *name,
        [".var", "app", name] => *name,
        _ => return true,
    };
    let n = name.trim_start_matches('.').to_lowercase();
    matches!(
        n.as_str(),
        "ssh"
            | "aws"
            | "azure"
            | "kube"
            | "git"
            | "docker"
            | "cups"
            | "gstreamer-1.0"
            | "gegl-0.4"
            | "babl"
            | "glib-2.0"
            | "mesa_shader_cache"
            | "mesa_shader_cache_db"
            | "mesa_shader_cache_sf"
            | "dbus"
            | "ibus"
            | "at-spi"
            | "gcloud"
            | "goa-1.0"
            | "accountsservice"
            | "baloofilerc"
            | "baloo"
            | "gnupg"
            | "pki"
            | "keyrings"
            | "password-store"
            | "passwordstore"
            | "trash"
            | "applications"
            | "icons"
            | "fonts"
            | "mime"
            | "gnome-shell"
            | "dconf"
            | "systemd"
            | "pulse"
            | "pipewire"
            | "wireplumber"
            | "gvfs"
            | "evolution"
            | "flatpak"
            | "containers"
            | "local"
            | "config"
            | "cache"
            | "var"
            | "autostart"
            | "environment.d"
            | "fontconfig"
            | "kdeglobals"
            | "kglobalshortcutsrc"
            | "kactivitymanagerd"
            | "nautilus"
            | "tracker"
            | "tracker3"
            | "lost+found"
    ) || n.starts_with("gtk-")
        || n.starts_with("kde")
        || n.starts_with("user-dirs")
        || n.starts_with("recently-used")
        || n.starts_with("gnome")
        || n.starts_with("org.gnome")
        || n.starts_with("tracker")
        || n.starts_with("gvfs")
        || n.starts_with("evolution")
        || n.starts_with("nautilus")
        || matches!(
            normalize(&n).as_str(),
            "magnifico" | "bitwarden" | "keepassxc" | "keepass" | "1password" | "seahorse"
        )
        || n == crate::APP_ID.to_lowercase()
}

fn history_line(line: &str, removed: &mut HashMap<String, String>) {
    let Some((stamp, event)) = line.split_once("] [ALPM] ") else {
        return;
    };
    let Some(date) = stamp.strip_prefix('[').and_then(|stamp| stamp.get(..10)) else {
        return;
    };
    if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return;
    }
    let mut words = event.split_whitespace();
    let (Some(action), Some(pkg)) = (words.next(), words.next()) else {
        return;
    };
    match action {
        "removed" => {
            removed.insert(pkg.into(), date.to_string());
        }
        "installed" | "reinstalled" | "upgraded" | "downgraded" => {
            removed.remove(pkg);
        }
        _ => {}
    }
}
#[derive(Default)]
struct InstalledIndex {
    uncertain: bool,
    keys: HashSet<String>,
    exact: HashSet<String>,
    apps: HashMap<String, (String, String)>,
    removed: HashMap<String, String>,
}
impl InstalledIndex {
    /// Folders like `codex-runtimes` or `claude-cli-nodejs` belong to an
    /// installed `codex` or `claude`. Short names are skipped so `go` or `qt`
    /// do not claim everything; a wrong match only hides a folder, which is
    /// the safe direction.
    fn prefix_match(&self, key: &str) -> bool {
        const MIN: usize = 4;
        key.len() >= MIN
            && self.keys.iter().any(|installed| {
                installed.len() >= MIN
                    && (key.starts_with(installed.as_str()) || installed.starts_with(key))
            })
    }
    fn add(&mut self, name: &str) {
        if !name.is_empty() {
            self.keys.insert(normalize(name));
        }
    }
    fn load(home: &Path, cancel: &AtomicBool) -> Self {
        let mut index = Self::default();
        let mut roots: Vec<PathBuf> = std::env::split_paths(
            &std::env::var_os("XDG_DATA_DIRS")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
        )
        .map(|p| p.join("applications"))
        .collect();
        roots.extend([
            home.join(".local/share/applications"),
            PathBuf::from("/var/lib/flatpak/exports/share/applications"),
            home.join(".local/share/flatpak/exports/share/applications"),
        ]);
        if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
            roots.push(PathBuf::from(data).join("applications"));
        }
        roots.sort();
        roots.dedup();
        for root in roots {
            if fs::read_dir(&root).is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound) {
                index.uncertain = true;
            }
            for (number, entry) in WalkDir::new(&root)
                .max_depth(4)
                .follow_links(false)
                .into_iter()
                .take(20001)
                .enumerate()
            {
                if number == 20000 {
                    index.uncertain = true;
                    break;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    // One unreadable entry only hides that app, not the rest.
                    Err(_) => continue,
                };
                if cancel.load(Ordering::Relaxed) {
                    return index;
                }
                if entry.path().extension().is_none_or(|e| e != "desktop") {
                    continue;
                }
                if fs::metadata(entry.path()).is_ok_and(|m| m.len() > 1024 * 1024) {
                    continue;
                }
                let Ok(text) = fs::read_to_string(entry.path()) else {
                    continue;
                };
                let id = entry
                    .path()
                    .strip_prefix(&root)
                    .unwrap_or(entry.path())
                    .to_string_lossy()
                    .replace('/', "-");
                let Some(app) = parse_desktop(&id, &text) else {
                    continue;
                };
                for value in [&app.id, &app.flatpak] {
                    if !value.is_empty() {
                        index.exact.insert(value.to_lowercase());
                    }
                }
                for value in [
                    &app.id,
                    &app.exec,
                    &app.wm_class,
                    &app.name,
                    &app.flatpak,
                    &app.icon,
                ] {
                    index.add(value);
                    if !value.is_empty() {
                        index
                            .apps
                            .insert(normalize(value), (app.name.clone(), app.icon.clone()));
                    }
                }
            }
        }
        for root in [
            PathBuf::from("/var/lib/flatpak/app"),
            home.join(".local/share/flatpak/app"),
        ] {
            if fs::read_dir(&root).is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound) {
                index.uncertain = true;
            }
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let id = entry.file_name().to_string_lossy().into_owned();
                        index.add(&id);
                        index.exact.insert(id.to_lowercase());
                    }
                }
            }
        }
        if fs::read_dir("/var/lib/pacman/local")
            .is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound)
        {
            index.uncertain = true;
        }
        if let Ok(entries) = fs::read_dir("/var/lib/pacman/local") {
            for entry in entries.flatten() {
                if cancel.load(Ordering::Relaxed) {
                    return index;
                }
                if !entry.path().is_dir() {
                    continue;
                }
                let Ok(desc) = fs::read_to_string(entry.path().join("desc")) else {
                    continue;
                };
                let mut lines = desc.lines();
                while let Some(line) = lines.next() {
                    if line == "%NAME%" {
                        if let Some(name) = lines.next() {
                            index.add(name);
                        }
                        break;
                    }
                }
            }
        }
        let dpkg_status = fs::read_to_string("/var/lib/dpkg/status");
        if dpkg_status
            .as_ref()
            .is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound)
        {
            index.uncertain = true;
        }
        if let Ok(status) = dpkg_status {
            for record in status.split("\n\n") {
                if record.lines().any(|l| l == "Status: install ok installed") {
                    if let Some(pkg) = record.lines().find_map(|l| l.strip_prefix("Package: ")) {
                        index.add(pkg);
                    }
                }
            }
        }
        // The desktop session's PATH often misses per-user tool managers, so
        // also look where they install command-line apps.
        let mut bin_dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).collect())
            .unwrap_or_default();
        bin_dirs.extend(
            [
                ".local/bin",
                ".cargo/bin",
                ".bun/bin",
                ".deno/bin",
                ".volta/bin",
                ".npm-global/bin",
                "go/bin",
                ".asdf/shims",
                ".local/share/mise/shims",
            ]
            .iter()
            .map(|dir| home.join(dir)),
        );
        bin_dirs.push(PathBuf::from("/usr/local/bin"));
        for pattern_root in [
            home.join(".local/share/mise/installs"),
            home.join(".nvm/versions"),
        ] {
            // <root>/<tool>/<version>/bin
            for tool in fs::read_dir(&pattern_root).into_iter().flatten().flatten() {
                for version in fs::read_dir(tool.path()).into_iter().flatten().flatten() {
                    bin_dirs.push(version.path().join("bin"));
                }
            }
        }
        // Apps installed under /opt keep their folder name.
        for entry in fs::read_dir("/opt").into_iter().flatten().flatten() {
            index.add(&entry.file_name().to_string_lossy());
        }
        bin_dirs.sort();
        bin_dirs.dedup();
        {
            for root in bin_dirs {
                if let Ok(entries) = fs::read_dir(root) {
                    for entry in entries.flatten().take(20000) {
                        if cancel.load(Ordering::Relaxed) {
                            return index;
                        }
                        // Follow symlinks: npm and tool managers link their commands.
                        if fs::metadata(entry.path())
                            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                        {
                            index.add(&entry.file_name().to_string_lossy());
                        }
                    }
                }
            }
        }
        if let Ok(log) = fs::File::open("/var/log/pacman.log") {
            // Skip oversized history entirely: truncation could miss a later reinstall.
            if log.metadata().is_ok_and(|m| m.len() > 128 * 1024 * 1024) {
                return index;
            }
            for line in BufReader::new(log).lines().map_while(Result::ok) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                history_line(&line, &mut index.removed);
            }
        }
        index
    }
    fn classify(&self, home: &Path, locations: Vec<Location>) -> AppLeftover {
        let folder = locations[0]
            .target
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let key = normalize(&folder);
        let exact = locations.iter().any(|l| {
            self.exact.contains(
                &l.target
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_lowercase(),
            )
        });
        let removed_on = self
            .removed
            .iter()
            .filter(|(pkg, _)| normalize(pkg) == key)
            .map(|(_, date)| date.clone())
            .max();
        let flatpak = locations.iter().any(|l| {
            l.target.path.parent() == Some(home.join(".var/app").as_path())
                && flatpak_id(&l.target.path.file_name().unwrap().to_string_lossy())
        });
        let status = if locations.iter().any(|l| protected(home, &l.target.path)) {
            Status::Unknown
        } else if exact || self.keys.contains(&key) || self.prefix_match(&key) {
            Status::Installed
        } else if generic(&key) || self.uncertain {
            Status::Unknown
        } else {
            Status::NotInstalled
        };
        let confidence = if status != Status::Unknown
            && !generic(&key)
            && (exact || removed_on.is_some() || flatpak)
        {
            Confidence::High
        } else {
            Confidence::Medium
        };
        let (name, icon) = self
            .apps
            .get(&key)
            .cloned()
            .unwrap_or((folder, String::new()));
        AppLeftover {
            key,
            name,
            icon: (!icon.is_empty()).then_some(icon),
            status,
            confidence,
            removed_on,
            locations,
        }
    }
}

pub fn scan(home: &Path, cancel: &AtomicBool) -> Result<Vec<AppLeftover>, String> {
    let home = fs::canonicalize(home).map_err(|e| e.to_string())?;
    let home_device = fs::metadata(&home).map_err(|e| e.to_string())?.dev();
    let index = InstalledIndex::load(&home, cancel);
    let mut candidates = Vec::new();
    for root in [
        "",
        ".config",
        ".cache",
        ".local/share",
        ".local/state",
        ".var/app",
    ] {
        let root = home.join(root);
        // Reject redirected XDG parents as well as symlink candidates.
        if fs::canonicalize(&root).ok().as_ref() != Some(&root) {
            continue;
        }
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                if candidates.len() >= 10000 || cancel.load(Ordering::Relaxed) {
                    break;
                }
                let path = entry.path();
                if !protected(&home, &path)
                    && fs::symlink_metadata(&path).is_ok_and(|m| {
                        m.is_dir() && !m.file_type().is_symlink() && m.dev() == home_device
                    })
                {
                    candidates.push(path);
                }
            }
        }
    }
    let budget = AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .map_err(|e| e.to_string())?;
    let locations: Vec<Location> = pool.install(|| {
        candidates
            .par_iter()
            .filter_map(|path| inspect(path, cancel, &budget))
            .collect()
    });
    if cancel.load(Ordering::Relaxed) {
        return Err(String::new());
    }
    let apps = group_locations(&home, &index, locations);
    Ok(apps)
}

fn group_locations(
    home: &Path,
    index: &InstalledIndex,
    locations: Vec<Location>,
) -> Vec<AppLeftover> {
    let mut groups: HashMap<String, Vec<Location>> = HashMap::new();
    for location in locations {
        groups
            .entry(normalize(
                &location.target.path.file_name().unwrap().to_string_lossy(),
            ))
            .or_default()
            .push(location);
    }
    let mut apps: Vec<_> = groups
        .into_values()
        .map(|locations| index.classify(home, locations))
        .collect();
    apps.sort_by_key(|a| std::cmp::Reverse(a.size()));
    apps
}
fn inspect(path: &Path, cancel: &AtomicBool, budget: &AtomicUsize) -> Option<Location> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || fs::canonicalize(path).ok().as_deref() != Some(path)
    {
        return None;
    }
    let mut location = Location {
        target: TrashTarget {
            path: path.into(),
            kind: StorageNodeKind::Directory,
            size: 0,
            device: meta.dev(),
            inode: meta.ino(),
        },
        modified: meta.modified().ok(),
        complete: meta.uid() == unsafe { libc::getuid() },
    };
    let mut seen = HashSet::new();
    let mut walker = WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
        .same_file_system(true)
        .max_open(16)
        .into_iter();
    let mut count = 0;
    while let Some(entry) = walker.next() {
        count += 1;
        if cancel.load(Ordering::Relaxed)
            || count > 100_000
            || budget.fetch_add(1, Ordering::Relaxed) >= 1_000_000
        {
            location.complete = false;
            break;
        }
        let Ok(entry) = entry else {
            location.complete = false;
            continue;
        };
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            location.complete = false;
            continue;
        };
        if meta.dev() != location.target.device {
            location.complete = false;
            walker.skip_current_dir();
            continue;
        }
        if let Ok(modified) = meta.modified() {
            location.modified = Some(location.modified.map_or(modified, |old| old.max(modified)));
        }
        if !meta.file_type().is_symlink() && seen.insert((meta.dev(), meta.ino())) {
            location.target.size = location
                .target
                .size
                .saturating_add(meta.blocks().saturating_mul(512));
        }
    }
    Some(location)
}

#[cfg(test)]
fn remove_trashed(apps: &mut Vec<AppLeftover>, moved: &[(PathBuf, u64)]) {
    let moved: HashSet<_> = moved.iter().map(|(path, _)| path).collect();
    for app in apps.iter_mut() {
        app.locations.retain(|l| !moved.contains(&l.target.path));
    }
    apps.retain(|app| !app.locations.is_empty());
    apps.sort_by_key(|app| std::cmp::Reverse(app.size()));
}

/// Re-index immediately before removal, since installation state can change after a scan.
pub fn trash_with_progress(
    home: &Path,
    apps: &[AppLeftover],
    progress: Option<&super::CleanProgress>,
) -> TrashResult {
    let mut result = TrashResult::default();
    let cancel = AtomicBool::new(false);
    let index = InstalledIndex::load(home, &cancel);
    let busy = busy_paths();
    for app in apps {
        if app.locations.is_empty() {
            continue;
        }
        let current = index.classify(home, app.locations.clone());
        for location in &app.locations {
            let path = &location.target.path;
            if !app.selectable()
                || !current.selectable()
                || protected(home, path)
                // Only something open inside the folder blocks it; an open parent
                // directory (e.g. ~/.local/share held by an indexer) does not.
                || busy.iter().any(|b| b.starts_with(path))
            {
                result.failed.push((
                    path.clone(),
                    crate::i18n::tr("Protected, installed, or currently in use"),
                ));
                continue;
            }
            // Inspect again to catch new mounts and unreadable contents before moving a tree.
            if inspect(path, &cancel, &AtomicUsize::new(0)).is_none_or(|l| !l.complete) {
                result.failed.push((
                    path.clone(),
                    crate::i18n::tr("The folder could not be fully checked"),
                ));
                continue;
            }
            let moved = super::move_to_trash_with_progress(
                home,
                std::slice::from_ref(&location.target),
                progress,
            );
            result.moved.extend(moved.moved);
            result.failed.extend(moved.failed);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_and_normalization() {
        for (name, key) in [
            (".mozilla", "firefox"),
            ("Code - OSS", "vscode"),
            ("vscodium", "vscode"),
            ("vscodium-bin", "vscode"),
            ("google-chrome-stable", "chrome"),
            ("com.jetbrains.IntelliJ-IDEA-Community", "jetbrains"),
            ("com.visualstudio.code", "vscode"),
            (".steam", "steam"),
            ("com.valvesoftware.Steam", "steam"),
            ("BraveSoftware", "brave"),
            ("google-chrome", "chrome"),
            ("org.example.My-App", "myapp"),
            ("app_Example-cache", "example"),
        ] {
            assert_eq!(normalize(name), key);
        }
        assert_ne!(normalize("chromium"), normalize("google-chrome"));
    }
    #[test]
    fn desktop_entry_fields_and_sections() {
        let app = parse_desktop("org.test.Editor.desktop", "[Desktop Entry]\nName=My\\sEditor\nExec=env FOO=bar /usr/bin/editor %U\nStartupWMClass=Editor\nIcon=editor-icon\nX-Flatpak=org.test.Editor\n[Desktop Action New]\nName=Wrong\nExec=wrong").unwrap();
        assert_eq!(app.name, "My Editor");
        assert_eq!(app.exec, "editor");
        assert_eq!(app.wm_class, "Editor");
        assert_eq!(app.icon, "editor-icon");
        assert_eq!(app.flatpak, "org.test.Editor");
        assert_eq!(app.id, "org.test.Editor");
        assert_eq!(
            parse_desktop(
                "editor.desktop",
                "[Desktop Entry]\nName=Editor\nExec=\"/opt/App Suite/editor\" %U"
            )
            .unwrap()
            .exec,
            "editor"
        );
        assert!(parse_desktop(
            "hidden.desktop",
            "[Desktop Entry]\nName=Hidden\nHidden=true"
        )
        .is_none());
    }
    #[test]
    fn removal_history_respects_reinstallation() {
        let mut removed = HashMap::new();
        for line in [
            "[2025-01-01T12:00:00+0000] [ALPM] removed foo (1)",
            "[2025-01-02T12:00:00+0000] [ALPM] installed foo (2)",
            "[2025-02-01T12:00:00+0000] [ALPM] removed bar (1)",
            "[2025-03-01T12:00:00+0000] [ALPM] removed bar (2)",
        ] {
            history_line(line, &mut removed);
        }
        assert!(!removed.contains_key("foo"));
        assert_eq!(removed["bar"], "2025-03-01");
    }
    #[test]
    fn protects_shared_and_sensitive_data() {
        let home = Path::new("/home/test");
        for path in [
            ".ssh",
            ".gnupg",
            ".pki",
            ".password-store",
            ".local/share/keyrings",
            ".local/share/Trash",
            ".local/share/applications",
            ".local/share/flatpak",
            ".config/dconf",
            ".config/gtk-4.0",
            ".config/gnome-shell",
            ".local/share/containers",
            ".config/autostart",
            ".config/environment.d",
            ".config/pulse",
            ".config/pipewire",
            ".config/wireplumber",
            ".local/share/evolution",
            ".local/share/icons",
            ".local/share/fonts",
            ".local/share/mime",
            ".config/kdeglobals",
            ".local/bin",
            ".cache",
            ".config/magnifico",
            ".local/share/io.github.wfpaisa.Magnifico",
            ".config/../victim",
        ] {
            let path = home.join(path);
            assert!(protected(home, &path), "{}", path.display());
            let app = InstalledIndex::default().classify(
                home,
                vec![Location {
                    target: TrashTarget {
                        path,
                        kind: StorageNodeKind::Directory,
                        size: 4096,
                        device: 0,
                        inode: 0,
                    },
                    modified: None,
                    complete: true,
                }],
            );
            assert!(!app.selectable());
            assert!(!app.preselected());
        }
        assert!(protected(home, Path::new("/tmp/app")));
        assert!(!protected(home, &home.join(".config/discord")));
    }
    #[test]
    fn groups_locations_and_never_selects_installed_or_unknown() {
        let home = tempfile::tempdir().unwrap();
        let mut locations = Vec::new();
        for path in [
            ".config/Code",
            ".cache/code-oss",
            ".var/app/com.visualstudio.code",
        ] {
            let path = home.path().join(path);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("data"), b"test").unwrap();
            let location = inspect(&path, &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
            locations.push(location);
        }
        let mut index = InstalledIndex::default();
        let mut apps = group_locations(home.path(), &index, locations);
        assert_eq!(apps.len(), 1);
        let app = apps.remove(0);
        assert_eq!(app.locations.len(), 3);
        assert!(app.size() > 0);
        assert!(app.preselected());
        index.add("vscodium");
        let installed = index.classify(home.path(), app.locations);
        assert_eq!(installed.status, Status::Installed);
        assert!(!installed.selectable());
        let mut unknown = installed;
        unknown.status = Status::Unknown;
        assert!(!unknown.selectable());
        unknown.status = Status::NotInstalled;
        unknown.locations[0].complete = false;
        assert!(!unknown.selectable());
    }
    #[test]
    fn installed_command_claims_folders_named_after_it() {
        let mut index = InstalledIndex::default();
        index.add("codex");
        index.add("go");
        assert!(index.prefix_match(&normalize("codex-runtimes")));
        assert!(!index.prefix_match(&normalize("gocryptfs-old")));
        assert!(!index.prefix_match(&normalize("voxtype")));
    }
    #[test]
    fn exact_ids_and_uncertain_inventory_are_conservative() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".var/app/org.example.Editor");
        fs::create_dir_all(&path).unwrap();
        let location = inspect(&path, &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
        let mut index = InstalledIndex::default();
        index.exact.insert("org.example.editor".into());
        let installed = index.classify(home.path(), vec![location.clone()]);
        assert_eq!(installed.status, Status::Installed);
        assert_eq!(installed.confidence, Confidence::High);
        assert!(!installed.selectable());
        index.exact.clear();
        index.uncertain = true;
        assert_eq!(
            index.classify(home.path(), vec![location]).status,
            Status::Unknown
        );
    }
    #[test]
    fn removal_evidence_and_developer_folders_have_distinct_defaults() {
        let home = tempfile::tempdir().unwrap();
        let mut index = InstalledIndex::default();
        for (folder, pkg) in [(".config/retired-app", "retired-app"), (".npm", "npm")] {
            let path = home.path().join(folder);
            fs::create_dir_all(&path).unwrap();
            index.removed.insert(pkg.into(), "2025-02-01".into());
            let location = inspect(&path, &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
            let app = index.classify(home.path(), vec![location]);
            assert_eq!(app.removed_on.as_deref(), Some("2025-02-01"));
            assert_eq!(app.preselected(), pkg == "retired-app");
            if pkg == "npm" {
                assert_eq!(app.confidence, Confidence::Medium);
            }
        }
    }
    #[test]
    fn partial_trash_updates_sizes_and_keeps_failed_locations() {
        let home = tempfile::tempdir().unwrap();
        let mut locations = Vec::new();
        for folder in [".config/retired-app", ".cache/retired-app"] {
            let path = home.path().join(folder);
            fs::create_dir_all(&path).unwrap();
            locations.push(inspect(&path, &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap());
        }
        let mut apps = group_locations(home.path(), &InstalledIndex::default(), locations);
        let before = apps[0].size();
        let moved = apps[0].locations[0].target.clone();
        remove_trashed(&mut apps, &[(moved.path, moved.size)]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].locations.len(), 1);
        assert_eq!(apps[0].size(), before - moved.size);
        let last = apps[0].locations[0].target.clone();
        remove_trashed(&mut apps, &[(last.path, last.size)]);
        assert!(apps.is_empty());
    }
    #[test]
    fn traversal_does_not_follow_symlinks_and_cancellation_is_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), vec![0; 65536]).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let location = inspect(dir.path(), &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
        assert!(location.target.size < 65536);
        assert!(inspect(
            &dir.path().join("link"),
            &AtomicBool::new(false),
            &AtomicUsize::new(0)
        )
        .is_none());
        assert!(
            !inspect(dir.path(), &AtomicBool::new(true), &AtomicUsize::new(0))
                .unwrap()
                .complete
        );
    }
}
