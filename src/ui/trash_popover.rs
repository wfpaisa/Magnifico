// Inspired by Purge TrashStore.swift (MIT, © 2026 Jithin Sabu).
// See THIRD_PARTY_LICENSES.md.
use super::{
    removal_dialog::{self, RemovalItem},
    review_widgets::size,
};
use crate::{
    i18n::{tr, tr_args},
    services::{trash_store, CleanProgress},
};
use gtk4::{glib, prelude::*};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub fn button(label: &gtk4::Label) -> gtk4::MenuButton {
    let button = gtk4::MenuButton::new();
    button.add_css_class("pc-trash-footer");
    let face = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    face.append(&gtk4::Image::from_icon_name("pc-user-trash-symbolic"));
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.set_max_width_chars(1);
    label.set_hexpand(true);
    label.set_xalign(0.0);
    // An empty Trash is not worth a red alert: tell the style when it is.
    let weak = button.downgrade();
    label.connect_label_notify(move |label| {
        let text = label.text();
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let empty = tokens.windows(2).any(|pair| pair == ["0", "B"]);
        // Texts with no size ("Measuring Trash…") keep what was shown.
        if empty || text.chars().any(|c| c.is_ascii_digit()) {
            if let Some(button) = weak.upgrade() {
                if empty {
                    button.add_css_class("pc-trash-empty");
                } else {
                    button.remove_css_class("pc-trash-empty");
                }
            }
        }
    });
    face.append(label);
    button.set_child(Some(&face));
    let popover = gtk4::Popover::new();
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    let total = gtk4::Label::new(Some(&tr("Measuring Trash…")));
    content.append(&total);
    let open = gtk4::Button::with_label(&tr("Open Trash"));
    open.connect_clicked(|_| {
        let _ = gio::AppInfo::launch_default_for_uri("trash:///", None::<&gio::AppLaunchContext>);
    });
    content.append(&open);
    let empty = gtk4::Button::with_label(&tr("Empty Trash"));
    empty.add_css_class("destructive-action");
    content.append(&empty);
    popover.set_child(Some(&content));
    button.set_popover(Some(&popover));
    let entries = Rc::new(RefCell::new(Vec::new()));
    let snapshot = entries.clone();
    let empty_clone = empty.clone();
    let label = label.clone();
    let total_after = total.clone();
    let label_after = label.clone();
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        empty_clone.set_sensitive(false);
        total.set_text(&tr("Measuring Trash…"));
        label.set_text(&tr("Measuring Trash…"));
        snapshot.borrow_mut().clear();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(trash_store::entries().map_err(|e| e.to_string()));
        });
        let (snapshot, total, empty, label) = (
            snapshot.clone(),
            total.clone(),
            empty_clone.clone(),
            label.clone(),
        );
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(items)) => {
                    let text = tr_args(
                        "{size} in Trash",
                        &[("{size}", &size(items.iter().map(|i| i.bytes).sum()))],
                    );
                    total.set_text(&text);
                    label.set_text(&text);
                    empty.set_sensitive(!items.is_empty());
                    snapshot.replace(items);
                }
                Ok(Err(e)) => {
                    total.set_text(&e);
                    label.set_text(&e);
                }
                Err(_) => {
                    total.set_text(&tr("Could not read Trash"));
                    label.set_text(&tr("Could not read Trash"));
                }
            }
        });
    });
    let refresh_on_show = refresh.clone();
    popover.connect_show(move |_| refresh_on_show());
    let weak = button.downgrade();
    empty.connect_clicked(move |_| {
        let Some(button) = weak.upgrade() else {
            return;
        };
        let parent = button.root().and_downcast::<gtk4::Window>();
        if let Some(main) = parent
            .as_ref()
            .and_then(|p| p.downcast_ref::<super::MainWindow>())
        {
            if main.operation_is_running() {
                main.show_operation_running_dialog();
                return;
            }
        }
        popover.popdown();
        let items = entries.borrow().clone();
        let cards = items
            .iter()
            .map(|i| RemovalItem {
                name: i.name.clone(),
                source: tr("Trash"),
                bytes: i.bytes,
                safe: false,
            })
            .collect();
        let entries = entries.clone();
        let total = total_after.clone();
        let label = label_after.clone();
        removal_dialog::present(
            &button,
            "Empty Trash",
            cards,
            false,
            &tr("Files will be permanently deleted. This cannot be undone."),
            move |picked| {
                let Some(picked) = picked else {
                    return;
                };
                let items = removal_dialog::pick(items, &picked);
                let progress = Arc::new(CleanProgress::default());
                let screen = super::clean_progress::CleanProgressWindow::show(
                    parent.as_ref(),
                    items.iter().map(|i| i.bytes).sum(),
                    items.len(),
                    progress.clone(),
                );
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(trash_store::empty(&items, &progress));
                });
                glib::spawn_future_local(async move {
                    let result = rx
                        .recv()
                        .await
                        .unwrap_or_else(|e| trash_store::EmptyResult {
                            errors: vec![e.to_string()],
                            removed: Vec::new(),
                            bytes: 0,
                        });
                    screen.close();
                    entries
                        .borrow_mut()
                        .retain(|e| !result.removed.contains(&e.uri));
                    let text = tr_args(
                        "{size} in Trash",
                        &[(
                            "{size}",
                            &size(entries.borrow().iter().map(|e| e.bytes).sum()),
                        )],
                    );
                    total.set_text(&text);
                    label.set_text(&text);
                    let errors = result.errors;
                    if let Some(main) = parent.and_downcast::<super::MainWindow>() {
                        main.apply_emptied_trash(
                            result.bytes,
                            entries.borrow().iter().map(|e| e.bytes).sum(),
                        );
                        main.record_cleanup_report(&format!(
                            "{}\n{}",
                            tr("Empty Trash"),
                            errors.join("\n")
                        ));
                        if !errors.is_empty() {
                            main.show_cleanup_report(&errors.join("\n"), true);
                        }
                    }
                });
            },
        );
    });
    button
}
