//! Read-only package inventories shared by System and Overview.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

/// Files directly in `dir` whose name passes `keep`, with their sizes.
fn files_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<(String, u64)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(String, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            (meta.is_file() && keep(&name)).then_some((name, meta.len()))
        })
        .collect();
    files.sort();
    files
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `name ver-rel` from `pacman -Q`.
fn pacman_installed() -> HashMap<String, String> {
    run("pacman", &["-Q"])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (name, version) = line.split_once(' ')?;
            Some((name.to_string(), version.to_string()))
        })
        .collect()
}

/// `(name, ver-rel)` of a package file name such as
/// `foo-bar-1:2.0-3-x86_64.pkg.tar.zst`.
fn package_file_version(file: &str) -> Option<(&str, String)> {
    let stem = &file[..file.find(".pkg.tar")?];
    let mut parts = stem.rsplitn(4, '-');
    let _arch = parts.next()?;
    let rel = parts.next()?;
    let ver = parts.next()?;
    let name = parts.next()?;
    Some((name, format!("{ver}-{rel}")))
}

/// Cached files `pacman -Sc` deletes: every version that is not the one
/// installed.
fn pacman_unused(
    files: Vec<(String, u64)>,
    installed: &HashMap<String, String>,
) -> Vec<(String, u64)> {
    files
        .into_iter()
        .filter(|(file, _)| {
            package_file_version(file)
                .is_some_and(|(name, version)| installed.get(name) != Some(&version))
        })
        .collect()
}

pub(crate) fn pacman_cache_bytes() -> u64 {
    let files = files_in(Path::new("/var/cache/pacman/pkg"), |name| {
        name.contains(".pkg.tar") && !name.ends_with(".sig")
    });
    pacman_unused(files, &pacman_installed())
        .iter()
        .map(|(_, bytes)| bytes)
        .sum()
}

pub(crate) fn pacman_orphan_names() -> Vec<String> {
    run("pacman", &["-Qdtq"])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|name| {
            !name.is_empty()
                && !name.starts_with('-')
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"@._+-".contains(&c))
        })
        .map(str::to_owned)
        .collect()
}

pub(crate) fn pacman_installed_bytes(names: &[String]) -> u64 {
    if names.is_empty() {
        return 0;
    }
    let mut args = vec!["-Qi"];
    args.extend(names.iter().map(String::as_str));
    run("pacman", &args)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let value = line
                .strip_prefix("Installed Size")?
                .split_once(':')?
                .1
                .trim();
            let mut parts = value.split_whitespace();
            let n: f64 = parts.next()?.parse().ok()?;
            let factor = match parts.next()? {
                "B" => 1.,
                "KiB" => 1024.,
                "MiB" => 1048576.,
                "GiB" => 1073741824.,
                _ => return None,
            };
            Some((n * factor) as u64)
        })
        .sum()
}

fn split_ref(reference: &str) -> Option<(&str, &str)> {
    let mut parts = reference.split('/');
    let id = parts.next()?;
    let _arch = parts.next()?;
    let branch = parts.next()?;
    Some((id, branch))
}

/// Runtimes no app uses, as Flatpak would see them. Extensions belong to
/// the runtime whose id prefixes theirs (`org.gnome.Platform.Locale`) and
/// go with it when the branch is one of that runtime's; one with a branch
/// of its own (a GL driver) or no base at all (a theme) is kept.
pub(crate) fn flatpak_unused(apps: &[String], runtimes: &[String]) -> Vec<String> {
    let used: HashSet<(&str, &str)> = apps.iter().filter_map(|r| split_ref(r)).collect();
    let installed: Vec<(&str, &str)> = runtimes.iter().filter_map(|r| split_ref(r)).collect();
    let mut unused = Vec::new();
    for (reference, (id, branch)) in runtimes
        .iter()
        .zip(runtimes.iter().filter_map(|r| split_ref(r)))
    {
        let keep = if used.contains(&(id, branch)) {
            true
        } else if let Some(&(base, _)) = installed
            .iter()
            .filter(|(base, _)| {
                id.len() > base.len() && id.starts_with(base) && id[base.len()..].starts_with('.')
            })
            .max_by_key(|(base, _)| base.len())
        {
            let base_branches: Vec<&str> = installed
                .iter()
                .filter(|(b, _)| *b == base)
                .map(|(_, br)| *br)
                .collect();
            let of_base = base_branches
                .iter()
                .find(|br| branch == **br || branch.starts_with(&format!("{br}-")));
            match of_base {
                Some(br) => used.contains(&(base, br)),
                None => true,
            }
        } else {
            // A runtime proper goes; an add-on with no base installed
            // (a theme, a GL driver) is kept.
            !(id.ends_with(".Platform") || id.ends_with(".Sdk"))
        };
        if !keep {
            unused.push(reference.clone());
        }
    }
    unused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pacman_file_names() {
        assert_eq!(
            package_file_version("lib32-foo-bar-1:2.0.1-3-x86_64.pkg.tar.zst"),
            Some(("lib32-foo-bar", "1:2.0.1-3".to_string()))
        );
        assert_eq!(package_file_version("notes.txt"), None);
    }

    #[test]
    fn pacman_keeps_only_installed_versions() {
        let installed = HashMap::from([("foo".to_string(), "1.0-2".to_string())]);
        let files = vec![
            ("foo-1.0-2-x86_64.pkg.tar.zst".to_string(), 1),
            ("foo-0.9-1-x86_64.pkg.tar.zst".to_string(), 1),
            ("gone-1.0-1-any.pkg.tar.zst".to_string(), 1),
        ];
        let names: Vec<String> = pacman_unused(files, &installed)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            names,
            ["foo-0.9-1-x86_64.pkg.tar.zst", "gone-1.0-1-any.pkg.tar.zst"]
        );
    }

    #[test]
    fn flatpak_unused_follows_runtimes_and_their_extensions() {
        let apps = vec!["org.gnome.Platform/x86_64/50".to_string()];
        let runtimes: Vec<String> = [
            "org.gnome.Platform/x86_64/49",
            "org.gnome.Platform/x86_64/50",
            "org.gnome.Platform.Locale/x86_64/49",
            "org.gnome.Platform.Locale/x86_64/50",
            "org.freedesktop.Platform/x86_64/25.08",
            "org.freedesktop.Platform.GL.default/x86_64/25.08-extra",
            "org.freedesktop.Platform.GL.nvidia-1-2/x86_64/1.4",
            "org.gtk.Gtk3theme.adw-gtk3-dark/x86_64/3.22",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            flatpak_unused(&apps, &runtimes),
            [
                "org.gnome.Platform/x86_64/49",
                "org.gnome.Platform.Locale/x86_64/49",
                "org.freedesktop.Platform/x86_64/25.08",
                "org.freedesktop.Platform.GL.default/x86_64/25.08-extra",
            ]
        );
    }
}
