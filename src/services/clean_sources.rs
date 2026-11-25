// Adapted from Purge CacheScanner.swift, DevScanner.swift and
// OldToolVersionScanPolicy.swift (MIT, © 2026 Jithin Sabu).
// https://github.com/jithin-sabu/purge-app — see THIRD_PARTY_LICENSES.md.
use super::{ScanOptions, Scanner};
use crate::i18n::{tr, tr_args};
use crate::{
    models::{explanations::explanation, BrowserDataType, Safety, ScanResult},
    storage::Storage,
};
use gio::prelude::*;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanPageKind {
    AppCaches,
    DevTools,
    System,
}
impl CleanPageKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::AppCaches => "App Caches",
            Self::DevTools => "Dev Tools",
            Self::System => "System",
        }
    }
    pub fn index(self) -> usize {
        match self {
            Self::AppCaches => 0,
            Self::DevTools => 1,
            Self::System => 2,
        }
    }
}
#[derive(Clone, Debug)]
pub struct CommandCleanup {
    pub program: String,
    pub args: Vec<String>,
    pub admin: bool,
    pub kernels: bool,
}
#[derive(Clone, Debug)]
pub struct CleanItem {
    pub key: &'static str,
    pub title: String,
    pub explanation: &'static str,
    pub icon: &'static str,
    pub app_icons: Vec<String>,
    pub safety: Safety,
    pub bytes: u64,
    pub scan: ScanResult,
    pub command: Option<CommandCleanup>,
}
impl CleanItem {
    fn files(key: &str, title: Option<String>, scan: ScanResult) -> Self {
        let info = explanation(key);
        Self {
            key: info.key,
            app_icons: app_icons(key, title.as_deref(), &scan),
            title: title.unwrap_or_else(|| info.title.into()),
            explanation: info.text,
            icon: if key == "browser" {
                "pc-web-browser-symbolic"
            } else {
                "pc-folder-symbolic"
            },
            safety: info.safety,
            bytes: scan.total_size,
            scan,
            command: None,
        }
    }
}

fn expand(path: &Path) -> Vec<PathBuf> {
    glob::glob(&glob::Pattern::escape(&path.to_string_lossy()).replace("[*]", "*"))
        .map(|paths| paths.flatten().collect())
        .unwrap_or_default()
}
fn children(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}
fn cache_key(path: &Path) -> &'static str {
    match path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "npm" | "_cacache" => "npm-cache",
        "_logs" => "npm-logs",
        "_npx" => "npm-npx-cache",
        "yarn" => "yarn-cache",
        "pnpm" | "store" => "pnpm-store",
        "bun" => "bun-cache",
        "pip" => "pip",
        "uv" => "uv-cache",
        "node-gyp" => "node-gyp",
        "go-build" | "go" => "go",
        "thumbnails" => "thumbnails",
        "deno" => "deno-cache",
        "pre-commit" => "pre-commit-cache",
        "jetbrains" => "jetbrains",
        "composer" => "composer",
        "electron" => "electron",
        name if name.ends_with("-updater") => "updater",
        name => explanation(name).key,
    }
}

// Claim the precise dev roots before the general cache walk, so rows never
// overlap even when a custom rule names a child of another cache.
fn dev_definitions(home: &Path, cache: &Path) -> Vec<(String, String, Vec<PathBuf>)> {
    let mut rows = Vec::new();
    for rule in crate::models::AppRule::defaults()
        .into_iter()
        .filter(|r| r.is_developer_tool())
    {
        if let Some(path) = rule.expanded_path() {
            let key = if path.starts_with(home.join(".cargo")) {
                "cargo"
            } else if path.starts_with(home.join(".m2")) {
                "maven"
            } else if path.starts_with(home.join(".gradle")) {
                "gradle-global-cache"
            } else if path.to_string_lossy().contains("/Code/")
                || path.to_string_lossy().contains("com.visualstudio.code")
            {
                "vscode"
            } else if path.starts_with(home.join("go/pkg/mod")) {
                "go"
            } else if path.starts_with(home.join(".pub-cache")) {
                "pub-cache"
            } else if path.starts_with(home.join(".bun")) {
                "bun-cache"
            } else if path.to_string_lossy().contains("/pnpm/") {
                "pnpm-store"
            } else {
                cache_key(&path)
            };
            rows.push((key.into(), rule.name, expand(&path)));
        }
    }
    for (key, paths) in [
        ("npm-cache", vec![home.join(".npm/_cacache")]),
        (
            "yarn-cache",
            vec![cache.join("yarn"), home.join(".yarn/berry/cache")],
        ),
        (
            "pnpm-store",
            vec![cache.join("pnpm"), home.join(".local/share/pnpm/store")],
        ),
        ("bun-cache", vec![home.join(".bun/install/cache")]),
        (
            "cargo",
            vec![
                home.join(".cargo/registry/cache"),
                home.join(".cargo/registry/src"),
                home.join(".cargo/registry/index"),
            ],
        ),
        ("pip", vec![cache.join("pip")]),
        ("uv-cache", vec![cache.join("uv")]),
        ("go", vec![cache.join("go-build")]),
        ("node-gyp", vec![cache.join("node-gyp")]),
        (
            "cursor",
            vec![
                home.join(".config/Cursor/Cache"),
                home.join(".config/Cursor/Code Cache"),
                home.join(".config/Cursor/CachedData"),
                home.join(".config/Cursor/GPUCache"),
            ],
        ),
        (
            "vscode",
            vec![
                home.join(".config/Code/Cache"),
                home.join(".config/Code/Code Cache"),
                home.join(".config/Code/CachedData"),
                home.join(".config/Code/GPUCache"),
            ],
        ),
    ] {
        rows.push((key.into(), explanation(key).title.into(), paths));
    }
    rows
}

pub fn scan(kind: CleanPageKind, storage: &Storage, cancel: Arc<AtomicBool>) -> Vec<CleanItem> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let cache = dirs::cache_dir().unwrap_or_else(|| home.join(".cache"));
    let settings = storage.get_settings();
    let scanner = Scanner::with_options_and_cancellation(
        ScanOptions {
            log_retention_days: settings.log_retention_days,
        },
        cancel.clone(),
    );
    if kind == CleanPageKind::System {
        return scan_system(settings.log_retention_days, &cancel);
    }
    let definitions = dev_definitions(&home, &cache);
    let claimed: Vec<PathBuf> = definitions
        .iter()
        .flat_map(|(_, _, paths)| paths.clone())
        .collect();
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |key: &str, title: Option<String>, mut result: ScanResult| {
        // Deduplicate overlapping rules and roots by path and inode, preserving
        // the scanner's safety exclusions and disabling whole-directory shortcuts
        // if an earlier row owns part of the same tree.
        use std::os::unix::fs::MetadataExt;
        let mut left_out: Vec<PathBuf> = Vec::new();
        if key == "jetbrains" {
            result.files.retain(|f| {
                let keep = !f.path.components().any(|c| c.as_os_str() == "LocalHistory");
                if !keep {
                    left_out.push(f.path.clone());
                }
                keep
            });
        }
        // Hard links inside one row (a build folder links its outputs) count
        // once but do not make the folder incomplete; a file an earlier row
        // already owns does.
        let mut in_row = HashSet::new();
        result.files.retain(|f| {
            let Ok(meta) = std::fs::symlink_metadata(&f.path) else {
                left_out.push(f.path.clone());
                return false;
            };
            let id = (meta.dev(), meta.ino());
            if seen.contains(&id) {
                left_out.push(f.path.clone());
                return false;
            }
            in_row.insert(id)
        });
        seen.extend(in_row);
        result.total_size = result.files.iter().map(|f| f.size).sum();
        result.file_count = result.files.len();
        // A folder still taken completely goes to the trash as one unit.
        // One that lost a file to the filters above, or to an earlier row,
        // goes file by file so nothing outside this row is removed.
        result
            .whole_dirs
            .retain(|dir| !left_out.iter().any(|path| path.starts_with(dir)));
        let whole = std::mem::take(&mut result.whole_dirs);
        result
            .directories
            .retain(|d| whole.iter().any(|dir| d.path.starts_with(dir)));
        result.dir_count = result.directories.len();
        result.whole_dirs = whole;
        if result.total_size > 0 {
            rows.push(CleanItem::files(key, title, result));
        }
    };
    match kind {
        CleanPageKind::DevTools => {
            for (key, title, paths) in definitions {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                add(&key, Some(title), scanner.scan_paths(&paths));
            }
            add(
                "old-cli-versions",
                None,
                scanner.scan_paths(&old_claude_versions(&home)),
            );
            let busy = super::open_files::working_paths();
            let projects = super::projects::find_inactive_projects(
                &settings.project_folders,
                Duration::from_secs(u64::from(settings.pin_recent_days) * 86400),
                SystemTime::now(),
                &busy,
                &cancel,
            );
            for project in projects {
                let targets: Vec<_> = project
                    .artifacts
                    .into_iter()
                    .filter(|p| {
                        p.file_name().is_some_and(|n| n == "target")
                            && project.root.join("Cargo.toml").is_file()
                    })
                    .collect();
                add(
                    "projects",
                    Some(tr_args(
                        "{title} · {path}",
                        &[
                            ("{title}", &tr(explanation("projects").title)),
                            ("{path}", &project.root.display().to_string()),
                        ],
                    )),
                    scanner.scan_paths(&targets),
                );
            }
            if let Some(item) = docker_item(&cancel) {
                rows.push(item);
            }
        }
        CleanPageKind::AppCaches => {
            // Only browser cache rules are eligible: cookies, history, sessions
            // and passwords never enter these regenerable-cache pages.
            let browser_roots: Vec<_> = crate::models::BrowserType::all()
                .iter()
                .flat_map(|b| b.cache_roots())
                .collect();
            for mut rule in storage
                .get_browser_rules()
                .into_iter()
                .filter(|r| r.data_type == BrowserDataType::Cache)
            {
                rule.enabled = true;
                add(
                    "browser",
                    Some(rule.display_name()),
                    scanner.scan_browser_rules(&[rule]),
                );
            }
            for mut rule in crate::models::AppRule::defaults().into_iter().filter(|r| {
                !r.is_developer_tool() && r.path.to_string_lossy().contains("cache")
                    || (!r.is_developer_tool() && r.path.to_string_lossy().contains("Cache"))
            }) {
                let Some(path) = rule.expanded_path() else {
                    continue;
                };
                if browser_roots
                    .iter()
                    .any(|r| r.starts_with(&path) || path.starts_with(r))
                {
                    continue;
                }
                rule.enabled = true;
                let result = scanner.scan_app_rules(&[rule.clone()]);
                let mut key = cache_policy(&path, &result);
                if key == "generic-cache" {
                    if let Some(parent) = path.parent().and_then(|p| p.file_name()) {
                        let known = cache_key(Path::new(parent));
                        if known != "unknown" {
                            key = known;
                        }
                    }
                }
                add(key, Some(rule.name.clone()), result);
            }
            let mut roots = children(&cache);
            if cache != home.join(".cache") {
                roots.extend(children(&home.join(".cache")));
            }
            for app in children(&home.join(".var/app")) {
                roots.push(app.join("cache"));
            }
            // Downloaded models are listed one by one under AI Models.
            let models: Vec<PathBuf> = super::big_items::find_ai_models(&home)
                .into_iter()
                .flat_map(|item| item.paths)
                .collect();
            for path in roots {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if models
                    .iter()
                    .any(|m| m.starts_with(&real) || real.starts_with(m))
                {
                    continue;
                }
                if browser_roots
                    .iter()
                    .any(|r| r.starts_with(&path) || path.starts_with(r))
                {
                    continue;
                }
                if claimed
                    .iter()
                    .any(|dev| dev.starts_with(&path) || path.starts_with(dev))
                {
                    continue;
                }
                let mut result = scanner.scan_paths(std::slice::from_ref(&path));
                let key = cache_policy(&path, &result);
                let title = Some(cache_title(&path, key));
                add(key, title, std::mem::take(&mut result));
            }
        }
        CleanPageKind::System => unreachable!(),
    }
    // Known app/tool definitions own a single card even when several rules
    // contribute caches (for example Code Cache, GPUCache and CachedData).
    let mut grouped: Vec<CleanItem> = Vec::new();
    for item in rows {
        if item.command.is_none()
            && item.key != "unknown"
            && item.key != "projects"
            && item.key != "browser"
            && !matches!(
                item.key,
                "generic-cache" | "model-cache" | "keyring-cache" | "session-cache"
            )
        {
            if let Some(existing) = grouped
                .iter_mut()
                .find(|other| other.key == item.key && other.command.is_none())
            {
                existing.bytes += item.bytes;
                existing.scan.merge(item.scan);
                existing.title = explanation(existing.key).title.into();
                continue;
            }
        }
        grouped.push(item);
    }
    grouped
}

fn cache_title(path: &Path, key: &str) -> String {
    if !matches!(
        key,
        "generic-cache" | "model-cache" | "keyring-cache" | "session-cache"
    ) {
        return explanation(key).title.into();
    }
    let folder = if path.file_name().is_some_and(|n| n == "cache") {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let raw = folder.file_name().unwrap_or_default().to_string_lossy();
    let known = gio::AppInfo::all()
        .into_iter()
        .find(|app| {
            app.id()
                .is_some_and(|id| id.trim_end_matches(".desktop").eq_ignore_ascii_case(&raw))
                || app.name().eq_ignore_ascii_case(&raw)
        })
        .map(|app| app.name().to_string());
    let name = known.unwrap_or_else(|| {
        let mut chars = raw.trim_start_matches('.').chars();
        chars
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
            .unwrap_or_default()
    });
    tr_args("{app} Cache", &[("{app}", &name)])
}

fn cache_policy(path: &Path, scan: &ScanResult) -> &'static str {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let known = cache_key(path);
    if matches!(known, "paru" | "yay") {
        return known;
    }
    let name = if name == "cache" {
        path.parent()
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    } else {
        name
    };
    if [
        "vllm",
        "comfyui",
        "huggingface",
        "ollama",
        "torch",
        "transformers",
        "modelscope",
        "lm-studio",
        "lmstudio",
        "whisper",
        "diffusers",
        "models",
        "stable-diffusion",
    ]
    .iter()
    .any(|part| name.contains(part))
    {
        return "model-cache";
    }
    if scan.files.iter().any(|f| {
        f.path.extension().is_some_and(|ext| {
            ["gguf", "safetensors", "ckpt", "pth", "onnx"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
    }) {
        return "model-cache";
    }
    if ["keyring", "wallet", "keepass"]
        .iter()
        .any(|part| name.contains(part))
    {
        return "keyring-cache";
    }
    if scan.files.iter().any(|f| {
        let name = f
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        name == "cookies"
            || name == "login data"
            || name.starts_with("sessionstore")
            || ((name.contains("session") || name.contains("keyring"))
                && (name.ends_with(".db") || name.contains("sqlite")))
    }) {
        return "session-cache";
    }
    let known = cache_key(path);
    if known == "unknown" {
        "generic-cache"
    } else {
        known
    }
}
fn app_icons(key: &str, title: Option<&str>, scan: &ScanResult) -> Vec<String> {
    let mut candidates = Vec::new();
    for file in scan.files.iter().take(16) {
        let parts: Vec<_> = file
            .path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        for pair in parts.windows(2) {
            if pair[0] == "app" && parts.iter().any(|p| p == ".var") {
                candidates.push(pair[1].clone());
            }
            if pair[0] == ".cache" {
                candidates.push(pair[1].clone());
                candidates.push(pair[1].to_lowercase());
            }
        }
    }
    let identity = format!("{key} {} {}", title.unwrap_or(""), candidates.join(" ")).to_lowercase();
    for (aliases, icons) in [
        (&["firefox"][..], &["firefox", "org.mozilla.firefox"][..]),
        (
            &["vscode", "visual studio", "vs code"],
            &["code", "com.visualstudio.code"],
        ),
        (&["cursor"], &["cursor"]),
        (&["slack"], &["slack", "com.slack.Slack"]),
        (&["discord"], &["discord", "com.discordapp.Discord"]),
        (&["spotify"], &["spotify", "com.spotify.Client"]),
        (&["chromium"], &["chromium", "org.chromium.Chromium"]),
        (&["chrome"], &["google-chrome", "com.google.Chrome"]),
        (&["brave"], &["brave", "brave-browser", "com.brave.Browser"]),
        (
            &["telegram"],
            &["telegram", "telegram-desktop", "org.telegram.desktop"],
        ),
    ] {
        if aliases.iter().any(|name| identity.contains(name)) {
            candidates.extend(icons.iter().map(|s| s.to_string()));
        }
    }
    candidates
}

fn version(name: &str) -> Option<Vec<u64>> {
    let parts: Vec<_> = name
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .ok()?;
    (parts.len() >= 2).then_some(parts)
}
fn old_claude_versions(home: &Path) -> Vec<PathBuf> {
    let root = home.join(".local/share/claude/versions");
    // Keep the user's selected executable and every executable currently used
    // by a process, even following an update or a downgrade.
    let Ok(current) = home.join(".local/bin/claude").canonicalize() else {
        return Vec::new();
    };
    let running: HashSet<_> = children(Path::new("/proc"))
        .into_iter()
        .filter_map(|p| p.join("exe").canonicalize().ok())
        .collect();
    old_versions_in(&root, &current, &running)
}
fn old_versions_in(root: &Path, current: &Path, running: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let versions: Vec<_> = children(root)
        .into_iter()
        .filter_map(|p| {
            let meta = std::fs::symlink_metadata(&p).ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((version(p.file_name()?.to_str()?)?, p))
        })
        .collect();
    let newest = versions.iter().map(|(v, _)| v).max();
    versions
        .iter()
        .filter(|(v, p)| {
            Some(v) != newest
                && p.canonicalize()
                    .is_ok_and(|resolved| resolved != current && !running.contains(&resolved))
        })
        .map(|(_, p)| p.clone())
        .collect()
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let executable = executable(program)?;
    let out = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
fn executable(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/usr/sbin", "/bin", "/sbin"]
        .iter()
        .map(|root| Path::new(root).join(name))
        .find(|p| p.is_file())
}
fn measured(
    paths: &[PathBuf],
    cancel: &AtomicBool,
    keep: impl Fn(&Path, &std::fs::Metadata) -> bool,
) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut seen = HashSet::new();
    let mut bytes = 0;
    for root in paths {
        for e in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            if cancel.load(Ordering::Relaxed) {
                return bytes;
            }
            if !e.file_type().is_file() {
                continue;
            }
            if let Ok(m) = e.metadata() {
                if keep(e.path(), &m) && seen.insert((m.dev(), m.ino())) {
                    bytes += m.len();
                }
            }
        }
    }
    bytes
}
fn command_item(
    key: &str,
    title: String,
    bytes: u64,
    program: &str,
    args: Vec<String>,
    admin: bool,
) -> CleanItem {
    let mut item = CleanItem::files(key, Some(title), ScanResult::new());
    item.bytes = bytes;
    item.icon = "pc-applications-engineering-symbolic";
    item.command = Some(CommandCleanup {
        program: program.into(),
        args,
        admin,
        kernels: key == "kernels",
    });
    item
}
fn scan_system(days: u32, cancel: &AtomicBool) -> Vec<CleanItem> {
    let mut rows = Vec::new();
    for (program, paths, args) in [
        ("apt-get", vec!["/var/cache/apt/archives"], vec!["clean"]),
        (
            "pacman",
            vec!["/var/cache/pacman/pkg"],
            vec!["-Sc", "--noconfirm"],
        ),
        (
            "dnf",
            vec!["/var/cache/dnf", "/var/cache/libdnf5"],
            vec!["clean", "all"],
        ),
    ] {
        if executable(program).is_none() {
            continue;
        }
        let paths: Vec<_> = paths.into_iter().map(PathBuf::from).collect();
        let bytes = if program == "pacman" {
            super::system_preview::pacman_cache_bytes()
        } else {
            measured(&paths, cancel, |_, _| true)
        };
        if bytes > 0 {
            let item = command_item(
                "packages",
                tr_args("{manager} Package Manager Cache", &[("{manager}", program)]),
                bytes,
                program,
                args.into_iter().map(str::to_owned).collect(),
                true,
            );
            rows.push(item);
        }
    }
    let bytes = super::log_cleanup::system_journal_size(cancel);
    if bytes > 0 || Path::new("/var/log/journal").is_dir() {
        rows.push(command_item(
            "journal",
            explanation("journal").title.into(),
            bytes,
            "journalctl",
            vec![format!("--vacuum-time={}d", days.clamp(1, 3650))],
            true,
        ));
    }
    let orphans = super::system_preview::pacman_orphan_names();
    if !orphans.is_empty() {
        let bytes = super::system_preview::pacman_installed_bytes(&orphans);
        let mut args = vec!["-Rns".into(), "--noconfirm".into()];
        args.extend(orphans);
        rows.push(command_item(
            "orphans",
            explanation("orphans").title.into(),
            bytes,
            "pacman",
            args,
            true,
        ));
    }
    for (scope, base) in [
        (
            "--user",
            dirs::data_local_dir().unwrap_or_default().join("flatpak"),
        ),
        ("--system", PathBuf::from("/var/lib/flatpak")),
    ] {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(apps) = output("flatpak", &[scope, "list", "--app", "--columns=runtime"]) else {
            continue;
        };
        let Some(runtimes) = output("flatpak", &[scope, "list", "--runtime", "--columns=ref"])
        else {
            continue;
        };
        let lines = |s: &str| {
            s.lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let unused = super::system_preview::flatpak_unused(&lines(&apps), &lines(&runtimes));
        let paths: Vec<_> = unused
            .iter()
            .filter_map(|r| {
                let r = r.strip_prefix("runtime/").unwrap_or(r);
                if r.split('/').count() != 3 || r.split('/').any(|s| s.is_empty() || s == "..") {
                    return None;
                }
                let root = base.join("runtime");
                root.join(r)
                    .join("active")
                    .canonicalize()
                    .ok()
                    .filter(|p| p.starts_with(&root))
            })
            .collect();
        if !unused.is_empty() {
            rows.push(command_item(
                "flatpak",
                tr_args(
                    "Unused Flatpak Runtimes ({scope})",
                    &[(
                        "{scope}",
                        &tr(if scope == "--user" { "User" } else { "System" }),
                    )],
                ),
                measured(&paths, cancel, |_, _| true),
                "flatpak",
                vec![
                    scope.into(),
                    "uninstall".into(),
                    "--unused".into(),
                    "--noninteractive".into(),
                    "-y".into(),
                ],
                scope == "--system",
            ));
        }
    }
    if !cancel.load(Ordering::Relaxed) {
        if let Ok(plan) = super::build_plan(2) {
            if let Some(args) = super::removal_command(&plan) {
                let paths: Vec<_> = plan
                    .removable
                    .iter()
                    .flat_map(|k| {
                        [
                            PathBuf::from("/usr/lib/modules").join(&k.version),
                            PathBuf::from("/boot").join(format!("vmlinuz-{}", k.version)),
                        ]
                    })
                    .collect();
                let mut item = command_item(
                    "kernels",
                    explanation("kernels").title.into(),
                    measured(&paths, cancel, |_, _| true),
                    &args[0],
                    args[1..].to_vec(),
                    true,
                );
                item.icon = "pc-computer-symbolic";
                rows.push(item);
            }
        }
    }
    rows
}
fn docker_item(cancel: &AtomicBool) -> Option<CleanItem> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    // Ask the daemon read-only; never scan /var/lib/docker as removable files.
    let text = output(
        "docker",
        &["system", "df", "--format", "{{.Type}}\t{{.Reclaimable}}"],
    )?;
    let mut bytes = text
        .lines()
        .filter_map(|line| {
            let (kind, value) = line.split_once('\t')?;
            if kind != "Containers" && kind != "Build Cache" {
                return None;
            }
            parse_size(value.split_whitespace().next()?)
        })
        .sum::<u64>();
    if let Some(images) = output(
        "docker",
        &[
            "images",
            "--filter",
            "dangling=true",
            "--format",
            "{{.Size}}",
        ],
    ) {
        bytes += images.lines().filter_map(parse_size).sum::<u64>();
    }
    Some(command_item(
        "docker",
        explanation("docker").title.into(),
        bytes,
        "docker",
        vec!["system".into(), "prune".into(), "--force".into()],
        false,
    ))
}
fn parse_size(value: &str) -> Option<u64> {
    let split = value.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let number: f64 = value[..split].parse().ok()?;
    let factor = match &value[split..] {
        "B" => 1.,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((number * factor) as u64)
}

/// Recheck protected CLI versions immediately before passing files to Cleaner.
/// A command symlink can change while the confirmation dialog is open.
pub fn revalidate_versions(scan: &mut ScanResult) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let root = home.join(".local/share/claude/versions");
    if !scan.files.iter().any(|f| f.path.starts_with(&root)) {
        return;
    }
    let allowed: HashSet<_> = old_claude_versions(&home).into_iter().collect();
    scan.files
        .retain(|f| !f.path.starts_with(&root) || allowed.contains(&f.path));
    scan.total_size = scan.files.iter().map(|f| f.size).sum();
    scan.file_count = scan.files.len();
}

pub fn run_command(cleanup: &CommandCleanup) -> Result<String, String> {
    // The package plan is rebuilt at execution time so an update or reboot
    // between Scan and Clean cannot remove the running/newest kernel.
    if cleanup.program == "journalctl" {
        let days = cleanup
            .args
            .first()
            .and_then(|arg| arg.strip_prefix("--vacuum-time="))
            .and_then(|arg| arg.strip_suffix('d'))
            .and_then(|days| days.parse().ok())
            .ok_or("Invalid journal retention period")?;
        return super::vacuum_system_journal(days);
    }
    let mut cleanup = cleanup.clone();
    if cleanup.kernels {
        let plan = super::build_plan(2)?;
        let Some(args) = super::removal_command(&plan) else {
            return Ok(plan.summary());
        };
        cleanup.program = args[0].clone();
        cleanup.args = args[1..].to_vec();
    }
    if cleanup.program == "pacman" && cleanup.args.first().is_some_and(|a| a == "-Rns") {
        let current: HashSet<_> = super::system_preview::pacman_orphan_names()
            .into_iter()
            .collect();
        cleanup
            .args
            .retain(|arg| arg.starts_with('-') || current.contains(arg));
        if cleanup.args.len() <= 2 {
            return Ok(tr("No orphaned packages"));
        }
    }
    let program = executable(&cleanup.program)
        .ok_or_else(|| format!("{}: executable not found", cleanup.program))?;
    let mut command = if cleanup.admin {
        let mut cmd = Command::new(executable("pkexec").ok_or("pkexec: executable not found")?);
        cmd.arg(program);
        cmd
    } else {
        Command::new(program)
    };
    let out = command
        .args(&cleanup.args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_versions_keep_current_newest_running_and_unknown_entries() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["2.1.9", "2.1.10", "2.1.11", "2.1.12", "partial"] {
            std::fs::write(tmp.path().join(name), b"cli").unwrap();
        }
        std::os::unix::fs::symlink("2.1.9", tmp.path().join("1.0.0")).unwrap();
        let old = old_versions_in(
            tmp.path(),
            &tmp.path().join("2.1.10"),
            &HashSet::from([tmp.path().join("2.1.11")]),
        );
        assert_eq!(old, vec![tmp.path().join("2.1.9")]);
    }
    #[test]
    fn cache_names_and_ai_aur_policies() {
        let empty = ScanResult::new();
        for name in [
            "vllm",
            "huggingface",
            "ollama",
            "torch",
            "whisper",
            "lm-studio",
            "comfyui",
        ] {
            assert_eq!(
                cache_policy(Path::new(name), &empty),
                "model-cache",
                "{name}"
            );
            assert_eq!(
                explanation(cache_policy(Path::new(name), &empty)).safety,
                Safety::ReviewFirst
            );
        }
        for (name, title) in [("paru", "Paru Cache"), ("yay", "Yay Cache")] {
            assert_eq!(cache_policy(Path::new(name), &empty), name);
            assert_eq!(explanation(name).safety, Safety::Safe);
            assert_eq!(cache_title(Path::new(name), name), title);
        }
        assert_eq!(
            cache_title(Path::new("/test/.cache/shelly"), "generic-cache"),
            "Shelly Cache"
        );
        assert_eq!(
            cache_title(Path::new("/test/.cache/codex"), "generic-cache"),
            "Codex Cache"
        );
    }
    #[test]
    fn parses_docker_decimal_sizes() {
        assert_eq!(parse_size("1.25GB"), Some(1_250_000_000));
        assert_eq!(parse_size("12MB"), Some(12_000_000));
    }
}

#[cfg(test)]
mod inventory_tests {
    use super::*;
    #[test]
    fn linux_inventory_is_disjoint_and_preserves_private_data() {
        // Isolate HOME in a subprocess: dirs and the security auditor read it,
        // and changing process-wide environment would race the other tests.
        if std::env::var_os("MAGNIFICO_SCAN_FIXTURE").is_none() {
            let home = tempfile::tempdir().unwrap();
            let out = Command::new(std::env::current_exe().unwrap())
                .env("MAGNIFICO_SCAN_FIXTURE", "1").env("HOME",home.path())
                .env_remove("XDG_CACHE_HOME").env_remove("XDG_DATA_HOME").env_remove("XDG_CONFIG_HOME")
                .args(["--exact", "services::clean_sources::inventory_tests::linux_inventory_is_disjoint_and_preserves_private_data", "--nocapture"])
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
        let file = |relative: &str, bytes: u64| {
            let path = home.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::File::create(path).unwrap().set_len(bytes).unwrap();
        };
        file(".cache/thumbnails/normal/image", 100);
        file(".cache/custom-app/cache", 200);
        file(".cache/huggingface/hub/model.gguf", 1200);
        file(".cache/login-app/sessions.sqlite", 1300);
        file(".cache/keyrings/login.keyring", 1400);
        file(".cache/other-data/large.safetensors", 1500);
        file(".cache/go-build/build", 300);
        file(".npm/_cacache/package", 400);
        file(".config/Code/CachedData/bundle", 500);
        file(".m2/repository/local-build.jar", 600);
        file(".cache/JetBrains/IDE/LocalHistory/changes", 700);
        file(".cache/JetBrains/IDE/index", 800);
        file(".cache/uv/package", 900);
        file(".ssh/key", 1000);
        file(".cargo/bin/installed-tool", 1100);
        std::os::unix::fs::symlink(home.join(".ssh"), home.join(".cache/linked-private-data"))
            .unwrap();
        let storage = Storage::new();
        storage
            .update_app_rules(|rules| {
                rules.push(crate::models::AppRule::new(
                    "Installed tools",
                    "Keep executables",
                    "~/.cargo/bin",
                ))
            })
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let apps = scan(CleanPageKind::AppCaches, &storage, cancel.clone());
        let dev = scan(CleanPageKind::DevTools, &storage, cancel);
        assert_eq!(apps.iter().map(|i| i.bytes).sum::<u64>(), 5700);
        assert!(apps.iter().filter(|i| i.key == "generic-cache").all(|i| i.safety == Safety::Safe && i.explanation == "This is a known cache folder that apps or developer tools recreate automatically."));
        for key in ["model-cache", "keyring-cache", "session-cache"] {
            assert!(apps
                .iter()
                .any(|i| i.key == key && i.safety == Safety::ReviewFirst));
        }
        assert_eq!(
            dev.iter()
                .filter(|i| i.command.is_none())
                .map(|i| i.bytes)
                .sum::<u64>(),
            3500
        );
        let all: Vec<_> = apps.iter().chain(dev.iter()).collect();
        assert!(all.iter().flat_map(|i| &i.scan.files).all(|f| !f
            .path
            .to_string_lossy()
            .contains("LocalHistory")
            && !f.path.to_string_lossy().contains(".ssh")));
        assert_eq!(
            all.iter()
                .filter(|i| i.safety == Safety::Safe)
                .map(|i| i.bytes)
                .sum::<u64>(),
            2300
        );
        assert_eq!(
            all.iter()
                .filter(|i| i.safety == Safety::ReviewFirst && i.command.is_none())
                .map(|i| i.bytes)
                .sum::<u64>(),
            6900
        );
        let paths: Vec<_> = all
            .iter()
            .flat_map(|i| i.scan.files.iter().map(|f| &f.path))
            .collect();
        assert_eq!(paths.len(), paths.iter().collect::<HashSet<_>>().len());
        assert!(home.join(".ssh/key").exists());
        assert!(home
            .join(".cache/JetBrains/IDE/LocalHistory/changes")
            .exists());
    }
}
