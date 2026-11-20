// Inspired by Purge SettingsView.swift (MIT, © 2026 Jithin Sabu).
// See THIRD_PARTY_LICENSES.md.
use crate::i18n::tr;
use crate::models::{AppLanguage, ColorScheme, RemovalMethod, ScheduleDay};
use crate::storage::Storage;
use crate::theme;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SettingsPage {
        pub storage: RefCell<Option<Arc<Storage>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SettingsPage {
        const NAME: &'static str = "MagnificoSettingsPage";
        type Type = super::SettingsPage;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for SettingsPage {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl WidgetImpl for SettingsPage {}
    impl BoxImpl for SettingsPage {}
}

glib::wrapper! {
    pub struct SettingsPage(ObjectSubclass<imp::SettingsPage>)
        @extends gtk4::Widget, gtk4::Box;
}

impl SettingsPage {
    pub fn new(storage: Arc<Storage>) -> Self {
        let page: Self = glib::Object::builder()
            .property("orientation", gtk4::Orientation::Vertical)
            .property("spacing", 0)
            .build();

        page.imp().storage.replace(Some(storage));
        page.setup_ui();
        page
    }

    fn setup_ui(&self) {
        let scrolled = gtk4::ScrolledWindow::builder()
            .vexpand(true)
            .hexpand(true)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .build();
        self.append(&scrolled);

        let content = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(24)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .hexpand(true)
            .build();
        scrolled.set_child(Some(&content));
        content.add_css_class("pc-settings");
        let title = gtk4::Label::new(Some(&tr("Settings")));
        title.add_css_class("pc-page-title");
        title.set_xalign(0.0);
        content.append(&title);

        let general_group = self.create_general_settings();
        content.append(&general_group);
        content.append(&self.create_deleted_apps_settings());

        let automatic_group = self.create_automatic_cleanup_settings();
        content.append(&automatic_group);

        let log_cleanup_group = self.create_log_cleanup_settings();
        content.append(&log_cleanup_group);

        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        content.append(&super::project_settings::build(storage.clone()));
        content.append(&super::CustomPage::new(storage));

        let safety_group = self.create_safety_settings();
        content.append(&safety_group);

        let deletion_group = self.create_deletion_settings();
        content.append(&deletion_group);

        let appearance_group = self.create_appearance_settings();
        content.append(&appearance_group);

        content.append(&self.create_diagnostics_settings());

        crate::i18n::translate_widget_tree(self);
    }

    fn create_general_settings(&self) -> gtk4::Widget {
        let group = adw::PreferencesGroup::new();
        group.set_title("Startup");

        let autostart_row = adw::ActionRow::new();
        autostart_row.set_title("Start Magnífico at Login");
        autostart_row.set_subtitle(
            "Start in the background so scheduled cleanup can run while you are signed in",
        );

        let autostart_switch = gtk4::Switch::new();
        autostart_switch.set_active(crate::autostart::is_enabled());
        autostart_switch.set_valign(gtk4::Align::Center);

        let changing_programmatically = Rc::new(std::cell::Cell::new(false));
        let changing_clone = changing_programmatically.clone();
        let row_clone = autostart_row.clone();
        autostart_switch.connect_active_notify(move |switch| {
            if changing_clone.get() {
                return;
            }

            let enabled = switch.is_active();
            match crate::autostart::set_enabled(enabled) {
                Ok(()) => row_clone.set_subtitle(if enabled {
                    "Starts in the background at login; scheduled cleanup can then run"
                } else {
                    "Off. Magnífico will not start automatically at login"
                }),
                Err(error) => {
                    row_clone.set_subtitle(&format!("Could not update login setting: {error}"));
                    changing_clone.set(true);
                    switch.set_active(!enabled);
                    changing_clone.set(false);
                }
            }
        });

        autostart_row.add_suffix(&autostart_switch);
        autostart_row.set_activatable_widget(Some(&autostart_switch));
        group.add(&autostart_row);
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let row = adw::ComboRow::new();
        row.set_title(&tr("When closing the window"));
        row.set_subtitle(&tr(
            "In the system tray, scheduled cleanup stays available.",
        ));
        row.set_model(Some(&gtk4::StringList::new(&[
            &tr("Quit completely"),
            &tr("Keep in the system tray"),
        ])));
        row.set_selected(u32::from(storage.get_settings().keep_in_tray));
        group.add(&row);
        row.connect_selected_notify(move |row| {
            let keep = row.selected() == 1;
            if let Err(e) = storage.update_settings(|s| s.keep_in_tray = keep) {
                tracing::warn!("Could not save tray setting: {e}");
            }
            if let Some(app) = gio::Application::default()
                .and_downcast::<crate::application::MagnificoApplication>()
            {
                app.sync_tray_preference();
            }
        });

        group.upcast()
    }

    fn create_deleted_apps_settings(&self) -> gtk4::Widget {
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let group = adw::PreferencesGroup::new();
        group.set_title(&tr("Deleted Apps"));
        let row = adw::ActionRow::new();
        row.set_title(&tr("Review leftovers when an app is deleted"));
        row.set_subtitle(&tr("Watch changes to installed desktop apps while Magnífico is running. Review leftover files before removing them."));
        let switch = gtk4::Switch::new();
        switch.set_valign(gtk4::Align::Center);
        switch.set_active(storage.get_settings().review_deleted_apps);
        row.add_suffix(&switch);
        row.set_activatable_widget(Some(&switch));
        switch.connect_active_notify(move |sw| {
            if let Err(e) = storage.update_settings(|s| s.review_deleted_apps = sw.is_active()) {
                tracing::warn!("Could not save deleted apps setting: {e}");
            }
        });
        group.add(&row);
        let review = adw::ActionRow::new();
        review.set_title(&tr("Review app leftovers"));
        let button = gtk4::Button::with_label(&tr("Review"));
        button.set_valign(gtk4::Align::Center);
        button.set_action_name(Some("win.open-page"));
        button.set_action_target_value(Some(&"applications-leftovers".to_variant()));
        review.add_suffix(&button);
        review.set_activatable_widget(Some(&button));
        group.add(&review);
        group.upcast()
    }

    fn create_diagnostics_settings(&self) -> gtk4::Widget {
        let group = adw::PreferencesGroup::new();
        group.set_title(&tr("Diagnostics"));

        let row = adw::ActionRow::new();
        row.set_title(&tr("System compatibility and environment information"));
        let button = gtk4::Button::with_label(&tr("Open Diagnostics"));
        button.set_valign(gtk4::Align::Center);
        row.add_suffix(&button);
        row.set_activatable_widget(Some(&button));
        group.add(&row);

        let page = self.downgrade();
        button.connect_clicked(move |_| {
            let Some(parent) = page
                .upgrade()
                .and_then(|page| page.root())
                .and_then(|root| root.downcast::<gtk4::Window>().ok())
            else {
                return;
            };

            let window = adw::Window::builder()
                .title(tr("Diagnostics"))
                .transient_for(&parent)
                .modal(true)
                .destroy_with_parent(true)
                .default_width(parent.width().clamp(360, 760))
                .default_height(parent.height().clamp(320, 700))
                .build();
            super::dialogs::prepare_window(&window);

            let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            content.append(&adw::HeaderBar::new());
            content.append(&super::DiagnosticsPage::new());
            window.set_content(Some(&content));
            window.present();
        });

        group.upcast()
    }

    fn create_automatic_cleanup_settings(&self) -> gtk4::Widget {
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let settings = storage.get_settings();

        let group = adw::PreferencesGroup::new();
        group.set_title("Cleaning");
        group.set_description(Some(
            "Runs selected cleanup rules only while Magnífico is open, with the same safety limits.",
        ));

        let schedule_row = adw::ActionRow::new();
        schedule_row.set_title("Scheduled Automatic Cleanup");
        schedule_row.set_subtitle(&Self::schedule_status(&settings));

        let schedule_switch = gtk4::Switch::new();
        schedule_switch.set_active(settings.scheduled_cleanup_enabled);
        schedule_switch.set_valign(gtk4::Align::Center);
        schedule_row.add_suffix(&schedule_switch);
        schedule_row.set_activatable_widget(Some(&schedule_switch));
        group.add(&schedule_row);

        let day_row = adw::ExpanderRow::new();
        day_row.set_title("Cleanup Days");
        day_row.set_subtitle(&Self::schedule_days_status(&settings));
        day_row.set_sensitive(settings.scheduled_cleanup_enabled);

        let selected_days = settings.effective_scheduled_cleanup_days();
        let selected_days_state = Rc::new(RefCell::new(selected_days.clone()));
        for day in ScheduleDay::weekdays() {
            let weekday_row = adw::ActionRow::new();
            weekday_row.set_title(day.display_name());

            let weekday_switch = gtk4::Switch::new();
            weekday_switch.set_active(selected_days.contains(day));
            weekday_switch.set_valign(gtk4::Align::Center);

            let storage_clone = storage.clone();
            let selected_days_clone = selected_days_state.clone();
            let schedule_row_clone = schedule_row.clone();
            let day_row_clone = day_row.clone();
            let selected_day = *day;
            weekday_switch.connect_active_notify(move |switch| {
                let mut days = selected_days_clone.borrow_mut();
                if switch.is_active() {
                    if !days.contains(&selected_day) {
                        days.push(selected_day);
                    }
                } else {
                    days.retain(|day| *day != selected_day);
                }

                // An enabled schedule must always have at least one day.
                if days.is_empty() {
                    drop(days);
                    switch.set_active(true);
                    return;
                }

                let selected: Vec<_> = ScheduleDay::weekdays()
                    .iter()
                    .copied()
                    .filter(|day| days.contains(day))
                    .collect();
                *days = selected.clone();
                drop(days);

                if let Err(error) = storage_clone.update_settings(|current| {
                    current.set_scheduled_cleanup_days(selected);
                    current.mark_schedule_changed();
                }) {
                    schedule_row_clone.set_subtitle(&format!("Could not save schedule: {error}"));
                    return;
                }
                Self::refresh_schedule_status(&storage_clone, &schedule_row_clone);
                Self::refresh_schedule_days_status(&storage_clone, &day_row_clone);
            });

            weekday_row.add_suffix(&weekday_switch);
            weekday_row.set_activatable_widget(Some(&weekday_switch));
            day_row.add_row(&weekday_row);
        }
        group.add(&day_row);

        let time_row = adw::ActionRow::new();
        time_row.set_title("Cleanup Time");
        time_row.set_subtitle("Uses your current local time");
        time_row.set_sensitive(settings.scheduled_cleanup_enabled);

        let time_controls = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        time_controls.set_valign(gtk4::Align::Center);

        let hour_spin = gtk4::SpinButton::with_range(0.0, 23.0, 1.0);
        hour_spin.set_digits(0);
        hour_spin.set_numeric(true);
        hour_spin.set_wrap(true);
        hour_spin.set_width_chars(2);
        hour_spin.set_value(settings.scheduled_cleanup_hour as f64);

        let separator = gtk4::Label::new(Some(":"));
        separator.add_css_class("heading");

        let minute_spin = gtk4::SpinButton::with_range(0.0, 59.0, 1.0);
        minute_spin.set_digits(0);
        minute_spin.set_numeric(true);
        minute_spin.set_wrap(true);
        minute_spin.set_width_chars(2);
        minute_spin.set_value(settings.scheduled_cleanup_minute as f64);

        time_controls.append(&hour_spin);
        time_controls.append(&separator);
        time_controls.append(&minute_spin);
        time_row.add_suffix(&time_controls);
        group.add(&time_row);

        let storage_clone = storage.clone();
        let day_row_clone = day_row.clone();
        let time_row_clone = time_row.clone();
        let schedule_row_clone = schedule_row.clone();
        schedule_switch.connect_active_notify(move |switch| {
            let enabled = switch.is_active();
            day_row_clone.set_sensitive(enabled);
            time_row_clone.set_sensitive(enabled);
            if let Err(error) = storage_clone.update_settings(|current| {
                current.scheduled_cleanup_enabled = enabled;
                current.mark_schedule_changed();
            }) {
                schedule_row_clone.set_subtitle(&format!("Could not save schedule: {error}"));
                return;
            }
            Self::refresh_schedule_status(&storage_clone, &schedule_row_clone);
        });

        let storage_clone = storage.clone();
        let schedule_row_clone = schedule_row.clone();
        hour_spin.connect_value_changed(move |spin| {
            if let Err(error) = storage_clone.update_settings(|current| {
                current.scheduled_cleanup_hour = spin.value() as u8;
                current.mark_schedule_changed();
            }) {
                schedule_row_clone.set_subtitle(&format!("Could not save schedule: {error}"));
                return;
            }
            Self::refresh_schedule_status(&storage_clone, &schedule_row_clone);
        });

        let storage_clone = storage.clone();
        let schedule_row_clone = schedule_row.clone();
        minute_spin.connect_value_changed(move |spin| {
            if let Err(error) = storage_clone.update_settings(|current| {
                current.scheduled_cleanup_minute = spin.value() as u8;
                current.mark_schedule_changed();
            }) {
                schedule_row_clone.set_subtitle(&format!("Could not save schedule: {error}"));
                return;
            }
            Self::refresh_schedule_status(&storage_clone, &schedule_row_clone);
        });

        group.upcast()
    }

    fn refresh_schedule_status(storage: &Storage, row: &adw::ActionRow) {
        let settings = storage.get_settings();
        row.set_subtitle(&Self::schedule_status(&settings));
    }

    fn refresh_schedule_days_status(storage: &Storage, row: &adw::ExpanderRow) {
        let settings = storage.get_settings();
        row.set_subtitle(&Self::schedule_days_status(&settings));
    }

    fn schedule_days_status(settings: &crate::models::AppSettings) -> String {
        let days = settings.effective_scheduled_cleanup_days();
        if days.len() == ScheduleDay::weekdays().len() {
            crate::i18n::tr("Every day")
        } else {
            days.iter()
                .map(|day| crate::i18n::tr(day.display_name()))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    fn schedule_status(settings: &crate::models::AppSettings) -> String {
        if settings.scheduled_cleanup_enabled {
            let days = settings.effective_scheduled_cleanup_days();
            let day_phrase = if days.len() == ScheduleDay::weekdays().len() {
                crate::i18n::tr("every day")
            } else {
                format!(
                    "{} {}",
                    crate::i18n::tr("on"),
                    Self::schedule_days_status(settings)
                )
            };
            let time = format!(
                "{:02}:{:02}",
                settings.scheduled_cleanup_hour, settings.scheduled_cleanup_minute
            );
            crate::i18n::tr_args(
                "Runs selected rules {days} at {time} while the app is open",
                &[("{days}", &day_phrase), ("{time}", &time)],
            )
        } else {
            crate::i18n::tr("Off. Enable to run selected rules at the chosen day and time")
        }
    }

    fn create_safety_settings(&self) -> gtk4::Widget {
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let settings = storage.get_settings();

        let group = adw::PreferencesGroup::new();
        group.set_title("Safety");
        group.set_description(Some("These settings help prevent accidental data loss"));

        let max_files_row = adw::ActionRow::new();
        max_files_row.set_title("Maximum Files Per Operation");
        max_files_row.set_subtitle("Stop after deleting this many files");

        let max_files_spin = gtk4::SpinButton::with_range(
            100.0,
            crate::models::AppSettings::MAX_FILES_PER_OPERATION_LIMIT as f64,
            100.0,
        );
        max_files_spin.set_digits(0);
        max_files_spin.set_value(settings.max_files_per_operation as f64);
        max_files_spin.set_valign(gtk4::Align::Center);

        let storage_clone = storage.clone();
        max_files_spin.connect_value_changed(move |row| {
            let value = (row.value() as usize).clamp(
                100,
                crate::models::AppSettings::MAX_FILES_PER_OPERATION_LIMIT,
            );
            if let Err(e) = storage_clone.update_settings(|s| {
                s.max_files_per_operation = value;
            }) {
                tracing::warn!("Failed to update settings: {}", e);
            }
        });

        max_files_row.add_suffix(&max_files_spin);
        group.add(&max_files_row);

        let gib = 1024_f64 * 1024.0 * 1024.0;
        let max_size_row = adw::ActionRow::new();
        let max_size_limit_gib =
            crate::models::AppSettings::MAX_SIZE_PER_OPERATION_LIMIT as f64 / gib;

        max_size_row.set_title("Maximum Data Per Operation");
        max_size_row.set_subtitle("Block cleanup when selected data exceeds this many GiB");

        // The upper bound now matches the model's 32 TiB cap instead of the
        // old 1 TiB spin-button range that made multi-TB disks uncleanable.
        let max_size_spin = gtk4::SpinButton::with_range(1.0, max_size_limit_gib, 1.0);
        max_size_spin.set_digits(0);
        max_size_spin.set_value(settings.max_size_per_operation as f64 / gib);
        max_size_spin.set_valign(gtk4::Align::Center);

        let storage_clone = storage.clone();
        max_size_spin.connect_value_changed(move |row| {
            let size_bytes = ((row.value() * gib) as u64).clamp(
                1024 * 1024,
                crate::models::AppSettings::MAX_SIZE_PER_OPERATION_LIMIT,
            );
            if let Err(e) = storage_clone.update_settings(|s| {
                s.max_size_per_operation = size_bytes;
            }) {
                tracing::warn!("Failed to update settings: {}", e);
            }
        });

        max_size_row.add_suffix(&max_size_spin);
        group.add(&max_size_row);

        group.upcast()
    }

    fn create_deletion_settings(&self) -> gtk4::Widget {
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let settings = storage.get_settings();

        let group = adw::PreferencesGroup::new();
        group.set_title("Deletion");
        group.set_description(Some("Choose how cleanup removes files"));

        let method_row = adw::ComboRow::new();
        method_row.set_title("Removal Method");
        let method_names = [
            crate::i18n::tr("Delete"),
            crate::i18n::tr("Move to Trash"),
            crate::i18n::tr("Secure Delete"),
        ];
        let method_refs: Vec<&str> = method_names.iter().map(String::as_str).collect();
        method_row.set_model(Some(&gtk4::StringList::new(&method_refs)));
        method_row.set_selected(settings.removal_method.settings_index());
        method_row.set_subtitle(&Self::removal_method_subtitle(settings.removal_method));

        let storage_clone = storage.clone();
        method_row.connect_selected_notify(move |row| {
            let method = RemovalMethod::from_settings_index(row.selected());
            row.set_subtitle(&Self::removal_method_subtitle(method));
            if let Err(e) = storage_clone.update_settings(|s| {
                s.removal_method = method;
            }) {
                tracing::warn!("Failed to update settings: {}", e);
            }
        });
        group.add(&method_row);

        let browser_row = adw::ActionRow::new();
        browser_row.set_title("Securely Delete Browser Data");
        browser_row
            .set_subtitle("Always overwrite cookies, history and sessions before deleting them");
        let browser_switch = gtk4::Switch::new();
        browser_switch.set_active(settings.secure_browser_data);
        browser_switch.set_valign(gtk4::Align::Center);
        let storage_clone = storage.clone();
        browser_switch.connect_active_notify(move |sw| {
            if let Err(e) = storage_clone.update_settings(|s| {
                s.secure_browser_data = sw.is_active();
            }) {
                tracing::warn!("Failed to update settings: {}", e);
            }
        });
        browser_row.add_suffix(&browser_switch);
        browser_row.set_activatable_widget(Some(&browser_switch));
        group.add(&browser_row);

        // Say plainly what an overwrite can guarantee on this disk.
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
        let facts = crate::services::storage_facts(&home);
        let disk_row = adw::ActionRow::new();
        disk_row.set_title("Your Disk");
        let (icon, text) = if facts.encrypted {
            (
                "pc-security-high-symbolic",
                "Encrypted. Deleted data cannot be read without your password.",
            )
        } else if facts.copy_on_write {
            (
                "pc-dialog-warning-symbolic",
                "Not encrypted, copy-on-write filesystem. Overwriting writes to new blocks, so old copies may remain. Disk encryption (LUKS) is the real protection.",
            )
        } else if facts.solid_state {
            (
                "pc-dialog-warning-symbolic",
                "Not encrypted, SSD. The drive may keep old copies of overwritten data until it reuses them. Disk encryption (LUKS) is the real protection.",
            )
        } else {
            (
                "pc-security-medium-symbolic",
                "Not encrypted, hard disk. Secure Delete overwrites the data in place.",
            )
        };
        disk_row.set_subtitle(text);
        disk_row.add_prefix(&gtk4::Image::from_icon_name(icon));
        group.add(&disk_row);

        group.upcast()
    }

    fn removal_method_subtitle(method: RemovalMethod) -> String {
        crate::i18n::tr(match method {
            RemovalMethod::Permanent => {
                "Fast. Files are gone from the folder, but the data stays on disk until it is reused"
            }
            RemovalMethod::Trash => {
                "Files can be restored from the trash. Space is freed only when you empty it"
            }
            RemovalMethod::Secure => {
                "Overwrites each file with random data before deleting it. Slower, cannot be undone"
            }
        })
    }

    fn create_log_cleanup_settings(&self) -> gtk4::Widget {
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let settings = storage.get_settings();

        let group = adw::PreferencesGroup::new();
        group.set_title("Log Cleanup");
        group.set_description(Some(
            "Optional cleanup of older log data. Both cleanup types are off by default.",
        ));

        let application_row = adw::ActionRow::new();
        application_row.set_title("Application Logs");
        application_row.set_subtitle(
            "Clean only old .log and rotated .log.* files from your user state and cache folders",
        );
        let application_switch = gtk4::Switch::new();
        application_switch.set_active(settings.application_log_cleanup_enabled);
        application_switch.set_valign(gtk4::Align::Center);
        application_row.add_suffix(&application_switch);
        application_row.set_activatable_widget(Some(&application_switch));
        group.add(&application_row);

        let journal_row = adw::ActionRow::new();
        journal_row.set_title("System Journal");
        let journal_available = crate::services::system_journal_available();
        let system_journal_enabled = journal_available && settings.system_journal_cleanup_enabled;
        if settings.system_journal_cleanup_enabled && !journal_available {
            if let Err(error) = storage.update_settings(|current| {
                current.system_journal_cleanup_enabled = false;
            }) {
                tracing::warn!("Failed to disable unavailable system journal cleanup: {error}");
            }
        }
        journal_row.set_subtitle(if journal_available {
            "Vacuum archived systemd journal data during manual cleanup; administrator approval is required"
        } else {
            "Unavailable: system journal cleanup requires pkexec and journalctl"
        });
        let journal_switch = gtk4::Switch::new();
        journal_switch.set_active(system_journal_enabled);
        journal_switch.set_sensitive(journal_available);
        journal_switch.set_valign(gtk4::Align::Center);
        journal_row.add_suffix(&journal_switch);
        journal_row.set_activatable_widget(Some(&journal_switch));
        group.add(&journal_row);

        let retention_row = adw::ActionRow::new();
        retention_row.set_title("Keep Recent Logs");
        retention_row.set_subtitle("Only clean log data older than this retention period");
        retention_row
            .set_sensitive(settings.application_log_cleanup_enabled || system_journal_enabled);

        let retention_controls = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        retention_controls.set_valign(gtk4::Align::Center);
        let retention_spin = gtk4::SpinButton::with_range(1.0, 3650.0, 1.0);
        retention_spin.set_digits(0);
        retention_spin.set_numeric(true);
        retention_spin.set_value(settings.log_retention_days as f64);
        retention_spin.set_width_chars(4);
        retention_controls.append(&retention_spin);
        retention_controls.append(&gtk4::Label::new(Some("days")));
        retention_row.add_suffix(&retention_controls);
        group.add(&retention_row);

        let storage_clone = storage.clone();
        let journal_switch_clone = journal_switch.clone();
        let retention_row_clone = retention_row.clone();
        application_switch.connect_active_notify(move |switch| {
            let enabled = switch.is_active();
            if let Err(error) = storage_clone.update_settings(|current| {
                current.application_log_cleanup_enabled = enabled;
            }) {
                tracing::warn!("Failed to update application log cleanup setting: {error}");
            }
            retention_row_clone.set_sensitive(enabled || journal_switch_clone.is_active());
        });

        let storage_clone = storage.clone();
        let application_switch_clone = application_switch.clone();
        let retention_row_clone = retention_row.clone();
        journal_switch.connect_active_notify(move |switch| {
            let enabled = switch.is_active();
            if let Err(error) = storage_clone.update_settings(|current| {
                current.system_journal_cleanup_enabled = enabled;
            }) {
                tracing::warn!("Failed to update system journal cleanup setting: {error}");
            }
            retention_row_clone.set_sensitive(enabled || application_switch_clone.is_active());
        });

        let storage_clone = storage;
        retention_spin.connect_value_changed(move |spin| {
            if let Err(error) = storage_clone.update_settings(|current| {
                current.log_retention_days = (spin.value() as u32).clamp(1, 3650);
            }) {
                tracing::warn!("Failed to update log retention setting: {error}");
            }
        });

        group.upcast()
    }

    fn create_appearance_settings(&self) -> gtk4::Widget {
        let storage = self.imp().storage.borrow().as_ref().unwrap().clone();
        let settings = storage.get_settings();

        let group = adw::PreferencesGroup::new();
        group.set_title("Appearance");

        let language_row = adw::ComboRow::new();
        language_row.set_title("Application Language");
        language_row.set_subtitle("Choose a language or follow your system automatically");

        let language_names = [
            crate::i18n::tr("Automatic"),
            "English".to_string(),
            "Ελληνικά".to_string(),
            "Italiano".to_string(),
            "Español".to_string(),
            "Deutsch".to_string(),
            "Français".to_string(),
        ];
        let language_name_refs: Vec<&str> = language_names.iter().map(String::as_str).collect();
        let languages = gtk4::StringList::new(&language_name_refs);
        language_row.set_model(Some(&languages));
        language_row.set_selected(settings.language.settings_index());

        let storage_clone = storage.clone();
        let language_row_weak = language_row.downgrade();
        let language_change_in_progress = Rc::new(std::cell::Cell::new(false));
        let language_change_guard = language_change_in_progress.clone();
        language_row.connect_selected_notify(move |row| {
            if language_change_guard.get() {
                return;
            }

            let window = row
                .root()
                .and_then(|root| root.downcast::<gtk4::Window>().ok())
                .and_then(|window| window.downcast::<crate::ui::MainWindow>().ok());
            if let Some(window) = window.as_ref() {
                if window.operation_is_running() {
                    window.show_operation_running_dialog();
                    language_change_guard.set(true);
                    row.set_selected(storage_clone.get_settings().language.settings_index());
                    language_change_guard.set(false);
                    return;
                }
            }

            let language = AppLanguage::from_settings_index(row.selected());
            if let Err(error) = storage_clone.update_settings(|settings| {
                settings.language = language;
            }) {
                tracing::warn!("Failed to update application language: {error}");
                return;
            }

            crate::i18n::set_language(language);
            let language_row_weak = language_row_weak.clone();
            glib::idle_add_local_once(move || {
                let Some(row) = language_row_weak.upgrade() else {
                    return;
                };
                let Some(root) = row.root() else {
                    return;
                };
                if let Ok(window) = root.downcast::<gtk4::Window>() {
                    if let Ok(window) = window.downcast::<crate::ui::MainWindow>() {
                        window.reload_translations();
                    }
                }
            });
        });
        group.add(&language_row);

        let theme_row = adw::ActionRow::new();
        theme_row.set_title(&tr("Theme"));
        theme_row.set_subtitle(&tr(
            "Choose a light or dark look, or match your system setting.",
        ));
        let choices = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        let mut buttons: Vec<gtk4::ToggleButton> = Vec::new();
        for (index, name) in ["Auto", "Light", "Dark"].iter().enumerate() {
            let button = gtk4::ToggleButton::new();
            button.add_css_class("pc-theme-choice");
            button.set_tooltip_text(Some(&tr(name)));
            if let Some(first) = buttons.first() {
                button.set_group(Some(first));
            }
            let content = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
            let drawing = gtk4::DrawingArea::new();
            drawing.set_content_width(80);
            drawing.set_content_height(52);
            drawing.set_draw_func(move |_, cr, width, height| {
                let (w, h) = (width as f64, height as f64);
                for part in 0..2 {
                    let dark = index == 2 || (index == 0 && part == 1);
                    cr.set_source_rgb(
                        if dark { 0.12 } else { 0.97 },
                        if dark { 0.115 } else { 0.96 },
                        if dark { 0.11 } else { 0.95 },
                    );
                    cr.rectangle(part as f64 * w / 2.0, 0.0, w / 2.0, h);
                    let _ = cr.fill();
                }
                cr.set_source_rgba(0.5, 0.5, 0.5, 0.18);
                cr.rectangle(0.0, 0.0, w * 0.26, h);
                let _ = cr.fill();
                for line in 0..3 {
                    cr.rectangle(7.0, 14.0 + f64::from(line) * 9.0, 9.0, 3.0);
                    let _ = cr.fill();
                    cr.rectangle(w * 0.34, 14.0 + f64::from(line) * 10.0, w * 0.56, 6.0);
                    let _ = cr.fill();
                }
            });
            content.append(&drawing);
            content.append(&gtk4::Label::new(Some(&tr(name))));
            button.set_child(Some(&content));
            button.set_active(
                index as u32
                    == match settings.color_scheme {
                        ColorScheme::System => 0,
                        ColorScheme::Light => 1,
                        ColorScheme::Dark => 2,
                    },
            );
            let storage = storage.clone();
            button.connect_toggled(move |button| {
                if !button.is_active() {
                    return;
                }
                let scheme = match index {
                    1 => ColorScheme::Light,
                    2 => ColorScheme::Dark,
                    _ => ColorScheme::System,
                };
                theme::apply_color_scheme(&adw::StyleManager::default(), scheme);
                if let Err(e) = storage.update_settings(|s| s.color_scheme = scheme) {
                    tracing::warn!("Could not save theme: {e}");
                }
            });
            choices.append(&button);
            buttons.push(button);
        }
        let weak_buttons: Vec<_> = buttons.iter().map(|b| b.downgrade()).collect();
        adw::StyleManager::default().connect_color_scheme_notify(move |manager| {
            let index = match manager.color_scheme() {
                adw::ColorScheme::ForceLight => 1,
                adw::ColorScheme::ForceDark => 2,
                _ => 0,
            };
            if let Some(button) = weak_buttons[index].upgrade() {
                button.set_active(true);
            }
        });
        theme_row.add_suffix(&choices);
        group.add(&theme_row);

        // Only offered where GTK can ask the compositor for the blur.
        if theme::blur_supported() {
            let blur_row = adw::ActionRow::new();
            blur_row.set_title("Blur Behind the Sidebar");
            blur_row.set_subtitle("The desktop shows through the sidebar, blurred");
            let blur_switch = gtk4::Switch::new();
            blur_switch.set_active(settings.window_blur);
            blur_switch.set_valign(gtk4::Align::Center);
            blur_row.add_suffix(&blur_switch);
            blur_row.set_activatable_widget(Some(&blur_switch));
            let storage_clone = storage.clone();
            blur_switch.connect_active_notify(move |switch| {
                let enabled = switch.is_active();
                if let Some(window) = switch.root() {
                    theme::apply_blur(&window, enabled);
                }
                if let Err(e) = storage_clone.update_settings(|s| s.window_blur = enabled) {
                    tracing::warn!("Failed to update settings: {}", e);
                }
            });
            group.add(&blur_row);
        }

        group.upcast()
    }
}
