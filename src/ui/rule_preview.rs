//! "View files" button for a rule switch: while the switch is on, a button
//! next to it scans just that rule and shows what it would remove, in the
//! same layout as the dashboard's category details. Nothing is deleted.

use super::category_detail::{build_preview, Heading};
use super::dialogs;
use crate::i18n::tr;
use crate::models::ScanResult;
use crate::services::Scanner;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::sync::Arc;

/// Scans one rule. Runs on a worker thread.
pub type RuleScan = Arc<dyn Fn(&Scanner) -> ScanResult + Send + Sync>;

/// Add a "View files" button before `switch` on `row`, shown only while the
/// switch is on. Call this before adding the switch to the row.
pub fn attach(row: &adw::ActionRow, switch: &gtk4::Switch, title: &str, scan: RuleScan) {
    let button = gtk4::Button::from_icon_name("pc-view-list-symbolic");
    button.add_css_class("flat");
    button.set_valign(gtk4::Align::Center);
    button.set_tooltip_text(Some(&tr("View the files this would clean")));
    switch
        .bind_property("active", &button, "visible")
        .sync_create()
        .build();
    let title = title.to_string();
    button.connect_clicked(move |button| {
        let parent = button
            .root()
            .and_then(|root| root.downcast::<gtk4::Window>().ok());
        open(parent.as_ref(), &title, scan.clone());
    });
    row.add_suffix(&button);
}

fn open(parent: Option<&gtk4::Window>, title: &str, scan: RuleScan) {
    let window = adw::Window::builder()
        .modal(true)
        .default_width(980)
        .default_height(720)
        .build();
    window.set_transient_for(parent);
    dialogs::prepare_window(&window);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.set_margin_bottom(18);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let waiting = adw::StatusPage::new();
    waiting.set_title(&tr("Looking for files…"));
    let spinner = super::cleanup_fx::Spinner::spinning_widget(32);
    waiting.set_child(Some(&spinner));
    waiting.set_vexpand(true);
    content.append(&waiting);

    let toolbar = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    toolbar.append(&adw::HeaderBar::new());
    toolbar.append(&content);
    window.set_content(Some(&toolbar));
    window.present();

    let (sender, receiver) = async_channel::bounded::<ScanResult>(1);
    crate::runtime().spawn_blocking(move || {
        let _ = sender.send_blocking(scan(&Scanner::new()));
    });

    let title = title.to_string();
    let window_weak = window.downgrade();
    glib::spawn_future_local(async move {
        let Ok(found) = receiver.recv().await else {
            return;
        };
        let Some(window) = window_weak.upgrade() else {
            return;
        };
        content.remove(&waiting);
        if found.files.is_empty() {
            let empty = adw::StatusPage::new();
            empty.set_icon_name(Some("pc-emblem-ok-symbolic"));
            empty.set_title(&tr("Nothing to clean"));
            empty.set_description(Some(&title));
            empty.set_vexpand(true);
            content.append(&empty);
            return;
        }
        let close = window.downgrade();
        let heading = Heading {
            title,
            explanation: tr("Preview only. Nothing is deleted from this window."),
            icon: "pc-view-list-symbolic",
            safety: None,
            rules_page: None,
        };
        let view = build_preview(heading, found, move || {
            if let Some(w) = close.upgrade() {
                w.close();
            }
        });
        view.set_vexpand(true);
        content.append(&view);
    });
}
