use super::clean_progress::CleanProgressWindow;
use super::dialogs;
use super::scan_overview::{CategoryResults, ScanOverview};
use crate::models::{ScanCategory, ScanResult};
use crate::services::{
    vacuum_system_journal, CleanOptions, CleanProgress, Cleaner, ScanOptions, Scanner,
};
use crate::storage::Storage;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use sysinfo::Disks;

struct DiskUsage {
    total: u64,
    available: u64,
    device_name: String,
    mount_point: PathBuf,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct DashboardPage {
        pub storage: RefCell<Option<Arc<Storage>>>,
        pub space_label: RefCell<Option<gtk4::Label>>,
        pub disk_summary_label: RefCell<Option<gtk4::Label>>,
        pub disk_context_label: RefCell<Option<gtk4::Label>>,
        pub disk_available_label: RefCell<Option<gtk4::Label>>,
        pub disk_percent_label: RefCell<Option<gtk4::Label>>,
        pub targets_label: RefCell<Option<gtk4::Label>>,
        pub last_clean_label: RefCell<Option<gtk4::Label>>,
        pub scan_button: RefCell<Option<gtk4::Button>>,
        pub scan_result: RefCell<Option<ScanResult>>,
        /// Storage revision the visible plan was built at.
        pub scan_revision: Cell<u64>,
        /// Storage revision of the last rule scan (tray/scheduled cleanups), kept
        /// apart so it never validates an older plan.
        pub rule_scan_revision: Cell<u64>,
        pub donut_area: RefCell<Option<gtk4::DrawingArea>>,
        pub reclaimable_bytes: RefCell<u64>,
        pub total_disk_bytes: RefCell<u64>,
        pub available_disk_bytes: RefCell<u64>,
        pub has_scanned: RefCell<bool>,
        pub operation_running: RefCell<Option<Arc<AtomicBool>>>,
        pub operation_cancel: RefCell<Option<Arc<AtomicBool>>>,
        pub disk_refresh_running: Cell<bool>,
        pub category_results: RefCell<Vec<(ScanCategory, ScanResult)>>,
        pub overview: RefCell<Option<super::ScanOverview>>,
        pub auto_scan_done: Cell<bool>,
        /// Everything the last scan found, before the user unticked anything.
        pub found_categories: RefCell<Vec<(ScanCategory, ScanResult)>>,
        /// Paths the user unticked in a category's details.
        pub excluded: RefCell<std::collections::HashSet<std::path::PathBuf>>,
        pub view_stack: RefCell<Option<gtk4::Stack>>,
        /// The cleanup screen: disk bar, goal, mosaic and details.
        pub cleanup_view: RefCell<Option<crate::ui::CleanupView>>,
        /// The user accepted going over a safety limit for the countdown
        /// now running; consumed by the cleanup it starts.
        pub limit_approved: Cell<bool>,
        /// A plan from an interactive scan is on screen.
        pub has_plan: Cell<bool>,
        /// Start the cleanup countdown once the next plan scan finishes.
        pub countdown_after_plan: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DashboardPage {
        const NAME: &'static str = "MagnificoDashboardPage";
        type Type = super::DashboardPage;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for DashboardPage {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl WidgetImpl for DashboardPage {}
    impl BoxImpl for DashboardPage {}
}

glib::wrapper! {
    pub struct DashboardPage(ObjectSubclass<imp::DashboardPage>)
        @extends gtk4::Widget, gtk4::Box;
}

impl DashboardPage {
    pub fn new(storage: Arc<Storage>) -> Self {
        let page: Self = glib::Object::builder()
            .property("orientation", gtk4::Orientation::Vertical)
            .property("spacing", 0)
            .build();

        page.imp().storage.replace(Some(storage));
        page.imp()
            .operation_running
            .replace(Some(Arc::new(AtomicBool::new(false))));
        page.setup_ui();
        page.refresh();
        page
    }

    fn setup_ui(&self) {
        self.set_margin_top(0);
        self.set_margin_bottom(24);
        self.set_margin_start(24);
        self.set_margin_end(24);

        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        content.set_margin_top(0);

        let page_weak = self.downgrade();
        let cleanup_view = crate::ui::CleanupView::new(move |private, bulk| {
            if let Some(page) = page_weak.upgrade() {
                page.execute_plan_clean(private, bulk);
            }
        });
        let page_weak = self.downgrade();
        cleanup_view.set_confirm(move |private, bulk, proceed| {
            if let Some(page) = page_weak.upgrade() {
                page.confirm_plan_clean(&private, &bulk, proceed);
            }
        });
        let scan_button = cleanup_view.scan_button().clone();
        let page = self.downgrade();
        scan_button.connect_clicked(move |button| {
            if let Some(page) = page.upgrade() {
                if page.operation_is_running() {
                    page.cancel_operation(button);
                } else {
                    page.on_scan_clicked();
                }
            }
        });
        self.imp().scan_button.replace(Some(scan_button));
        content.append(cleanup_view.widget());
        // The trash button floats in the bottom-right corner over the
        // scrolled content, so it stays in reach however far the list is
        // scrolled.
        let clean_button = cleanup_view.clean_button().clone();
        clean_button.set_halign(gtk4::Align::End);
        clean_button.set_valign(gtk4::Align::End);
        clean_button.set_margin_bottom(4);
        clean_button.set_margin_end(4);
        self.imp().cleanup_view.replace(Some(cleanup_view));

        // Scan on its own the first time the dashboard is shown, so the
        // user sees what can be freed without pressing anything.
        self.connect_map(|page| {
            if page.imp().auto_scan_done.get() {
                return;
            }
            let page = page.downgrade();
            glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                if let Some(page) = page.upgrade() {
                    if page.imp().auto_scan_done.replace(true) {
                        return;
                    }
                    let has_rules = page
                        .imp()
                        .storage
                        .borrow()
                        .as_ref()
                        .is_some_and(|s| s.count_enabled_rules() > 0);
                    if has_rules
                        && !page.imp().has_plan.get()
                        && !*page.imp().has_scanned.borrow()
                        && page.imp().scan_result.borrow().is_none()
                        && !page.operation_is_running()
                    {
                        page.start_scan(false, true);
                    }
                }
            });
        });

        scrolled.set_child(Some(&content));
        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&scrolled));
        overlay.add_overlay(&clean_button);
        let stack = gtk4::Stack::new();
        stack.set_vexpand(true);
        stack.set_transition_type(gtk4::StackTransitionType::SlideLeftRight);
        stack.add_named(&overlay, Some("home"));
        self.append(&stack);
        self.imp().view_stack.replace(Some(stack));
    }

    /// True when the shown results were taken before the rules last changed.
    fn results_are_stale(&self) -> bool {
        let revision = self
            .imp()
            .storage
            .borrow()
            .as_ref()
            .map(|s| s.revision())
            .unwrap_or_default();
        if self.imp().has_plan.get() {
            self.imp().scan_revision.get() != revision
        } else {
            self.imp().scan_result.borrow().is_some()
                && self.imp().rule_scan_revision.get() != revision
        }
    }

    fn update_scan_button(&self) {
        let Some(button) = self.imp().scan_button.borrow().clone() else {
            return;
        };
        let running = self.operation_is_running();
        let external_busy = self
            .root()
            .and_then(|root| root.downcast::<gtk4::Window>().ok())
            .and_then(|window| window.downcast::<crate::ui::MainWindow>().ok())
            .is_some_and(|window| {
                window.storage_analyzer_operation_is_running()
                    || window.system_operation_is_running()
            });
        let can_cancel = self
            .imp()
            .operation_cancel
            .borrow()
            .as_ref()
            .is_some_and(|cancel| !cancel.load(Ordering::Relaxed));
        button.set_sensitive(!external_busy && (!running || can_cancel));
        let stale = self.results_are_stale() && !running;
        if stale {
            button.add_css_class("scan-stale");
        } else {
            button.remove_css_class("scan-stale");
        }
        button.set_child(Some(&crate::ui::icon_image(if running {
            "pc-process-stop-symbolic"
        } else {
            "ui/reload"
        })));
        button.set_tooltip_text(Some(&crate::i18n::tr(if running {
            "Cancel Operation"
        } else if stale {
            "Rules changed — scan again"
        } else {
            "Scan for Files (Ctrl+Shift+R)"
        })));
        if let Some(view) = self.imp().cleanup_view.borrow().as_ref() {
            view.set_enabled(!running && !stale && !external_busy);
        }
    }

    fn close_category(&self) {
        if let Some(stack) = self.imp().view_stack.borrow().as_ref() {
            stack.set_visible_child_name("home");
        }
    }

    /// Rebuild what will be cleaned from the last scan minus unticked paths.
    fn apply_exclusions(&self) {
        let excluded = self.imp().excluded.borrow().clone();
        let filtered: CategoryResults = self
            .imp()
            .found_categories
            .borrow()
            .iter()
            .map(|(c, r)| (*c, super::category_detail::without_excluded(r, &excluded)))
            .collect();
        let mut result = ScanResult::new();
        for (_, r) in &filtered {
            result.merge(r.clone());
        }
        if let Some(overview) = self.imp().overview.borrow().as_ref() {
            overview.set_results(&filtered);
        }
        self.imp().scan_result.replace(Some(result));
        self.imp().category_results.replace(filtered);
    }

    fn get_home_disk_usage() -> Option<DiskUsage> {
        let disks = Disks::new_with_refreshed_list();
        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

        // The longest matching mount path is the filesystem that actually owns
        // the home directory when /home is on a separate partition.
        let disk = disks
            .list()
            .iter()
            .filter(|disk| home_dir.starts_with(disk.mount_point()))
            .max_by_key(|disk| disk.mount_point().components().count())
            .or_else(|| disks.list().first())?;

        Some(DiskUsage {
            total: disk.total_space(),
            available: disk.available_space().min(disk.total_space()),
            device_name: disk.name().to_string_lossy().into_owned(),
            mount_point: disk.mount_point().to_path_buf(),
        })
    }

    /// Scan/cancel button, placed by the window in its header bar.
    pub fn scan_button(&self) -> Option<gtk4::Button> {
        self.imp().scan_button.borrow().clone()
    }

    pub fn refresh_disk_usage(&self) {
        if let Some(donut) = self.imp().donut_area.borrow().as_ref() {
            donut.queue_draw();
        }
        if self.imp().disk_refresh_running.replace(true) {
            return;
        }

        let (sender, receiver) = async_channel::bounded(1);
        crate::runtime().spawn_blocking(move || {
            let _ = sender.send_blocking(Self::get_home_disk_usage());
        });

        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let usage = receiver.recv().await.ok().flatten();
            if let Some(page) = page.upgrade() {
                page.imp().disk_refresh_running.set(false);
                page.apply_disk_usage(usage);
            }
        });
    }

    fn apply_disk_usage(&self, usage: Option<DiskUsage>) {
        let (total, available) = usage
            .as_ref()
            .map(|usage| (usage.total, usage.available))
            .unwrap_or((0, 0));
        let used = total.saturating_sub(available);

        self.imp().total_disk_bytes.replace(total);
        self.imp().available_disk_bytes.replace(available);
        if let Some(view) = self.imp().cleanup_view.borrow().as_ref() {
            view.set_disk(total, used);
        }

        if let Some(label) = self.imp().disk_percent_label.borrow().as_ref() {
            let percentage = if total > 0 {
                used as f64 / total as f64 * 100.0
            } else {
                0.0
            };
            let text = if total > 0 {
                format!("{percentage:.0}%")
            } else {
                "--".to_string()
            };
            label.set_text(&text);
        }

        if let Some(label) = self.imp().disk_summary_label.borrow().as_ref() {
            let text = if total > 0 {
                let used = bytesize::ByteSize(used).to_string();
                crate::i18n::tr_args("{used} used", &[("{used}", &used)])
            } else {
                crate::i18n::tr("Disk information unavailable")
            };
            label.set_text(&text);
        }

        if let Some(label) = self.imp().disk_context_label.borrow().as_ref() {
            let context = usage.as_ref().map(|usage| {
                let total = bytesize::ByteSize(usage.total).to_string();
                let mount = usage.mount_point.display().to_string();
                crate::i18n::tr_args(
                    "of {total} total on {device} (mounted at {mount})",
                    &[
                        ("{total}", &total),
                        ("{device}", &usage.device_name),
                        ("{mount}", &mount),
                    ],
                )
            });
            label.set_text(
                context
                    .as_deref()
                    .unwrap_or(&crate::i18n::tr("Home filesystem could not be detected")),
            );
        }

        if let Some(label) = self.imp().disk_available_label.borrow().as_ref() {
            let text = if total > 0 {
                bytesize::ByteSize(available).to_string()
            } else {
                "--".to_string()
            };
            label.set_text(&text);
        }

        if let Some(donut) = self.imp().donut_area.borrow().as_ref() {
            if total > 0 {
                let used = bytesize::ByteSize(used).to_string();
                let available = bytesize::ByteSize(available).to_string();
                donut.set_tooltip_text(Some(&crate::i18n::tr_args(
                    "{used} used, {available} available",
                    &[("{used}", &used), ("{available}", &available)],
                )));
            }
            donut.queue_draw();
        }
    }

    pub fn refresh(&self) {
        let storage = self.imp().storage.borrow();
        let storage = storage.as_ref().unwrap();

        // Navigating only updates the figures. Results and selections stay
        // until the user asks for another scan.
        self.update_scan_button();

        // Update enabled targets count
        let enabled = storage.count_enabled_rules();
        if let Some(label) = self.imp().targets_label.borrow().as_ref() {
            label.set_text(&enabled.to_string());
        }

        // Update last cleanup time
        let settings = storage.get_settings();
        if let Some(label) = self.imp().last_clean_label.borrow().as_ref() {
            let text = if let Some(time) = settings.last_cleanup {
                time.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            } else {
                crate::i18n::tr("Never")
            };
            label.set_text(&text);
        }

        // Only reset the scan-result visuals when there is no pending scan to
        // display. Otherwise refreshing on navigation would erase the user's
        // last scan output even though the underlying data is still around.
        if self.imp().scan_result.borrow().is_none() && !self.imp().has_plan.get() {
            if let Some(view) = self.imp().cleanup_view.borrow().as_ref() {
                if !view.is_scanning() {
                    view.show_status(&crate::i18n::tr("Scan to see what can be cleaned"), false);
                }
            }
            if let Some(label) = self.imp().space_label.borrow().as_ref() {
                label.set_text(&crate::i18n::tr("Scan to estimate"));
            }
            self.imp().category_results.borrow_mut().clear();
            self.imp().found_categories.borrow_mut().clear();
            if let Some(overview) = self.imp().overview.borrow().as_ref() {
                overview.show_placeholder(&crate::i18n::tr("Scan to see what can be cleaned"));
            }
            self.imp().reclaimable_bytes.replace(0);
            self.imp().has_scanned.replace(false);
        }

        self.refresh_disk_usage();
    }

    fn on_scan_clicked(&self) {
        self.start_scan(false, false);
    }

    fn cancel_operation(&self, button: &gtk4::Button) {
        if let Some(cancel) = self.imp().operation_cancel.borrow().as_ref() {
            cancel.store(true, Ordering::Relaxed);
            button.set_tooltip_text(Some(&crate::i18n::tr("Cancelling…")));
            button.set_sensitive(false);
        }
    }

    fn set_operation_controls(&self, running: bool) {
        if !running {
            // A scan that ended without results must not leave the animation on.
            if let Some(overview) = self.imp().overview.borrow().as_ref() {
                if overview.is_scanning() {
                    overview.show_placeholder(&crate::i18n::tr("Scan to see what can be cleaned"));
                }
            }
        }
        if let Some(view) = self.imp().cleanup_view.borrow().as_ref() {
            if !running && view.is_scanning() {
                view.show_status(&crate::i18n::tr("Scan to see what can be cleaned"), false);
            }
        }
        if let Some(button) = self.imp().scan_button.borrow().as_ref() {
            button.set_sensitive(true);
        }
        self.update_scan_button();
    }

    fn start_scan(&self, clean_after_scan: bool, automatic: bool) -> bool {
        if self
            .imp()
            .cleanup_view
            .borrow()
            .as_ref()
            .is_some_and(|view| view.is_launching())
        {
            return false;
        }
        if let Some(window) = self
            .root()
            .and_then(|root| root.downcast::<gtk4::Window>().ok())
            .and_then(|window| window.downcast::<crate::ui::MainWindow>().ok())
        {
            if window.storage_analyzer_operation_is_running()
                || window.system_operation_is_running()
            {
                if !automatic {
                    self.show_info_dialog(
                        "Operation in Progress",
                        "Cancel the active scan or wait for the administrator operation to finish before cleaning.",
                    );
                }
                return false;
            }
        }

        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let settings = storage.get_settings();

        // Guard against concurrent operations
        let Some(running_flag) = self.imp().operation_running.borrow().as_ref().cloned() else {
            return false;
        };
        if running_flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            if !automatic {
                self.show_info_dialog(
                    "Operation in Progress",
                    "A scan or cleanup is already running. Please wait for it to complete.",
                );
            }
            return false;
        }

        // A scan the user started (or the first one on opening) builds the
        // cleanup screen. Tray and scheduled cleanups keep the rule scan.
        self.imp().auto_scan_done.set(true);
        if !clean_after_scan {
            return self.start_plan_scan(running_flag, storage);
        }

        // Update UI
        if let Some(label) = self.imp().space_label.borrow().as_ref() {
            label.set_text(&crate::i18n::tr("Scanning..."));
        }
        self.close_category();
        if let Some(overview) = self.imp().overview.borrow().as_ref() {
            overview.show_scanning();
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        self.imp()
            .operation_cancel
            .replace(Some(cancellation.clone()));
        self.set_operation_controls(true);

        // Run scan in background
        let (sender, receiver) = async_channel::bounded::<CategoryResults>(1);

        let revision = storage.revision();
        let browser_rules = storage.get_browser_rules();
        let (dev_tool_rules, other_app_rules): (Vec<_>, Vec<_>) = storage
            .get_app_rules()
            .into_iter()
            .partition(|rule| rule.is_developer_tool());
        let custom_rules = storage.get_custom_rules();
        let system_rules = storage.get_system_rules();
        let scan_options = ScanOptions {
            log_retention_days: settings.log_retention_days,
        };
        let scan_application_logs = settings.application_log_cleanup_enabled;
        // Never in a scheduled cleanup: project folders are only removed
        // after the user has seen them.
        let project_scan = (settings.inactive_projects_enabled && !automatic).then(|| {
            (
                settings.project_folders.clone(),
                settings.inactive_project_days,
            )
        });

        crate::runtime().spawn_blocking(move || {
            let scanner = Scanner::with_options_and_cancellation(scan_options, cancellation);
            // Same order as before, so a path claimed by two rules is
            // counted in the first category only.
            let mut categories: CategoryResults = vec![
                (
                    ScanCategory::Browsers,
                    scanner.scan_browser_rules(&browser_rules),
                ),
                (
                    ScanCategory::Applications,
                    scanner.scan_app_rules(&other_app_rules),
                ),
                (
                    ScanCategory::DevTools,
                    scanner.scan_app_rules(&dev_tool_rules),
                ),
                (
                    ScanCategory::Custom,
                    scanner.scan_custom_rules(&custom_rules),
                ),
                (
                    ScanCategory::System,
                    scanner.scan_system_rules(&system_rules),
                ),
            ];
            if scan_application_logs {
                categories.push((ScanCategory::Logs, scanner.scan_application_logs()));
            }
            if let Some((folders, days)) = project_scan {
                categories.push((
                    ScanCategory::Projects,
                    scanner.scan_inactive_projects(&folders, days),
                ));
            }
            let _ = sender.send_blocking(dedupe_categories(categories));
        });

        // Handle result
        let page = self.clone();
        glib::spawn_future_local(async move {
            let mut continue_to_clean = false;
            if let Ok(categories) = receiver.recv().await {
                let mut result = ScanResult::new();
                for (_, category_result) in &categories {
                    result.merge(category_result.clone());
                }
                if result.cancelled {
                    page.imp().category_results.borrow_mut().clear();
                    page.imp().found_categories.borrow_mut().clear();
                    if let Some(overview) = page.imp().overview.borrow().as_ref() {
                        overview.show_placeholder(&crate::i18n::tr("Scan cancelled"));
                    }
                    if let Some(label) = page.imp().space_label.borrow().as_ref() {
                        label.set_text("Scan cancelled");
                    }
                    page.imp().scan_result.replace(None);
                    page.imp().reclaimable_bytes.replace(0);
                    page.imp().has_scanned.replace(false);
                } else {
                    // Update the label with formatted size
                    if let Some(label) = page.imp().space_label.borrow().as_ref() {
                        label.set_text(&result.formatted_size());
                    }

                    // Update donut chart
                    page.imp().reclaimable_bytes.replace(result.total_size);
                    page.imp().has_scanned.replace(true);
                    page.refresh_disk_usage();

                    page.imp().rule_scan_revision.set(revision);
                    page.imp().found_categories.replace(categories);
                    page.imp().excluded.borrow_mut().clear();
                    page.close_category();
                    page.apply_exclusions();
                    page.update_scan_button();

                    continue_to_clean = clean_after_scan;
                }
            }
            page.imp().operation_cancel.replace(None);
            running_flag.store(false, Ordering::SeqCst);
            page.set_operation_controls(false);
            if continue_to_clean {
                if automatic {
                    page.on_automatic_clean_ready();
                } else {
                    page.on_clean_clicked();
                }
            }
        });

        true
    }

    /// Scan everything the cleanup screen offers, item by item.
    fn start_plan_scan(&self, running_flag: Arc<AtomicBool>, storage: Arc<Storage>) -> bool {
        let settings = storage.get_settings();
        self.close_category();
        if let Some(view) = self.imp().cleanup_view.borrow().as_ref() {
            view.show_scanning();
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        self.imp()
            .operation_cancel
            .replace(Some(cancellation.clone()));
        self.set_operation_controls(true);

        let revision = storage.revision();
        let inputs = crate::services::PlanInputs {
            system_rules: storage.get_system_rules(),
            app_rules: storage.get_app_rules(),
            browser_rules: storage.get_browser_rules(),
            custom_rules: storage.get_custom_rules(),
            scan_logs: settings.application_log_cleanup_enabled,
            project_folders: settings
                .inactive_projects_enabled
                .then(|| settings.project_folders.clone()),
            pin_recent_days: settings.pin_recent_days,
        };
        let scan_options = ScanOptions {
            log_retention_days: settings.log_retention_days,
        };
        let (sender, receiver) = async_channel::bounded(1);
        crate::runtime().spawn_blocking(move || {
            let scanner = Scanner::with_options_and_cancellation(scan_options, cancellation);
            let _ = sender.send_blocking(crate::services::build_cleanup_plan(&scanner, &inputs));
        });

        let page = self.clone();
        glib::spawn_future_local(async move {
            let mut countdown = false;
            if let Ok((plan, cancelled)) = receiver.recv().await {
                countdown = page.imp().countdown_after_plan.replace(false);
                if cancelled {
                    countdown = false;
                    page.imp().has_plan.set(false);
                    if let Some(view) = page.imp().cleanup_view.borrow().as_ref() {
                        view.show_status(&crate::i18n::tr("Scan cancelled"), false);
                    }
                } else {
                    page.imp().scan_revision.set(revision);
                    page.imp().has_plan.set(true);
                    page.imp().has_scanned.replace(true);
                    page.imp().reclaimable_bytes.replace(plan.total_size());
                    page.imp().scan_result.replace(None);
                    page.imp().found_categories.borrow_mut().clear();
                    page.imp().category_results.borrow_mut().clear();
                    if let Some(view) = page.imp().cleanup_view.borrow().as_ref() {
                        view.set_plan(plan);
                    }
                    page.refresh_disk_usage();
                }
            }
            page.imp().operation_cancel.replace(None);
            running_flag.store(false, Ordering::SeqCst);
            page.set_operation_controls(false);
            page.update_scan_button();
            if countdown && !page.results_are_stale() {
                if let Some(view) = page.imp().cleanup_view.borrow().as_ref() {
                    view.start_clean_countdown();
                }
            }
        });
        true
    }

    /// Clean what the cleanup screen has ticked. Private data uses the
    /// user's removal method; regenerable bulk is unlinked.
    fn execute_plan_clean(&self, private: ScanResult, bulk: ScanResult) {
        let approved = self.imp().limit_approved.replace(false);
        if !self.rules_unchanged() {
            return;
        }
        if !approved && self.over_safety_limit(&private).is_some() {
            // Normally asked before the countdown; this is only a fallback.
            let page = self.downgrade();
            self.confirm_plan_clean(
                &private.clone(),
                &bulk.clone(),
                Box::new(move || {
                    if let Some(page) = page.upgrade() {
                        page.imp().limit_approved.set(false);
                        page.execute_clean_parts(&private, Some(bulk), true, true);
                    }
                }),
            );
            return;
        }
        self.execute_clean_parts(&private, Some(bulk), true, approved);
    }

    /// Results from before a rules change are never cleaned.
    fn rules_unchanged(&self) -> bool {
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        if self.imp().scan_revision.get() != storage.revision() {
            self.show_info_dialog(
                "Your cleanup rules changed",
                "The results below are from before the change. Scan again to see what can be freed now.",
            );
            return false;
        }
        true
    }

    /// Why the selection goes over a safety limit, if it does.
    fn over_safety_limit(&self, private: &ScanResult) -> Option<String> {
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let settings = storage.get_settings();
        if private.file_count > settings.max_files_per_operation {
            Some(crate::i18n::tr_args(
                "It has {count} files and your safety limit is {limit} per cleanup.",
                &[
                    ("{count}", &crate::i18n::format_count(private.file_count)),
                    (
                        "{limit}",
                        &crate::i18n::format_count(settings.max_files_per_operation),
                    ),
                ],
            ))
        } else if private.total_size > settings.max_size_per_operation {
            Some(crate::i18n::tr_args(
                "It is {size} and your safety limit is {limit} per cleanup.",
                &[
                    ("{size}", &private.formatted_size()),
                    (
                        "{limit}",
                        &bytesize::ByteSize(settings.max_size_per_operation).to_string(),
                    ),
                ],
            ))
        } else {
            None
        }
    }

    /// Run before the countdown: stale rules stop here, and going over a
    /// safety limit asks first. Safety limits guard blind cleanups; here
    /// the user picked every item, so they may go over it this once.
    fn confirm_plan_clean(
        &self,
        private: &ScanResult,
        bulk: &ScanResult,
        proceed: Box<dyn FnOnce()>,
    ) {
        self.imp().limit_approved.set(false);
        if !self.rules_unchanged() {
            return;
        }
        let detail = self.over_safety_limit(private);
        let mut scan = private.clone();
        scan.merge(bulk.clone());
        let cards = super::removal_dialog::scan_items(&scan);
        let settings = self.imp().storage.borrow().as_ref().unwrap().get_settings();
        let notice = format!(
            "{}\n{}",
            crate::i18n::tr(
                "Regenerable caches are deleted directly. Other files use your removal method."
            ),
            detail.clone().unwrap_or_default()
        );
        let page = self.downgrade();
        let parent = self
            .root()
            .and_downcast::<gtk4::Window>()
            .map(|w| w.upcast::<gtk4::Widget>())
            .unwrap_or_else(|| self.clone().upcast());
        super::removal_dialog::present_fixed(
            &parent,
            "Confirm Cleanup",
            cards,
            settings.removal_method == crate::models::RemovalMethod::Trash
                && bulk.is_empty()
                && private.file_count <= crate::services::TRASH_MAX_FILES,
            &notice,
            move |confirmed| {
                if confirmed {
                    if let Some(page) = page.upgrade() {
                        page.imp().limit_approved.set(detail.is_some());
                        proceed();
                    }
                }
            },
        );
    }

    /// Keyboard shortcut: scan now unless a scan or cleanup is running.
    pub fn request_scan(&self) {
        if !self.operation_is_running() {
            self.start_scan(false, false);
        }
    }

    pub fn request_clean(&self) {
        // Tray cleanup always starts from a fresh scan so it cannot act on a
        // stale preview, then goes through the same countdown as the button.
        self.imp().countdown_after_plan.set(true);
        if !self.start_scan(false, false) {
            self.imp().countdown_after_plan.set(false);
        }
    }

    pub fn request_scheduled_clean(&self) -> bool {
        // Automatic cleanup is explicitly enabled in Settings, so it skips
        // interactive dialogs while retaining all scan and safety checks.
        self.start_scan(true, true)
    }

    pub fn operation_is_running(&self) -> bool {
        self.imp()
            .operation_running
            .borrow()
            .as_ref()
            .map(|flag| flag.load(Ordering::SeqCst))
            .unwrap_or(false)
            || self
                .imp()
                .cleanup_view
                .borrow()
                .as_ref()
                .is_some_and(|view| view.is_launching())
    }

    pub fn show_operation_running_dialog(&self) {
        self.show_info_dialog(
            "Operation in Progress",
            "Wait for the current scan or cleanup to finish before closing Magnífico.",
        );
    }

    fn on_clean_clicked(&self) {
        if self.imp().has_plan.get() {
            if let Some(view) = self.imp().cleanup_view.borrow().as_ref() {
                view.start_clean_countdown();
            }
            return;
        }
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let settings = storage.get_settings();

        // Check if we have scan results that still match the current rules
        let scan_result = self
            .imp()
            .scan_result
            .borrow()
            .clone()
            .filter(|_| self.imp().rule_scan_revision.get() == storage.revision());

        if scan_result.is_none() {
            self.show_info_dialog(
                "Your cleanup rules changed",
                "The results below are from before the change. Scan again to see what can be freed now.",
            );
            return;
        }

        let scan_result = scan_result.unwrap();

        if scan_result.is_empty() && !settings.system_journal_cleanup_enabled {
            self.show_info_dialog(
                "Nothing to Clean",
                "No files were found to clean with the current settings.",
            );
            return;
        }

        self.execute_clean(&scan_result, true);
    }

    fn on_automatic_clean_ready(&self) {
        if self
            .imp()
            .storage
            .borrow()
            .as_ref()
            .is_some_and(|storage| self.imp().rule_scan_revision.get() != storage.revision())
        {
            tracing::info!(
                "Scheduled cleanup skipped because cleanup rules changed during its scan"
            );
            return;
        }
        let Some(scan_result) = self.imp().scan_result.borrow().clone() else {
            return;
        };

        if scan_result.is_empty() {
            tracing::info!("Scheduled cleanup found nothing to delete");
            self.refresh();
            return;
        }

        self.execute_clean(&scan_result, false);
    }

    fn execute_clean(&self, scan_result: &ScanResult, interactive: bool) {
        self.execute_clean_parts(scan_result, None, interactive, false);
    }

    fn execute_clean_parts(
        &self,
        scan_result: &ScanResult,
        bulk: Option<ScanResult>,
        interactive: bool,
        lift_limits: bool,
    ) {
        let bulk = bulk.unwrap_or_default();
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let settings = storage.get_settings();
        let clean_options = CleanOptions {
            max_files_per_operation: if lift_limits {
                usize::MAX
            } else {
                settings.max_files_per_operation
            },
            max_size_per_operation: if lift_limits {
                u64::MAX
            } else {
                settings.max_size_per_operation
            },
            removal_method: settings.removal_method,
            secure_browser_data: settings.secure_browser_data,
        };

        // Guard against concurrent operations
        let Some(running_flag) = self.imp().operation_running.borrow().as_ref().cloned() else {
            return;
        };
        if running_flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            if interactive {
                self.show_info_dialog(
                    "Operation in Progress",
                    "A scan or cleanup is already running. Please wait for it to complete.",
                );
            }
            return;
        }

        if let Err(reason) = Cleaner::with_options(clean_options).validate_operation(scan_result) {
            let mut blocked_result = crate::models::CleanResult::new();
            blocked_result.blocked(reason.clone());
            self.store_cleanup_log(
                &blocked_result,
                scan_result,
                None,
                !interactive,
                settings.system_journal_cleanup_enabled,
            );
            if interactive {
                self.show_cleanup_result("Cleanup Blocked", &reason, true);
                if let Some(window) = self
                    .root()
                    .and_then(|root| root.downcast::<gtk4::Window>().ok())
                    .and_then(|window| window.downcast::<crate::ui::MainWindow>().ok())
                {
                    window.show_blocked_dialog(&reason);
                }
            } else {
                tracing::warn!("Scheduled cleanup blocked: {}", reason);
            }
            running_flag.store(false, Ordering::SeqCst);
            self.set_operation_controls(false);
            return;
        }

        let cancellation = Arc::new(AtomicBool::new(false));
        self.imp()
            .operation_cancel
            .replace(Some(cancellation.clone()));
        self.set_operation_controls(true);

        let (sender, receiver) = async_channel::bounded(1);
        let result = scan_result.clone();
        let mut log_scan_result = scan_result.clone();
        log_scan_result.merge(bulk.clone());
        // System-journal cleanup requires an interactive PolicyKit prompt and
        // is therefore intentionally skipped by scheduled background runs.
        let clean_system_journal = interactive && settings.system_journal_cleanup_enabled;
        let log_retention_days = settings.log_retention_days;

        let progress = Arc::new(CleanProgress::default());
        let progress_window = interactive.then(|| {
            let window = self.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            CleanProgressWindow::show(
                window.as_ref(),
                scan_result.total_size + bulk.total_size,
                scan_result.file_count + bulk.file_count,
                progress.clone(),
            )
        });

        crate::runtime().spawn_blocking(move || {
            let cleaner =
                Cleaner::with_options_and_cancellation(clean_options, cancellation.clone())
                    .with_progress(progress.clone());
            let mut clean_result = if result.is_empty() {
                crate::models::CleanResult::new()
            } else {
                cleaner.clean(&result)
            };
            // Builds, games and models: nothing private, and far too many
            // files for the per-cleanup limits, which guard the rules.
            if !bulk.is_empty() && !clean_result.cancelled && clean_result.blocked_reason.is_none()
            {
                let bulk_options = CleanOptions {
                    removal_method: crate::models::RemovalMethod::Permanent,
                    secure_browser_data: false,
                    ..CleanOptions::default()
                };
                let bulk_result =
                    Cleaner::with_options_and_cancellation(bulk_options, cancellation)
                        .with_progress(progress)
                        .clean(&bulk);
                clean_result.merge(bulk_result);
            }
            let journal_result = if clean_system_journal && !clean_result.cancelled {
                Some(vacuum_system_journal(log_retention_days))
            } else {
                None
            };
            let _ = sender.send_blocking((clean_result, journal_result));
        });

        let page = self.clone();
        glib::spawn_future_local(async move {
            let received = receiver.recv().await;
            if let Some(window) = progress_window {
                window.close();
            }
            if let Ok((result, journal_result)) = received {
                if let Some(reason) = result.blocked_reason.as_deref() {
                    page.store_cleanup_log(
                        &result,
                        &log_scan_result,
                        journal_result.as_ref(),
                        !interactive,
                        settings.system_journal_cleanup_enabled,
                    );
                    if interactive {
                        page.show_cleanup_result("Cleanup Blocked", reason, true);
                    } else {
                        tracing::warn!("Scheduled cleanup blocked: {}", reason);
                    }
                    running_flag.store(false, Ordering::SeqCst);
                    page.imp().operation_cancel.replace(None);
                    page.set_operation_controls(false);
                    return;
                }

                // Update last cleanup time
                if let Err(e) = storage.update_settings(|s| {
                    s.last_cleanup = Some(chrono::Utc::now());
                }) {
                    tracing::warn!("Failed to update last cleanup time: {}", e);
                }

                page.store_cleanup_log(
                    &result,
                    &log_scan_result,
                    journal_result.as_ref(),
                    !interactive,
                    settings.system_journal_cleanup_enabled,
                );

                let journal_failed = matches!(&journal_result, Some(Err(_)));
                let journal_note = match &journal_result {
                    Some(Ok(message)) => crate::i18n::tr_args(
                        "\nSystem journal: {message}.",
                        &[("{message}", message)],
                    ),
                    Some(Err(error)) => crate::i18n::tr_args(
                        "\nSystem journal failed: {error}",
                        &[("{error}", error)],
                    ),
                    None => String::new(),
                };
                let total_failures = result.failed_count() + usize::from(journal_failed);
                let mut journal_note = journal_note;
                if result.files_overwritten > 0 {
                    journal_note.push_str(&crate::i18n::tr_args(
                        "\n{count} files were overwritten before deletion.",
                        &[(
                            "{count}",
                            &crate::i18n::format_count(result.files_overwritten),
                        )],
                    ));
                }
                if result.files_too_many_for_trash > 0 {
                    journal_note.push_str(&crate::i18n::tr_args(
                        "\n{count} files were too many for the trash and were deleted permanently.",
                        &[(
                            "{count}",
                            &crate::i18n::format_count(result.files_too_many_for_trash),
                        )],
                    ));
                }
                if result.files_trashed > 0 {
                    journal_note.push_str(&crate::i18n::tr_args(
                        "\n{count} files were moved to the trash. Their space is freed when you empty it.",
                        &[("{count}", &crate::i18n::format_count(result.files_trashed))],
                    ));
                }

                if interactive {
                    page.show_cleanup_result(
                        if result.cancelled {
                            "Cleanup Cancelled"
                        } else {
                            "Cleanup Complete"
                        },
                        &crate::i18n::tr_args(
                            "Deleted {count} files and freed {size}.\n{failed} items failed.{journal}",
                            &[
                                ("{count}", &crate::i18n::format_count(result.files_deleted_count())),
                                ("{size}", &result.formatted_bytes_freed()),
                                ("{failed}", &total_failures.to_string()),
                                ("{journal}", &journal_note),
                            ],
                        ),
                        total_failures > 0 || result.cancelled,
                    );
                } else if total_failures > 0 || result.cancelled {
                    tracing::warn!(
                        "Scheduled cleanup finished with {} failures (cancelled: {})",
                        total_failures,
                        result.cancelled
                    );
                }

                let cleaned_paths = result
                    .deleted_files
                    .iter()
                    .chain(&result.deleted_directories)
                    .cloned()
                    .collect();
                // A scheduled cleanup may also have removed items of the plan
                // left open on screen.
                {
                    let mut categories = page.imp().found_categories.borrow_mut();
                    for (_, scan) in categories.iter_mut() {
                        scan.remove_cleaned(&cleaned_paths);
                    }
                    categories.retain(|(_, scan)| !scan.is_empty());
                }
                page.apply_exclusions();
                if page.imp().has_plan.get() {
                    if let Some(view) = page.imp().cleanup_view.borrow().as_ref() {
                        let remaining = view.remove_cleaned(&cleaned_paths);
                        page.imp().reclaimable_bytes.replace(remaining);
                    }
                }
                // Cleaning does not change the rules: keep scan_revision so real
                // changes made during the operation still mark results stale.
                page.refresh();
            }
            page.imp().operation_cancel.replace(None);
            running_flag.store(false, Ordering::SeqCst);
            page.set_operation_controls(false);
        });
    }

    fn store_cleanup_log(
        &self,
        clean_result: &crate::models::CleanResult,
        scan_result: &ScanResult,
        journal_result: Option<&Result<String, String>>,
        automatic: bool,
        system_journal_enabled: bool,
    ) {
        if let Some(window) = self
            .root()
            .and_then(|root| root.downcast::<gtk4::Window>().ok())
            .and_then(|window| window.downcast::<crate::ui::MainWindow>().ok())
        {
            window.set_cleanup_log(
                clean_result,
                scan_result,
                journal_result,
                automatic,
                system_journal_enabled,
            );
        }
    }

    fn show_cleanup_result(&self, title: &str, message: &str, warning: bool) {
        if let Some(window) = self
            .root()
            .and_then(|root| root.downcast::<gtk4::Window>().ok())
            .and_then(|window| window.downcast::<crate::ui::MainWindow>().ok())
        {
            window.show_cleanup_report(&format!("{}\n{message}", crate::i18n::tr(title)), warning);
        }
    }

    fn show_info_dialog(&self, title: &str, message: &str) {
        let window = self.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
        let dialog = dialogs::message_dialog(
            window.as_ref(),
            &crate::i18n::tr(title),
            &crate::i18n::tr(message),
        );

        dialog.add_response("ok", "OK");
        dialog.set_default_response(Some("ok"));
        crate::i18n::translate_widget_tree(&dialog);
        dialog.present();
    }
}

/// Keep each path in the first category that found it, so totals per
/// category add up to the overall total.
fn dedupe_categories(categories: CategoryResults) -> CategoryResults {
    let mut seen = std::collections::HashSet::new();
    categories
        .into_iter()
        .map(|(category, result)| {
            let mut unique = ScanResult::new();
            let mut filtered = result.clone();
            filtered.files.retain(|f| seen.insert(f.path.clone()));
            filtered.directories.retain(|d| seen.insert(d.path.clone()));
            unique.merge(filtered);
            unique.cancelled = result.cancelled;
            (category, unique)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FileEntry;

    #[test]
    fn a_path_found_twice_counts_in_the_first_category_only() {
        let shared = PathBuf::from("/home/u/.cache/shared");
        let mut browsers = ScanResult::new();
        browsers.add_file(FileEntry::new(shared.clone(), 10, false, false));
        let mut system = ScanResult::new();
        system.add_file(FileEntry::new(shared, 10, false, false));
        system.add_file(FileEntry::new(PathBuf::from("/tmp/x"), 5, false, false));

        let result = dedupe_categories(vec![
            (ScanCategory::Browsers, browsers),
            (ScanCategory::System, system),
        ]);
        assert_eq!(result[0].1.total_size, 10);
        assert_eq!(result[1].1.total_size, 5);
        assert_eq!(result[1].1.file_count, 1);
    }
}
