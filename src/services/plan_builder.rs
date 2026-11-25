//! Turn rules and detectors into the cleanup screen's plan: one item per
//! rule, project build folder, game or model, each sized separately so the
//! user can tick them one by one.

use super::big_items::{find_ai_models, find_games};
use super::projects::find_projects;
use super::Scanner;
use crate::models::{
    AppRule, BrowserRule, CleanupPlan, CustomRule, GroupKind, PlanGroup, PlanItem, ScanResult,
    SystemRule,
};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Everything the plan scan needs, taken from storage on the UI thread.
pub struct PlanInputs {
    pub system_rules: Vec<SystemRule>,
    pub app_rules: Vec<AppRule>,
    pub browser_rules: Vec<BrowserRule>,
    pub custom_rules: Vec<CustomRule>,
    pub scan_logs: bool,
    /// Folders searched for projects; `None` turns the project scan off.
    pub project_folders: Option<Vec<PathBuf>>,
    pub pin_recent_days: u32,
}

/// Folder names that belong to Python or JavaScript tooling.
const PYTHON_NODE: &[&str] = &[
    "node_modules",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".parcel-cache",
    ".turbo",
    ".angular",
    ".venv",
    "venv",
    ".tox",
    ".nox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
];

pub fn build_cleanup_plan(scanner: &Scanner, inputs: &PlanInputs) -> (CleanupPlan, bool) {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/nonexistent"));
    // Specific finds come first so a path also covered by a broad rule is
    // counted where it makes most sense.
    let mut groups: Vec<PlanGroup> = Vec::new();

    if let Some(folders) = &inputs.project_folders {
        groups.extend(project_groups(scanner, folders, inputs.pin_recent_days));
    }

    let games: Vec<PlanItem> = find_games(&home)
        .into_par_iter()
        .map(|(_, item)| {
            let detail = display_path(&item.paths[0], &home);
            // The shader cache has a fixed English name; games keep theirs.
            let fixed_name = item.paths[0].ends_with("shadercache");
            let plan_item = PlanItem::new(item.label, detail, scanner.scan_paths(&item.paths));
            if fixed_name {
                plan_item.translatable()
            } else {
                plan_item
            }
        })
        .collect();
    groups.push(PlanGroup {
        kind: GroupKind::Games,
        items: games,
    });

    let models: Vec<PlanItem> = find_ai_models(&home)
        .into_par_iter()
        .map(|item| {
            let detail = display_path(&item.paths[0], &home);
            PlanItem::new(item.label, detail, scanner.scan_paths(&item.paths))
        })
        .collect();
    groups.push(PlanGroup {
        kind: GroupKind::AiModels,
        items: models,
    });

    let (dev_rules, app_rules): (Vec<&AppRule>, Vec<&AppRule>) = inputs
        .app_rules
        .iter()
        .filter(|rule| rule.enabled)
        .partition(|rule| rule.is_developer_tool());

    let browsers = inputs
        .browser_rules
        .par_iter()
        .filter(|rule| rule.enabled)
        .map(|rule| {
            PlanItem::new(
                rule.display_name(),
                "",
                scanner.scan_browser_rules(std::slice::from_ref(rule)),
            )
            .translatable()
        })
        .collect();
    groups.push(PlanGroup {
        kind: GroupKind::Browsers,
        items: browsers,
    });

    for (kind, rules) in [
        (GroupKind::AppCaches, app_rules),
        (GroupKind::DevCaches, dev_rules),
    ] {
        let items = rules
            .par_iter()
            .map(|rule| {
                PlanItem::new(
                    rule.name.clone(),
                    rule.path.display().to_string(),
                    scanner.scan_app_rules(std::slice::from_ref(*rule)),
                )
                .translatable()
            })
            .collect();
        groups.push(PlanGroup { kind, items });
    }

    let custom = inputs
        .custom_rules
        .par_iter()
        .filter(|rule| rule.enabled)
        .map(|rule| {
            PlanItem::new(
                rule.name.clone(),
                "",
                scanner.scan_custom_rules(std::slice::from_ref(rule)),
            )
        })
        .collect();
    groups.push(PlanGroup {
        kind: GroupKind::Custom,
        items: custom,
    });

    let system = inputs
        .system_rules
        .par_iter()
        .filter(|rule| rule.enabled)
        .map(|rule| {
            PlanItem::new(
                rule.display_name(),
                "",
                scanner.scan_system_rules(std::slice::from_ref(rule)),
            )
            .translatable()
        })
        .collect();
    groups.push(PlanGroup {
        kind: GroupKind::System,
        items: system,
    });

    if inputs.scan_logs {
        groups.push(PlanGroup {
            kind: GroupKind::Logs,
            items: vec![
                PlanItem::new("Application logs", "", scanner.scan_application_logs())
                    .translatable(),
            ],
        });
    }

    let cancelled = scanner
        .cancellation()
        .load(std::sync::atomic::Ordering::Relaxed);
    dedupe(&mut groups);
    mark_other_disks(&mut groups, &home);
    (CleanupPlan::new(groups), cancelled)
}

fn project_groups(scanner: &Scanner, folders: &[PathBuf], pin_days: u32) -> Vec<PlanGroup> {
    let busy = super::open_files::working_paths();
    let projects = find_projects(
        folders,
        Duration::from_secs(u64::from(pin_days) * 24 * 60 * 60),
        SystemTime::now(),
        &busy,
        scanner.cancellation(),
    );
    let home = dirs::home_dir().unwrap_or_default();
    let found: Vec<(GroupKind, PlanItem)> = projects
        .par_iter()
        .flat_map(|project| {
            project
                .artifacts
                .par_iter()
                .map(move |artifact| (project, artifact))
        })
        .map(|(project, artifact)| {
            let name = file_name(&project.root);
            let folder = file_name(artifact);
            let kind = if folder == "target" && project.root.join("Cargo.toml").is_file() {
                GroupKind::RustBuilds
            } else if PYTHON_NODE.contains(&folder.as_str()) {
                GroupKind::PythonNode
            } else {
                GroupKind::OtherBuilds
            };
            let label = if kind == GroupKind::RustBuilds {
                name
            } else {
                format!("{name} · {folder}")
            };
            let mut item = PlanItem::new(
                label,
                display_path(artifact, &home),
                scanner.scan_paths(std::slice::from_ref(artifact)),
            );
            item.pinned = project.recently_changed;
            item.busy = project.busy;
            (kind, item)
        })
        .collect();

    [
        GroupKind::RustBuilds,
        GroupKind::PythonNode,
        GroupKind::OtherBuilds,
    ]
    .into_iter()
    .map(|kind| PlanGroup {
        kind,
        items: found
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, item)| item.clone())
            .collect(),
    })
    .collect()
}

/// A path found by two items stays with the first one, so sizes add up.
fn dedupe(groups: &mut [PlanGroup]) {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for group in groups.iter_mut() {
        for item in &mut group.items {
            let old = std::mem::take(&mut item.result);
            let mut fresh = ScanResult::new();
            // Paths another item already took: a whole folder holding one
            // is no longer whole for this item.
            let mut taken: Vec<PathBuf> = Vec::new();
            for file in old.files {
                if seen.insert(file.path.clone()) {
                    fresh.add_file(file);
                } else {
                    taken.push(file.path);
                }
            }
            for dir in old.directories {
                if seen.insert(dir.path.clone()) {
                    fresh.add_directory(dir);
                } else {
                    taken.push(dir.path);
                }
            }
            let whole: HashSet<&Path> = old.whole_dirs.iter().map(PathBuf::as_path).collect();
            let broken: HashSet<&Path> = taken
                .iter()
                .flat_map(|path| path.ancestors())
                .filter(|dir| whole.contains(dir))
                .collect();
            fresh.whole_dirs = old
                .whole_dirs
                .iter()
                .filter(|dir| !broken.contains(dir.as_path()))
                .cloned()
                .collect();
            fresh.skipped = old.skipped;
            fresh.security_violations = old.security_violations;
            fresh.errors = old.errors;
            fresh.cancelled = old.cancelled;
            item.result = fresh;
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `~/code/app` instead of `/home/name/code/app`.
/// Tag items that live on another disk than `home` with that disk's mount
/// point, so the screen does not count them as space freed on the home disk.
fn mark_other_disks(groups: &mut [PlanGroup], home: &Path) {
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;
    let Ok(home_dev) = std::fs::metadata(home).map(|meta| meta.dev()) else {
        return;
    };
    let mut mounts: HashMap<u64, String> = HashMap::new();
    for item in groups.iter_mut().flat_map(|group| group.items.iter_mut()) {
        let Some(path) = item
            .result
            .files
            .first()
            .or(item.result.directories.first())
            .map(|entry| entry.path.clone())
        else {
            continue;
        };
        let Ok(dev) = std::fs::symlink_metadata(&path).map(|meta| meta.dev()) else {
            continue;
        };
        if dev != home_dev {
            let mount = mounts
                .entry(dev)
                .or_insert_with(|| mount_point(&path, dev).display().to_string());
            item.disk = Some(mount.clone());
        }
    }
}

/// The highest folder above `path` still on device `dev`: its mount point.
fn mount_point(path: &Path, dev: u64) -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let mut mount = path.to_path_buf();
    for parent in path.ancestors().skip(1) {
        match std::fs::metadata(parent) {
            Ok(meta) if meta.dev() == dev => mount = parent.to_path_buf(),
            _ => break,
        }
    }
    mount
}

fn display_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod disk_tests {
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn finds_the_mount_point_of_a_path() {
        let dev = std::fs::metadata("/proc/self").unwrap().dev();
        assert_eq!(
            super::mount_point(std::path::Path::new("/proc/self"), dev),
            std::path::PathBuf::from("/proc")
        );
    }
}
