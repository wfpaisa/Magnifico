// Adapted from Purge LargeFilesView.swift and LargeFileScanner.swift (MIT,
// © 2026 Jithin Sabu). See THIRD_PARTY_LICENSES.md.
use super::{
    purge_widgets as widgets,
    review_scan_store::{Kind, LargeInventory, ReviewScanStore},
    review_widgets as review,
};
use crate::{
    i18n::{format_count, tr, tr_args},
    services::{self, FileKind, FoundFile},
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
const LIMITS: [u64; 6] = [
    5_000_000,
    50_000_000,
    100_000_000,
    200_000_000,
    500_000_000,
    1_000_000_000,
];
const AGES: [u64; 5] = [0, 7, 30, 90, 365];
const TYPES: [&str; 9] = [
    "All",
    "Duplicates",
    "Videos",
    "Audio",
    "Images",
    "PDFs",
    "Archives",
    "Other",
    "AI Models",
];
#[derive(Clone)]
pub struct LargeFilesPage(Rc<Inner>);
struct Inner {
    widget: gtk4::Box,
    loader: super::scan_loading::ScanLoading,
    subtitle: gtk4::Label,
    scan: gtk4::Button,
    delete: gtk4::Button,
    minimum: gtk4::DropDown,
    age: gtk4::DropDown,
    search: gtk4::SearchEntry,
    sort: gtk4::DropDown,
    chips: gtk4::Box,
    list: gtk4::Box,
    select: gtk4::CheckButton,
    files: RefCell<Vec<FoundFile>>,
    duplicates: RefCell<HashSet<PathBuf>>,
    snapshot: RefCell<Option<Arc<LargeInventory>>>,
    store: ReviewScanStore,
    selected: RefCell<HashSet<PathBuf>>,
    filter: Cell<usize>,
    busy: Cell<bool>,
    syncing: Cell<bool>,
    generation: Cell<u64>,
}
impl LargeFilesPage {
    pub fn new(store: ReviewScanStore) -> Self {
        let widget = gtk4::Box::new(gtk4::Orientation::Vertical, 18);
        widget.add_css_class("pc-review-page");
        let subtitle = gtk4::Label::new(Some(&tr("Scan to find large files.")));
        let scan = review::action("pc-view-refresh-symbolic", "Scan", false);
        let delete = review::action("pc-user-trash-symbolic", "Delete Selected", true);
        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        actions.append(&scan);
        actions.append(&delete);
        widget.append(&widgets::page_header("Large Files", &subtitle, &actions));
        let controls = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let minimum = review::dropdown(&[
            "Larger than 5 MB",
            "Larger than 50 MB",
            "Larger than 100 MB",
            "Larger than 200 MB",
            "Larger than 500 MB",
            "Larger than 1 GB",
        ]);
        let age = review::dropdown(&[
            "Last used: Any time",
            "Last used: Over a week ago",
            "Last used: Over a month ago",
            "Last used: Over 3 months ago",
            "Last used: Over a year ago",
        ]);
        let search = review::search("Search");
        controls.append(&minimum);
        controls.append(&age);
        controls.append(&search);
        widget.append(&review::horizontal_strip(&controls));
        let chips = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        widget.append(&review::horizontal_strip(&chips));
        let selection = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let select = gtk4::CheckButton::with_label(&tr("Select All"));
        select.set_hexpand(true);
        let sort = review::dropdown(&["Largest", "Name"]);
        selection.append(&select);
        selection.append(&sort);
        widget.append(&selection);
        let list = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));
        widget.append(&scroll);
        let loader = super::scan_loading::ScanLoading::wrap(&widget, "Large Files");
        let page = Self(Rc::new(Inner {
            widget,
            loader,
            subtitle,
            scan,
            delete,
            minimum,
            age,
            search,
            sort,
            chips,
            list,
            select,
            files: RefCell::default(),
            duplicates: RefCell::default(),
            snapshot: RefCell::default(),
            store,
            selected: RefCell::default(),
            filter: Cell::new(0),
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
        page.0.minimum.connect_selected_notify(move |_| {
            if let Some(inner) = weak.upgrade() {
                if !inner.syncing.get() {
                    Self(inner).scan();
                }
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.age.connect_selected_notify(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).render();
            }
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
            for file in p.visible(None) {
                if check.is_active() {
                    p.0.selected.borrow_mut().insert(file.path);
                } else {
                    p.0.selected.borrow_mut().remove(&file.path);
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
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let p = Self(inner);
            p.0.store.request(Kind::LargeFiles, false);
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
    fn scan(&self) {
        if self.0.busy.get() {
            return;
        }
        self.0
            .store
            .request_large(LIMITS[self.0.minimum.selected() as usize], true);
    }
    fn sync_store(&self) {
        if let Some(index) = LIMITS.iter().position(|n| *n == self.0.store.minimum()) {
            self.0.syncing.set(true);
            self.0.minimum.set_selected(index as u32);
            self.0.syncing.set(false);
        }
        if let Some(snapshot) = self.0.store.large() {
            let changed = self
                .0
                .snapshot
                .borrow()
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &snapshot));
            if changed {
                *self.0.files.borrow_mut() = snapshot.files.clone();
                *self.0.duplicates.borrow_mut() = snapshot.duplicates.clone();
                *self.0.snapshot.borrow_mut() = Some(snapshot);
                self.0.selected.borrow_mut().clear();
            }
        }
        self.render();
    }
    fn visible(&self, kind: Option<usize>) -> Vec<FoundFile> {
        let query = self.0.search.text().to_lowercase();
        let days = AGES[self.0.age.selected() as usize];
        self.0
            .files
            .borrow()
            .iter()
            .filter(|f| {
                (days == 0
                    || f.accessed
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|d| d.as_secs() >= days * 86400))
                    && f.path.to_string_lossy().to_lowercase().contains(&query)
                    && match kind.unwrap_or(self.0.filter.get()) {
                        0 => true,
                        1 => self.0.duplicates.borrow().contains(&f.path),
                        k => category(f) == k,
                    }
            })
            .cloned()
            .collect()
    }
    fn render(&self) {
        if !self.0.widget.is_mapped() {
            return;
        }
        let loading =
            self.0.snapshot.borrow().is_none() && self.0.store.error(Kind::LargeFiles).is_none();
        self.0.loader.set_loading(loading, "Large Files");
        if loading {
            self.update();
            return;
        }
        while let Some(c) = self.0.chips.first_child() {
            self.0.chips.remove(&c);
        }
        let mut first = None;
        for (i, title) in TYPES.iter().enumerate() {
            let chip = widgets::filter_chip(
                [
                    "pc-view-grid-symbolic",
                    "pc-edit-copy-symbolic",
                    "pc-video-x-generic-symbolic",
                    "pc-audio-x-generic-symbolic",
                    "pc-image-x-generic-symbolic",
                    "pc-application-pdf-symbolic",
                    "pc-package-x-generic-symbolic",
                    "pc-text-x-generic-symbolic",
                    "pc-applications-science-symbolic",
                ][i],
                title,
                self.visible(Some(i)).len(),
                i == self.0.filter.get(),
            );
            if let Some(first) = &first {
                chip.set_group(Some(first));
            } else {
                first = Some(chip.clone());
            }
            let weak = Rc::downgrade(&self.0);
            chip.connect_toggled(move |b| {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let p = Self(inner);
                if b.is_active() {
                    p.0.filter.set(i);
                    p.render_rows();
                }
            });
            self.0.chips.append(&chip);
        }
        self.render_rows();
    }
    fn render_rows(&self) {
        while let Some(c) = self.0.list.first_child() {
            self.0.list.remove(&c);
        }
        let mut files = self.visible(None);
        files.sort_by(|a, b| {
            if self.0.sort.selected() == 1 {
                a.path.file_name().cmp(&b.path.file_name())
            } else {
                b.size.cmp(&a.size)
            }
        });
        if files.is_empty() {
            self.0.list.append(&gtk4::Label::new(Some(&tr(
                "No files match these filters.",
            ))));
        }
        let generation = self.0.generation.get().wrapping_add(1);
        self.0.generation.set(generation);
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            for (index, file) in files.into_iter().enumerate() {
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
                let check = gtk4::CheckButton::new();
                check.set_active(page.0.selected.borrow().contains(&file.path));
                let icon = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
                icon.set_size_request(36, 36);
                icon.set_valign(gtk4::Align::Center);
                let snapshot = page.0.snapshot.borrow();
                let preview = snapshot.as_ref().and_then(|s| s.previews.get(&file.path));
                if let Some(thumbnail) = preview.and_then(|p| p.thumbnail.as_ref()) {
                    let pixels = glib::Bytes::from_owned(thumbnail.pixels.clone());
                    let texture = gtk4::gdk::MemoryTexture::new(
                        thumbnail.width,
                        thumbnail.height,
                        if thumbnail.alpha {
                            gtk4::gdk::MemoryFormat::R8g8b8a8
                        } else {
                            gtk4::gdk::MemoryFormat::R8g8b8
                        },
                        &pixels,
                        thumbnail.stride,
                    );
                    let picture = gtk4::Picture::for_paintable(&texture);
                    picture.set_can_shrink(true);
                    picture.set_content_fit(gtk4::ContentFit::Cover);
                    picture.set_size_request(36, 36);
                    icon.add_css_class("pc-file-thumbnail");
                    icon.set_overflow(gtk4::Overflow::Hidden);
                    icon.append(&picture);
                } else {
                    let image = gtk4::Image::new();
                    if let Some(gicon) = preview
                        .and_then(|p| p.icon.as_deref())
                        .and_then(|s| gio::Icon::for_string(s).ok())
                    {
                        image.set_from_gicon(&gicon);
                    } else {
                        let (mime, _) = gio::content_type_guess(Some(&file.path), &[]);
                        image.set_from_gicon(&gio::content_type_get_icon(&mime));
                    }
                    image.set_pixel_size(32);
                    icon.append(&image);
                }
                drop(snapshot);
                let name = file.path.file_name().unwrap_or_default().to_string_lossy();
                let description = format!(
                    "{} · {}",
                    review::home_path(file.path.parent().unwrap_or(&file.path)),
                    last_used(file.accessed)
                );
                let row = widgets::item_card(
                    Some(&check),
                    &icon,
                    &name,
                    &description,
                    &review::size(file.size),
                    None,
                );
                row.button
                    .set_tooltip_text(Some(&review::home_path(&file.path)));
                let target = file.path.clone();
                super::file_actions::path_clicks(&row.button, move || Some(target.clone()));
                let c = check.clone();
                row.button
                    .connect_clicked(move |_| c.set_active(!c.is_active()));
                let weak = Rc::downgrade(&page.0);
                let path = file.path;
                check.connect_toggled(move |c| {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    let p = Self(inner);
                    if c.is_active() {
                        p.0.selected.borrow_mut().insert(path.clone());
                    } else {
                        p.0.selected.borrow_mut().remove(&path);
                    }
                    p.update();
                });
                page.0.list.append(&row.button);
            }
        });
        self.update();
    }
    fn update(&self) {
        let p = &self.0;
        let busy = p.busy.get() || p.store.loading(Kind::LargeFiles);
        for w in [&p.minimum, &p.age, &p.sort] {
            w.set_sensitive(!busy);
        }
        p.scan.set_sensitive(!busy);
        p.chips.set_sensitive(!busy);
        p.list.set_sensitive(!busy);
        p.search.set_sensitive(!busy);
        let visible = self.visible(None);
        let count = visible
            .iter()
            .filter(|f| p.selected.borrow().contains(&f.path))
            .count();
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
                "{count} files · {size} to review",
                &[
                    ("{count}", &format_count(visible.len())),
                    (
                        "{size}",
                        &review::size(visible.iter().map(|f| f.size).sum()),
                    ),
                ],
            ));
        }
    }
    fn confirm(&self) {
        let files: Vec<_> = self
            .visible(None)
            .into_iter()
            .filter(|f| self.0.selected.borrow().contains(&f.path))
            .collect();
        if files.is_empty() || self.0.busy.get() {
            return;
        }
        let cards = files
            .iter()
            .map(|f| super::removal_dialog::RemovalItem {
                name: f
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                source: review::home_path(&f.path),
                bytes: f.size,
                safe: false,
            })
            .collect();
        let p = self.clone();
        super::removal_dialog::present(
            &self.0.widget,
            "Delete Selected",
            cards,
            true,
            &tr("Selected files will be moved to Trash."),
            move |picked| {
                let Some(picked) = picked else {
                    return;
                };
                let files = super::removal_dialog::pick(files, &picked);
                if p.0.busy.replace(true) {
                    return;
                }
                p.update();
                let targets: Vec<_> = files.iter().map(FoundFile::trash_target).collect();
                let progress = Arc::new(services::CleanProgress::default());
                let parent = p.0.widget.root().and_downcast::<gtk4::Window>();
                let screen = super::clean_progress::CleanProgressWindow::show(
                    parent.as_ref(),
                    files.iter().map(|f| f.size).sum(),
                    files.len(),
                    progress.clone(),
                );
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let result = services::move_to_trash_with_progress(
                        &dirs::home_dir().unwrap_or_default(),
                        &targets,
                        Some(&progress),
                    );
                    let _ = tx.send_blocking(result);
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
fn category(file: &FoundFile) -> usize {
    if file
        .path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return 5;
    }
    match file.kind {
        FileKind::Video => 2,
        FileKind::Audio => 3,
        FileKind::Image => 4,
        FileKind::Archive => 6,
        FileKind::AiModel => 8,
        _ => 7,
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
