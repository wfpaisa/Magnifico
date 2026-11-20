mod app_rule;
mod browser_rule;
mod cleanup_plan;
mod custom_rule;
pub mod explanations;
mod scan_category;
mod scan_result;
mod settings;
mod system_rule;

pub use app_rule::AppRule;
pub use browser_rule::{BrowserDataType, BrowserRule, BrowserType};
pub use cleanup_plan::{CleanupPlan, GroupKind, Impact, PlanGroup, PlanItem, Tick};
pub use custom_rule::{CustomRule, DeletionMode};
pub use scan_category::{Safety, ScanCategory};
pub use scan_result::{CleanError, CleanResult, FileEntry, ScanResult, TrashedFile};
pub use settings::{AppLanguage, AppSettings, ColorScheme, RemovalMethod, ScheduleDay};
pub use system_rule::{SystemRule, SystemRuleType};
