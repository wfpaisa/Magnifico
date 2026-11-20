//! Successful removals only: the same delta feeds every cached inventory.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default)]
pub struct RemovalUpdate {
    pub paths: HashSet<PathBuf>,
    pub sizes: Vec<(PathBuf, u64)>,
    pub uninstalled: Vec<String>,
    pub commands: Vec<String>,
    pub freed_bytes: u64,
    pub trashed_bytes: u64,
}
impl RemovalUpdate {
    pub fn removes(&self, path: &Path) -> bool {
        path.ancestors().any(|p| self.paths.contains(p))
    }
    pub fn bytes_under(&self, path: &Path) -> u64 {
        self.sizes
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(_, size)| size)
            .sum()
    }
    pub fn merge(&mut self, other: Self) {
        self.paths.extend(other.paths);
        self.sizes.extend(other.sizes);
        self.uninstalled.extend(other.uninstalled);
        self.commands.extend(other.commands);
        self.freed_bytes += other.freed_bytes;
        self.trashed_bytes += other.trashed_bytes;
    }
}
impl From<&crate::models::CleanResult> for RemovalUpdate {
    fn from(result: &crate::models::CleanResult) -> Self {
        let trashed_bytes = result.trashed_files.iter().map(|f| f.size).sum();
        Self {
            paths: result
                .deleted_files
                .iter()
                .chain(&result.deleted_directories)
                .cloned()
                .collect(),
            sizes: result.removed_sizes.clone(),
            freed_bytes: result.bytes_freed.saturating_sub(trashed_bytes),
            trashed_bytes,
            ..Default::default()
        }
    }
}
impl From<&super::storage_analyzer::TrashResult> for RemovalUpdate {
    fn from(result: &super::storage_analyzer::TrashResult) -> Self {
        Self {
            paths: result.moved.iter().map(|(path, _)| path.clone()).collect(),
            sizes: result.moved.clone(),
            trashed_bytes: result.bytes_moved(),
            ..Default::default()
        }
    }
}
