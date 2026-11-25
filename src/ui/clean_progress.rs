// Adapted from Purge DeletionProgress.swift (MIT, © 2026 Jithin Sabu).
// See THIRD_PARTY_LICENSES.md.
//! Full-window progress while a cleanup runs: one big number counting up to
//! the total and a thin progress bar.

use super::dialogs;
use super::scan_overview::format_size;
use crate::i18n::{tr, tr_args};
use crate::services::CleanProgress;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

pub struct CleanProgressWindow {
    window: Option<adw::Window>,
    parent: Option<glib::WeakRef<super::MainWindow>>,
    timer: Option<glib::SourceId>,
}

impl CleanProgressWindow {
    pub fn show(
        parent: Option<&gtk4::Window>,
        total_bytes: u64,
        total_files: usize,
        progress: Arc<CleanProgress>,
    ) -> Self {
        // In the main window there is no progress screen: the page stays
        // where it is and the map animates the result.
        if let Some(main) = parent.and_then(|p| p.downcast_ref::<super::MainWindow>()) {
            main.begin_cleaning();
            return Self {
                window: None,
                parent: Some(main.downgrade()),
                timer: None,
            };
        }
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        content.set_valign(gtk4::Align::Center);
        content.set_margin_top(36);
        content.set_margin_bottom(36);
        content.set_margin_start(48);
        content.set_margin_end(48);

        let big = gtk4::Label::new(Some(&format_size(0)));
        big.add_css_class("title-1");
        big.add_css_class("clean-progress-number");
        let of_total = gtk4::Label::new(Some(&tr_args(
            "of {size}",
            &[("{size}", &format_size(total_bytes))],
        )));
        of_total.add_css_class("dim-label");
        let bar = gtk4::ProgressBar::new();
        bar.set_margin_top(12);
        bar.add_css_class("pc-cleaning-bar");
        content.set_hexpand(true);
        content.set_vexpand(true);
        content.set_halign(gtk4::Align::Fill);
        let status = gtk4::Label::new(Some(&tr("Cleaning…")));
        status.add_css_class("dim-label");

        content.append(&big);
        content.append(&of_total);
        content.append(&bar);
        content.append(&status);
        let window = adw::Window::builder()
            .modal(true)
            .deletable(false)
            .default_width(900)
            .default_height(650)
            .build();
        window.set_transient_for(parent);
        dialogs::prepare_window(&window);
        window.set_content(Some(&content));
        window.present();
        let window = Some(window);

        let timer = glib::timeout_add_local(Duration::from_millis(100), move || {
            let bytes = progress.bytes_done.load(Ordering::Relaxed);
            let files = progress.files_done.load(Ordering::Relaxed);
            big.set_text(&format_size(bytes.min(total_bytes)));
            let fraction = if total_bytes > 0 {
                bytes as f64 / total_bytes as f64
            } else if total_files > 0 {
                files as f64 / total_files as f64
            } else {
                1.0
            };
            bar.set_fraction(fraction.clamp(0.0, 1.0));
            glib::ControlFlow::Continue
        });

        Self {
            window,
            parent: None,
            timer: Some(timer),
        }
    }

    pub fn close(mut self) {
        if let Some(timer) = self.timer.take() {
            timer.remove();
        }
        if let Some(window) = self.window {
            window.close();
        }
        if let Some(parent) = self.parent.and_then(|p| p.upgrade()) {
            parent.finish_cleaning();
        }
    }
}
