// Adapted from Purge UninstallView.swift and UninstallAppGroupViews.swift
// (MIT, © 2026 Jithin Sabu). See THIRD_PARTY_LICENSES.md.
use super::{
    purge_widgets as widgets,
    review_scan_store::{AppInventory, Kind, ReviewScanStore},
    review_widgets as review,
};
use crate::{
    i18n::{format_count, tr, tr_args},
    services::{
        app_leftovers::{self, AppLeftover},
        app_uninstall::{self, InstalledApp},
    },
};
use gtk4::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    sync::Arc,
};

/// Index, name, icon, size, source, selectable and a folder to reveal.
type Entry = (
    usize,
    String,
    Option<String>,
    u64,
    String,
    bool,
    Option<std::path::PathBuf>,
);
#[derive(Clone)]
pub struct UninstallerPage(Rc<Inner>);
struct Inner {
    widget: gtk4::Box,
    loader: super::scan_loading::ScanLoading,
    subtitle: gtk4::Label,
    scan: gtk4::Button,
    uninstall: gtk4::Button,
    installed_tab: gtk4::ToggleButton,
    leftovers_tab: gtk4::ToggleButton,
    search: gtk4::SearchEntry,
    sort: gtk4::DropDown,
    grid_toggle: gtk4::ToggleButton,
    content: gtk4::Box,
    apps: RefCell<Vec<InstalledApp>>,
    leftovers: RefCell<Vec<AppLeftover>>,
    selected: RefCell<HashSet<usize>>,
    busy: Cell<bool>,
    snapshot: RefCell<Option<Arc<AppInventory>>>,
    store: ReviewScanStore,
    generation: Cell<u64>,
    dirty: Cell<bool>,
}
impl UninstallerPage {
    pub fn new(store: ReviewScanStore) -> Self {
        let widget = gtk4::Box::new(gtk4::Orientation::Vertical, 18);
        widget.add_css_class("pc-review-page");
        let subtitle = gtk4::Label::new(Some(&tr("Scan to find installed apps.")));
        let scan = review::action("pc-view-refresh-symbolic", "Rescan", false);
        let uninstall = review::action("pc-user-trash-symbolic", "Uninstall", true);
        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        actions.append(&scan);
        actions.append(&uninstall);
        widget.append(&widgets::page_header(
            "App Uninstaller",
            &subtitle,
            &actions,
        ));
        let controls = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let installed_tab =
            widgets::filter_chip("pc-view-grid-symbolic", "Installed Apps", 0, true);
        let leftovers_tab =
            widgets::filter_chip("pc-edit-clear-all-symbolic", "Leftovers", 0, false);
        leftovers_tab.set_group(Some(&installed_tab));
        let sort = review::dropdown(&["Name", "Largest", "Smallest"]);
        let search = review::search("Search apps");
        let list_toggle = gtk4::ToggleButton::new();
        list_toggle.set_icon_name("pc-view-list-symbolic");
        list_toggle.add_css_class("pc-chip");
        list_toggle.set_tooltip_text(Some(&tr("List view")));
        let grid_toggle = gtk4::ToggleButton::new();
        grid_toggle.set_icon_name("pc-view-grid-symbolic");
        grid_toggle.add_css_class("pc-chip");
        grid_toggle.set_group(Some(&list_toggle));
        grid_toggle.set_active(true);
        grid_toggle.set_tooltip_text(Some(&tr("Grid view")));
        for w in [
            installed_tab.clone().upcast::<gtk4::Widget>(),
            leftovers_tab.clone().upcast(),
            sort.clone().upcast(),
            search.clone().upcast(),
            list_toggle.upcast(),
            grid_toggle.clone().upcast(),
        ] {
            controls.append(&w);
        }
        widget.append(&review::horizontal_strip(&controls));
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&content));
        widget.append(&scroll);
        let loader = super::scan_loading::ScanLoading::wrap(&widget, "Applications");
        let page = Self(Rc::new(Inner {
            widget,
            loader,
            subtitle,
            scan,
            uninstall,
            installed_tab,
            leftovers_tab,
            search,
            sort,
            grid_toggle,
            content,
            apps: RefCell::default(),
            leftovers: RefCell::default(),
            selected: RefCell::default(),
            busy: Cell::new(false),
            snapshot: RefCell::default(),
            store,
            generation: Cell::new(0),
            dirty: Cell::new(true),
        }));
        let weak = Rc::downgrade(&page.0);
        page.0.scan.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).scan();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.leftovers_tab.connect_toggled(move |_| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let p = Self(inner);
            p.0.selected.borrow_mut().clear();
            p.render();
        });
        let weak = Rc::downgrade(&page.0);
        page.0.search.connect_search_changed(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).render();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.sort.connect_selected_notify(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).render();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.grid_toggle.connect_toggled(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).render();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.uninstall.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).confirm();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.widget.connect_map(move |_| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let p = Self(inner);
            p.0.store.request(Kind::Apps, false);
            p.sync_store();
        });
        let weak = Rc::downgrade(&page.0);
        page.0.store.subscribe(move || {
            if let Some(inner) = weak.upgrade() {
                Self(inner).sync_store();
            }
        });
        page.sync_store();
        page
    }
    pub fn widget(&self) -> &gtk4::Box {
        &self.0.widget
    }
    pub fn show_leftovers(&self, active: bool) {
        if active {
            self.0.leftovers_tab.set_active(true);
        } else {
            self.0.installed_tab.set_active(true);
        }
    }
    fn scan(&self) {
        if !self.0.busy.get() {
            self.0.store.request(Kind::Apps, true);
        }
    }
    fn sync_store(&self) {
        if let Some(snapshot) = self.0.store.apps() {
            let changed = self
                .0
                .snapshot
                .borrow()
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &snapshot));
            if changed {
                self.0.dirty.set(true);
                *self.0.apps.borrow_mut() = snapshot.apps.clone();
                *self.0.leftovers.borrow_mut() = snapshot.leftovers.clone();
                *self.0.snapshot.borrow_mut() = Some(snapshot);
                self.0.selected.borrow_mut().clear();
            }
        }
        if self.0.dirty.get() || self.0.content.first_child().is_none() {
            self.render();
        } else {
            self.update();
        }
    }
    fn entries(&self) -> Vec<Entry> {
        let mut entries: Vec<_> = if self.0.leftovers_tab.is_active() {
            self.0
                .leftovers
                .borrow()
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    (
                        i,
                        l.name.clone(),
                        l.icon.clone(),
                        l.size(),
                        tr("Review first"),
                        l.selectable(),
                        l.locations.first().map(|l| l.target.path.clone()),
                    )
                })
                .collect()
        } else {
            self.0
                .apps
                .borrow()
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    (
                        i,
                        a.name.clone(),
                        a.icon.clone(),
                        a.size(),
                        if a.admin() {
                            format!("{} · {}", tr(a.source()), tr("Requires admin"))
                        } else {
                            tr(a.source())
                        },
                        a.selectable(),
                        a.targets
                            .iter()
                            .chain(&a.data_targets)
                            .next()
                            .map(|t| t.path.clone()),
                    )
                })
                .collect()
        };
        let query = self.0.search.text().to_lowercase();
        entries.retain(|(_, name, _, _, _, _, _)| name.to_lowercase().contains(&query));
        entries.sort_by(|a, b| match self.0.sort.selected() {
            1 => b.3.cmp(&a.3),
            2 => a.3.cmp(&b.3),
            _ => a.1.to_lowercase().cmp(&b.1.to_lowercase()),
        });
        entries
    }
    fn render(&self) {
        let p = &self.0;
        if !p.widget.is_mapped() {
            return;
        }
        p.dirty.set(false);
        while let Some(child) = p.content.first_child() {
            p.content.remove(&child);
        }
        let generation = p.generation.get().wrapping_add(1);
        p.generation.set(generation);
        if p.snapshot.borrow().is_none()
            && !p.store.has_summary()
            && p.store.error(Kind::Apps).is_none()
        {
            p.loader.set_loading(true, "Applications");
            self.update();
            return;
        }
        p.loader.set_loading(false, "Applications");
        set_tab(&p.installed_tab, "Installed Apps", p.apps.borrow().len());
        set_tab(&p.leftovers_tab, "Leftovers", p.leftovers.borrow().len());
        let entries = self.entries();
        let grid = p.grid_toggle.is_active();
        let flow = gtk4::FlowBox::new();
        flow.set_selection_mode(gtk4::SelectionMode::None);
        flow.set_homogeneous(true);
        flow.set_min_children_per_line(1);
        flow.set_max_children_per_line(4);
        flow.set_column_spacing(12);
        flow.set_row_spacing(12);
        if grid {
            p.content.append(&flow);
        }
        if entries.is_empty() {
            p.content.append(&gtk4::Label::new(Some(
                &p.store
                    .error(Kind::Apps)
                    .unwrap_or_else(|| tr("No apps match these filters.")),
            )));
        }
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            for (index, (i, name, icon, bytes, source, selectable, path)) in
                entries.into_iter().enumerate()
            {
                if index % 24 == 0 {
                    glib::timeout_future(std::time::Duration::from_millis(1)).await;
                }
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let page = Self(inner);
                if page.0.generation.get() != generation {
                    return;
                }
                let p = &page.0;
                let check = gtk4::CheckButton::new();
                check.add_css_class("pc-radio-selection");
                check.set_active(p.selected.borrow().contains(&i));
                check.set_sensitive(selectable);
                check.set_tooltip_text(Some(&name));
                let image = app_icon(icon.as_deref());
                image.set_pixel_size(if grid { 64 } else { 40 });
                let button = gtk4::Button::new();
                button.add_css_class("pc-item-row");
                if grid {
                    button.add_css_class("pc-app-card");
                    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
                    check.set_halign(gtk4::Align::Start);
                    content.append(&check);
                    image.set_halign(gtk4::Align::Center);
                    content.append(&image);
                    let title = gtk4::Label::new(Some(&name));
                    title.add_css_class("pc-row-title");
                    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    title.set_max_width_chars(18);
                    content.append(&title);
                    let size = gtk4::Label::new(Some(&review::size(bytes)));
                    size.add_css_class("pc-secondary");
                    content.append(&size);
                    let source = gtk4::Label::new(Some(&source));
                    source.add_css_class("caption");
                    source.add_css_class("pc-secondary");
                    source.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    source.set_max_width_chars(22);
                    content.append(&source);
                    button.set_child(Some(&content));
                } else {
                    let icon = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
                    icon.append(&image);
                    let row = widgets::item_card(
                        Some(&check),
                        &icon,
                        &name,
                        &source,
                        &review::size(bytes),
                        None,
                    );
                    if let Some(child) = row.button.child() {
                        row.button.set_child(None::<&gtk4::Widget>);
                        button.set_child(Some(&child));
                    }
                }
                button.set_tooltip_text(Some(&format!("{name}\n{source}")));
                if let Some(path) = path {
                    super::file_actions::path_clicks(&button, move || Some(path.clone()));
                }
                button.set_sensitive(selectable);
                let c = check.clone();
                button.connect_clicked(move |_| c.set_active(!c.is_active()));
                let weak = Rc::downgrade(&page.0);
                check.connect_toggled(move |c| {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    let page = Self(inner);
                    if c.is_active() {
                        page.0.selected.borrow_mut().insert(i);
                    } else {
                        page.0.selected.borrow_mut().remove(&i);
                    }
                    page.update();
                });
                if grid {
                    flow.insert(&button, -1);
                } else {
                    p.content.append(&button);
                }
            }
        });
        self.update();
    }
    fn update(&self) {
        let p = &self.0;
        let busy = p.busy.get() || p.store.loading(Kind::Apps);
        p.scan.set_sensitive(!busy);
        p.content.set_sensitive(!busy);
        p.installed_tab.set_sensitive(!busy);
        p.leftovers_tab.set_sensitive(!busy);
        p.sort.set_sensitive(!busy);
        p.search.set_sensitive(!busy);
        p.grid_toggle.set_sensitive(!busy);
        let entries = self.entries();
        p.uninstall
            .set_sensitive(!busy && entries.iter().any(|e| p.selected.borrow().contains(&e.0)));
        if busy || p.snapshot.borrow().is_none() && p.store.error(Kind::Apps).is_none() {
            p.subtitle.set_text(&tr("Scanning…"));
        } else {
            p.subtitle.set_text(&tr_args(
                "{count} apps · {size}",
                &[
                    ("{count}", &format_count(entries.len())),
                    ("{size}", &review::size(entries.iter().map(|e| e.3).sum())),
                ],
            ));
        }
    }
    fn confirm(&self) {
        if self.0.busy.get() {
            return;
        }
        let visible: HashSet<_> = self.entries().into_iter().map(|e| e.0).collect();
        let selected = |i: &usize| visible.contains(i) && self.0.selected.borrow().contains(i);
        let leftovers: Vec<_> = if self.0.leftovers_tab.is_active() {
            self.0
                .leftovers
                .borrow()
                .iter()
                .enumerate()
                .filter(|(i, l)| selected(i) && l.selectable())
                .map(|(_, l)| l.clone())
                .collect()
        } else {
            vec![]
        };
        let apps: Vec<_> = if !self.0.leftovers_tab.is_active() {
            self.0
                .apps
                .borrow()
                .iter()
                .enumerate()
                .filter(|(i, a)| selected(i) && a.selectable())
                .map(|(_, a)| a.clone())
                .collect()
        } else {
            vec![]
        };
        if apps.is_empty() && leftovers.is_empty() {
            return;
        }
        let notice = tr("Review the apps, commands and data below. Listed user files will be moved to Trash. Package removal cannot be undone from Trash.");
        let paths = |targets: &[crate::services::TrashTarget]| {
            targets
                .iter()
                .map(|t| review::home_path(&t.path))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let cards = apps
            .iter()
            .map(|app| super::removal_dialog::RemovalItem {
                name: app.name.clone(),
                bytes: app.bytes,
                safe: false,
                source: [
                    tr(app.source()),
                    app.command().map(|c| c.join(" ")).unwrap_or_default(),
                    paths(&app.targets),
                    if app.data_targets.is_empty() {
                        String::new()
                    } else {
                        tr_args(
                            "Data ({size}): {paths}",
                            &[
                                ("{size}", &review::size(app.data_bytes)),
                                ("{paths}", &paths(&app.data_targets)),
                            ],
                        )
                    },
                ]
                .into_iter()
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            })
            .chain(leftovers.iter().map(|app| {
                super::removal_dialog::RemovalItem {
                    name: app.name.clone(),
                    bytes: app.size(),
                    safe: false,
                    source: app
                        .locations
                        .iter()
                        .map(|l| review::home_path(&l.target.path))
                        .collect::<Vec<_>>()
                        .join("\n"),
                }
            }))
            .collect();
        let data_bytes: u64 = apps.iter().map(|a| a.data_bytes).sum();
        let option = apps.iter().any(|a| !a.data_targets.is_empty()).then(|| {
            super::removal_dialog::RemovalOption {
                label: tr_args(
                    "Also remove app data (caches, settings and saved data) — {size}",
                    &[("{size}", &review::size(data_bytes))],
                ),
                item_bytes: apps
                    .iter()
                    .map(|a| a.data_bytes)
                    .chain(leftovers.iter().map(|_| 0))
                    .collect(),
                active: true,
            }
        });
        let p = self.clone();
        super::removal_dialog::present_with_option(
            &self.0.widget,
            "Confirm Uninstall",
            cards,
            true,
            &notice,
            option,
            move |answer| {
                let Some((picked, include_data)) = answer else {
                    return;
                };
                let first = apps.len();
                let apps = super::removal_dialog::pick(apps, &picked);
                let leftovers = super::removal_dialog::pick(
                    leftovers,
                    &picked
                        .iter()
                        .filter_map(|i| i.checked_sub(first))
                        .collect::<Vec<_>>(),
                );
                if p.0.busy.replace(true) {
                    return;
                }
                p.update();

                let progress = Arc::new(crate::services::CleanProgress::default());
                let parent = p.0.widget.root().and_downcast::<gtk4::Window>();
                let screen = super::clean_progress::CleanProgressWindow::show(
                    parent.as_ref(),
                    apps.iter()
                        .map(|a| a.bytes + if include_data { a.data_bytes } else { 0 })
                        .sum::<u64>()
                        + leftovers.iter().map(|a| a.size()).sum::<u64>(),
                    apps.len() + leftovers.len(),
                    progress.clone(),
                );
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let home = dirs::home_dir().unwrap_or_default();
                    let mut lines = Vec::new();
                    let mut update = crate::services::removal_update::RemovalUpdate::default();
                    for app in apps {
                        lines.push(
                            match app_uninstall::uninstall_with_progress(
                                &home,
                                &app,
                                include_data,
                                Some(&progress),
                            ) {
                                Ok(outcome) => {
                                    update.merge(outcome.update);
                                    if outcome.errors.is_empty() {
                                        tr_args("Uninstalled {app}", &[("{app}", &app.name)])
                                    } else {
                                        outcome.errors.join("\n")
                                    }
                                }
                                Err(e) => format!("{}: {e}", app.name),
                            },
                        );
                    }
                    if !leftovers.is_empty() {
                        let result =
                            app_leftovers::trash_with_progress(&home, &leftovers, Some(&progress));
                        lines.push(tr_args(
                            "Moved {size} to Trash.",
                            &[("{size}", &review::size(result.bytes_moved()))],
                        ));
                        update.merge((&result).into());
                        for (path, e) in result.failed {
                            lines.push(format!("{}: {e}", path.display()));
                        }
                    }
                    let _ = tx.send_blocking((lines.join("\n"), update));
                });
                let p = p.clone();
                glib::spawn_future_local(async move {
                    if let Ok((report, update)) = rx.recv().await {
                        if !review::removed(&p.0.widget, update.clone()).await {
                            p.0.store.apply_removed(&update);
                        }
                        review::report(&p.0.widget, &report);
                    }
                    p.0.busy.set(false);
                    screen.close();
                    p.update();
                });
            },
        );
    }
}
fn app_icon(icon: Option<&str>) -> gtk4::Image {
    let image = gtk4::Image::new();
    if let Some(icon) = icon.and_then(|s| gio::Icon::for_string(s).ok()) {
        image.set_from_gicon(&icon);
    } else {
        image.set_icon_name(Some("application-x-executable"));
    }
    image
}
fn set_tab(button: &gtk4::ToggleButton, title: &str, count: usize) {
    if let Some(label) = button
        .child()
        .and_downcast::<gtk4::Box>()
        .and_then(|b| b.last_child())
        .and_downcast::<gtk4::Label>()
    {
        label.set_text(&tr_args(
            "{label} {count}",
            &[("{label}", &tr(title)), ("{count}", &format_count(count))],
        ));
    }
}
