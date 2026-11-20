use crate::storage::Storage;
use crate::theme;
use crate::ui::MainWindow;
use chrono::{Local, Utc};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, gio, glib};
use libadwaita as adw;
use libadwaita::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MagnificoApplication {
        pub storage: RefCell<Option<Arc<Storage>>>,
        pub window: RefCell<Option<MainWindow>>,
        pub tray_cmd: RefCell<Option<async_channel::Sender<crate::tray::TrayCmd>>>,
        pub tray_hint_shown: Cell<bool>,
        pub tray_available: Cell<bool>,
        pub start_hidden: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MagnificoApplication {
        const NAME: &'static str = "MagnificoApplication";
        type Type = super::MagnificoApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for MagnificoApplication {
        fn constructed(&self) {
            self.parent_constructed();

            let app = self.obj();
            app.set_application_id(Some(crate::APP_ID));
            app.set_resource_base_path(Some("/io/github/wfpaisa/Magnifico"));
            app.setup_actions();
        }
    }

    impl ApplicationImpl for MagnificoApplication {
        fn activate(&self) {
            let app = self.obj();
            let start_hidden = self.start_hidden.replace(false);

            if self.storage.borrow().is_none() {
                let storage = Arc::new(Storage::new());
                crate::i18n::set_language(storage.get_settings().language);
                theme::apply_color_scheme(
                    &adw::StyleManager::default(),
                    storage.get_settings().color_scheme,
                );
                *self.storage.borrow_mut() = Some(storage);
            }

            if let Some(window) = self.window.borrow().as_ref() {
                if !start_hidden {
                    window.present();
                }
                return;
            }

            let storage = self.storage.borrow().as_ref().unwrap().clone();
            let window = MainWindow::new(&app, storage.clone());
            window.set_hide_on_close(
                self.tray_available.get() && storage.get_settings().keep_in_tray,
            );

            let app_weak = app.downgrade();
            let size_storage = storage.clone();
            window.connect_close_request(move |window| {
                if window.operation_is_running() {
                    window.show_operation_running_dialog();
                    return glib::Propagation::Stop;
                }
                if !window.is_maximized() && !window.is_fullscreen() {
                    let size = window.default_size();
                    if let Err(error) =
                        size_storage.update_settings(|settings| settings.window_size = Some(size))
                    {
                        tracing::warn!("Could not save the window size: {}", error);
                    }
                }
                if let Some(app) = app_weak.upgrade() {
                    if window.property::<bool>("hide-on-close") {
                        app.notify_tray_hint_once();
                    }
                }
                glib::Propagation::Proceed
            });

            app.load_css();

            *self.window.borrow_mut() = Some(window.clone());
            if !start_hidden {
                window.present();
            }
        }

        fn startup(&self) {
            self.parent_startup();
            crate::autostart::is_enabled();
            self.obj()
                .connect_window_added(|_, window| crate::ui::pointer_cursor::install(window));

            if self.storage.borrow().is_none() {
                let storage = Arc::new(Storage::new());
                crate::i18n::set_language(storage.get_settings().language);
                theme::apply_color_scheme(
                    &adw::StyleManager::default(),
                    storage.get_settings().color_scheme,
                );
                *self.storage.borrow_mut() = Some(storage);
            }

            if let Some(display) = gdk::Display::default() {
                let icon_theme = gtk4::IconTheme::for_display(&display);

                if let Ok(exe_path) = std::env::current_exe() {
                    if let Some(exe_dir) = exe_path.parent() {
                        let dev_icons = exe_dir.join("../../data/icons");
                        if dev_icons.exists() {
                            if let Some(path_str) = dev_icons
                                .canonicalize()
                                .ok()
                                .and_then(|p| p.to_str().map(String::from))
                            {
                                icon_theme.add_search_path(&path_str);
                            }
                        }
                    }
                }

                icon_theme.add_search_path("data/icons");
                // Symbolic icons of the cleanup screen, built into the binary.
                icon_theme.add_resource_path("/io/github/wfpaisa/Magnifico/icons");
            }

            gtk4::Window::set_default_icon_name(crate::APP_ID);
            self.obj().setup_tray();
            self.obj().setup_in_app_scheduler();
        }
    }

    impl GtkApplicationImpl for MagnificoApplication {}
    impl AdwApplicationImpl for MagnificoApplication {}
}

glib::wrapper! {
    pub struct MagnificoApplication(ObjectSubclass<imp::MagnificoApplication>)
        @extends gio::Application, gtk4::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl MagnificoApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", crate::APP_ID)
            .property("flags", gio::ApplicationFlags::FLAGS_NONE)
            .build()
    }

    pub fn set_start_hidden(&self, start_hidden: bool) {
        self.imp().start_hidden.set(start_hidden);
    }

    fn setup_actions(&self) {
        let quit_action = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| {
                app.request_quit();
            })
            .build();

        let preferences_action = gio::ActionEntry::builder("preferences")
            .activate(|app: &Self, _, _| {
                app.show_preferences();
            })
            .build();

        let scan_action = gio::ActionEntry::builder("scan")
            .activate(|app: &Self, _, _| {
                if let Some(window) = app.imp().window.borrow().as_ref() {
                    window.present();
                    window.request_scan();
                }
            })
            .build();

        self.add_action_entries([scan_action, quit_action, preferences_action]);

        self.set_accels_for_action("app.quit", &["<Control>q"]);
        self.set_accels_for_action("app.scan", &["<Control><Shift>r"]);
        self.set_accels_for_action("window.close", &["<Control>w"]);
        self.set_accels_for_action("app.preferences", &["<Control>comma"]);
    }

    fn setup_tray(&self) {
        // Rasterised here, on the GTK main thread, because it goes through
        // gdk-pixbuf. The tray thread only ever receives finished pixels.
        //
        // The glyph colour is fixed to the panel foreground rather than the
        // application's colour scheme — see TRAY_FOREGROUND in tray.rs — so
        // there is nothing to re-render when the desktop switches light/dark.
        let (cmd_tx, action_rx, status_rx) =
            crate::tray::start_tray_service(crate::tray::render_tray_icons());
        *self.imp().tray_cmd.borrow_mut() = Some(cmd_tx.clone());

        let app_weak = self.downgrade();
        glib::spawn_future_local(async move {
            while let Ok(action) = action_rx.recv().await {
                let Some(app) = app_weak.upgrade() else {
                    break;
                };
                app.on_tray_action(action);
            }
        });

        let app_weak = self.downgrade();
        glib::spawn_future_local(async move {
            while let Ok(status) = status_rx.recv().await {
                let Some(app) = app_weak.upgrade() else {
                    break;
                };
                let available = matches!(status, crate::tray::TrayStatus::Available);
                app.imp().tray_available.set(available);
                let window = app.imp().window.borrow().as_ref().cloned();
                if let Some(window) = window {
                    window.set_hide_on_close(
                        available && app.storage().is_some_and(|s| s.get_settings().keep_in_tray),
                    );
                }
            }
        });

        self.sync_tray_preference();
    }

    pub fn sync_tray_preference(&self) {
        let enabled = self.storage().is_none_or(|s| s.get_settings().keep_in_tray);
        if let Some(tx) = self.imp().tray_cmd.borrow().as_ref() {
            let _ = tx.try_send(if enabled {
                crate::tray::TrayCmd::Enable
            } else {
                crate::tray::TrayCmd::Disable
            });
        }
        if let Some(window) = self.imp().window.borrow().as_ref() {
            window.set_hide_on_close(enabled && self.imp().tray_available.get());
        }
    }

    fn setup_in_app_scheduler(&self) {
        let app_weak = self.downgrade();
        glib::timeout_add_seconds_local(15, move || {
            let Some(app) = app_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };

            app.run_scheduled_cleanup_if_due();
            glib::ControlFlow::Continue
        });
    }

    fn run_scheduled_cleanup_if_due(&self) {
        let Some(storage) = self.storage() else {
            return;
        };
        let settings = storage.get_settings();
        if !settings.scheduled_cleanup_enabled {
            return;
        }

        // Settings saved before catch-up existed have no baseline. Start
        // counting from now so upgrading never triggers an immediate run.
        if settings.last_scheduled_cleanup.is_none() {
            if let Err(error) = storage.update_settings(|current| current.mark_schedule_changed()) {
                tracing::warn!("Could not save the cleanup schedule baseline: {error}");
            }
            return;
        }

        if !settings.scheduled_cleanup_due(&Local::now()) {
            return;
        }

        // A busy window refuses the request; the next tick retries.
        let started = self
            .imp()
            .window
            .borrow()
            .as_ref()
            .map(MainWindow::request_scheduled_clean)
            .unwrap_or(false);
        if started {
            tracing::info!("Starting scheduled cleanup");
            if let Err(error) = storage.update_settings(|current| {
                current.last_scheduled_cleanup = Some(Utc::now());
            }) {
                tracing::warn!("Could not record the scheduled cleanup time: {error}");
            }
        }
    }

    fn on_tray_action(&self, action: crate::tray::TrayAction) {
        match action {
            crate::tray::TrayAction::Open => self.present_main_window(),
            crate::tray::TrayAction::Clean => {
                self.present_main_window();
                if let Some(window) = self.imp().window.borrow().as_ref() {
                    window.request_clean();
                }
            }
            crate::tray::TrayAction::Close => self.request_quit(),
        }
    }

    fn present_main_window(&self) {
        if let Some(window) = self.imp().window.borrow().as_ref() {
            window.present();
        } else {
            self.activate();
        }
    }

    fn notify_tray_hint_once(&self) {
        if self.imp().tray_hint_shown.replace(true) {
            return;
        }

        let notification = gio::Notification::new(crate::APP_NAME);
        notification.set_body(Some(
            "Magnífico is still running in the system tray. Use the tray menu's Close action to exit.",
        ));
        notification.set_icon(&gio::ThemedIcon::new(crate::APP_ID));
        self.send_notification(Some("tray-hint"), &notification);
    }

    fn request_quit(&self) {
        if let Some(window) = self.imp().window.borrow().as_ref() {
            if window.operation_is_running() {
                window.present();
                window.show_operation_running_dialog();
                return;
            }
        }

        if let Some(sender) = self.imp().tray_cmd.borrow().as_ref() {
            let _ = sender.try_send(crate::tray::TrayCmd::Disable);
        }
        self.quit();
    }

    fn load_css(&self) {
        let provider = gtk4::CssProvider::new();
        Self::load_css_provider(&provider);

        let Some(display) = gtk4::gdk::Display::default() else {
            tracing::warn!("Could not get default display; skipping CSS provider registration");
            return;
        };
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        crate::theme::install_blur_css(&display);

        self.refresh_theme_state();

        let style_manager = adw::StyleManager::default();
        let app_weak = self.downgrade();
        style_manager.connect_color_scheme_notify(move |_| {
            if let Some(app) = app_weak.upgrade() {
                app.refresh_theme_state();
                app.queue_redraw_all_windows();
            }
        });

        if style_manager.find_property("accent-color").is_some() {
            let app_weak = self.downgrade();
            style_manager.connect_notify_local(Some("accent-color"), move |_, _| {
                if let Some(app) = app_weak.upgrade() {
                    app.refresh_theme_state();
                    app.queue_redraw_all_windows();
                }
            });
        }

        let app_weak = self.downgrade();
        style_manager.connect_dark_notify(move |_| {
            if let Some(app) = app_weak.upgrade() {
                app.refresh_theme_state();
                app.queue_redraw_all_windows();
            }
        });

        let app_weak = self.downgrade();
        style_manager.connect_high_contrast_notify(move |_| {
            if let Some(app) = app_weak.upgrade() {
                app.refresh_theme_state();
                app.queue_redraw_all_windows();
            }
        });

        let Some(gtk_settings) = gtk4::Settings::default() else {
            tracing::warn!("Could not get GTK settings; skipping settings notifications");
            return;
        };

        let app_weak = self.downgrade();
        gtk_settings.connect_gtk_theme_name_notify(move |_| {
            if let Some(app) = app_weak.upgrade() {
                app.refresh_theme_state();
                app.queue_redraw_all_windows();
            }
        });

        let app_weak = self.downgrade();
        gtk_settings.connect_gtk_icon_theme_name_notify(move |_| {
            if let Some(app) = app_weak.upgrade() {
                app.refresh_theme_state();
                app.queue_redraw_all_windows();
            }
        });

        let app_weak = self.downgrade();
        gtk_settings.connect_gtk_application_prefer_dark_theme_notify(move |_| {
            if let Some(app) = app_weak.upgrade() {
                app.refresh_theme_state();
                app.queue_redraw_all_windows();
            }
        });

        let app_weak = self.downgrade();
        gtk_settings.connect_gtk_enable_animations_notify(move |_| {
            if let Some(app) = app_weak.upgrade() {
                app.refresh_theme_state();
                app.queue_redraw_all_windows();
            }
        });
    }

    fn queue_redraw_all_windows(&self) {
        for widget in gtk4::Window::list_toplevels() {
            widget.queue_draw();
        }
    }

    fn refresh_theme_state(&self) {
        theme::sync_light_palette();
        // `self.windows()` only contains windows registered with the
        // application. Message dialogs and other transient windows are not
        // registered, but they still need the runtime classes to keep their
        // surface in sync with the main window.
        for widget in gtk4::Window::list_toplevels() {
            let Ok(window) = widget.downcast::<gtk4::Window>() else {
                continue;
            };
            if let Ok(main_window) = window.clone().downcast::<MainWindow>() {
                main_window.sync_theme_preferences();
            } else if window.has_css_class("magnifico-dialog") {
                theme::sync_runtime_classes(&window);
            }
        }
    }

    fn load_css_provider(provider: &gtk4::CssProvider) {
        if gio::resources_lookup_data(
            "/io/github/wfpaisa/Magnifico/style.css",
            gio::ResourceLookupFlags::NONE,
        )
        .is_ok()
        {
            provider.load_from_resource("/io/github/wfpaisa/Magnifico/style.css");
        } else {
            provider.load_from_data(include_str!("../data/style.css"));
        }
    }

    fn show_preferences(&self) {
        if let Some(window) = self.imp().window.borrow().as_ref() {
            window.navigate_to_settings();
        }
    }

    pub fn storage(&self) -> Option<Arc<Storage>> {
        self.imp().storage.borrow().as_ref().cloned()
    }
}

impl Default for MagnificoApplication {
    fn default() -> Self {
        Self::new()
    }
}
