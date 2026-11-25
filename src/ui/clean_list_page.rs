// Adapted from Purge GeneralModeView.swift, DevModeView.swift, ScanListRow.swift,
// SafetyFilterBar.swift, FilterChip.swift and PageHeaderActionBar.swift.
// MIT, © 2026 Jithin Sabu — https://github.com/jithin-sabu/purge-app.
// See THIRD_PARTY_LICENSES.md.
use super::{
    clean_scan_store::{CleanScanStore, Inventory},
    purge_widgets::{self as widgets, BadgeKind},
};
use crate::{
    i18n::{format_count, tr, tr_args},
    models::Safety,
    services::clean_sources::{CleanItem, CleanPageKind},
    storage::Storage,
};
use gtk4::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

#[derive(Clone)]
pub struct CleanListPage(Rc<Page>);
struct Page {
    widget: gtk4::Box,
    loader: super::scan_loading::ScanLoading,
    subtitle: gtk4::Label,
    scan: gtk4::Button,
    clean: gtk4::Button,
    clean_label: gtk4::Label,
    chips: gtk4::Box,
    select_all: gtk4::CheckButton,
    list: gtk4::Box,
    status: gtk4::Label,
    kind: CleanPageKind,
    storage: Arc<Storage>,
    items: RefCell<Vec<(Arc<CleanItem>, bool)>>,
    filter: Cell<usize>,
    by_name: Cell<bool>,
    syncing: Cell<bool>,
    loading: Cell<bool>,
    cleaning: Cell<bool>,
    scanned: Cell<bool>,
    generation: Cell<u64>,
    dirty: Cell<bool>,
    store: CleanScanStore,
    snapshot: RefCell<Option<Inventory>>,
}

impl CleanListPage {
    pub fn widget(&self) -> &gtk4::Box {
        &self.0.widget
    }
    pub fn operation_is_running(&self) -> bool {
        self.0.cleaning.get()
    }
    pub fn new(kind: CleanPageKind, storage: Arc<Storage>, store: CleanScanStore) -> Self {
        let widget = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        widget.add_css_class("pc-overview");
        widget.add_css_class("pc-clean-list");
        let subtitle = gtk4::Label::new(Some(&tr("Scanning…")));
        let scan = gtk4::Button::new();
        scan.add_css_class("pc-secondary-button");
        let scan_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        scan_content.append(&gtk4::Image::from_icon_name("pc-view-refresh-symbolic"));
        scan_content.append(&gtk4::Label::new(Some(&tr("Scan"))));
        scan.set_child(Some(&scan_content));
        let clean = gtk4::Button::new();
        clean.add_css_class("pc-primary");
        let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        content.append(&super::cleanup_fx::bolt_icon(18));
        let clean_label = gtk4::Label::new(None);
        content.append(&clean_label);
        clean.set_child(Some(&content));
        clean.set_sensitive(false);
        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        actions.set_valign(gtk4::Align::Start);
        actions.append(&scan);
        actions.append(&clean);
        widget.append(&widgets::page_header(kind.title(), &subtitle, &actions));
        let chips = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        widget.append(&chips);
        let controls = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        let select_all = gtk4::CheckButton::with_label(&tr("Select All"));
        select_all.set_hexpand(true);
        controls.append(&select_all);
        let sort = gtk4::DropDown::from_strings(&[&tr("Largest"), &tr("Name")]);
        sort.add_css_class("pc-sort");
        controls.append(&sort);
        widget.append(&controls);
        let status = gtk4::Label::new(None);
        status.set_wrap(true);
        status.set_xalign(0.0);
        status.add_css_class("pc-secondary");
        widget.append(&status);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_vexpand(true);
        let list = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        scroll.set_child(Some(&list));
        widget.append(&scroll);
        let loader = super::scan_loading::ScanLoading::wrap(&widget, kind.title());
        let page = Self(Rc::new(Page {
            widget,
            loader,
            subtitle,
            scan,
            clean,
            clean_label,
            chips,
            select_all,
            list,
            status,
            kind,
            storage,
            items: RefCell::new(Vec::new()),
            filter: Cell::new(if kind == CleanPageKind::AppCaches {
                1
            } else {
                0
            }),
            by_name: Cell::new(false),
            syncing: Cell::new(false),
            loading: Cell::new(false),
            cleaning: Cell::new(false),
            scanned: Cell::new(false),
            generation: Cell::new(0),
            dirty: Cell::new(true),
            store: store.clone(),
            snapshot: RefCell::new(None),
        }));
        let weak = Rc::downgrade(&page.0);
        page.0.scan.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                Self(p).refresh();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.clean.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                Self(p).confirm_clean();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.select_all.connect_toggled(move |check| {
            if let Some(p) = weak.upgrade() {
                if p.syncing.get() {
                    return;
                }
                for (item, selected) in p.items.borrow_mut().iter_mut() {
                    if matches_filter(item, p.filter.get()) {
                        *selected = check.is_active();
                    }
                }
                Self(p).render_rows();
            }
        });
        let weak = Rc::downgrade(&page.0);
        sort.connect_selected_notify(move |sort| {
            if let Some(p) = weak.upgrade() {
                p.by_name.set(sort.selected() == 1);
                Self(p).render_rows();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.widget.connect_map(move |_| {
            if let Some(p) = weak.upgrade() {
                p.store.request(p.kind, false);
                Self(p).sync_store();
            }
        });
        let weak = Rc::downgrade(&page.0);
        store.subscribe(move |kind| {
            if let Some(p) = weak.upgrade() {
                if p.kind == kind {
                    Self(p).sync_store();
                }
            }
        });
        page.sync_store();
        page
    }
    pub fn refresh(&self) {
        if !self.0.cleaning.get() {
            self.0.store.request(self.0.kind, true);
        }
    }
    fn sync_store(&self) {
        let p = &self.0;
        p.loading.set(p.store.loading(p.kind));
        p.loader.set_loading(
            p.store.items(p.kind).is_none(),
            p.kind.title(),
        );
        if let Some(items) = p.store.items(p.kind) {
            let changed = p
                .snapshot
                .borrow()
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &items));
            if changed {
                p.dirty.set(true);
                *p.items.borrow_mut() = items
                    .iter()
                    .cloned()
                    .map(|item| {
                        let selected = initially_selected(p.kind, &item);
                        (item, selected)
                    })
                    .collect();
                *p.snapshot.borrow_mut() = Some(items);
                p.scanned.set(true);
            }
        }
        p.scan.set_sensitive(!p.loading.get() && !p.cleaning.get());
        if p.loading.get() {
            p.subtitle.set_text(&tr("Scanning…"));
        }
        if p.dirty.get() || p.chips.first_child().is_none() {
            self.render();
        } else {
            self.update_selection();
        }
    }
    fn render(&self) {
        let p = &self.0;
        if !p.widget.is_mapped() {
            return;
        }
        p.dirty.set(false);
        p.syncing.set(true);
        let mut first: Option<gtk4::ToggleButton> = None;
        let mut child = p.chips.first_child();
        for (index, (icon, title)) in [
            ("pc-view-grid-symbolic", "All"),
            ("pc-emblem-ok-symbolic", "Safe to Clean"),
            ("pc-dialog-question-symbolic", "Check First"),
        ]
        .into_iter()
        .enumerate()
        {
            let count = p
                .items
                .borrow()
                .iter()
                .filter(|(i, _)| matches_filter(i, index))
                .count();
            let chip = if let Some(existing) = child.take() {
                child = existing.next_sibling();
                existing.downcast::<gtk4::ToggleButton>().unwrap()
            } else {
                let chip = widgets::filter_chip(icon, title, count, false);
                if let Some(first) = &first {
                    chip.set_group(Some(first));
                }
                if index == 2 {
                    chip.add_css_class("pc-check");
                }
                let weak = Rc::downgrade(p);
                chip.connect_toggled(move |chip| {
                    if let Some(p) = weak.upgrade() {
                        if chip.is_active() && !p.syncing.get() {
                            p.filter.set(index);
                            Self(p).render_rows();
                        }
                    }
                });
                p.chips.append(&chip);
                chip
            };
            if first.is_none() {
                first = Some(chip.clone());
            }
            if let Some(label) = chip
                .child()
                .and_downcast::<gtk4::Box>()
                .and_then(|content| content.last_child())
                .and_downcast::<gtk4::Label>()
            {
                label.set_text(&tr_args(
                    "{label} {count}",
                    &[("{label}", &tr(title)), ("{count}", &format_count(count))],
                ));
            }
            chip.set_active(p.filter.get() == index);
        }
        p.syncing.set(false);
        self.render_rows();
    }
    fn render_rows(&self) {
        let p = &self.0;
        let generation = p.generation.get().wrapping_add(1);
        p.generation.set(generation);
        while let Some(child) = p.list.first_child() {
            p.list.remove(&child);
        }
        if p.loading.get() && !p.scanned.get() {
            self.update_selection();
            return;
        }
        let items = p.items.borrow();
        let mut indices: Vec<_> = (0..items.len())
            .filter(|&i| matches_filter(&items[i].0, p.filter.get()))
            .collect();
        indices.sort_by(|&a, &b| {
            if p.by_name.get() {
                tr(&items[a].0.title)
                    .to_lowercase()
                    .cmp(&tr(&items[b].0.title).to_lowercase())
            } else {
                items[b]
                    .0
                    .bytes
                    .cmp(&items[a].0.bytes)
                    .then_with(|| items[a].0.title.cmp(&items[b].0.title))
            }
        });
        drop(items);
        self.update_selection();
        let weak = Rc::downgrade(p);
        glib::spawn_future_local(async move {
            for (position, index) in indices.into_iter().enumerate() {
                if position % 24 == 0 {
                    glib::timeout_future(std::time::Duration::from_millis(1)).await;
                }
                let Some(p) = weak.upgrade() else {
                    return;
                };
                if p.generation.get() != generation {
                    return;
                }
                let (item, selected) = p.items.borrow()[index].clone();
                let check = gtk4::CheckButton::new();
                check.set_active(selected);
                check.set_valign(gtk4::Align::Center);
                check.set_tooltip_text(Some(&tr(&item.title)));
                let icon = item_icon(&item);
                let card = widgets::item_card(
                    Some(&check),
                    &icon,
                    &item.title,
                    item.explanation,
                    &size(item.bytes),
                    None,
                );
                card.button.add_css_class("pc-clean-item");
                if item.command.is_none() {
                    let target = item.clone();
                    super::file_actions::path_clicks(&card.button, move || {
                        super::file_actions::common_folder(
                            target
                                .scan
                                .files
                                .iter()
                                .chain(&target.scan.directories)
                                .map(|f| f.path.as_path()),
                        )
                    });
                }
                card.description
                    .set_ellipsize(gtk4::pango::EllipsizeMode::None);
                card.description.set_wrap(true);
                card.description
                    .set_wrap_mode(gtk4::pango::WrapMode::WordChar);
                card.description.set_max_width_chars(80);
                // Keep the overview's component but place this list's size and
                // safety badge together at the trailing edge, as in Purge.
                if let Some(content) = card.button.child().and_downcast::<gtk4::Box>() {
                    content.remove(&card.percent);
                    content.remove(&card.size);
                    let trailing = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
                    trailing.set_valign(gtk4::Align::Center);
                    trailing.set_halign(gtk4::Align::End);
                    trailing.append(&card.size);
                    let badge = widgets::badge(if item.safety == Safety::Safe {
                        BadgeKind::Safe
                    } else {
                        BadgeKind::Check
                    });
                    badge.set_halign(gtk4::Align::End);
                    trailing.append(&badge);
                    content.append(&trailing);
                }
                let toggle = check.clone();
                card.button
                    .connect_clicked(move |_| toggle.set_active(!toggle.is_active()));
                let weak = Rc::downgrade(&p);
                check.connect_toggled(move |check| {
                    if let Some(p) = weak.upgrade() {
                        p.items.borrow_mut()[index].1 = check.is_active();
                        Self(p).update_selection();
                    }
                });
                p.list.append(&card.button);
            }
        });
    }
    fn update_selection(&self) {
        let p = &self.0;
        p.list.set_sensitive(!p.loading.get() && !p.cleaning.get());
        p.chips.set_sensitive(!p.loading.get() && !p.cleaning.get());
        let items = p.items.borrow();
        let summary = selection_summary(&items, p.filter.get());
        super::selection_bus::report(
            match p.kind {
                CleanPageKind::AppCaches => 0,
                CleanPageKind::DevTools => 1,
                CleanPageKind::System => 2,
            },
            summary.selected_bytes,
            summary.bytes,
        );
        p.clean_label.set_text(&tr_args(
            "Clean Selected ({size})",
            &[("{size}", &size(summary.selected_bytes))],
        ));
        p.clean
            .set_sensitive(summary.selected_count > 0 && !p.loading.get() && !p.cleaning.get());
        p.select_all
            .set_sensitive(summary.count > 0 && !p.loading.get() && !p.cleaning.get());
        p.syncing.set(true);
        p.select_all
            .set_active(summary.count > 0 && summary.selected_count == summary.count);
        p.select_all
            .set_inconsistent(summary.selected_count > 0 && summary.selected_count < summary.count);
        p.syncing.set(false);
        if p.loading.get() {
            p.status.set_visible(false);
        }
        if !p.loading.get() {
            p.subtitle.set_text(&tr_args(
                "{count} items · {size} recoverable",
                &[
                    ("{count}", &format_count(summary.count)),
                    ("{size}", &size(summary.bytes)),
                ],
            ));
            p.status.set_visible(summary.count == 0);
            p.status.set_text(&tr(if summary.count == 0 {
                if items.is_empty() {
                    "No cleanable items found. Scan again to check for changes."
                } else {
                    "No items match this filter."
                }
            } else {
                ""
            }));
        }
    }
    fn window(&self) -> Option<crate::ui::MainWindow> {
        self.0
            .widget
            .root()
            .and_downcast::<gtk4::Window>()
            .and_downcast::<crate::ui::MainWindow>()
    }
    fn confirm_clean(&self) {
        if let Some(window) = self.window() {
            if window.operation_is_running() {
                window.show_operation_running_dialog();
                return;
            }
        }
        let Some(window) = self.window() else {
            return;
        };
        let items = self
            .0
            .items
            .borrow()
            .iter()
            .filter(|(i, s)| *s && matches_filter(i, self.0.filter.get()))
            .map(|(i, _)| (self.0.kind, i.clone()))
            .collect();
        self.0.cleaning.set(true);
        self.0.scan.set_sensitive(false);
        self.update_selection();
        let page = self.clone();
        super::clean_workflow::confirm(
            &window,
            self.0.storage.clone(),
            self.0.store.clone(),
            items,
            move || {
                page.0.cleaning.set(false);
                page.0.scan.set_sensitive(true);
                page.update_selection();
            },
        );
    }
}
fn initially_selected(kind: CleanPageKind, item: &CleanItem) -> bool {
    kind == CleanPageKind::AppCaches && item.safety == Safety::Safe
}
#[derive(Debug, Default, PartialEq)]
struct SelectionSummary {
    count: usize,
    bytes: u64,
    selected_count: usize,
    selected_bytes: u64,
}
fn selection_summary(items: &[(Arc<CleanItem>, bool)], filter: usize) -> SelectionSummary {
    let mut summary = SelectionSummary::default();
    for (item, selected) in items
        .iter()
        .filter(|(item, _)| matches_filter(item, filter))
    {
        summary.count += 1;
        summary.bytes += item.bytes;
        if *selected {
            summary.selected_count += 1;
            summary.selected_bytes += item.bytes;
        }
    }
    summary
}

fn matches_filter(item: &CleanItem, filter: usize) -> bool {
    filter == 0 || (filter == 1) == (item.safety == Safety::Safe)
}
fn size(bytes: u64) -> String {
    bytesize::ByteSize(bytes).to_string()
}

fn item_icon(item: &CleanItem) -> gtk4::Box {
    let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    container.set_valign(gtk4::Align::Center);
    container.set_size_request(28, 28);
    let theme = gtk4::IconTheme::for_display(&gtk4::gdk::Display::default().unwrap());
    let real = item.app_icons.iter().find(|name| theme.has_icon(name));
    let fallback = match item.key {
        "updater" => "pc-folder-download-symbolic",
        "unknown" | "generic-cache" | "sensitive-cache" => "pc-folder-saved-search-symbolic",
        "journal" => "pc-text-x-generic-symbolic",
        "kernels" => "pc-computer-symbolic",
        "browser" => "pc-web-browser-symbolic",
        _ => "pc-package-x-generic-symbolic",
    };
    let image = gtk4::Image::from_icon_name(match real {
        Some(name) => name.as_str(),
        None if matches!(item.key, "unknown" | "generic-cache") => "pc-folder-unknown-symbolic",
        None => fallback,
    });
    image.set_pixel_size(28);
    container.append(&image);
    container
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(safety: Safety, bytes: u64) -> Arc<CleanItem> {
        Arc::new(CleanItem {
            key: "generic-cache",
            title: "App".into(),
            explanation: "",
            icon: "pc-folder-symbolic",
            app_icons: Vec::new(),
            safety,
            bytes,
            scan: crate::models::ScanResult::new(),
            command: None,
        })
    }
    #[test]
    fn changing_filter_changes_counts_recoverable_bytes_and_clean_selection() {
        let items = vec![
            (item(Safety::Safe, 100), true),
            (item(Safety::Safe, 200), false),
            (item(Safety::ReviewFirst, 400), false),
        ];
        assert_eq!(
            selection_summary(&items, 1),
            SelectionSummary {
                count: 2,
                bytes: 300,
                selected_count: 1,
                selected_bytes: 100
            }
        );
        assert_eq!(
            selection_summary(&items, 2),
            SelectionSummary {
                count: 1,
                bytes: 400,
                selected_count: 0,
                selected_bytes: 0
            }
        );
        assert_eq!(
            selection_summary(&items, 0),
            SelectionSummary {
                count: 3,
                bytes: 700,
                selected_count: 1,
                selected_bytes: 100
            }
        );
    }
    #[test]
    fn only_safe_app_caches_start_selected() {
        let safe = item(Safety::Safe, 1);
        let check = item(Safety::ReviewFirst, 1);
        for kind in [
            CleanPageKind::AppCaches,
            CleanPageKind::DevTools,
            CleanPageKind::System,
        ] {
            assert_eq!(
                initially_selected(kind, &safe),
                kind == CleanPageKind::AppCaches
            );
            assert!(!initially_selected(kind, &check));
        }
    }
}
