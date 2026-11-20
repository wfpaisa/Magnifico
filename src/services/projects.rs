//! Find the dependency and build folders (node_modules, target, .venv…) of
//! projects nobody has worked on for a while. They are rebuilt the next time
//! the project is built, so they are dead weight until then.
//!
//! The idea follows Purge's inactive-project scan (MIT); the detection rules
//! here are written for Linux toolchains.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

/// How deep below a search folder projects are looked for.
const MAX_SEARCH_DEPTH: usize = 8;
/// How deep inside a project its own files are checked for recent changes.
const MAX_ACTIVITY_DEPTH: usize = 6;
/// A project with more files than this is treated as active rather than
/// guessed at: better to keep a folder than to delete one in use.
const MAX_ACTIVITY_ENTRIES: usize = 50_000;

/// One kind of project: files that mark its root, and the folders its tools
/// regenerate.
struct ProjectKind {
    markers: &'static [&'static str],
    artifacts: &'static [Artifact],
}

struct Artifact {
    name: &'static str,
    /// A file that must exist inside, so a hand-made folder that happens to
    /// share the name is never taken for a build folder.
    proof: Option<&'static [&'static str]>,
}

const fn any(name: &'static str) -> Artifact {
    Artifact { name, proof: None }
}

const fn proven(name: &'static str, proof: &'static [&'static str]) -> Artifact {
    Artifact {
        name,
        proof: Some(proof),
    }
}

const KINDS: &[ProjectKind] = &[
    // Rust
    ProjectKind {
        markers: &["Cargo.toml"],
        artifacts: &[proven("target", &["CACHEDIR.TAG", ".rustc_info.json"])],
    },
    // JavaScript / TypeScript
    ProjectKind {
        markers: &["package.json"],
        artifacts: &[
            any("node_modules"),
            any(".next"),
            any(".nuxt"),
            any(".svelte-kit"),
            any(".parcel-cache"),
            any(".turbo"),
            any(".angular"),
        ],
    },
    // Python
    ProjectKind {
        markers: &[
            "pyproject.toml",
            "requirements.txt",
            "setup.py",
            "setup.cfg",
            "Pipfile",
        ],
        artifacts: &[
            proven(".venv", &["pyvenv.cfg"]),
            proven("venv", &["pyvenv.cfg"]),
            any(".tox"),
            any(".nox"),
            any(".mypy_cache"),
            any(".pytest_cache"),
            any(".ruff_cache"),
        ],
    },
    // Gradle (Java, Kotlin, Android)
    ProjectKind {
        markers: &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        artifacts: &[any("build"), any(".gradle")],
    },
    // Maven
    ProjectKind {
        markers: &["pom.xml"],
        artifacts: &[proven(
            "target",
            &["classes", "maven-status", "maven-archiver"],
        )],
    },
    // Dart / Flutter
    ProjectKind {
        markers: &["pubspec.yaml"],
        artifacts: &[any(".dart_tool"), any("build")],
    },
    // Elixir
    ProjectKind {
        markers: &["mix.exs"],
        artifacts: &[any("_build"), any("deps")],
    },
    // Zig
    ProjectKind {
        markers: &["build.zig"],
        artifacts: &[any(".zig-cache"), any("zig-cache"), any("zig-out")],
    },
    // Haskell Stack
    ProjectKind {
        markers: &["stack.yaml"],
        artifacts: &[any(".stack-work")],
    },
];

/// Git files that change when someone commits, checks out or fetches.
const GIT_ACTIVITY_FILES: &[&str] = &["HEAD", "index", "FETCH_HEAD", "ORIG_HEAD", "logs/HEAD"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InactiveProject {
    pub root: PathBuf,
    /// Regenerable folders found in the project, never symlinks.
    pub artifacts: Vec<PathBuf>,
}

/// Projects under `search_roots` with nothing changed for at least
/// `min_age`, and no running program working inside them (`busy_paths`).
pub fn find_inactive_projects(
    search_roots: &[PathBuf],
    min_age: Duration,
    now: SystemTime,
    busy_paths: &[PathBuf],
    cancelled: &AtomicBool,
) -> Vec<InactiveProject> {
    let cutoff = now.checked_sub(min_age).unwrap_or(SystemTime::UNIX_EPOCH);
    let skip: HashSet<&str> = all_artifact_names().collect();
    project_dirs(search_roots, &skip, cancelled)
        .into_iter()
        .filter(|(dir, _)| !busy_paths.iter().any(|busy| busy.starts_with(dir)))
        .filter(|(dir, _)| is_inactive_since(dir, cutoff, &skip))
        .map(|(root, artifacts)| InactiveProject { root, artifacts })
        .collect()
}

/// A project with build folders, whatever its age.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    /// Regenerable folders found in the project, never symlinks.
    pub artifacts: Vec<PathBuf>,
    /// Something in the project changed within the `recent` window: the user
    /// is probably working on it, so it starts pinned.
    pub recently_changed: bool,
    /// A running program has a file open or its working folder inside.
    pub busy: bool,
}

/// Every project under `search_roots` that has build or dependency folders.
pub fn find_projects(
    search_roots: &[PathBuf],
    recent: Duration,
    now: SystemTime,
    busy_paths: &[PathBuf],
    cancelled: &AtomicBool,
) -> Vec<Project> {
    let cutoff = now.checked_sub(recent).unwrap_or(SystemTime::UNIX_EPOCH);
    let skip: HashSet<&str> = all_artifact_names().collect();
    let mut found = Vec::new();
    for (dir, artifacts) in project_dirs(search_roots, &skip, cancelled) {
        let busy = busy_paths.iter().any(|path| path.starts_with(&dir));
        let recently_changed = !is_inactive_since(&dir, cutoff, &skip);
        found.push(Project {
            root: dir,
            artifacts,
            recently_changed,
            busy,
        });
    }
    found
}

/// Folders under `search_roots` that are project roots with build folders.
fn project_dirs(
    search_roots: &[PathBuf],
    skip: &HashSet<&str>,
    cancelled: &AtomicBool,
) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for search_root in search_roots {
        let walker = WalkDir::new(search_root)
            .follow_links(false)
            .max_depth(MAX_SEARCH_DEPTH)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0
                    || (entry.file_type().is_dir() && !is_hidden(entry.path()) && {
                        let name = entry.file_name().to_string_lossy();
                        !skip.contains(name.as_ref())
                    })
            });
        for entry in walker.flatten() {
            if cancelled.load(Ordering::Relaxed) {
                return found;
            }
            if !entry.file_type().is_dir() || !seen.insert(entry.path().to_path_buf()) {
                continue;
            }
            let artifacts = project_artifacts(entry.path());
            if !artifacts.is_empty() {
                found.push((entry.path().to_path_buf(), artifacts));
            }
        }
    }
    found
}

fn all_artifact_names() -> impl Iterator<Item = &'static str> {
    KINDS
        .iter()
        .flat_map(|kind| kind.artifacts.iter().map(|artifact| artifact.name))
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .map(|name| name.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
}

/// Build folders present in `dir` if it is the root of a known project.
fn project_artifacts(dir: &Path) -> Vec<PathBuf> {
    let mut artifacts = Vec::new();
    for kind in KINDS {
        if !kind.markers.iter().any(|marker| dir.join(marker).is_file()) {
            continue;
        }
        for artifact in kind.artifacts {
            let path = dir.join(artifact.name);
            // symlink_metadata: a link to a folder elsewhere is not ours.
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_dir() || artifacts.contains(&path) {
                continue;
            }
            let proven = artifact
                .proof
                .is_none_or(|proof| proof.iter().any(|p| path.join(p).exists()));
            if proven {
                artifacts.push(path);
            }
        }
    }
    artifacts
}

/// True when no source file, folder or git state in the project changed
/// after `cutoff`. Build folders are left out: building updates them
/// without anyone working on the project.
fn is_inactive_since(project: &Path, cutoff: SystemTime, skip: &HashSet<&str>) -> bool {
    let recent = |path: &Path| {
        fs::symlink_metadata(path)
            .and_then(|meta| meta.modified())
            .map(|modified| modified > cutoff)
            .unwrap_or(false)
    };

    let git = project.join(".git");
    if GIT_ACTIVITY_FILES
        .iter()
        .any(|file| recent(&git.join(file)))
    {
        return false;
    }

    let walker = WalkDir::new(project)
        .follow_links(false)
        .max_depth(MAX_ACTIVITY_DEPTH)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0 || {
                let name = entry.file_name().to_string_lossy();
                !name.starts_with('.') && !skip.contains(name.as_ref())
            }
        });
    for (count, entry) in walker.enumerate() {
        if count >= MAX_ACTIVITY_ENTRIES {
            return false;
        }
        let Ok(entry) = entry else { continue };
        let modified = entry.metadata().ok().and_then(|meta| meta.modified().ok());
        if modified.is_some_and(|modified| modified > cutoff) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::TempDir;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn age_tree(path: &Path, when: SystemTime) {
        for entry in WalkDir::new(path)
            .contents_first(true)
            .into_iter()
            .flatten()
        {
            if entry.path_is_symlink() {
                continue;
            }
            let file = if entry.file_type().is_dir() {
                File::open(entry.path())
            } else {
                File::options().write(true).open(entry.path())
            };
            file.unwrap().set_modified(when).unwrap();
        }
    }

    fn node_project(parent: &Path, name: &str) -> PathBuf {
        let root = parent.join(name);
        fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::write(root.join("index.js"), "").unwrap();
        fs::write(root.join("node_modules/left-pad/index.js"), "").unwrap();
        root
    }

    fn find(roots: &[PathBuf], busy: &[PathBuf]) -> Vec<InactiveProject> {
        find_inactive_projects(
            roots,
            90 * DAY,
            SystemTime::now(),
            busy,
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn finds_old_projects_and_leaves_recent_ones() {
        let dir = TempDir::new().unwrap();
        let old = node_project(dir.path(), "old");
        let recent = node_project(dir.path(), "recent");
        age_tree(&old, SystemTime::now() - 200 * DAY);
        age_tree(&recent, SystemTime::now() - 200 * DAY);
        fs::write(recent.join("index.js"), "changed").unwrap();

        let found = find(&[dir.path().to_path_buf()], &[]);
        assert_eq!(
            found,
            vec![InactiveProject {
                root: old.clone(),
                artifacts: vec![old.join("node_modules")],
            }]
        );
    }

    #[test]
    fn a_recent_build_does_not_make_a_project_active() {
        let dir = TempDir::new().unwrap();
        let project = node_project(dir.path(), "app");
        age_tree(&project, SystemTime::now() - 200 * DAY);
        fs::write(project.join("node_modules/left-pad/index.js"), "new").unwrap();

        assert_eq!(find(&[dir.path().to_path_buf()], &[]).len(), 1);
    }

    #[test]
    fn a_recent_git_commit_keeps_the_project() {
        let dir = TempDir::new().unwrap();
        let project = node_project(dir.path(), "app");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(project.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        age_tree(&project, SystemTime::now() - 200 * DAY);
        File::options()
            .write(true)
            .open(project.join(".git/HEAD"))
            .unwrap()
            .set_modified(SystemTime::now() - DAY)
            .unwrap();

        assert!(find(&[dir.path().to_path_buf()], &[]).is_empty());
    }

    #[test]
    fn skips_projects_a_program_is_working_in() {
        let dir = TempDir::new().unwrap();
        let project = node_project(dir.path(), "app");
        age_tree(&project, SystemTime::now() - 200 * DAY);

        let busy = vec![project.join("node_modules/left-pad/index.js")];
        assert!(find(&[dir.path().to_path_buf()], &busy).is_empty());
    }

    #[test]
    fn needs_proof_before_taking_a_target_folder() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("crate");
        fs::create_dir_all(project.join("target/debug")).unwrap();
        fs::write(project.join("Cargo.toml"), "").unwrap();
        age_tree(&project, SystemTime::now() - 200 * DAY);
        assert!(find(&[dir.path().to_path_buf()], &[]).is_empty());

        fs::write(project.join("target/CACHEDIR.TAG"), "").unwrap();
        age_tree(&project, SystemTime::now() - 200 * DAY);
        let found = find(&[dir.path().to_path_buf()], &[]);
        assert_eq!(found[0].artifacts, vec![project.join("target")]);
    }

    #[test]
    fn lists_projects_of_any_age_and_marks_recent_ones() {
        let dir = TempDir::new().unwrap();
        let old = node_project(dir.path(), "old");
        let recent = node_project(dir.path(), "recent");
        age_tree(&old, SystemTime::now() - 200 * DAY);
        let mut found = find_projects(
            &[dir.path().to_path_buf()],
            7 * DAY,
            SystemTime::now(),
            &[],
            &AtomicBool::new(false),
        );
        found.sort_by(|a, b| a.root.cmp(&b.root));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].root, old);
        assert!(!found[0].recently_changed);
        assert_eq!(found[1].root, recent);
        assert!(found[1].recently_changed);
    }

    #[test]
    fn never_follows_a_symlinked_build_folder() {
        let dir = TempDir::new().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let project = dir.path().join("app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("package.json"), "{}").unwrap();
        std::os::unix::fs::symlink(&elsewhere, project.join("node_modules")).unwrap();
        age_tree(dir.path(), SystemTime::now() - 200 * DAY);

        assert!(find(&[dir.path().to_path_buf()], &[]).is_empty());
    }
}
