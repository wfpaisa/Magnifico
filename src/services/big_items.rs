//! Big, re-downloadable data that no cleanup rule covers: installed games
//! and AI models. Each find returns labelled paths; the scanner sizes them.
//!
//! Locations (Linux defaults, native and Flatpak):
//! - Steam: `~/.local/share/Steam` (also `~/.steam/steam`, Flatpak
//!   `~/.var/app/com.valvesoftware.Steam/.local/share/Steam`). Extra
//!   libraries are listed in `steamapps/libraryfolders.vdf`; each game is an
//!   `appmanifest_<id>.acf` with its `installdir` under `steamapps/common`.
//!   `compatdata` holds Proton prefixes with save games and is never listed.
//! - Heroic: `~/.config/heroic` (Flatpak `~/.var/app/com.heroicgameslauncher.hgl/config/heroic`).
//!   Epic games via Legendary in `legendaryConfig/legendary/installed.json`,
//!   GOG in `gog_store/installed.json`, Amazon in `nile_config/nile/installed.json`.
//! - Hugging Face: `$HF_HUB_CACHE`, `$HF_HOME/hub`, or `~/.cache/huggingface/hub`,
//!   one `models--org--name` or `datasets--org--name` folder per download.
//! - Ollama: `$OLLAMA_MODELS` or `~/.ollama/models` (blobs shared between
//!   models, so it is offered as one item).
//! - LM Studio: `~/.lmstudio/models/<publisher>/<model>` (older
//!   `~/.cache/lm-studio/models`).
//! - vLLM: `~/.cache/vllm` (compile cache, rebuilt on the next start).
//! - PyTorch Hub: `~/.cache/torch/hub`.
//! - Loose games: each folder in `~/Games` that no launcher above lists.
//!
//! Roots are resolved through symlinks first (`~/.cache/huggingface` or
//! `~/Games` often point to another disk): the deletion checks refuse any
//! path that goes through a symlink, so the real path is what gets listed.

use std::fs;
use std::path::{Path, PathBuf};

/// One thing the user can pick, made of one or more paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigItem {
    pub label: String,
    pub paths: Vec<PathBuf>,
}

impl BigItem {
    fn new(label: impl Into<String>, paths: Vec<PathBuf>) -> Self {
        Self {
            label: label.into(),
            paths,
        }
    }
}

/// What a game launcher has installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    Steam,
    Heroic,
    /// A folder in `~/Games` that no launcher claims.
    Folder,
}

pub fn find_games(home: &Path) -> Vec<(Launcher, BigItem)> {
    let mut games: Vec<(Launcher, BigItem)> = steam_games(home)
        .into_iter()
        .map(|item| (Launcher::Steam, item))
        .collect();
    games.extend(
        heroic_games(home)
            .into_iter()
            .map(|item| (Launcher::Heroic, item)),
    );
    let listed: Vec<PathBuf> = games
        .iter()
        .flat_map(|(_, item)| item.paths.clone())
        .collect();
    games.extend(
        loose_games(home, &listed)
            .into_iter()
            .map(|item| (Launcher::Folder, item)),
    );
    games
}

/// Folders in `~/Games` not already listed by a launcher, nor holding or
/// inside a listed install (Heroic's default is `~/Games/Heroic/<game>`).
fn loose_games(home: &Path, listed: &[PathBuf]) -> Vec<BigItem> {
    read_dirs(&real(&home.join("Games")))
        .into_iter()
        .filter(|dir| {
            !listed
                .iter()
                .any(|path| path.starts_with(dir) || dir.starts_with(path))
        })
        .map(|dir| BigItem::new(file_name(&dir), vec![dir]))
        .collect()
}

/// `path` with every symlink resolved, or as given when it does not exist.
fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn find_ai_models(home: &Path) -> Vec<BigItem> {
    let mut items = Vec::new();

    let hf_hub = std::env::var_os("HF_HUB_CACHE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HF_HOME").map(|h| PathBuf::from(h).join("hub")))
        .unwrap_or_else(|| home.join(".cache/huggingface/hub"));
    for entry in read_dirs(&real(&hf_hub)) {
        let name = file_name(&entry);
        let label = name
            .strip_prefix("models--")
            .or_else(|| name.strip_prefix("datasets--"))
            .map(|rest| format!("Hugging Face · {}", rest.replace("--", "/")));
        if let Some(label) = label {
            items.push(BigItem::new(label, vec![entry]));
        }
    }

    let ollama = std::env::var_os("OLLAMA_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".ollama/models"));
    let ollama = real(&ollama);
    if ollama.is_dir() {
        items.push(BigItem::new("Ollama", vec![ollama]));
    }

    for root in [
        home.join(".lmstudio/models"),
        home.join(".cache/lm-studio/models"),
    ] {
        for publisher in read_dirs(&real(&root)) {
            for model in read_dirs(&publisher) {
                let label = format!("LM Studio · {}", file_name(&model));
                items.push(BigItem::new(label, vec![model]));
            }
        }
    }

    for (label, path) in [
        ("vLLM", home.join(".cache/vllm")),
        ("PyTorch Hub", home.join(".cache/torch/hub")),
        ("Whisper", home.join(".cache/whisper")),
    ] {
        let path = real(&path);
        if path.is_dir() {
            items.push(BigItem::new(label, vec![path]));
        }
    }
    items
}

fn steam_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for candidate in [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ] {
        let Ok(real) = fs::canonicalize(&candidate) else {
            continue;
        };
        if real.join("steamapps").is_dir() && !roots.contains(&real) {
            roots.push(real);
        }
    }
    roots
}

/// Every Steam library folder (the main one plus extra drives).
fn steam_libraries(home: &Path) -> Vec<PathBuf> {
    let mut libraries = Vec::new();
    for root in steam_roots(home) {
        if !libraries.contains(&root) {
            libraries.push(root.clone());
        }
        let vdf = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
        for (key, value) in vdf_pairs(&vdf) {
            if key == "path" {
                let path = fs::canonicalize(&value).unwrap_or_else(|_| PathBuf::from(&value));
                if path.join("steamapps").is_dir() && !libraries.contains(&path) {
                    libraries.push(path);
                }
            }
        }
    }
    libraries
}

fn steam_games(home: &Path) -> Vec<BigItem> {
    let mut items = Vec::new();
    for library in steam_libraries(home) {
        let steamapps = library.join("steamapps");
        let Ok(entries) = fs::read_dir(&steamapps) else {
            continue;
        };
        for entry in entries.flatten() {
            let manifest = entry.path();
            let name = file_name(&manifest);
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let text = fs::read_to_string(&manifest).unwrap_or_default();
            let pairs = vdf_pairs(&text);
            let get = |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v.clone())
            };
            let Some(installdir) = get("installdir").filter(|d| safe_component(d)) else {
                continue;
            };
            let folder = steamapps.join("common").join(&installdir);
            if !folder.is_dir() {
                continue;
            }
            let title = get("name").unwrap_or(installdir);
            // Removing the manifest too makes Steam show the game as not
            // installed instead of broken.
            items.push(BigItem::new(title, vec![folder, manifest]));
        }
        let shaders = steamapps.join("shadercache");
        if shaders.is_dir() {
            items.push(BigItem::new("Steam · shader cache", vec![shaders]));
        }
    }
    items
}

fn heroic_games(home: &Path) -> Vec<BigItem> {
    let mut items = Vec::new();
    for config in [
        home.join(".config/heroic"),
        home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic"),
    ] {
        // Legendary (Epic) and Nile (Amazon): { "id": { "title", "install_path" } }
        for file in [
            config.join("legendaryConfig/legendary/installed.json"),
            config.join("nile_config/nile/installed.json"),
        ] {
            let Some(json) = read_json(&file) else {
                continue;
            };
            let games: Vec<&serde_json::Value> = match &json {
                serde_json::Value::Object(map) => map.values().collect(),
                serde_json::Value::Array(list) => list.iter().collect(),
                _ => Vec::new(),
            };
            for game in games {
                push_install(&mut items, game, &["title", "app_name", "id"]);
            }
        }
        // GOG: { "installed": [ { "appName", "install_path" } ] }
        if let Some(json) = read_json(&config.join("gog_store/installed.json")) {
            if let Some(list) = json.get("installed").and_then(|v| v.as_array()) {
                for game in list {
                    push_install(&mut items, game, &["title"]);
                }
            }
        }
    }
    items
}

fn push_install(items: &mut Vec<BigItem>, game: &serde_json::Value, title_keys: &[&str]) {
    let Some(path) = game.get("install_path").and_then(|v| v.as_str()) else {
        return;
    };
    let path = real(Path::new(path));
    if !path.is_dir() || items.iter().any(|item| item.paths.contains(&path)) {
        return;
    }
    let title = title_keys
        .iter()
        .find_map(|key| game.get(*key).and_then(|v| v.as_str()))
        .map(str::to_owned)
        .unwrap_or_else(|| file_name(&path));
    items.push(BigItem::new(title, vec![path]));
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// `"key" "value"` pairs of a Valve KeyValues text file, in order. Nesting
/// is ignored: callers only look for unique keys.
fn vdf_pairs(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let quoted: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        if let [key, value] = quoted.as_slice() {
            pairs.push(((*key).to_owned(), value.replace("\\\\", "\\")));
        }
    }
    pairs
}

/// A single folder name: no separators, not `.` or `..`.
fn safe_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

fn read_dirs(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn reads_steam_manifests_and_skips_missing_folders() {
        let home = TempDir::new().unwrap();
        let apps = home.path().join(".local/share/Steam/steamapps");
        fs::create_dir_all(apps.join("common/Portal")).unwrap();
        fs::write(
            apps.join("appmanifest_400.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"400\"\n\t\"name\"\t\t\"Portal\"\n\t\"installdir\"\t\t\"Portal\"\n}\n",
        )
        .unwrap();
        fs::write(
            apps.join("appmanifest_500.acf"),
            "\"name\" \"Gone\"\n\"installdir\" \"Gone\"\n",
        )
        .unwrap();
        fs::write(
            apps.join("appmanifest_600.acf"),
            "\"name\" \"Evil\"\n\"installdir\" \"..\"\n",
        )
        .unwrap();

        let games = steam_games(home.path());
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].label, "Portal");
        assert!(games[0].paths[0].ends_with("steamapps/common/Portal"));
        assert!(games[0].paths[1].ends_with("appmanifest_400.acf"));
    }

    #[test]
    fn lists_hugging_face_downloads_by_name() {
        let home = TempDir::new().unwrap();
        let hub = home.path().join(".cache/huggingface/hub");
        fs::create_dir_all(hub.join("models--Qwen--Qwen3-8B")).unwrap();
        fs::create_dir_all(hub.join(".locks")).unwrap();
        std::env::remove_var("HF_HUB_CACHE");
        std::env::remove_var("HF_HOME");
        let labels: Vec<String> = find_ai_models(home.path())
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(labels, vec!["Hugging Face · Qwen/Qwen3-8B".to_string()]);
    }

    #[test]
    fn follows_symlinked_roots_to_the_real_disk() {
        let home = TempDir::new().unwrap();
        let disk = TempDir::new().unwrap();
        fs::create_dir_all(disk.path().join("hf/hub/models--gpt2")).unwrap();
        fs::create_dir_all(disk.path().join("games/Portal")).unwrap();
        fs::create_dir_all(home.path().join(".cache")).unwrap();
        std::os::unix::fs::symlink(
            disk.path().join("hf"),
            home.path().join(".cache/huggingface"),
        )
        .unwrap();
        std::os::unix::fs::symlink(disk.path().join("games"), home.path().join("Games")).unwrap();
        std::env::remove_var("HF_HUB_CACHE");
        std::env::remove_var("HF_HOME");
        let real_disk = fs::canonicalize(disk.path()).unwrap();

        let models = find_ai_models(home.path());
        assert_eq!(models[0].paths, vec![real_disk.join("hf/hub/models--gpt2")]);

        let games = loose_games(home.path(), &[]);
        assert_eq!(games[0].label, "Portal");
        assert_eq!(games[0].paths, vec![real_disk.join("games/Portal")]);
    }

    #[test]
    fn loose_games_skip_launcher_installs() {
        let home = TempDir::new().unwrap();
        let games = home.path().join("Games");
        fs::create_dir_all(games.join("Heroic/Hades")).unwrap();
        fs::create_dir_all(games.join("Mine")).unwrap();
        let real_games = fs::canonicalize(&games).unwrap();
        let found = loose_games(home.path(), &[real_games.join("Heroic/Hades")]);
        let labels: Vec<String> = found.into_iter().map(|item| item.label).collect();
        assert_eq!(labels, vec!["Mine".to_string()]);
    }
}
