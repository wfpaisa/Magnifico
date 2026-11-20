pub mod app_leftovers;
pub mod app_uninstall;
mod big_items;
pub mod clean_sources;
mod cleaner;
pub mod cleanup_trash;
mod file_finder;
mod icon_resolver;
mod kernel_cleanup;
mod log_cleanup;
mod open_files;
mod plan_builder;
mod projects;
pub mod review_items;
mod scanner;
mod secure_delete;
mod security;
mod storage_analyzer;
pub mod system_preview;
pub mod trash_store;

pub use cleaner::{CleanOptions, CleanProgress, Cleaner, TRASH_MAX_FILES};
pub use file_finder::{find_duplicates, find_large_files, DuplicateProgress, FileKind, FoundFile};
pub use icon_resolver::{DesktopEnvironment, DisplayServer, IconDiagnostics, IconResolver};
pub use kernel_cleanup::{build_plan, removal_command};
pub use log_cleanup::{system_journal_available, vacuum_system_journal};
pub use plan_builder::{build_cleanup_plan, PlanInputs};
pub use scanner::{ScanOptions, Scanner};
pub use secure_delete::storage_facts;
pub use security::SecurityAuditor;
pub use storage_analyzer::{
    analyze_storage, move_to_trash_with_progress, StorageAnalysis, StorageAnalysisError,
    StorageNode, StorageScanOptions, TrashTarget,
};
