// Review list for AI models and games, in the style of Purge's Large Files
// view (MIT, © 2026 Jithin Sabu). See THIRD_PARTY_LICENSES.md.
use super::{
    purge_widgets::{self as widgets, BadgeKind},
    review_scan_store::{ItemInventory, Kind, ReviewScanStore},
    review_widgets as review,
};
use crate::{
    i18n::{format_count, tr, tr_args},
    services::review_items::{self, ReviewItem},
};
use gtk4::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::SystemTime,
};

/// Text and look that differ between the two pages.
pub struct Spec {
    pub kind: Kind,
    pub title: &'static str,
    pub color: &'static str,
    pub icon: &'static str,
    pub notice: &'static str,
}

pub const MODELS: Spec = Spec {
    kind: Kind::Models,
    title: "AI Models",
    color: "pc-models",
    icon: "pc-magnifico-ai-symbolic",
    notice: "Models are downloaded again the next time an app needs them.",
};

pub const GAMES: Spec = Spec {
    kind: Kind::Games,
    title: "Games",
    color: "pc-games",
    icon: "pc-input-gaming-symbolic",
    notice: "Games can be installed again from their launcher. Saved games in the cloud or in compatdata are kept.",
};

#[derive(Clone)]
pub struct ReviewItemsPage(Rc<Inner>);
struct Inner {
    spec: &'static Spec,
    widget: gtk4::Box,
    loader: super::scan_loading::ScanLoading,
    subtitle: gtk4::Label,
    scan: gtk4::Button,
    delete: gtk4::Button,
    search: gtk4::SearchEntry,
    sort: gtk4::DropDown,
    list: gtk4::Box,
    select: gtk4::CheckButton,
    snapshot: RefCell<Option<Arc<ItemInventory>>>,
    store: ReviewScanStore,
    selected: RefCell<HashSet<PathBuf>>,
    busy: Cell<bool>,
    syncing: Cell<bool>,
    generation: Cell<u64>,
}

impl ReviewItemsPage {
    pub fn new(store: ReviewScanStore, spec: &'static Spec) -> Self {
        let widget = gtk4::Box::new(gtk4::Orientation::Vertical, 18);
        widget.add_css_class("pc-review-page");
        let subtitle = gtk4::Label::new(Some(&tr("Scanning…")));
        let scan = review::action("pc-view-refresh-symbolic", "Scan", false);
        let delete = review::action("pc-user-trash-symbolic", "Delete Selected", true);
        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        actions.append(&scan);
        actions.append(&delete);
        widget.append(&widgets::page_header(spec.title, &subtitle, &actions));
        let selection = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let select = gtk4::CheckButton::with_label(&tr("Select All"));
        let search = review::search("Search");
        let sort = review::dropdown(&["Largest", "Name", "Last used"]);
        selection.append(&select);
        selection.append(&search);
        selection.append(&sort);
        widget.append(&selection);
        let list = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));
        widget.append(&scroll);
        let loader = super::scan_loading::ScanLoading::wrap(&widget, spec.title);
        let page = Self(Rc::new(Inner {
            spec,
            widget,
            loader,
            subtitle,
            scan,
            delete,
            search,
            sort,
            list,
            select,
            snapshot: RefCell::default(),
            store,
            selected: RefCell::default(),
            busy: Cell::new(false),
            syncing: Cell::new(false),
            generation: Cell::new(0),
        }));
        let weak = Rc::downgrade(&page.0);
        page.0.scan.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).scan();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.search.connect_search_changed(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).render_rows();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.sort.connect_selected_notify(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).render_rows();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.select.connect_toggled(move |check| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let p = Self(inner);
            if p.0.syncing.get() {
                return;
            }
            for item in p.visible() {
                if check.is_active() {
                    p.0.selected.borrow_mut().insert(item.location);
                } else {
                    p.0.selected.borrow_mut().remove(&item.location);
                }
            }
            p.render_rows();
        });
        let weak = Rc::downgrade(&page.0);
        page.0.delete.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).confirm();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.widget.connect_map(move |_| {
            if let Some(inner) = weak.upgrade() {
                let p = Self(inner);
                p.0.store.request(p.0.spec.kind, false);
                p.sync_store();
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.store.subscribe(move || {
            if let Some(inner) = weak.upgrade() {
                Self(inner).sync_store();
            }
        });
        page
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.0.widget
    }

    fn scan(&self) {
        if !self.0.busy.get() {
            self.0.store.request(self.0.spec.kind, true);
        }
    }

    fn sync_store(&self) {
        if let Some(snapshot) = self.0.store.items(self.0.spec.kind) {
            let changed = self
                .0
                .snapshot
                .borrow()
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &snapshot));
            if changed {
                *self.0.snapshot.borrow_mut() = Some(snapshot);
                self.0.selected.borrow_mut().clear();
                if self.0.widget.is_mapped() {
                    self.render_rows();
                }
            }
        }
        if self.0.widget.is_mapped() {
            let loading = self.0.snapshot.borrow().is_none()
                && self.0.store.error(self.0.spec.kind).is_none();
            self.0.loader.set_loading(loading, self.0.spec.title);
            if !loading && self.0.list.first_child().is_none() {
                self.render_rows();
            }
        }
        self.update();
    }

    fn visible(&self) -> Vec<ReviewItem> {
        let query = self.0.search.text().to_lowercase();
        let snapshot = self.0.snapshot.borrow();
        let mut items: Vec<_> = snapshot
            .as_ref()
            .map(|s| s.items.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|i| {
                query.is_empty()
                    || i.title.to_lowercase().contains(&query)
                    || i.source.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        match self.0.sort.selected() {
            1 => items.sort_by_key(|i| i.title.to_lowercase()),
            2 => items.sort_by_key(|i| i.last_used),
            _ => items.sort_by_key(|i| std::cmp::Reverse(i.bytes)),
        }
        items
    }

    fn render_rows(&self) {
        while let Some(c) = self.0.list.first_child() {
            self.0.list.remove(&c);
        }
        let items = self.visible();
        if items.is_empty() {
            let empty = gtk4::Label::new(Some(&tr("Nothing found.")));
            empty.add_css_class("pc-secondary");
            empty.set_margin_top(24);
            self.0.list.append(&empty);
        }
        let generation = self.0.generation.get().wrapping_add(1);
        self.0.generation.set(generation);
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            for (index, item) in items.into_iter().enumerate() {
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
                page.0.list.append(&page.row(item));
            }
        });
        self.update();
    }

    fn row(&self, item: ReviewItem) -> gtk4::Button {
        let check = gtk4::CheckButton::new();
        check.set_active(self.0.selected.borrow().contains(&item.location));
        let icon = self.icon(&item);
        let description = format!(
            "{} · {} · {}",
            item.source,
            review::home_path(&item.location),
            last_used(item.last_used)
        );
        let row = widgets::item_card(
            Some(&check),
            &icon,
            &item.title,
            &description,
            &review::size(item.bytes),
            Some(BadgeKind::Review),
        );
        row.button.set_tooltip_text(Some(
            &item
                .targets
                .iter()
                .map(|t| review::home_path(&t.path))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
        let target = item.location.clone();
        super::file_actions::path_clicks(&row.button, move || Some(target.clone()));
        let c = check.clone();
        row.button
            .connect_clicked(move |_| c.set_active(!c.is_active()));
        let weak = Rc::downgrade(&self.0);
        let key = item.location;
        check.connect_toggled(move |c| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let p = Self(inner);
            if c.is_active() {
                p.0.selected.borrow_mut().insert(key.clone());
            } else {
                p.0.selected.borrow_mut().remove(&key);
            }
            p.update();
        });
        row.button
    }

    fn icon(&self, item: &ReviewItem) -> gtk4::Box {
        let snapshot = self.0.snapshot.borrow();
        let Some(cover) = snapshot.as_ref().and_then(|s| s.covers.get(&item.location)) else {
            return widgets::icon_tile(self.0.spec.color, self.0.spec.icon);
        };
        let pixels = glib::Bytes::from_owned(cover.pixels.clone());
        let texture = gtk4::gdk::MemoryTexture::new(
            cover.width,
            cover.height,
            if cover.alpha {
                gtk4::gdk::MemoryFormat::R8g8b8a8
            } else {
                gtk4::gdk::MemoryFormat::R8g8b8
            },
            &pixels,
            cover.stride,
        );
        let picture = gtk4::Picture::for_paintable(&texture);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk4::ContentFit::Cover);
        picture.set_size_request(34, 34);
        let frame = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        frame.set_size_request(34, 34);
        frame.set_valign(gtk4::Align::Center);
        frame.add_css_class("pc-file-thumbnail");
        frame.set_overflow(gtk4::Overflow::Hidden);
        frame.append(&picture);
        frame
    }

    fn update(&self) {
        let p = &self.0;
        let busy = p.busy.get() || p.store.loading(p.spec.kind);
        p.scan.set_sensitive(!busy);
        p.list.set_sensitive(!busy);
        p.search.set_sensitive(!busy);
        p.sort.set_sensitive(!busy);
        let visible = self.visible();
        let selected = p.selected.borrow();
        let count = visible
            .iter()
            .filter(|i| selected.contains(&i.location))
            .count();
        drop(selected);
        p.delete.set_sensitive(!busy && count > 0);
        p.select.set_sensitive(!busy && !visible.is_empty());
        p.syncing.set(true);
        p.select.set_active(count > 0 && count == visible.len());
        p.select
            .set_inconsistent(count > 0 && count < visible.len());
        p.syncing.set(false);
        if busy {
            p.subtitle.set_text(&tr("Scanning…"));
        } else {
            p.subtitle.set_text(&tr_args(
                "{count} items · {size} to review",
                &[
                    ("{count}", &format_count(visible.len())),
                    (
                        "{size}",
                        &review::size(visible.iter().map(|i| i.bytes).sum()),
                    ),
                ],
            ));
        }
    }

    fn confirm(&self) {
        if self.0.busy.get() {
            return;
        }
        let items: Vec<_> = {
            let selected = self.0.selected.borrow();
            self.visible()
                .into_iter()
                .filter(|i| selected.contains(&i.location))
                .collect()
        };
        if items.is_empty() {
            return;
        }
        if items.iter().any(|i| i.steam) && review_items::steam_running() {
            review::report(
                &self.0.widget,
                &tr("Close Steam before removing Steam games, then try again."),
            );
            return;
        }
        let cards = items
            .iter()
            .map(|i| super::removal_dialog::RemovalItem {
                name: i.title.clone(),
                source: format!("{} · {}", i.source, review::home_path(&i.location)),
                bytes: i.bytes,
                safe: false,
            })
            .collect();
        let mut notice = tr("Selected items will be moved to Trash.");
        notice.push(' ');
        notice.push_str(&tr(self.0.spec.notice));
        if items.iter().any(|i| i.steam) {
            notice.push(' ');
            notice.push_str(&tr("Keep Steam closed while they are removed."));
        }
        let p = self.clone();
        super::removal_dialog::present(
            &self.0.widget,
            "Delete Selected",
            cards,
            true,
            &notice,
            move |picked| {
                let Some(picked) = picked else {
                    return;
                };
                let items = super::removal_dialog::pick(items, &picked);
                if p.0.busy.replace(true) {
                    return;
                }
                p.update();
                let progress = Arc::new(crate::services::CleanProgress::default());
                let parent = p.0.widget.root().and_downcast::<gtk4::Window>();
                let screen = super::clean_progress::CleanProgressWindow::show(
                    parent.as_ref(),
                    items.iter().map(|i| i.bytes).sum(),
                    items.iter().map(|i| i.targets.len()).sum(),
                    progress.clone(),
                );
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(review_items::trash(&items, Some(&progress)));
                });
                let p = p.clone();
                glib::spawn_future_local(async move {
                    if let Ok(result) = rx.recv().await {
                        let mut text = tr_args(
                            "Moved {size} to Trash.",
                            &[("{size}", &review::size(result.bytes_moved()))],
                        );
                        for (path, reason) in result.failed {
                            text.push_str(&format!("\n{}: {}", review::home_path(&path), reason));
                        }
                        review::report(&p.0.widget, &text);
                    }
                    p.0.busy.set(false);
                    screen.close();
                    p.scan();
                });
            },
        );
    }
}

fn last_used(time: Option<SystemTime>) -> String {
    let Some(days) = time
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map(|d| d.as_secs() / 86400)
    else {
        return tr("Last used: Unknown");
    };
    if days == 0 {
        tr("Last used Today")
    } else {
        tr_args(
            "Last used {days} days ago",
            &[("{days}", &format_count(days as usize))],
        )
    }
}
