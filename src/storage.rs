//! Persistent storage for application configuration and rules.

use crate::models::{AppRule, AppSettings, BrowserRule, BrowserType, CustomRule, SystemRule};
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use thiserror::Error;
use tracing::{debug, error, warn};

const CONFIG_DIR_NAME: &str = "magnifico";

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Lock poisoned: {context}")]
    LockPoisoned { context: String },

    #[error("Failed to read file '{path}': {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Failed to write file '{path}': {source}")]
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Failed to parse JSON from '{path}': {source}")]
    ParseJson {
        path: PathBuf,
        source: serde_json::Error,
    },

    #[error("Failed to serialize to JSON: {0}")]
    SerializeJson(#[from] serde_json::Error),

    #[error("Failed to create config directory '{path}': {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub type StorageResult<T> = Result<T, StorageError>;

pub trait Enableable {
    fn is_enabled(&self) -> bool;
}

impl Enableable for BrowserRule {
    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Enableable for AppRule {
    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Enableable for CustomRule {
    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Enableable for SystemRule {
    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

pub struct Storage {
    config_dir: PathBuf,
    settings: RwLock<AppSettings>,
    browser_rules: RwLock<Vec<BrowserRule>>,
    app_rules: RwLock<Vec<AppRule>>,
    custom_rules: RwLock<Vec<CustomRule>>,
    system_rules: RwLock<Vec<SystemRule>>,
    /// Bumped only when a scan taken earlier no longer matches the rules or
    /// the settings that choose which files are cleaned.
    revision: AtomicU64,
}

impl Storage {
    /// Folder where the app keeps its settings and rules.
    pub fn data_dir() -> PathBuf {
        Self::resolve_config_dir(dirs::home_dir().as_deref(), Path::new("."))
    }

    pub fn new() -> Self {
        let config_dir = Self::data_dir();

        if !config_dir.exists() {
            if let Err(e) = fs::create_dir_all(&config_dir) {
                warn!("Failed to create config directory: {}", e);
            }
        }

        let storage = Self {
            config_dir: config_dir.clone(),
            settings: RwLock::new(AppSettings::default()),
            browser_rules: RwLock::new(BrowserRule::defaults()),
            app_rules: RwLock::new(AppRule::defaults()),
            custom_rules: RwLock::new(Vec::new()),
            system_rules: RwLock::new(SystemRule::defaults()),
            revision: AtomicU64::new(0),
        };

        storage.load_all_with_logging();
        storage
    }

    fn resolve_config_dir(home: Option<&Path>, fallback_base: &Path) -> PathBuf {
        match home {
            Some(home) => home.join(".local/share").join(CONFIG_DIR_NAME),
            None => {
                warn!("Home directory not available; falling back to current directory .magnifico");
                fallback_base.join(".magnifico")
            }
        }
    }

    fn load_all_with_logging(&self) {
        if let Err(e) = self.load_settings() {
            Self::log_load_fallback("settings", &e);
        }
        if let Err(e) = self.load_browser_rules() {
            Self::log_load_fallback("browser rules", &e);
        }
        if let Err(e) = self.load_app_rules() {
            Self::log_load_fallback("application rules", &e);
        }
        if let Err(e) = self.load_custom_rules() {
            Self::log_load_fallback("custom rules", &e);
        }
        if let Err(e) = self.load_system_rules() {
            Self::log_load_fallback("system rules", &e);
        }
    }

    fn log_load_fallback(context: &str, error: &StorageError) {
        if matches!(
            error,
            StorageError::ReadFile { source, .. }
                if source.kind() == std::io::ErrorKind::NotFound
        ) {
            debug!("No saved {context}; using defaults");
        } else {
            warn!("Could not load {context}; using defaults: {error}");
        }
    }

    fn load_into<T: DeserializeOwned>(
        &self,
        path: &PathBuf,
        lock: &RwLock<T>,
        context: &str,
    ) -> StorageResult<()> {
        let data = fs::read_to_string(path).map_err(|e| StorageError::ReadFile {
            path: path.clone(),
            source: e,
        })?;

        let parsed: T = match serde_json::from_str(&data) {
            Ok(parsed) => parsed,
            Err(source) => {
                let backup = path.with_extension(format!("json.corrupt-{}", uuid::Uuid::new_v4()));
                match fs::copy(path, &backup) {
                    Ok(_) => warn!(
                        "Preserved unreadable configuration {} as {}",
                        path.display(),
                        backup.display()
                    ),
                    Err(error) => warn!(
                        "Could not preserve unreadable configuration {}: {}",
                        path.display(),
                        error
                    ),
                }
                return Err(StorageError::ParseJson {
                    path: path.clone(),
                    source,
                });
            }
        };

        let mut guard = lock.write().map_err(|_| StorageError::LockPoisoned {
            context: context.to_string(),
        })?;
        *guard = parsed;

        debug!("Loaded {} from {:?}", context, path);
        Ok(())
    }

    fn save_from<T: Serialize>(
        &self,
        path: &PathBuf,
        lock: &RwLock<T>,
        context: &str,
    ) -> StorageResult<()> {
        let guard = lock.read().map_err(|_| StorageError::LockPoisoned {
            context: context.to_string(),
        })?;

        let data = serde_json::to_string_pretty(&*guard)?;
        drop(guard);

        let parent = path.parent().ok_or_else(|| StorageError::WriteFile {
            path: path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "configuration path has no parent",
            ),
        })?;
        fs::create_dir_all(parent).map_err(|e| StorageError::WriteFile {
            path: path.clone(),
            source: e,
        })?;

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config.json");
        let temporary_path = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

        let write_result = (|| -> std::io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary_path)?;
            file.write_all(data.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary_path, path)?;
            if let Ok(directory) = fs::File::open(parent) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();

        if let Err(source) = write_result {
            let _ = fs::remove_file(&temporary_path);
            return Err(StorageError::WriteFile {
                path: path.clone(),
                source,
            });
        }

        debug!("Saved {} to {:?}", context, path);
        Ok(())
    }

    fn get_clone<T: Clone>(&self, lock: &RwLock<T>, context: &str) -> StorageResult<T> {
        lock.read()
            .map(|guard| guard.clone())
            .map_err(|_| StorageError::LockPoisoned {
                context: context.to_string(),
            })
    }

    fn update_and_save<T: Serialize + Clone, F>(
        &self,
        lock: &RwLock<T>,
        path: &PathBuf,
        context: &str,
        updater: F,
        affects_scan: impl FnOnce(&T, &T) -> bool,
    ) -> StorageResult<()>
    where
        F: FnOnce(&mut T),
    {
        {
            let mut guard = lock.write().map_err(|_| StorageError::LockPoisoned {
                context: context.to_string(),
            })?;
            let before = guard.clone();
            updater(&mut guard);
            if affects_scan(&before, &guard) {
                self.revision.fetch_add(1, Ordering::SeqCst);
            }
        }
        // A failed write keeps the change in memory. The UI has already shown
        // it, so rolling back would make the app act on a choice the user
        // just undid (for example, cleaning cookies after switching them off).
        self.save_from(path, lock, context)
    }

    /// Revision of the rules and settings that decide what gets cleaned.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }

    fn count_enabled<T: Enableable>(&self, lock: &RwLock<Vec<T>>, context: &str) -> usize {
        lock.read()
            .map(|guard| guard.iter().filter(|r| r.is_enabled()).count())
            .unwrap_or_else(|e| {
                error!("Lock poisoned while counting {}: {}", context, e);
                0
            })
    }

    fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    fn load_settings(&self) -> StorageResult<()> {
        self.load_into(&self.settings_path(), &self.settings, "settings")
    }

    pub fn save_settings(&self) -> StorageResult<()> {
        self.save_from(&self.settings_path(), &self.settings, "settings")
    }

    pub fn get_settings(&self) -> AppSettings {
        let mut settings = self
            .get_clone(&self.settings, "settings")
            .unwrap_or_else(|e| {
                error!("Failed to read settings: {}", e);
                AppSettings::default()
            });
        settings.clamp_values();
        settings
    }

    pub fn update_settings<F>(&self, f: F) -> StorageResult<()>
    where
        F: FnOnce(&mut AppSettings),
    {
        self.update_and_save(
            &self.settings,
            &self.settings_path(),
            "settings",
            |settings| {
                f(settings);
                settings.clamp_values();
            },
            |before, after| {
                let mut before = before.clone();
                before.clamp_values();
                before.application_log_cleanup_enabled != after.application_log_cleanup_enabled
                    || before.system_journal_cleanup_enabled != after.system_journal_cleanup_enabled
                    || before.log_retention_days != after.log_retention_days
                    || before.inactive_projects_enabled != after.inactive_projects_enabled
                    || before.inactive_project_days != after.inactive_project_days
                    || before.pin_recent_days != after.pin_recent_days
                    || before.project_folders != after.project_folders
            },
        )
    }

    fn browser_rules_path(&self) -> PathBuf {
        self.config_dir.join("browser_rules.json")
    }

    fn load_browser_rules(&self) -> StorageResult<()> {
        self.load_into(
            &self.browser_rules_path(),
            &self.browser_rules,
            "browser_rules",
        )?;

        // Reconcile saved rules with the current browser/data-type catalog so
        // upgrades gain new cleanup choices without discarding prior choices.
        // Rules for browsers no longer detected are switched off.
        let installed: std::collections::HashSet<BrowserType> = BrowserType::all()
            .iter()
            .copied()
            .filter(BrowserType::is_installed)
            .collect();
        let defaults = BrowserRule::defaults();
        let mut changed = false;

        {
            let mut rules = self
                .browser_rules
                .write()
                .map_err(|_| StorageError::LockPoisoned {
                    context: "browser_rules".to_string(),
                })?;

            for rule in rules.iter_mut() {
                if rule.enabled && !installed.contains(&rule.browser) {
                    rule.enabled = false;
                    changed = true;
                }
            }

            for default_rule in defaults {
                let exists = rules.iter().any(|rule| {
                    rule.browser == default_rule.browser && rule.data_type == default_rule.data_type
                });
                if !exists {
                    rules.push(default_rule);
                    changed = true;
                }
            }
        }

        if changed {
            self.save_browser_rules()?;
        }

        Ok(())
    }

    pub fn save_browser_rules(&self) -> StorageResult<()> {
        self.save_from(
            &self.browser_rules_path(),
            &self.browser_rules,
            "browser_rules",
        )
    }

    pub fn get_browser_rules(&self) -> Vec<BrowserRule> {
        self.get_clone(&self.browser_rules, "browser_rules")
            .unwrap_or_else(|e| {
                error!("Failed to read browser rules: {}", e);
                BrowserRule::defaults()
            })
    }

    pub fn update_browser_rules<F>(&self, f: F) -> StorageResult<()>
    where
        F: FnOnce(&mut Vec<BrowserRule>),
    {
        self.update_and_save(
            &self.browser_rules,
            &self.browser_rules_path(),
            "browser_rules",
            f,
            PartialEq::ne,
        )
    }

    fn app_rules_path(&self) -> PathBuf {
        self.config_dir.join("app_rules.json")
    }

    fn load_app_rules(&self) -> StorageResult<()> {
        self.load_into(&self.app_rules_path(), &self.app_rules, "app_rules")?;
        // Built-in rules added in a newer version show up switched off.
        if let Ok(mut rules) = self.app_rules.write() {
            for rule in AppRule::defaults() {
                if !rules.iter().any(|r| r.builtin && r.path == rule.path) {
                    rules.push(rule);
                }
            }
        }
        Ok(())
    }

    pub fn save_app_rules(&self) -> StorageResult<()> {
        self.save_from(&self.app_rules_path(), &self.app_rules, "app_rules")
    }

    pub fn get_app_rules(&self) -> Vec<AppRule> {
        self.get_clone(&self.app_rules, "app_rules")
            .unwrap_or_else(|e| {
                error!("Failed to read app rules: {}", e);
                AppRule::defaults()
            })
    }

    pub fn update_app_rules<F>(&self, f: F) -> StorageResult<()>
    where
        F: FnOnce(&mut Vec<AppRule>),
    {
        self.update_and_save(
            &self.app_rules,
            &self.app_rules_path(),
            "app_rules",
            f,
            PartialEq::ne,
        )
    }

    fn custom_rules_path(&self) -> PathBuf {
        self.config_dir.join("custom_rules.json")
    }

    fn load_custom_rules(&self) -> StorageResult<()> {
        self.load_into(
            &self.custom_rules_path(),
            &self.custom_rules,
            "custom_rules",
        )
    }

    pub fn save_custom_rules(&self) -> StorageResult<()> {
        self.save_from(
            &self.custom_rules_path(),
            &self.custom_rules,
            "custom_rules",
        )
    }

    pub fn get_custom_rules(&self) -> Vec<CustomRule> {
        self.get_clone(&self.custom_rules, "custom_rules")
            .unwrap_or_else(|e| {
                error!("Failed to read custom rules: {}", e);
                Vec::new()
            })
    }

    pub fn update_custom_rules<F>(&self, f: F) -> StorageResult<()>
    where
        F: FnOnce(&mut Vec<CustomRule>),
    {
        self.update_and_save(
            &self.custom_rules,
            &self.custom_rules_path(),
            "custom_rules",
            f,
            PartialEq::ne,
        )
    }

    fn system_rules_path(&self) -> PathBuf {
        self.config_dir.join("system_rules.json")
    }

    fn load_system_rules(&self) -> StorageResult<()> {
        self.load_into(
            &self.system_rules_path(),
            &self.system_rules,
            "system_rules",
        )
    }

    pub fn save_system_rules(&self) -> StorageResult<()> {
        self.save_from(
            &self.system_rules_path(),
            &self.system_rules,
            "system_rules",
        )
    }

    pub fn get_system_rules(&self) -> Vec<SystemRule> {
        self.get_clone(&self.system_rules, "system_rules")
            .unwrap_or_else(|e| {
                error!("Failed to read system rules: {}", e);
                SystemRule::defaults()
            })
    }

    pub fn update_system_rules<F>(&self, f: F) -> StorageResult<()>
    where
        F: FnOnce(&mut Vec<SystemRule>),
    {
        self.update_and_save(
            &self.system_rules,
            &self.system_rules_path(),
            "system_rules",
            f,
            PartialEq::ne,
        )
    }

    pub fn count_enabled_rules(&self) -> usize {
        let settings = self.get_settings();
        self.count_enabled(&self.browser_rules, "browser_rules")
            + self.count_enabled(&self.app_rules, "app_rules")
            + self.count_enabled(&self.custom_rules, "custom_rules")
            + self.count_enabled(&self.system_rules, "system_rules")
            + usize::from(settings.application_log_cleanup_enabled)
            + usize::from(settings.system_journal_cleanup_enabled)
    }

    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn create_test_storage() -> (Storage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = Storage {
            config_dir: temp_dir.path().to_path_buf(),
            settings: RwLock::new(AppSettings::default()),
            browser_rules: RwLock::new(Vec::new()),
            app_rules: RwLock::new(Vec::new()),
            custom_rules: RwLock::new(Vec::new()),
            system_rules: RwLock::new(Vec::new()),
            revision: AtomicU64::new(0),
        };
        (storage, temp_dir)
    }

    #[test]
    fn failed_save_keeps_the_change_and_bumps_revision() {
        let dir = TempDir::new().unwrap();
        // A file where the config directory should be makes every write fail.
        let blocked = dir.path().join("not-a-directory");
        fs::write(&blocked, b"").unwrap();
        let storage = Storage {
            config_dir: blocked,
            settings: RwLock::new(AppSettings::default()),
            browser_rules: RwLock::new(Vec::new()),
            app_rules: RwLock::new(Vec::new()),
            custom_rules: RwLock::new(Vec::new()),
            system_rules: RwLock::new(Vec::new()),
            revision: AtomicU64::new(0),
        };
        let before = storage.get_settings().application_log_cleanup_enabled;

        assert!(storage
            .update_settings(|s| s.application_log_cleanup_enabled = !before)
            .is_err());
        assert_eq!(
            storage.get_settings().application_log_cleanup_enabled,
            !before
        );
        assert_eq!(storage.revision(), 1);
    }

    #[test]
    fn cleanup_bookkeeping_and_presentation_do_not_invalidate_scans() {
        let (storage, _dir) = create_test_storage();
        storage
            .update_settings(|s| {
                s.last_cleanup = Some(chrono::Utc::now());
                s.last_scheduled_cleanup = Some(chrono::Utc::now());
                s.window_size = Some((1000, 700));
                s.window_blur = !s.window_blur;
                s.color_scheme = crate::models::ColorScheme::Dark;
                s.language = crate::models::AppLanguage::Spanish;
                s.verbose_logging = true;
                s.scheduled_cleanup_enabled = true;
                s.mark_schedule_changed();
                s.max_files_per_operation = 5000;
                s.max_size_per_operation = 1024 * 1024;
                s.removal_method = crate::models::RemovalMethod::Trash;
                s.secure_browser_data = false;
            })
            .unwrap();
        assert_eq!(storage.revision(), 0);
        let saved: AppSettings =
            serde_json::from_str(&fs::read_to_string(storage.settings_path()).unwrap()).unwrap();
        assert!(saved.last_cleanup.is_some());
        assert!(saved.last_scheduled_cleanup.is_some());
        assert_eq!(saved.window_size, Some((1000, 700)));
    }

    #[test]
    fn only_changed_scan_settings_invalidate_results() {
        let changes: [fn(&mut AppSettings); 7] = [
            |s| s.application_log_cleanup_enabled = !s.application_log_cleanup_enabled,
            |s| s.system_journal_cleanup_enabled = !s.system_journal_cleanup_enabled,
            |s| s.log_retention_days += 1,
            |s| s.inactive_projects_enabled = !s.inactive_projects_enabled,
            |s| s.inactive_project_days += 1,
            |s| s.pin_recent_days += 1,
            |s| {
                s.project_folders
                    .push(PathBuf::from("/tmp/another-project-root"))
            },
        ];
        let (storage, _dir) = create_test_storage();
        for (index, change) in changes.into_iter().enumerate() {
            storage.update_settings(change).unwrap();
            assert_eq!(storage.revision(), index as u64 + 1);
            storage.update_settings(|_| {}).unwrap();
            assert_eq!(storage.revision(), index as u64 + 1);
        }
    }

    #[test]
    fn recording_cleanup_does_not_invalidate_clamped_legacy_settings() {
        let (storage, _dir) = create_test_storage();
        {
            let mut settings = storage.settings.write().unwrap();
            settings.log_retention_days = 0;
            settings.pin_recent_days = 0;
            settings.inactive_project_days = 0;
        }
        storage
            .update_settings(|s| s.last_cleanup = Some(chrono::Utc::now()))
            .unwrap();
        assert_eq!(storage.revision(), 0);
        assert_eq!(storage.get_settings().log_retention_days, 1);
    }

    #[test]
    fn unchanged_rules_do_not_invalidate_results() {
        let (storage, _dir) = create_test_storage();
        storage.update_browser_rules(|_| {}).unwrap();
        storage.update_app_rules(|_| {}).unwrap();
        storage.update_custom_rules(|_| {}).unwrap();
        storage.update_system_rules(|_| {}).unwrap();
        assert_eq!(storage.revision(), 0);
        storage
            .update_system_rules(|rules| rules.push(SystemRule::defaults().remove(0)))
            .unwrap();
        assert_eq!(storage.revision(), 1);
        storage.update_system_rules(|_| {}).unwrap();
        assert_eq!(storage.revision(), 1);
        storage
            .update_system_rules(|rules| rules[0].enabled = !rules[0].enabled)
            .unwrap();
        assert_eq!(storage.revision(), 2);
    }

    #[test]
    fn test_settings_roundtrip() {
        let (storage, _dir) = create_test_storage();

        storage
            .update_settings(|s| {
                s.max_files_per_operation = 5000;
            })
            .unwrap();

        let settings = storage.get_settings();
        assert_eq!(settings.max_files_per_operation, 5000);
        let entries: Vec<_> = fs::read_dir(storage.config_dir())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(entries
            .iter()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn corrupt_configuration_is_preserved_for_recovery() {
        let (storage, _dir) = create_test_storage();
        fs::write(storage.settings_path(), "{truncated").unwrap();

        assert!(storage.load_settings().is_err());
        let backups: Vec<_> = fs::read_dir(storage.config_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.corrupt-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), "{truncated");
    }

    #[test]
    fn test_custom_rules_crud() {
        let (storage, _dir) = create_test_storage();

        storage
            .update_custom_rules(|rules| {
                rules.push(CustomRule::new("Test Rule", "/tmp/test"));
            })
            .unwrap();

        let rules = storage.get_custom_rules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "Test Rule");

        let rule_id = rules[0].id;
        storage
            .update_custom_rules(|rules| {
                if let Some(rule) = rules.iter_mut().find(|r| r.id == rule_id) {
                    rule.enabled = true;
                }
            })
            .unwrap();

        assert_eq!(storage.count_enabled_rules(), 1);

        storage
            .update_custom_rules(|rules| {
                rules.retain(|r| r.id != rule_id);
            })
            .unwrap();

        assert!(storage.get_custom_rules().is_empty());
    }

    #[test]
    fn test_load_from_disk() {
        let (storage, dir) = create_test_storage();

        storage
            .update_settings(|s| {
                s.verbose_logging = true;
            })
            .unwrap();

        let storage2 = Storage {
            config_dir: dir.path().to_path_buf(),
            settings: RwLock::new(AppSettings::default()),
            browser_rules: RwLock::new(Vec::new()),
            app_rules: RwLock::new(Vec::new()),
            custom_rules: RwLock::new(Vec::new()),
            system_rules: RwLock::new(Vec::new()),
            revision: AtomicU64::new(0),
        };
        storage2.load_all_with_logging();

        let settings = storage2.get_settings();
        assert!(settings.verbose_logging);
    }

    #[test]
    fn test_count_enabled_rules() {
        let (storage, _dir) = create_test_storage();

        storage
            .update_browser_rules(|rules| {
                rules.push(BrowserRule::defaults().into_iter().next().unwrap());
                if let Some(r) = rules.first_mut() {
                    r.enabled = true;
                }
            })
            .unwrap();

        storage
            .update_app_rules(|rules| {
                rules.push(AppRule::defaults().into_iter().next().unwrap());
                if let Some(r) = rules.first_mut() {
                    r.enabled = true;
                }
            })
            .unwrap();

        assert_eq!(storage.count_enabled_rules(), 2);
    }

    #[test]
    fn test_concurrent_access() {
        let (storage, _dir) = create_test_storage();
        let storage = Arc::new(storage);

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let storage = Arc::clone(&storage);
                std::thread::spawn(move || {
                    storage
                        .update_custom_rules(|rules| {
                            rules.push(CustomRule::new(format!("Rule {}", i), "/tmp"));
                        })
                        .unwrap();
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(storage.get_custom_rules().len(), 10);
    }

    #[test]
    fn config_dir_lives_under_the_home_data_folder() {
        let home = Path::new("/home/user");
        assert_eq!(
            Storage::resolve_config_dir(Some(home), home),
            PathBuf::from("/home/user/.local/share/magnifico")
        );
        assert_eq!(
            Storage::resolve_config_dir(None, Path::new("/work")),
            PathBuf::from("/work/.magnifico")
        );
    }
}
