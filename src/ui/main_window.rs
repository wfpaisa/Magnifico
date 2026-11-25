// Sidebar layout adapted from Purge (MIT, © 2026 Jithin Sabu); see THIRD_PARTY_LICENSES.md.
use crate::storage::Storage;
use crate::theme;
use crate::ui::{CleanupLogPage, DashboardPage, SettingsPage, StorageAnalyzerPage};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

/// Navigation items for the sidebar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    Dashboard,
    StorageAnalyzer,
    LargeFiles,
    SystemCleanup,
    Browsers,
    Applications,
    UserSystem,
    AiModels,
    Games,
    Settings,
}

impl NavItem {
    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Dashboard => "pc-view-grid-symbolic",
            Self::Browsers => "pc-drive-harddisk-symbolic",
            Self::Applications => "pc-user-trash-symbolic",
            Self::StorageAnalyzer => "pc-drive-harddisk-symbolic",
            Self::LargeFiles => "pc-edit-find-symbolic",
            Self::SystemCleanup => "pc-computer-symbolic",
            Self::UserSystem => "pc-applications-engineering-symbolic",
            Self::AiModels => "pc-magnifico-ai-symbolic",
            Self::Games => "pc-input-gaming-symbolic",
            Self::Settings => "pc-preferences-system-symbolic",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Dashboard => "Overview",
            Self::Browsers => "App Caches",
            Self::Applications => "App Uninstaller",
            Self::StorageAnalyzer => "Storage Analyzer",
            Self::LargeFiles => "Large Files",
            Self::SystemCleanup => "System",
            Self::UserSystem => "Dev Tools",
            Self::AiModels => "AI Models",
            Self::Games => "Games",
            Self::Settings => "Settings",
        }
    }

    pub fn stack_name(&self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Browsers => "browsers",
            Self::Applications => "applications",
            Self::StorageAnalyzer => "storage-analyzer",
            Self::LargeFiles => "large-files",
            Self::SystemCleanup => "system-cleanup",
            Self::UserSystem => "dev-tools",
            Self::AiModels => "ai-models",
            Self::Games => "games",
            Self::Settings => "settings",
        }
    }

    pub fn all() -> &'static [NavItem] {
        &[
            Self::Dashboard,
            Self::Browsers,
            Self::UserSystem,
            Self::SystemCleanup,
            Self::LargeFiles,
            Self::Applications,
            Self::StorageAnalyzer,
            Self::AiModels,
            Self::Games,
        ]
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MainWindow {
        pub(super) about_page: RefCell<Option<super::super::about_page::AboutPage>>,
        pub shell: RefCell<Option<gtk4::Stack>>,
        pub cleaning: Cell<bool>,
        /// Removal numbers for the Overview that arrived while the progress
        /// screen covered it; applied when the page is back on screen so the
        /// cells fly from where the user is.
        pub pending_overview: RefCell<Option<crate::services::removal_update::RemovalUpdate>>,
        pub app_monitor: RefCell<Option<gio::AppInfoMonitor>>,
        pub storage: RefCell<Option<Arc<Storage>>>,
        pub sidebar_box: RefCell<Option<gtk4::Box>>,
        pub content_stack: RefCell<Option<gtk4::Stack>>,
        pub sidebar_list: RefCell<Option<gtk4::ListBox>>,
        pub dashboard_page: RefCell<Option<DashboardPage>>,
        pub overview_page: RefCell<Option<super::super::overview_page::OverviewPage>>,
        pub(super) large_files_page:
            RefCell<Option<super::super::large_files_page::LargeFilesPage>>,
        pub(super) uninstaller_page:
            RefCell<Option<super::super::uninstaller_page::UninstallerPage>>,
        pub reviewing: Cell<bool>,
        pub(super) summary: RefCell<Option<crate::services::scan_cache::ScanSummary>>,
        pub(super) sidebar_spinners: RefCell<Vec<(NavItem, super::super::cleanup_fx::Spinner)>>,
        pub sidebar_sizes: RefCell<Vec<gtk4::Label>>,
        pub(super) clean_pages: RefCell<Vec<super::super::clean_list_page::CleanListPage>>,
        pub(super) review_store: RefCell<Option<super::super::review_scan_store::ReviewScanStore>>,
        pub(super) review_item_pages:
            RefCell<Vec<super::super::review_items_page::ReviewItemsPage>>,
        pub(super) optional_sizes: RefCell<Vec<(NavItem, gtk4::Label)>>,
        pub(super) clean_store: RefCell<Option<super::super::clean_scan_store::CleanScanStore>>,
        pub trash_label: RefCell<Option<gtk4::Label>>,
        pub report_button: RefCell<Option<gtk4::MenuButton>>,
        pub report_label: RefCell<Option<gtk4::Label>>,
        pub cleanup_log_page: RefCell<Option<CleanupLogPage>>,
        pub storage_analyzer_page: RefCell<Option<StorageAnalyzerPage>>,
        pub content_title: RefCell<Option<adw::WindowTitle>>,
        pub nav_rows: RefCell<Vec<(NavItem, gtk4::ListBoxRow)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "MagnificoMainWindow";
        type Type = super::MainWindow;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for MainWindow {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl WidgetImpl for MainWindow {}
    impl WindowImpl for MainWindow {}
    impl ApplicationWindowImpl for MainWindow {}
    impl AdwApplicationWindowImpl for MainWindow {}
}

glib::wrapper! {
    pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
        @extends gtk4::Widget, gtk4::Window, gtk4::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl MainWindow {
    const SIDEBAR_EXPANDED_WIDTH: i32 = 260;

    /// Size of the window the first time it opens.
    const DEFAULT_SIZE: (i32, i32) = (1080, 760);

    pub fn new(app: &crate::application::MagnificoApplication, storage: Arc<Storage>) -> Self {
        // Reopen at the size the user left the window at.
        let (width, height) = storage
            .get_settings()
            .window_size
            .filter(|(w, h)| *w >= 600 && *h >= 480)
            .unwrap_or(Self::DEFAULT_SIZE);
        let monitor_width = gtk4::gdk::Display::default()
            .and_then(|d| d.monitors().item(0))
            .and_downcast::<gtk4::gdk::Monitor>()
            .map(|m| (m.geometry().width() - 32).max(600))
            .unwrap_or(Self::DEFAULT_SIZE.0);
        let width = width.min(monitor_width);
        // At least the default height, so every sidebar row fits when the screen allows.
        let monitor_height = gtk4::gdk::Display::default()
            .and_then(|d| d.monitors().item(0))
            .and_downcast::<gtk4::gdk::Monitor>()
            .map(|m| (m.geometry().height() - 64).max(480))
            .unwrap_or(Self::DEFAULT_SIZE.1);
        let height = height.max(Self::DEFAULT_SIZE.1).min(monitor_height);
        let window: Self = glib::Object::builder()
            .property("application", app)
            .property("title", crate::APP_NAME)
            .property("default-width", width)
            .property("default-height", height)
            .build();

        crate::theme::apply_blur(&window, storage.get_settings().window_blur);
        window.imp().storage.replace(Some(storage));
        window.setup_ui();
        window.setup_actions();
        window.watch_deleted_apps();
        window.sync_theme_preferences();
        window
    }

    fn setup_ui(&self) {
        self.imp().clean_pages.borrow_mut().clear();
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();

        let summary = crate::services::scan_cache::snapshot();
        self.imp().summary.replace(summary.clone());

        // Main horizontal layout: a dark navigation rail and a lighter,
        // uninterrupted content surface.
        let main_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        main_box.add_css_class("background");
        main_box.add_css_class("magnifico-window-layout");

        // === SIDEBAR ===
        let sidebar_box = self.create_sidebar(&storage);
        if let Some(summary) = &summary {
            for ((item, label), index) in self.imp().optional_sizes.borrow().iter().zip([6, 7]) {
                label.set_text(&bytesize::ByteSize(summary.bytes[index]).to_string());
                if let Some((_, row)) = self
                    .imp()
                    .nav_rows
                    .borrow()
                    .iter()
                    .find(|(nav, _)| nav == item)
                {
                    row.set_visible(summary.bytes[index] > 0);
                }
            }
        }

        // === CONTENT AREA ===
        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content_box.set_hexpand(true);
        content_box.add_css_class("magnifico-content");
        content_box.add_css_class("magnifico-view");
        content_box.add_css_class("view");

        // Kept for page title updates; not shown.
        let title = adw::WindowTitle::new(&crate::i18n::tr("Overview"), "");
        let report_button = gtk4::MenuButton::new();
        report_button.set_child(Some(&crate::ui::icon_image("ui/alert")));
        report_button.set_tooltip_text(Some(&crate::i18n::tr("Cleanup Result")));
        report_button.set_visible(false);
        report_button.add_css_class("flat");
        let report_popover = gtk4::Popover::new();
        let report_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        report_box.set_margin_top(12);
        report_box.set_margin_bottom(12);
        report_box.set_margin_start(12);
        report_box.set_margin_end(12);
        let report_label = gtk4::Label::new(None);
        report_label.set_wrap(true);
        report_label.set_xalign(0.0);
        report_label.set_max_width_chars(48);
        report_label.set_selectable(true);
        report_box.append(&report_label);
        let explanation = gtk4::Label::new(Some(&crate::i18n::tr(
            "Look again after installing or building something, or after changing your rules.",
        )));
        explanation.set_wrap(true);
        explanation.set_xalign(0.0);
        explanation.set_max_width_chars(48);
        explanation.add_css_class("dim-label");
        report_box.append(&explanation);
        report_popover.set_child(Some(&report_box));
        let window = self.downgrade();
        report_popover.connect_show(move |_| {
            if let Some(window) = window.upgrade() {
                if let Some(button) = window.imp().report_button.borrow().as_ref() {
                    button.remove_css_class("accent");
                }
            }
        });
        report_button.set_popover(Some(&report_popover));
        // No content header: the cleanup result sits in the sidebar header.
        if let Some(top) = sidebar_box
            .first_child()
            .and_downcast::<gtk4::WindowHandle>()
            .and_then(|handle| handle.child())
            .and_downcast::<gtk4::Box>()
        {
            report_button.set_valign(gtk4::Align::Center);
            top.append(&report_button);
        }
        self.imp().report_button.replace(Some(report_button));
        self.imp().report_label.replace(Some(report_label));

        // Content stack
        let content_stack = gtk4::Stack::new();
        content_stack.set_hhomogeneous(false);
        content_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        content_stack.set_transition_duration(theme::transition_duration(self, 200));
        content_stack.set_hexpand(true);
        content_stack.add_css_class("view");
        content_stack.add_css_class("magnifico-view");

        // Create pages
        let dashboard_page = DashboardPage::new(storage.clone());
        let cleanup_log_page = CleanupLogPage::new();
        let review_store = self
            .imp()
            .review_store
            .borrow_mut()
            .get_or_insert_with(super::review_scan_store::ReviewScanStore::new)
            .clone();
        review_store.set_summary_present(summary.is_some());
        let applications_page = super::uninstaller_page::UninstallerPage::new(review_store.clone());
        self.imp()
            .uninstaller_page
            .replace(Some(applications_page.clone()));
        let storage_analyzer_page = StorageAnalyzerPage::new();
        let settings_page = SettingsPage::new(storage.clone());
        let clean_store = self
            .imp()
            .clean_store
            .borrow_mut()
            .get_or_insert_with(|| super::clean_scan_store::CleanScanStore::new(storage.clone()))
            .clone();
        clean_store.set_summary_present(summary.is_some());
        for (kind, name) in [
            (
                crate::services::clean_sources::CleanPageKind::AppCaches,
                "browsers",
            ),
            (
                crate::services::clean_sources::CleanPageKind::DevTools,
                "dev-tools",
            ),
            (
                crate::services::clean_sources::CleanPageKind::System,
                "system-cleanup",
            ),
        ] {
            let page = super::clean_list_page::CleanListPage::new(
                kind,
                storage.clone(),
                clean_store.clone(),
            );
            content_stack.add_named(page.widget(), Some(name));
            self.imp().clean_pages.borrow_mut().push(page);
        }

        // Add pages to stack
        // The rule-based scheduler and safe cleanup share this controller.
        // Its legacy view stays out of navigation; confirmations belong to the window.
        dashboard_page.set_visible(false);
        main_box.append(&dashboard_page);
        let weak = self.downgrade();
        let flights = super::flight_layer::FlightLayer::new();
        let overview = super::overview_page::OverviewPage::new(
            storage.clone(),
            clean_store.clone(),
            review_store.clone(),
            self.imp().summary.borrow().clone(),
            move |sizes, trash| {
                if let Some(window) = weak.upgrade() {
                    let labels = window.imp().sidebar_sizes.borrow();
                    for (label, index) in labels.iter().zip([0, 1, 2, 3, 4]) {
                        let bytes = sizes[index];
                        label.set_text(&bytesize::ByteSize(bytes).to_string());
                    }
                    if let Some(label) = window.imp().trash_label.borrow().as_ref() {
                        label.set_text(&crate::i18n::tr_args(
                            "{size} in Trash",
                            &[("{size}", &bytesize::ByteSize(trash).to_string())],
                        ));
                    }
                }
            },
            flights.clone(),
        );
        // Cleaned cells fly to the sidebar's Trash button.
        if let Some(trash) = self
            .imp()
            .trash_label
            .borrow()
            .as_ref()
            .and_then(|label| label.ancestor(gtk4::MenuButton::static_type()))
        {
            overview.set_trash_target(&trash);
        }
        content_stack.add_named(&overview.widget, Some(NavItem::Dashboard.stack_name()));
        content_stack.set_visible_child_name(NavItem::Dashboard.stack_name());
        self.imp().overview_page.replace(Some(overview));
        // This page is deliberately absent from NavItem::all(), so it can be
        // opened after a cleanup without appearing in the sidebar.
        let about = super::about_page::AboutPage::new();
        content_stack.add_named(&about.widget, Some("about"));
        self.imp().about_page.replace(Some(about));

        content_stack.add_named(
            applications_page.widget(),
            Some(NavItem::Applications.stack_name()),
        );
        content_stack.add_named(
            &storage_analyzer_page,
            Some(NavItem::StorageAnalyzer.stack_name()),
        );
        // AI Models and Games: their sidebar rows appear only when something is found.
        for spec in [
            &super::review_items_page::MODELS,
            &super::review_items_page::GAMES,
        ] {
            let page = super::review_items_page::ReviewItemsPage::new(review_store.clone(), spec);
            let name = match spec.kind {
                super::review_scan_store::Kind::Models => NavItem::AiModels.stack_name(),
                _ => NavItem::Games.stack_name(),
            };
            content_stack.add_named(page.widget(), Some(name));
            self.imp().review_item_pages.borrow_mut().push(page);
        }
        let weak = self.downgrade();
        let watched = review_store.clone();
        review_store.subscribe(move || {
            if let Some(window) = weak.upgrade() {
                window.sync_optional_rows(&watched);
                window.sync_sidebar_spinners();
            }
        });
        let weak = self.downgrade();
        clean_store.subscribe(move |_| {
            if let Some(window) = weak.upgrade() {
                window.sync_sidebar_spinners();
            }
        });
        self.sync_optional_rows(&review_store);
        self.sync_sidebar_spinners();
        let large_files_page = super::large_files_page::LargeFilesPage::new(review_store);
        self.imp()
            .large_files_page
            .replace(Some(large_files_page.clone()));
        content_stack.add_named(
            large_files_page.widget(),
            Some(NavItem::LargeFiles.stack_name()),
        );
        content_stack.add_named(&settings_page, Some(NavItem::Settings.stack_name()));
        // Every category page shows the disk map with its own cells in colour.
        if let Some(overview) = self.imp().overview_page.borrow().as_ref() {
            for (name, category) in [
                (NavItem::Browsers, 0),
                (NavItem::UserSystem, 1),
                (NavItem::SystemCleanup, 2),
                (NavItem::LargeFiles, 3),
                (NavItem::Applications, 4),
                (NavItem::Applications, 5),
                (NavItem::AiModels, 6),
                (NavItem::Games, 7),
            ] {
                if let Some(page) = content_stack.child_by_name(name.stack_name()) {
                    overview.attach_strip(&page, category);
                }
            }
        }

        // A slim strip above each page: window buttons at the end, and the
        // rest moves the window, as a header would.
        let controls = gtk4::WindowControls::new(gtk4::PackType::End);
        controls.set_decoration_layout(Some(&window_buttons_layout()));
        controls.set_halign(gtk4::Align::End);
        controls.set_hexpand(true);
        let drag = gtk4::WindowHandle::new();
        drag.add_css_class("pc-content-bar");
        drag.set_child(Some(&controls));
        content_box.append(&drag);
        content_stack.set_vexpand(true);
        content_box.append(&content_stack);

        // Assemble main layout
        main_box.append(&sidebar_box);
        main_box.append(&content_box);

        // Store references
        self.imp().sidebar_box.replace(Some(sidebar_box));
        self.imp()
            .content_stack
            .replace(Some(content_stack.clone()));
        self.imp().dashboard_page.replace(Some(dashboard_page));
        self.imp().cleanup_log_page.replace(Some(cleanup_log_page));
        self.imp()
            .storage_analyzer_page
            .replace(Some(storage_analyzer_page));
        self.imp().content_title.replace(Some(title.clone()));

        // Handle navigation
        let sidebar_list = self.imp().sidebar_list.borrow().clone();
        if let Some(list) = sidebar_list {
            let stack = content_stack.clone();
            let title_widget = title.clone();
            let window_weak = self.downgrade();
            list.connect_row_selected(move |_, row| {
                if let Some(row) = row {
                    let Some(window) = window_weak.upgrade() else {
                        return;
                    };
                    let nav_item = window
                        .imp()
                        .nav_rows
                        .borrow()
                        .iter()
                        .find(|(_, nav_row)| nav_row == row)
                        .map(|(item, _)| *item);
                    if let Some(nav_item) = nav_item {
                        stack.set_visible_child_name(nav_item.stack_name());
                        title_widget.set_title(&crate::i18n::tr(nav_item.title()));
                        title_widget.set_visible(false);

                        // When returning to the Dashboard, re-read storage so
                        // the enabled-targets count and the Clean button state
                        // reflect any rule toggles the user made on other
                        // pages. Without this the dashboard shows stale data
                        // and the Clean button can stay disabled.
                        if nav_item == NavItem::Dashboard {
                            window.refresh_dashboard();
                        }
                    }
                }
            });
        }

        let shell = gtk4::Stack::new();
        shell.add_named(&main_box, Some("main"));
        // The flight layer paints over everything, sidebar included.
        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&shell));
        overlay.add_overlay(flights.widget());
        self.set_content(Some(&overlay));
        self.imp().shell.replace(Some(shell));
        crate::i18n::translate_widget_tree(&main_box);
    }

    fn create_sidebar(&self, _storage: &Arc<Storage>) -> gtk4::Box {
        use crate::i18n::tr;
        self.imp().sidebar_spinners.borrow_mut().clear();
        let sidebar = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        sidebar.set_width_request(Self::SIDEBAR_EXPANDED_WIDTH);
        sidebar.set_hexpand(false);
        sidebar.add_css_class("magnifico-sidebar");
        sidebar.add_css_class("pc-sidebar");
        // Top row: logo and name (window buttons live above the page), and
        // the cleanup-result button at the end. Dragging it moves the window.
        let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        top.add_css_class("pc-sidebar-top");
        let header = gtk4::WindowHandle::new();
        header.set_child(Some(&top));
        sidebar.append(&header);
        let brand = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        brand.add_css_class("pc-brand");
        let logo = gtk4::Image::from_icon_name(crate::APP_ID);
        logo.set_pixel_size(22);
        brand.append(&logo);
        let name = gtk4::Label::new(Some(crate::APP_NAME));
        name.add_css_class("pc-row-title");
        brand.append(&name);
        // Clicking the logo or name goes back to the Overview.
        brand.set_cursor_from_name(Some("pointer"));
        let home = gtk4::GestureClick::new();
        let weak = self.downgrade();
        home.connect_released(move |_, _, _, _| {
            if let Some(window) = weak.upgrade() {
                window.navigate_to_page("dashboard");
            }
        });
        brand.add_controller(home);
        brand.set_hexpand(true);
        brand.set_halign(gtk4::Align::Start);
        top.append(&brand);
        let list = gtk4::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_selection_mode(gtk4::SelectionMode::Single);
        let mut rows = Vec::new();
        let mut sizes = Vec::new();
        let mut optional = Vec::new();
        for item in NavItem::all() {
            let row = gtk4::ListBoxRow::new();
            row.set_widget_name(item.stack_name());
            let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            content.add_css_class("pc-nav-row");
            content.append(&gtk4::Image::from_icon_name(item.icon_name()));
            let short = match item {
                NavItem::Browsers => "App caches",
                NavItem::UserSystem => "Development",
                NavItem::Applications => "Uninstall apps",
                _ => item.title(),
            };
            let label = gtk4::Label::new(Some(&tr(short)));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_max_width_chars(1);
            label.set_tooltip_text(Some(&tr(item.title())));
            content.append(&label);
            if *item != NavItem::Dashboard {
                let size = gtk4::Label::new(Some(&tr("…")));
                size.add_css_class("pc-tertiary");
                size.set_ellipsize(gtk4::pango::EllipsizeMode::None);
                size.set_hexpand(false);
                let cached_index = match item {
                    NavItem::Browsers => 0,
                    NavItem::UserSystem => 1,
                    NavItem::SystemCleanup => 2,
                    NavItem::LargeFiles => 3,
                    NavItem::Applications => 4,
                    NavItem::AiModels => 6,
                    _ => 7,
                };
                if *item == NavItem::StorageAnalyzer {
                    size.set_text("");
                } else if let Some(summary) = self.imp().summary.borrow().as_ref() {
                    size.set_text(&bytesize::ByteSize(summary.bytes[cached_index]).to_string());
                }
                content.append(&size);
                let spinner = super::cleanup_fx::Spinner::new(12);
                spinner.widget().set_tooltip_text(Some(&tr("Refreshing…")));
                spinner.set_spinning(false);
                let space = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
                space.set_size_request(12, 12);
                space.set_valign(gtk4::Align::Center);
                space.append(spinner.widget());
                content.append(&space);
                self.imp()
                    .sidebar_spinners
                    .borrow_mut()
                    .push((*item, spinner));
                if matches!(item, NavItem::AiModels | NavItem::Games) {
                    row.set_visible(
                        self.imp()
                            .summary
                            .borrow()
                            .as_ref()
                            .is_some_and(|s| s.bytes[cached_index] > 0),
                    );
                    optional.push((*item, size));
                } else if *item != NavItem::StorageAnalyzer {
                    sizes.push(size);
                }
            }
            row.set_child(Some(&content));
            list.append(&row);
            rows.push((*item, row));
        }
        list.set_header_func(|row, _| {
            let section = match row.widget_name().as_str() {
                "browsers" => Some("CLEAN"),
                "large-files" => Some("REVIEW"),
                _ => None,
            };
            if let Some(section) = section {
                let heading = gtk4::Label::new(Some(&crate::i18n::tr(section)));
                heading.set_xalign(0.0);
                heading.add_css_class("pc-sidebar-section");
                row.set_header(Some(&heading));
            } else {
                row.set_header(gtk4::Widget::NONE);
            }
        });
        list.select_row(Some(&rows[0].1));
        self.imp().nav_rows.replace(rows);
        self.imp().sidebar_sizes.replace(sizes);
        self.imp().optional_sizes.replace(optional);
        self.imp().sidebar_list.replace(Some(list.clone()));
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));
        sidebar.append(&scroll);
        for (icon, text, action) in [
            (
                "pc-preferences-system-symbolic",
                "Settings",
                "win.open-page",
            ),
            ("pc-help-about-symbolic", "About", "win.about"),
        ] {
            let button = gtk4::Button::new();
            button.add_css_class("pc-sidebar-bottom");
            let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
            content.append(&gtk4::Image::from_icon_name(icon));
            content.append(&gtk4::Label::new(Some(&tr(text))));
            button.set_child(Some(&content));
            button.set_action_name(Some(action));
            if text == "Settings" {
                button.set_action_target_value(Some(&"settings".to_variant()));
            }
            sidebar.append(&button);
        }
        let label = gtk4::Label::new(Some(&crate::i18n::tr("Trash…")));
        let trash = super::trash_popover::button(&label);
        sidebar.append(&trash);
        self.imp().trash_label.replace(Some(label));
        sidebar
    }

    fn setup_actions(&self) {
        // Refresh action
        let refresh_action = gio::ActionEntry::builder("refresh")
            .activate(|window: &Self, _, _| {
                window.request_scan();
            })
            .build();

        let open_page = gio::SimpleAction::new("open-page", Some(glib::VariantTy::STRING));
        let weak = self.downgrade();
        open_page.connect_activate(move |_, target| {
            if let (Some(window), Some(name)) = (weak.upgrade(), target.and_then(|v| v.str())) {
                window.navigate_to_page(name);
            }
        });
        self.add_action(&open_page);

        let clean_action = gio::ActionEntry::builder("clean")
            .activate(|window: &Self, _, _| window.request_clean())
            .build();
        let review_action = gio::ActionEntry::builder("review-cleanup")
            .activate(|window: &Self, _, _| window.review_cleanup())
            .build();
        let about_action = gio::ActionEntry::builder("about")
            .activate(|window: &Self, _, _| {
                window.navigate_to_page("about");
            })
            .build();
        self.add_action_entries([refresh_action, clean_action, review_action, about_action]);
    }

    fn watch_deleted_apps(&self) {
        use std::{collections::HashSet, rc::Rc};
        let previous = Rc::new(RefCell::new(None::<HashSet<String>>));
        let generation = Rc::new(Cell::new(0u64));
        let weak = self.downgrade();
        let refresh: Rc<dyn Fn()> = Rc::new(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let Some(storage) = window.storage() else {
                return;
            };
            let current_generation = generation.get().wrapping_add(1);
            generation.set(current_generation);
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let current = gio::AppInfo::all()
                    .iter()
                    .filter_map(|a| a.id().map(|id| id.to_string()))
                    .collect::<HashSet<_>>();
                let enabled = storage.get_settings().review_deleted_apps;
                let _ = tx.send_blocking((current, enabled));
            });
            let weak = weak.clone();
            let previous = previous.clone();
            let generation = generation.clone();
            glib::spawn_future_local(async move {
                let Ok((current, enabled)) = rx.recv().await else {
                    return;
                };
                if generation.get() != current_generation {
                    return;
                }
                let removed = previous
                    .borrow()
                    .as_ref()
                    .is_some_and(|old| old.difference(&current).next().is_some());
                previous.replace(Some(current));
                let Some(window) = weak.upgrade() else {
                    return;
                };
                if !removed || window.operation_is_running() || !enabled {
                    return;
                }
                let dialog = super::dialogs::message_dialog(
                    Some(window.upcast_ref()),
                    &crate::i18n::tr("Deleted Apps"),
                    &crate::i18n::tr("An app was removed. Review the files it left behind."),
                );
                dialog.add_response("cancel", &crate::i18n::tr("Cancel"));
                dialog.add_response("review", &crate::i18n::tr("Review app leftovers"));
                let weak = window.downgrade();
                dialog.connect_response(None, move |_, response| {
                    if response == "review" {
                        if let Some(w) = weak.upgrade() {
                            w.navigate_to_page("applications-leftovers");
                        }
                    }
                });
                dialog.present();
            });
        });
        let monitor = gio::AppInfoMonitor::get();
        let changed = refresh.clone();
        monitor.connect_changed(move |_| changed());
        refresh();
        self.imp().app_monitor.replace(Some(monitor));
    }

    /// Show the AI Models and Games rows only when their scan found something.
    fn sync_optional_rows(&self, store: &super::review_scan_store::ReviewScanStore) {
        let rows = self.imp().nav_rows.borrow();
        for (item, label) in self.imp().optional_sizes.borrow().iter() {
            let kind = match item {
                NavItem::AiModels => super::review_scan_store::Kind::Models,
                _ => super::review_scan_store::Kind::Games,
            };
            let index = if *item == NavItem::AiModels { 6 } else { 7 };
            let bytes = store.items(kind).map(|i| i.bytes()).unwrap_or_else(|| {
                self.imp()
                    .summary
                    .borrow()
                    .as_ref()
                    .map_or(0, |s| s.bytes[index])
            });
            label.set_text(&bytesize::ByteSize(bytes).to_string());
            if let Some((_, row)) = rows.iter().find(|(i, _)| i == item) {
                row.set_visible(bytes > 0);
            }
        }
    }

    pub(crate) fn sync_sidebar_spinners(&self) {
        use super::review_scan_store::Kind;
        use crate::services::clean_sources::CleanPageKind;
        for (item, spinner) in self.imp().sidebar_spinners.borrow().iter() {
            let clean = match item {
                NavItem::Browsers => Some(CleanPageKind::AppCaches),
                NavItem::UserSystem => Some(CleanPageKind::DevTools),
                NavItem::SystemCleanup => Some(CleanPageKind::System),
                _ => None,
            };
            let review = match item {
                NavItem::LargeFiles => Some(Kind::LargeFiles),
                NavItem::Applications => Some(Kind::Apps),
                NavItem::AiModels => Some(Kind::Models),
                NavItem::Games => Some(Kind::Games),
                _ => None,
            };
            let loading = clean.is_some_and(|kind| {
                self.imp()
                    .clean_store
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| s.loading(kind))
            }) || review.is_some_and(|kind| {
                self.imp()
                    .review_store
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| s.loading(kind))
            });
            let loading = loading
                || (*item == NavItem::StorageAnalyzer
                    && self
                        .imp()
                        .storage_analyzer_page
                        .borrow()
                        .as_ref()
                        .is_some_and(|p| p.is_analyzing()));
            if spinner.is_spinning() != loading {
                spinner.set_spinning(loading);
            }
        }
    }

    pub fn save_scan_summary(&self) {
        if let Some(overview) = self.imp().overview_page.borrow().as_ref() {
            overview.save_summary();
        }
    }

    pub fn begin_cleaning(&self) {
        self.imp().cleaning.set(true);
    }

    pub fn finish_cleaning(&self) {
        self.imp().cleaning.set(false);
        if let Some(shell) = self.imp().shell.borrow().as_ref() {
            shell.set_visible_child_name("main");
            if let Some(child) = shell.child_by_name("cleaning") {
                shell.remove(&child);
            }
        }
        // Stay on the page the user was on; once it is on screen again the
        // category's own map sends its cells to the Trash.
        if let Some(update) = self.imp().pending_overview.borrow_mut().take() {
            let weak = self.downgrade();
            glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                if let Some(window) = weak.upgrade() {
                    if let Some(overview) = window.imp().overview_page.borrow().as_ref() {
                        overview.apply_removed(&update);
                    }
                }
            });
        }
    }

    pub fn refresh_dashboard(&self) {
        if let Some(dashboard) = self.imp().dashboard_page.borrow().as_ref() {
            dashboard.refresh();
        }
    }

    /// Publish successful removals everywhere without starting any scan.
    pub async fn apply_removed(&self, update: crate::services::removal_update::RemovalUpdate) {
        let update = Arc::new(update);
        if let Some(overview) = self.imp().overview_page.borrow().as_ref() {
            overview.begin_removal();
        }
        let review = self.imp().review_store.borrow().clone();
        let clean = self.imp().clean_store.borrow().clone();
        if let Some(review) = review {
            review.set_summary_present(true);
            review.apply_removed(&update);
        }
        if let Some(clean) = clean {
            clean.set_summary_present(true);
            clean.apply_removed(update.clone()).await;
        }
        if let Some(analyzer) = self.imp().storage_analyzer_page.borrow().as_ref() {
            analyzer.apply_removed(&update);
        }
        if self.imp().cleaning.get() {
            let mut pending = self.imp().pending_overview.borrow_mut();
            match pending.as_mut() {
                Some(earlier) => earlier.merge((*update).clone()),
                None => *pending = Some((*update).clone()),
            }
        } else if let Some(overview) = self.imp().overview_page.borrow().as_ref() {
            overview.apply_removed(&update);
        }
        self.refresh_dashboard();
    }

    pub fn apply_emptied_trash(&self, bytes: u64, remaining: u64) {
        if let Some(overview) = self.imp().overview_page.borrow().as_ref() {
            overview.empty_trash(bytes, remaining);
        }
    }

    pub fn set_cleanup_log(
        &self,
        clean_result: &crate::models::CleanResult,
        scan_result: &crate::models::ScanResult,
        journal_result: Option<&Result<String, String>>,
        automatic: bool,
        system_journal_enabled: bool,
    ) {
        if let Some(page) = self.imp().cleanup_log_page.borrow().as_ref() {
            page.set_result(
                clean_result,
                scan_result,
                journal_result,
                automatic,
                system_journal_enabled,
            );
            if let Some(about) = self.imp().about_page.borrow().as_ref() {
                about.record(format!(
                    "{}\n{}",
                    chrono::Local::now().format("%x %X"),
                    page.text()
                ));
            }
        }
    }

    pub fn record_cleanup_report(&self, text: &str) {
        if let Some(about) = self.imp().about_page.borrow().as_ref() {
            about.record(format!(
                "{}\n{}",
                chrono::Local::now().format("%x %X"),
                text
            ));
        }
    }

    pub fn append_cleanup_history(&self, text: &str) {
        if let Some(about) = self.imp().about_page.borrow().as_ref() {
            about.append_latest(text);
        }
    }

    pub fn show_cleanup_report(&self, text: &str, warning: bool) {
        if let Some(label) = self.imp().report_label.borrow().as_ref() {
            label.set_text(text);
        }
        if let Some(button) = self.imp().report_button.borrow().as_ref() {
            button.set_visible(true);
            button.remove_css_class("warning");
            button.add_css_class(if warning { "warning" } else { "accent" });
        }
    }

    /// A cleanup the safety limits stopped: say so in a dialog, with a way
    /// to the Settings where the limits live.
    /// With `allow`, an extra "Allow" button runs it: the cleanup again
    /// without the limits, for this time only.
    pub fn show_blocked_dialog(&self, reason: &str, allow: Option<Box<dyn Fn()>>) {
        let tr = crate::i18n::tr;
        let dialog =
            super::dialogs::message_dialog(Some(self.upcast_ref()), &tr("Cleanup Blocked"), reason);
        dialog.add_response("close", &tr("Close"));
        if allow.is_some() {
            dialog.add_response("allow", &tr("Allow"));
            dialog.set_response_appearance("allow", adw::ResponseAppearance::Destructive);
        }
        dialog.add_response("settings", &tr("Open Settings"));
        dialog.set_response_appearance("settings", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("settings"));
        dialog.set_close_response("close");
        let weak = self.downgrade();
        dialog.connect_response(None, move |_, response| {
            if response == "settings" {
                if let Some(window) = weak.upgrade() {
                    window.navigate_to_page("settings");
                }
            } else if response == "allow" {
                if let Some(allow) = &allow {
                    allow();
                }
            }
        });
        dialog.present();
    }

    /// Select the sidebar page with this stack name, as a click would.
    pub fn navigate_to_page(&self, stack_name: &str) {
        if stack_name == "applications" || stack_name == "applications-leftovers" {
            if let Some(page) = self.imp().uninstaller_page.borrow().as_ref() {
                page.show_leftovers(stack_name == "applications-leftovers");
            }
        }
        let stack_name = if stack_name == "applications-leftovers" {
            "applications"
        } else {
            stack_name
        };

        let row = self
            .imp()
            .nav_rows
            .borrow()
            .iter()
            .find(|(item, _)| item.stack_name() == stack_name)
            .map(|(_, row)| row.clone());
        if let (Some(sidebar), Some(row)) = (self.imp().sidebar_list.borrow().as_ref(), row) {
            sidebar.select_row(Some(&row));
        } else if let Some(stack) = self.imp().content_stack.borrow().as_ref() {
            if stack.child_by_name(stack_name).is_some() {
                stack.set_visible_child_name(stack_name);
                if let Some(sidebar) = self.imp().sidebar_list.borrow().as_ref() {
                    sidebar.unselect_all();
                }
            }
        }
    }

    pub fn navigate_to_dashboard(&self) {
        self.navigate_to_page(NavItem::Dashboard.stack_name());
    }

    /// Refresh the Overview readings (Ctrl+Shift+R).
    pub fn request_scan(&self) {
        if self.imp().cleaning.get() {
            return;
        }
        self.navigate_to_dashboard();
        if let Some(overview) = self.imp().overview_page.borrow().as_ref() {
            overview.scan_everything();
        }
    }

    fn review_cleanup(&self) {
        if self.operation_is_running() || self.imp().reviewing.get() {
            return;
        }
        let Some(store) = self.imp().clean_store.borrow().clone() else {
            return;
        };
        let Some(storage) = self.storage() else {
            return;
        };
        self.imp().reviewing.set(true);
        store.request_all(false);
        let window = self.clone();
        glib::spawn_future_local(async move {
            store.wait_ready().await;
            let items = super::clean_workflow::safe_items(&store);
            super::clean_workflow::confirm(&window.clone(), storage, store, items, move || {
                window.imp().reviewing.set(false);
            });
        });
    }

    pub fn request_clean(&self) {
        if self.operation_is_running() {
            self.show_operation_running_dialog();
            return;
        }
        self.navigate_to_dashboard();
        if let Some(dashboard) = self.imp().dashboard_page.borrow().as_ref() {
            dashboard.request_clean();
        }
    }

    pub fn request_scheduled_clean(&self) -> bool {
        if self.operation_is_running() {
            return false;
        }
        self.imp()
            .dashboard_page
            .borrow()
            .as_ref()
            .map(DashboardPage::request_scheduled_clean)
            .unwrap_or(false)
    }

    pub fn operation_is_running(&self) -> bool {
        self.imp().cleaning.get()
            || self.imp().reviewing.get()
            || self.dashboard_operation_is_running()
            || self.storage_analyzer_operation_is_running()
            || self.system_operation_is_running()
    }

    pub fn dashboard_operation_is_running(&self) -> bool {
        self.imp()
            .dashboard_page
            .borrow()
            .as_ref()
            .map(DashboardPage::operation_is_running)
            .unwrap_or(false)
    }

    pub fn storage_analyzer_operation_is_running(&self) -> bool {
        self.imp()
            .storage_analyzer_page
            .borrow()
            .as_ref()
            .map(StorageAnalyzerPage::operation_is_running)
            .unwrap_or(false)
    }

    pub fn system_operation_is_running(&self) -> bool {
        self.imp()
            .clean_pages
            .borrow()
            .iter()
            .any(|p| p.operation_is_running())
    }

    pub fn show_operation_running_dialog(&self) {
        if self.imp().cleaning.get() {
            let dialog = super::dialogs::message_dialog(
                Some(self.upcast_ref()),
                &crate::i18n::tr("Operation in Progress"),
                &crate::i18n::tr("Wait for the current cleanup to finish."),
            );
            dialog.add_response("close", &crate::i18n::tr("Close"));
            dialog.present();
            return;
        }
        if let Some(analyzer) = self.imp().storage_analyzer_page.borrow().as_ref() {
            if analyzer.operation_is_running() {
                analyzer.show_operation_running_dialog();
                return;
            }
        }
        if let Some(dashboard) = self.imp().dashboard_page.borrow().as_ref() {
            dashboard.show_operation_running_dialog();
        }
    }

    pub fn navigate_to_settings(&self) {
        self.navigate_to_page(NavItem::Settings.stack_name());
    }

    pub fn storage(&self) -> Option<Arc<Storage>> {
        self.imp().storage.borrow().as_ref().cloned()
    }

    pub fn sync_theme_preferences(&self) {
        theme::sync_runtime_classes(self);

        if let Some(stack) = self.imp().content_stack.borrow().as_ref() {
            stack.set_transition_duration(theme::transition_duration(self, 200));
        }
    }

    /// Rebuild the page tree so changing away from an already translated
    /// language always starts from the canonical English source strings.
    pub fn reload_translations(&self) {
        if self.operation_is_running() {
            self.show_operation_running_dialog();
            return;
        }
        self.setup_ui();
        self.sync_theme_preferences();
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    #[test]
    #[ignore = "Requires a GTK display; run with GDK_BACKEND=x11 and --ignored"]
    fn spanish_pages_keep_window_minimum_under_1000() {
        // A subprocess isolates locale, GTK and storage from the parallel tests.
        if std::env::var_os("MAGNIFICO_LAYOUT_FIXTURE").is_none() {
            let home = tempfile::tempdir().unwrap();
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .env("MAGNIFICO_LAYOUT_FIXTURE", "1")
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", home.path().join(".config"))
                .env("XDG_CACHE_HOME", home.path().join(".cache"))
                .env("XDG_DATA_HOME", home.path().join(".local/share"))
                .env("XDG_DATA_DIRS", home.path().join("empty"))
                .args([
                    "--exact",
                    "ui::main_window::layout_tests::spanish_pages_keep_window_minimum_under_1000",
                    "--ignored",
                    "--nocapture",
                ])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        gtk4::init().expect("GTK measurement requires a display connection");
        adw::init().unwrap();
        let display =
            gtk4::gdk::Display::default().expect("GTK measurement requires a display connection");
        crate::fonts::register();
        crate::i18n::set_language(crate::models::AppLanguage::Spanish);
        let css = gtk4::CssProvider::new();
        css.load_from_data(include_str!("../../data/style.css"));
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let window: MainWindow = glib::Object::builder()
            .property("default-width", 1000)
            .property("default-height", 830)
            .build();
        window.imp().storage.replace(Some(Arc::new(Storage::new())));
        window.setup_ui();
        window.add_css_class("magnifico-light");
        for label in window.imp().sidebar_sizes.borrow().iter() {
            label.set_text("128.1 GB");
        }
        window.present();
        let context = glib::MainContext::default();
        while context.pending() {
            context.iteration(false);
        }
        let sidebar = window.imp().sidebar_box.borrow().as_ref().unwrap().clone();
        for dark in [false, true] {
            let style = adw::StyleManager::default();
            style.set_color_scheme(if dark {
                adw::ColorScheme::ForceDark
            } else {
                adw::ColorScheme::ForceLight
            });
            window.sync_theme_preferences();
            let trash = window.imp().trash_label.borrow().as_ref().unwrap().clone();
            for text in [
                crate::i18n::tr("Trash…"),
                crate::i18n::tr("Measuring Trash…"),
                crate::i18n::tr_args("{size} in Trash", &[("{size}", "128.1 GB")]),
            ] {
                trash.set_text(&text);
                let (minimum, natural, _, _) = sidebar.measure(gtk4::Orientation::Horizontal, -1);
                assert_eq!(minimum, 260, "{text}: sidebar minimum");
                assert_eq!(natural, 260, "{text}: sidebar natural");
            }
            for name in [
                "dashboard",
                "browsers",
                "dev-tools",
                "system-cleanup",
                "large-files",
                "applications",
                "settings",
                "about",
            ] {
                window.navigate_to_page(name);
                let stack = window
                    .imp()
                    .content_stack
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .clone();
                let page = stack.visible_child().unwrap();
                let (page_min, _, _, _) = page.measure(gtk4::Orientation::Horizontal, -1);
                let (window_min, _, _, _) = window.measure(gtk4::Orientation::Horizontal, -1);
                println!("{name}: page minimum={page_min}, window minimum={window_min}");
                assert!(page_min <= 760, "{name}: content minimum {page_min}");
                assert!(window_min <= 1000, "{name}: window minimum {window_min}");
            }
        }
        window.close();
    }
}

/// The system's window buttons, all on the start side so they sit at the top
/// of the sidebar. Buttons that were on the end side are mirrored, keeping
/// close at the outer edge.
fn window_buttons_layout() -> String {
    let layout = gtk4::Settings::default()
        .and_then(|s| s.gtk_decoration_layout())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "menu:minimize,maximize,close".into());
    decoration_layout_at_end(&layout)
}

/// Window buttons only, all on the end side; buttons the system puts at the
/// start are mirrored so close stays on the outer edge.
fn decoration_layout_at_end(layout: &str) -> String {
    let (start, end) = layout.split_once(':').unwrap_or((layout, ""));
    let keep = |side: &str| {
        side.split(',')
            .map(str::trim)
            .filter(|b| matches!(*b, "close" | "minimize" | "maximize"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let mut buttons: Vec<_> = keep(start).into_iter().rev().collect();
    buttons.extend(keep(end));
    format!(":{}", buttons.join(","))
}

#[cfg(test)]
mod decoration_tests {
    #[test]
    fn window_buttons_go_to_the_end_without_menus() {
        assert_eq!(
            super::decoration_layout_at_end("appmenu:minimize,maximize,close"),
            ":minimize,maximize,close"
        );
        assert_eq!(
            super::decoration_layout_at_end("close,minimize:icon"),
            ":minimize,close"
        );
        assert_eq!(super::decoration_layout_at_end("menu:close"), ":close");
    }
}
