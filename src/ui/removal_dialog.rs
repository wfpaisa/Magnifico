// Adapted from Purge DeletionConfirmSheet, RemovalSheetFooter and RemovalSizeLine
// (MIT, © 2026 Jithin Sabu). See THIRD_PARTY_LICENSES.md.
use super::{
    purge_widgets::{badge, BadgeKind},
    review_widgets::size,
};
use crate::i18n::{format_count, tr, tr_args};
use gtk4::prelude::*;
use libadwaita::{self as adw, prelude::*};
use std::{cell::RefCell, rc::Rc};

pub struct RemovalItem {
    pub name: String,
    pub source: String,
    pub bytes: u64,
    pub safe: bool,
}

/// An extra choice shown above the footer, such as also removing app data.
/// `item_bytes[i]` is added to the total while the switch is on and item `i`
/// is selected.
pub struct RemovalOption {
    pub label: String,
    pub item_bytes: Vec<u64>,
    pub active: bool,
}

/// Keep the entries of `values` whose index was picked in the sheet.
pub fn pick<T>(values: Vec<T>, picked: &[usize]) -> Vec<T> {
    values
        .into_iter()
        .enumerate()
        .filter(|(index, _)| picked.contains(index))
        .map(|(_, value)| value)
        .collect()
}

/// Every item is listed and removed together; nothing can be unchecked.
pub fn present_fixed(
    parent: &impl IsA<gtk4::Widget>,
    title: &str,
    items: Vec<RemovalItem>,
    trash: bool,
    notice: &str,
    response: impl FnOnce(bool) + 'static,
) {
    build(parent, title, items, trash, notice, None, false, move |r| {
        response(r.is_some())
    });
}

/// Items are listed largest first, each with a check. The callback gets the
/// indices (in the order given) of the checked items, or `None` if cancelled.
pub fn present(
    parent: &impl IsA<gtk4::Widget>,
    title: &str,
    items: Vec<RemovalItem>,
    trash: bool,
    notice: &str,
    response: impl FnOnce(Option<Vec<usize>>) + 'static,
) {
    build(parent, title, items, trash, notice, None, true, move |r| {
        response(r.map(|(picked, _)| picked))
    });
}

/// Like [`present`], with an optional switch; the callback also receives
/// whether that switch was on.
pub fn present_with_option(
    parent: &impl IsA<gtk4::Widget>,
    title: &str,
    items: Vec<RemovalItem>,
    trash: bool,
    notice: &str,
    option: Option<RemovalOption>,
    response: impl FnOnce(Option<(Vec<usize>, bool)>) + 'static,
) {
    build(parent, title, items, trash, notice, option, true, response);
}

/// Shared state of an open sheet: what is checked and the labels that show it.
struct Sheet {
    bytes: Vec<u64>,
    extra: Vec<u64>,
    selected: RefCell<Vec<bool>>,
    checks: RefCell<Vec<(usize, gtk4::CheckButton)>>,
    syncing: std::cell::Cell<bool>,
    headline: gtk4::Label,
    total: gtk4::Label,
    apply: gtk4::Button,
    all: Option<gtk4::CheckButton>,
    switch: Option<gtk4::Switch>,
    trash: bool,
}

impl Sheet {
    fn picked(&self) -> Vec<usize> {
        let selected = self.selected.borrow();
        (0..selected.len()).filter(|&i| selected[i]).collect()
    }

    fn refresh(&self) {
        let picked = self.picked();
        let count = picked.len();
        let bytes: u64 = picked.iter().map(|&i| self.bytes[i]).sum();
        let on = self.switch.as_ref().is_some_and(|s| s.is_active());
        let extra: u64 = if on {
            picked
                .iter()
                .map(|&i| self.extra.get(i).copied().unwrap_or(0))
                .sum()
        } else {
            0
        };
        self.headline.set_text(&tr_args(
            "{count} items · {size}",
            &[("{count}", &format_count(count)), ("{size}", &size(bytes))],
        ));
        self.total.set_text(&tr_args(
            "Freeing {size}",
            &[("{size}", &size(bytes + extra))],
        ));
        self.apply.set_label(&tr_args(
            if self.trash {
                "Move {count} to Trash"
            } else {
                "Delete {count}"
            },
            &[("{count}", &format_count(count))],
        ));
        self.apply.set_sensitive(count > 0);
        if let Some(all) = &self.all {
            let total = self.bytes.len();
            self.syncing.set(true);
            all.set_active(count > 0 && count == total);
            all.set_inconsistent(count > 0 && count < total);
            self.syncing.set(false);
        }
    }

    fn set_all(&self, on: bool) {
        self.selected.borrow_mut().iter_mut().for_each(|s| *s = on);
        self.syncing.set(true);
        for (_, check) in self.checks.borrow().iter() {
            check.set_active(on);
        }
        self.syncing.set(false);
        self.refresh();
    }
}

#[allow(clippy::too_many_arguments)]
fn build(
    parent: &impl IsA<gtk4::Widget>,
    title: &str,
    items: Vec<RemovalItem>,
    trash: bool,
    notice: &str,
    option: Option<RemovalOption>,
    selectable: bool,
    response: impl FnOnce(Option<(Vec<usize>, bool)>) + 'static,
) {
    let dialog = adw::Dialog::builder()
        .content_width(680)
        .title(tr(title))
        .build();
    dialog.add_css_class("magnifico-dialog");
    crate::theme::sync_runtime_classes(&dialog);
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&gtk4::Label::new(Some(&tr(title)))));
    content.append(&header);
    let count = items.len();
    // Summary above the list: what will be removed and how it is removed.
    let summary = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    summary.add_css_class("pc-removal-summary");
    let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    let headline = gtk4::Label::new(None);
    headline.set_xalign(0.0);
    headline.set_hexpand(true);
    headline.add_css_class("pc-removal-headline");
    top.append(&headline);
    let all = (selectable && count > 1).then(|| {
        let all = gtk4::CheckButton::with_label(&tr("Select All"));
        all.set_valign(gtk4::Align::Center);
        top.append(&all);
        all
    });
    summary.append(&top);
    if !notice.is_empty() {
        let text = gtk4::Label::new(Some(notice.trim()));
        text.set_wrap(true);
        text.set_xalign(0.0);
        text.add_css_class("pc-secondary");
        summary.append(&text);
    }
    content.append(&summary);
    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    body.set_margin_top(4);
    body.set_margin_bottom(16);
    body.set_margin_start(24);
    body.set_margin_end(24);
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        // Rows arrive after the dialog is shown, so reserve their height now.
        .min_content_height((count as i32 * 72).clamp(96, 420))
        .max_content_height(420)
        .propagate_natural_height(true)
        .child(&body)
        .build();
    content.append(&scroll);
    let switch = option.as_ref().map(|option| {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        row.add_css_class("pc-removal-option");
        let label = gtk4::Label::new(Some(&option.label));
        label.set_wrap(true);
        label.set_xalign(0.0);
        label.set_hexpand(true);
        let switch = gtk4::Switch::new();
        switch.set_active(option.active);
        switch.set_valign(gtk4::Align::Center);
        row.append(&label);
        row.append(&switch);
        content.append(&row);
        switch
    });
    let footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    footer.add_css_class("pc-removal-footer");
    let total = gtk4::Label::new(None);
    total.set_hexpand(true);
    total.set_xalign(0.0);
    total.add_css_class("pc-secondary");
    let cancel = gtk4::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("pc-secondary-button");
    let apply = gtk4::Button::new();
    apply.add_css_class("pc-danger-button");
    footer.append(&total);
    footer.append(&cancel);
    footer.append(&apply);
    content.append(&footer);
    dialog.set_child(Some(&content));

    let sheet = Rc::new(Sheet {
        bytes: items.iter().map(|i| i.bytes).collect(),
        extra: option.map(|o| o.item_bytes).unwrap_or_default(),
        selected: RefCell::new(vec![true; count]),
        checks: RefCell::new(Vec::new()),
        syncing: std::cell::Cell::new(false),
        headline,
        total,
        apply: apply.clone(),
        all,
        switch,
        trash,
    });
    sheet.refresh();
    if let Some(all) = &sheet.all {
        let weak = Rc::downgrade(&sheet);
        all.connect_toggled(move |all| {
            if let Some(sheet) = weak.upgrade() {
                if !sheet.syncing.get() {
                    // A partial selection becomes "all"; a full one becomes "none".
                    let on = all.is_active() || all.is_inconsistent();
                    sheet.set_all(on);
                }
            }
        });
    }
    if let Some(switch) = &sheet.switch {
        let weak = Rc::downgrade(&sheet);
        switch.connect_active_notify(move |_| {
            if let Some(sheet) = weak.upgrade() {
                sheet.refresh();
            }
        });
    }

    // Largest first, so the items that matter most are seen first.
    let mut order: Vec<_> = items.into_iter().enumerate().collect();
    order.sort_by_key(|(_, item)| std::cmp::Reverse(item.bytes));
    let weak_body = body.downgrade();
    let weak_dialog = dialog.downgrade();
    let weak_sheet = Rc::downgrade(&sheet);
    glib::spawn_future_local(async move {
        for (row_index, (index, item)) in order.into_iter().enumerate() {
            if row_index % 24 == 0 {
                glib::timeout_future(std::time::Duration::from_millis(1)).await;
            }
            let (Some(body), Some(dialog), Some(sheet)) = (
                weak_body.upgrade(),
                weak_dialog.upgrade(),
                weak_sheet.upgrade(),
            ) else {
                return;
            };
            if !dialog.is_visible() {
                return;
            }
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
            row.add_css_class("pc-removal-card");
            if selectable {
                let check = gtk4::CheckButton::new();
                check.set_valign(gtk4::Align::Center);
                check.set_active(sheet.selected.borrow()[index]);
                let weak = Rc::downgrade(&sheet);
                check.connect_toggled(move |check| {
                    if let Some(sheet) = weak.upgrade() {
                        if !sheet.syncing.get() {
                            sheet.selected.borrow_mut()[index] = check.is_active();
                            sheet.refresh();
                        }
                    }
                });
                row.append(&check);
                sheet.checks.borrow_mut().push((index, check));
            }
            let labels = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
            labels.set_hexpand(true);
            let name = gtk4::Label::new(Some(&item.name));
            name.set_xalign(0.0);
            name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            name.add_css_class("heading");
            let source = gtk4::Label::new(Some(&item.source));
            source.set_xalign(0.0);
            source.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            source.set_single_line_mode(true);
            source.set_tooltip_text(Some(&item.source));
            source.add_css_class("pc-secondary");
            labels.append(&name);
            labels.append(&source);
            let trailing = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
            trailing.set_valign(gtk4::Align::Center);
            let amount = gtk4::Label::new(Some(&size(item.bytes)));
            amount.add_css_class("pc-removal-size");
            amount.set_halign(gtk4::Align::End);
            trailing.append(&amount);
            let badge = badge(if item.safe {
                BadgeKind::Safe
            } else {
                BadgeKind::Check
            });
            badge.set_halign(gtk4::Align::End);
            trailing.append(&badge);
            row.append(&labels);
            row.append(&trailing);
            body.append(&row);
        }
    });

    let callback = Rc::new(RefCell::new(Some(response)));
    let cb = callback.clone();
    let weak = dialog.downgrade();
    apply.connect_clicked(move |_| {
        if let Some(dialog) = weak.upgrade() {
            let callback = cb.borrow_mut().take();
            let picked = sheet.picked();
            let on = sheet.switch.as_ref().is_some_and(|s| s.is_active());
            dialog.close();
            if let Some(callback) = callback {
                callback(Some((picked, on)));
            }
        }
    });
    let weak = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });
    dialog.connect_closed(move |_| {
        if let Some(callback) = callback.borrow_mut().take() {
            callback(None);
        }
    });
    // No dark veil over the window while the sheet is open.
    if let Some(root) = parent.root().map(|root| root.upcast::<gtk4::Widget>()) {
        root.add_css_class("pc-no-dim");
        dialog.connect_closed(move |_| root.remove_css_class("pc-no-dim"));
    }
    dialog.present(Some(parent));
    cancel.grab_focus();
}

/// Group a rule-based scan by its visible source rather than adding thousands of rows.
pub fn scan_items(scan: &crate::models::ScanResult) -> Vec<RemovalItem> {
    let mut groups = std::collections::BTreeMap::new();
    for file in &scan.files {
        let path = file.path.parent().unwrap_or(&file.path).to_path_buf();
        *groups.entry(path).or_insert(0u64) += file.size;
    }
    groups
        .into_iter()
        .map(|(path, bytes)| RemovalItem {
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            source: super::review_widgets::home_path(&path),
            bytes,
            safe: false,
        })
        .collect()
}
