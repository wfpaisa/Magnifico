// Adapted from Purge OverviewView.swift (MIT, © 2026 Jithin Sabu).
// https://github.com/jithin-sabu/purge-app — see THIRD_PARTY_LICENSES.md.
use super::clean_scan_store::CleanScanStore;
use super::purge_widgets::{self as widgets, BadgeKind, Segment};
use super::review_scan_store::ReviewScanStore;
use crate::{
    i18n::{tr, tr_args},
    storage::Storage,
};
use gtk4::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Rows 6 (AI models) and 7 (games) only show when something was found.
const TITLES: [&str; 10] = [
    "App Caches",
    "Dev Tools",
    "System",
    "Large Files",
    "Installed apps",
    "Leftovers from deleted apps",
    "AI Models",
    "Games",
    "Everything else",
    "Free",
];
const COLORS: [&str; 10] = [
    "caches",
    "dev",
    "system",
    "large",
    "apps",
    "leftovers",
    "models",
    "games",
    "else",
    "free",
];
const PAGES: [&str; 10] = [
    "browsers",
    "dev-tools",
    "system-cleanup",
    "large-files",
    "applications",
    "applications-leftovers",
    "ai-models",
    "games",
    "storage-analyzer",
    "storage-analyzer",
];
const ICONS: [&str; 10] = [
    "pc-drive-harddisk-symbolic",
    "pc-applications-engineering-symbolic",
    "pc-computer-symbolic",
    "pc-text-x-generic-symbolic",
    "pc-view-grid-symbolic",
    "pc-package-x-generic-symbolic",
    "pc-magnifico-ai-symbolic",
    "pc-input-gaming-symbolic",
    "pc-view-more-symbolic",
    "pc-folder-symbolic",
];

#[derive(Clone)]
pub struct OverviewPage {
    pub widget: gtk4::ScrolledWindow,
    body: gtk4::Box,
    loader: super::scan_loading::ScanLoading,
    subtitle: gtk4::Label,
    scan: gtk4::Button,
    loading: Rc<Cell<bool>>,
    storage: Arc<Storage>,
    on_sizes: Rc<dyn Fn([u64; 6], u64)>,
    cancel: Arc<AtomicBool>,
    store: CleanScanStore,
    review: ReviewScanStore,
    latest: Rc<RefCell<Option<Readings>>>,
}

impl OverviewPage {
    pub fn new(
        storage: Arc<Storage>,
        store: CleanScanStore,
        review: ReviewScanStore,
        on_sizes: impl Fn([u64; 6], u64) + 'static,
    ) -> Self {
        let widget = gtk4::ScrolledWindow::new();
        widget.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        widget.set_vexpand(true);
        widget.set_propagate_natural_height(false);
        let body = gtk4::Box::new(gtk4::Orientation::Vertical, 24);
        body.add_css_class("pc-overview");
        let (loader, stack) = super::scan_loading::ScanLoading::new(&body, "App Caches");
        widget.set_child(Some(&stack));
        let subtitle = gtk4::Label::new(Some(&tr("Scanning…")));
        let scan = action_button("pc-view-refresh-symbolic", "Scan Everything", false);
        let clean = clean_go_button("Clean Safe Items");
        clean.set_tooltip_text(Some(&tr("Review the cleanup selection before cleaning.")));
        clean.set_action_name(Some("win.review-cleanup"));
        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        actions.set_valign(gtk4::Align::Start);
        actions.append(&scan);
        actions.append(&clean);
        body.append(&widgets::page_header("Overview", &subtitle, &actions));
        let page = Self {
            widget,
            body,
            loader,
            subtitle,
            scan,
            storage,
            loading: Rc::new(Cell::new(false)),
            on_sizes: Rc::new(on_sizes),
            cancel: Arc::new(AtomicBool::new(false)),
            store,
            review,
            latest: Rc::new(RefCell::new(None)),
        };
        let widget = page.widget.downgrade();
        let body = page.body.downgrade();
        let loader = page.loader.clone();
        let subtitle = page.subtitle.downgrade();
        let loading = page.loading.clone();
        let storage = page.storage.clone();
        let on_sizes = page.on_sizes.clone();
        let cancel = page.cancel.clone();
        let store = page.store.clone();
        let review = page.review.clone();
        let latest = page.latest.clone();
        page.scan.connect_clicked(move |scan| {
            if let (Some(widget), Some(body), Some(subtitle)) =
                (widget.upgrade(), body.upgrade(), subtitle.upgrade())
            {
                Self {
                    widget,
                    body,
                    loader: loader.clone(),
                    subtitle,
                    scan: scan.clone(),
                    loading: loading.clone(),
                    storage: storage.clone(),
                    on_sizes: on_sizes.clone(),
                    cancel: cancel.clone(),
                    store: store.clone(),
                    review: review.clone(),
                    latest: latest.clone(),
                }
                .scan_everything();
            }
        });
        let cancel = page.cancel.clone();
        page.widget.connect_unrealize(move |_| {
            cancel.store(true, Ordering::Relaxed);
        });
        let listener = page.listener();
        page.store.subscribe(move |_| listener());
        page.review.subscribe(page.listener());
        page.review.request_all(false);
        page.store.request_all(false);
        page.refresh();
        page
    }

    fn listener(&self) -> impl Fn() + 'static {
        let widget = self.widget.downgrade();
        let body = self.body.downgrade();
        let loader = self.loader.clone();
        let subtitle = self.subtitle.downgrade();
        let scan = self.scan.downgrade();
        let loading = self.loading.clone();
        let storage = self.storage.clone();
        let on_sizes = self.on_sizes.clone();
        let cancel = self.cancel.clone();
        let latest = self.latest.clone();
        let store = self.store.downgrade();
        let review = self.review.downgrade();
        move || {
            if let (
                Some(widget),
                Some(body),
                Some(subtitle),
                Some(scan),
                Some(store),
                Some(review),
            ) = (
                widget.upgrade(),
                body.upgrade(),
                subtitle.upgrade(),
                scan.upgrade(),
                store.upgrade(),
                review.upgrade(),
            ) {
                Self {
                    widget,
                    body,
                    loader: loader.clone(),
                    subtitle,
                    scan,
                    loading: loading.clone(),
                    storage: storage.clone(),
                    on_sizes: on_sizes.clone(),
                    cancel: cancel.clone(),
                    latest: latest.clone(),
                    store,
                    review,
                }
                .render_cached();
            }
        }
    }
    pub fn scan_everything(&self) {
        self.store.request_all(true);
        self.review.request_all(true);
        self.refresh();
    }
    pub fn refresh(&self) {
        if self.loading.replace(true) {
            return;
        }
        self.cancel.store(false, Ordering::Relaxed);
        self.scan.set_sensitive(false);
        self.subtitle.set_text(&tr("Scanning…"));
        self.clear_readings();
        self.update_loader();
        let (tx, rx) = async_channel::bounded(1);
        let storage = self.storage.clone();
        let cancel = self.cancel.clone();
        crate::runtime().spawn_blocking(move || {
            let _ = tx.send_blocking(readings(&storage, &cancel));
        });
        let page = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(data) = rx.recv().await {
                *page.latest.borrow_mut() = Some(data);
            } else {
                page.clear_readings();
                page.subtitle
                    .set_text(&tr("Scan could not finish. Try scanning again."));
            }
            page.loading.set(false);
            page.render_cached();
        });
    }

    fn update_loader(&self) {
        use super::review_scan_store::Kind;
        use crate::services::clean_sources::CleanPageKind;
        let phase = [
            CleanPageKind::AppCaches,
            CleanPageKind::DevTools,
            CleanPageKind::System,
        ]
        .into_iter()
        .find(|&k| self.store.loading(k))
        .map(|k| k.title())
        .or_else(|| {
            self.review
                .loading(Kind::LargeFiles)
                .then_some("Large Files")
        })
        .or_else(|| self.review.loading(Kind::Apps).then_some("Applications"));
        self.loader.set_loading(
            self.loading.get() || phase.is_some(),
            phase.unwrap_or("Disk usage"),
        );
    }
    fn render_cached(&self) {
        self.update_loader();
        self.scan
            .set_sensitive(!self.loading.get() && !self.store.busy() && !self.review.busy());
        let Some(mut data) = self.latest.borrow().clone() else {
            return;
        };
        for kind in [
            crate::services::clean_sources::CleanPageKind::AppCaches,
            crate::services::clean_sources::CleanPageKind::DevTools,
            crate::services::clean_sources::CleanPageKind::System,
        ] {
            let Some(items) = self.store.items(kind) else {
                return;
            };
            data.sizes[kind.index()] = items.iter().map(|i| i.bytes).sum();
            data.safe[kind.index()] = items
                .iter()
                .filter(|i| i.safety == crate::models::Safety::Safe)
                .map(|i| i.bytes)
                .sum();
        }
        // Counts and bytes come from the exact snapshots displayed by the
        // review pages; disk categorization must never replace these totals.
        let large = self.review.large();
        let apps = self.review.apps();
        data.sizes[3] = large.as_ref().map_or(0, |s| s.bytes());
        data.large_count = large.as_ref().map_or(0, |s| s.files.len());
        data.sizes[4] = apps.as_ref().map_or(0, |s| s.app_bytes());
        data.sizes[5] = apps.as_ref().map_or(0, |s| s.leftover_bytes());
        data.app_count = apps.as_ref().map_or(0, |s| s.apps.len());
        let models = self.review.items(super::review_scan_store::Kind::Models);
        let games = self.review.items(super::review_scan_store::Kind::Games);
        data.sizes[6] = models.as_ref().map_or(0, |s| s.bytes());
        data.sizes[7] = games.as_ref().map_or(0, |s| s.bytes());
        data.model_count = models.as_ref().map_or(0, |s| s.items.len());
        data.game_count = games.as_ref().map_or(0, |s| s.items.len());
        data.leftover_count = apps.as_ref().map_or(0, |s| s.leftovers.len());
        normalize(&mut data);
        self.clear_readings();
        self.render(&data);
        (self.on_sizes)(data.sizes[..6].try_into().unwrap(), data.trash);
        self.subtitle.set_text(&tr(
            if self.store.busy() || self.review.busy() || self.loading.get() {
                "Scanning…"
            } else {
                "Scanned just now"
            },
        ));
    }

    fn clear_readings(&self) {
        let Some(header) = self.body.first_child() else {
            return;
        };
        while let Some(child) = header.next_sibling() {
            self.body.remove(&child);
        }
    }

    fn render(&self, data: &Readings) {
        let summary = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let used = gtk4::Label::new(Some(&tr_args(
            "{size} used",
            &[("{size}", &size(data.used))],
        )));
        used.add_css_class("pc-disk-used");
        used.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        summary.append(&used);
        let total = gtk4::Label::new(Some(&tr_args(
            "of {size}",
            &[("{size}", &size(data.total))],
        )));
        total.add_css_class("pc-secondary");
        total.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        summary.append(&total);
        let free = gtk4::Label::new(Some(&tr_args(
            "{size} free",
            &[("{size}", &size(data.free))],
        )));
        free.set_hexpand(true);
        free.set_xalign(1.0);
        free.add_css_class("pc-secondary");
        free.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        summary.append(&free);
        self.body.append(&summary);
        if data.total == 0 {
            total.set_text(&tr("Disk usage unavailable"));
        }
        let rows: Vec<_> = (0..10)
            .map(|i| {
                let row = widgets::item_card(
                    None,
                    &widgets::icon_tile(&format!("pc-{}", COLORS[i]), ICONS[i]),
                    TITLES[i],
                    "",
                    &size(data.sizes[i]),
                    (3..8).contains(&i).then_some(BadgeKind::Review),
                );
                row.size.set_text(&size(data.sizes[i]));
                row.percent.set_text(&tr_args(
                    "{percent}%",
                    &[(
                        "{percent}",
                        &format!("{:.1}", fraction(data.sizes[i], data.total) * 100.0),
                    )],
                ));
                row.button.set_action_name(Some("win.open-page"));
                row.button
                    .set_action_target_value(Some(&PAGES[i].to_variant()));
                row
            })
            .collect();
        for (i, row) in rows.iter().enumerate() {
            let description = match i {
                0..=2 => tr_args("{size} safe to clean", &[("{size}", &size(data.safe[i]))]),
                3 => tr_args(
                    "{count} files over {size}",
                    &[
                        ("{count}", &crate::i18n::format_count(data.large_count)),
                        ("{size}", &size(self.review.minimum())),
                    ],
                ),
                4 => tr_args(
                    "{count} apps",
                    &[("{count}", &crate::i18n::format_count(data.app_count))],
                ),
                5 => tr_args(
                    "{count} items",
                    &[("{count}", &crate::i18n::format_count(data.leftover_count))],
                ),
                6 => tr_args(
                    "{count} models",
                    &[("{count}", &crate::i18n::format_count(data.model_count))],
                ),
                7 => tr_args(
                    "{count} games",
                    &[("{count}", &crate::i18n::format_count(data.game_count))],
                ),
                8 => tr("Linux, your documents and photos, and other files"),
                _ => tr("Available for new files"),
            };
            let pending = (i == 3 && self.review.large().is_none())
                || ((i == 4 || i == 5) && self.review.apps().is_none());
            row.description.set_text(&if pending {
                tr("Scanning…")
            } else {
                description
            });
            if pending {
                row.size.set_text(&tr("…"));
                row.percent.set_text("");
            }
            if i < 2 {
                row.button.set_tooltip_text(Some(&tr(
                    "Regenerable caches measured independently of cleanup selections.",
                )));
            } else if i == 2 {
                row.button.set_tooltip_text(Some(&tr("Package caches, journal files and unused Flatpak runtimes. Safe amount excludes journal files; review them in System.")));
            }
        }
        let highlighted = Rc::new(Cell::new(None));
        let buttons: Vec<_> = rows.iter().map(|r| r.button.downgrade()).collect();
        let hover: Rc<dyn Fn(Option<usize>)> = Rc::new(move |index| {
            for (i, button) in buttons.iter().enumerate() {
                let Some(button) = button.upgrade() else {
                    continue;
                };
                if index.is_some_and(|selected| selected != i) {
                    button.add_css_class("pc-dimmed");
                } else {
                    button.remove_css_class("pc-dimmed");
                }
                if index == Some(i) {
                    button.add_css_class("pc-highlighted");
                } else {
                    button.remove_css_class("pc-highlighted");
                }
            }
        });
        let chart_sizes = chart_sizes(data);
        let segments = (0..10)
            .map(|i| Segment {
                fraction: fraction(chart_sizes[i], data.total),
                color: match i {
                    0 => "pc_chart_caches",
                    1 => "pc_chart_dev",
                    2 => "pc_chart_system",
                    3 => "pc_chart_large",
                    4 => "pc_chart_apps",
                    5 => "pc_chart_leftovers",
                    6 => "pc_chart_models",
                    7 => "pc_chart_games",
                    8 => "pc_chart_else",
                    _ => "pc_chart_free",
                },
                label: tr_args(
                    "{category} · {size}",
                    &[
                        ("{category}", &tr(TITLES[i])),
                        ("{size}", &size(data.sizes[i])),
                    ],
                ),
            })
            .collect();
        let bar_weak = Rc::new(std::cell::RefCell::new(
            glib::WeakRef::<gtk4::DrawingArea>::new(),
        ));
        let state = highlighted.clone();
        let callback = hover.clone();
        let weak = bar_weak.clone();
        let bar = widgets::segmented_bar(
            segments,
            Rc::new(move |i| {
                state.set(i);
                callback(i);
                if let Some(bar) = weak.borrow().upgrade() {
                    bar.queue_draw();
                }
            }),
            highlighted.clone(),
        );
        bar_weak.borrow_mut().set(Some(&bar));
        // A theme switch must redraw the Cairo colors as well as the CSS tiles.
        let weak = bar.downgrade();
        let signal = libadwaita::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(bar) = weak.upgrade() {
                bar.queue_draw();
            }
        });
        let signal = std::cell::RefCell::new(Some(signal));
        bar.connect_destroy(move |_| {
            if let Some(signal) = signal.borrow_mut().take() {
                libadwaita::StyleManager::default().disconnect(signal);
            }
        });
        self.body.append(&bar);
        for (i, row) in rows.iter().enumerate() {
            let motion = gtk4::EventControllerMotion::new();
            let state = highlighted.clone();
            let callback = hover.clone();
            let weak = bar.downgrade();
            motion.connect_enter(move |_, _, _| {
                state.set(Some(i));
                callback(Some(i));
                if let Some(bar) = weak.upgrade() {
                    bar.queue_draw();
                }
            });
            let state = highlighted.clone();
            let callback = hover.clone();
            let weak = bar.downgrade();
            motion.connect_leave(move |_| {
                state.set(None);
                callback(None);
                if let Some(bar) = weak.upgrade() {
                    bar.queue_draw();
                }
            });
            row.button.add_controller(motion);
        }
        for range in [0..8, 8..10] {
            let card = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            card.add_css_class("pc-card");
            for i in range {
                // AI models and games appear only when the scan found some.
                if (6..8).contains(&i) && data.sizes[i] == 0 {
                    continue;
                }
                card.append(&rows[i].button);
            }
            self.body.append(&card);
        }
        let foot = gtk4::Label::new(Some(&tr_args("{size} of the used space is already in the Trash. Empty it in Files to free the space.", &[("{size}", &size(data.trash))])));
        foot.set_xalign(0.0);
        foot.set_wrap(true);
        foot.add_css_class("pc-tertiary");
        self.body.append(&foot);
    }
}

fn action_button(icon: &str, title: &str, primary: bool) -> gtk4::Button {
    let button = gtk4::Button::new();
    button.add_css_class(if primary {
        "pc-primary"
    } else {
        "pc-secondary-button"
    });
    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    content.append(&gtk4::Image::from_icon_name(icon));
    content.append(&gtk4::Label::new(Some(&tr(title))));
    button.set_child(Some(&content));
    button
}
/// Primary cleanup button: the old blue "Clean" pill with the app's bolt,
/// at the size of the Purge-style header buttons.
fn clean_go_button(title: &str) -> gtk4::Button {
    let button = gtk4::Button::new();
    button.add_css_class("pc-clean-go");
    let bolt = super::cleanup_fx::bolt_icon(18);
    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    content.append(&bolt);
    content.append(&gtk4::Label::new(Some(&tr(title))));
    button.set_child(Some(&content));
    button
}
fn size(bytes: u64) -> String {
    bytesize::ByteSize(bytes).to_string()
}
fn fraction(bytes: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        bytes as f64 / total as f64
    }
}

#[derive(Clone, Default)]
struct Readings {
    total: u64,
    used: u64,
    free: u64,
    sizes: [u64; 10],
    safe: [u64; 3],
    trash: u64,
    large_count: usize,
    model_count: usize,
    game_count: usize,
    leftover_count: usize,
    app_count: usize,
}

// Measurements are read-only and independent of enabled deletion rules. Each
// inode belongs to one category, including caches inside installed app data.
fn readings(storage: &Storage, cancel: &Arc<AtomicBool>) -> Readings {
    let Some(home) = dirs::home_dir() else {
        return Readings::default();
    };
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| home.join(".local/share"));
    let mut cache_roots = vec![
        dirs::cache_dir().unwrap_or_else(|| home.join(".cache")),
        home.join(".cache"),
    ];
    for rule in storage
        .get_browser_rules()
        .iter()
        .filter(|r| r.data_type == crate::models::BrowserDataType::Cache)
    {
        for root in rule.effective_paths() {
            for relative in rule.data_type.relative_paths(rule.browser) {
                cache_roots.extend(expand_pattern(&root.join(relative)));
            }
        }
    }
    let dev_roots: Vec<_> = storage
        .get_app_rules()
        .iter()
        .filter(|r| r.is_developer_tool())
        .filter_map(|r| r.expanded_path())
        .flat_map(|p| expand_pattern(&p))
        .collect();
    let orphan_paths = Vec::new();
    let mut system_roots = crate::models::SystemRuleType::PackageCache.paths();
    system_roots.extend([
        PathBuf::from("/var/cache/libdnf5"),
        PathBuf::from("/var/log/journal"),
    ]);
    system_roots.extend(unused_flatpak_paths(
        &data_dir,
        Path::new("/var/lib/flatpak"),
    ));
    let mut data = measure(
        &home,
        &data_dir,
        Path::new("/var/lib/flatpak"),
        &cache_roots,
        &dev_roots,
        &orphan_paths,
        &system_roots,
        cancel,
    );
    let disks = sysinfo::Disks::new_with_refreshed_list();
    if let Some(disk) = disks
        .list()
        .iter()
        .filter(|d| home.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().components().count())
    {
        data.total = disk.total_space();
        data.free = disk.available_space().min(data.total);
        data.used = data.total - data.free;
    }
    normalize(&mut data);
    data
}

fn expand_pattern(path: &Path) -> Vec<PathBuf> {
    glob::glob(&path.to_string_lossy())
        .ok()
        .map(|paths| paths.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

fn under(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| path.starts_with(r))
}

#[allow(clippy::too_many_arguments)]
fn measure(
    home: &Path,
    data_dir: &Path,
    system_flatpak: &Path,
    cache_roots: &[PathBuf],
    dev_roots: &[PathBuf],
    orphan_paths: &[PathBuf],
    system_roots: &[PathBuf],
    cancel: &AtomicBool,
) -> Readings {
    use std::os::unix::fs::MetadataExt;
    let mut data = Readings::default();
    let trash = data_dir.join("Trash");
    let app_roots = vec![
        home.join(".var/app"),
        data_dir.join("flatpak/app"),
        system_flatpak.join("app"),
        home.join("Applications"),
    ];
    data.app_count = installed_app_count(home, data_dir, system_flatpak);
    let mut seen = std::collections::HashSet::new();
    // Visit explicit categories first so large files cannot claim package
    // caches, runtime deployments or app payloads under a different mount.
    let mut roots = vec![trash.clone()];
    roots.extend(system_roots.iter().cloned());
    roots.extend(cache_roots.iter().cloned());
    roots.extend(dev_roots.iter().cloned());
    roots.extend(app_roots.iter().cloned());
    roots.push(home.to_path_buf());
    for root in roots {
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if cancel.load(Ordering::Relaxed) {
                return data;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !seen.insert((meta.dev(), meta.ino())) {
                continue;
            }
            let path = entry.path();
            let bytes = meta.len();
            if path.starts_with(&trash) {
                data.trash += bytes;
                continue;
            }
            let index = if under(path, system_roots) {
                Some(2)
            } else if under(path, dev_roots) {
                Some(1)
            } else if under(path, cache_roots) || flatpak_cache(path, home) {
                Some(0)
            } else if under(path, orphan_paths) {
                Some(5)
            } else if under(path, &app_roots) {
                Some(4)
            } else if bytes > 5_000_000 {
                data.large_count += 1;
                Some(3)
            } else {
                None
            };
            if let Some(index) = index {
                data.sizes[index] += bytes;
                if index < 3 {
                    // Journal retention is decided in System; reporting the
                    // whole active journal as safe would overstate cleanup.
                    let journal = system_roots
                        .iter()
                        .any(|r| r.ends_with("var/log/journal") && path.starts_with(r));
                    if index != 2 || !journal {
                        data.safe[index] += bytes;
                    }
                }
            }
        }
    }
    data
}

fn flatpak_lines(scope: &str, args: &[&str]) -> Vec<String> {
    std::process::Command::new("flatpak")
        .arg(scope)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn unused_flatpak_paths(data_dir: &Path, system_flatpak: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for (scope, base) in [
        ("--user", data_dir.join("flatpak")),
        ("--system", system_flatpak.to_path_buf()),
    ] {
        let apps = flatpak_lines(scope, &["list", "--app", "--columns=runtime"]);
        let runtimes = flatpak_lines(scope, &["list", "--runtime", "--columns=ref"]);
        for reference in crate::services::system_preview::flatpak_unused(&apps, &runtimes) {
            // Resolve only the deployment's active link, never walk arbitrary
            // symlinks within the deployment or follow the OSTree repo twice.
            let relative = reference.strip_prefix("runtime/").unwrap_or(&reference);
            if relative.split('/').count() != 3
                || relative.split('/').any(|s| s == ".." || s.is_empty())
            {
                continue;
            }
            if let Ok(active) = base
                .join("runtime")
                .join(relative)
                .join("active")
                .canonicalize()
            {
                paths.push(active);
            }
        }
    }
    paths
}

fn installed_app_count(home: &Path, data_dir: &Path, system_flatpak: &Path) -> usize {
    let mut apps = std::collections::HashSet::new();
    for root in [data_dir.join("flatpak/app"), system_flatpak.join("app")] {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten().filter(|e| e.path().is_dir()) {
                apps.insert(format!("desktop:{}", entry.file_name().to_string_lossy()));
            }
        }
    }
    let image_key = |path: &Path| {
        format!(
            "image:{}",
            path.canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .display()
        )
    };
    for entry in walkdir::WalkDir::new(home.join("Applications"))
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("appimage"))
        {
            apps.insert(image_key(entry.path()));
        }
    }
    for entry in walkdir::WalkDir::new(data_dir.join("applications"))
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "desktop") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let mut fields = std::collections::HashMap::new();
        let mut in_entry = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
            } else if in_entry {
                if let Some((k, v)) = line.split_once('=') {
                    fields.insert(k, v);
                }
            }
        }
        if fields.get("Type") != Some(&"Application") || fields.get("Hidden") == Some(&"true") {
            continue;
        }
        let exec_image = fields
            .get("Exec")
            .and_then(|exec| glib::shell_parse_argv(exec).ok())
            .and_then(|args| {
                args.into_iter().find(|arg| {
                    Path::new(arg)
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("appimage"))
                })
            });
        if let Some(image) = exec_image {
            apps.insert(image_key(Path::new(&image)));
        } else {
            let id = fields
                .get("X-Flatpak")
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    entry
                        .path()
                        .file_stem()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                });
            apps.insert(format!("desktop:{id}"));
        }
    }
    apps.len()
}

fn flatpak_cache(path: &Path, home: &Path) -> bool {
    path.strip_prefix(home.join(".var/app"))
        .ok()
        .and_then(|p| p.components().nth(1))
        .is_some_and(|c| c.as_os_str() == "cache")
}
fn normalize(data: &mut Readings) {
    data.sizes[8] = data.used.saturating_sub(data.sizes[..8].iter().sum());
    data.sizes[9] = data.free;
}

// System and Flatpak data may live outside the home volume. Keep the measured
// row sizes intact; fit only the chart to the home volume's used space.
fn chart_sizes(data: &Readings) -> [u64; 10] {
    let mut chart = data.sizes;
    let known: u64 = chart[..8].iter().sum();
    if known > data.used {
        for size in &mut chart[..8] {
            *size = ((*size as u128 * data.used as u128) / known as u128) as u64;
        }
    }
    chart[8] = data.used.saturating_sub(chart[..8].iter().sum());
    chart[9] = data.free;
    chart
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chart_fits_home_without_truncating_measured_sizes() {
        let mut data = Readings {
            total: 100,
            used: 70,
            free: 30,
            sizes: [50, 50, 40, 0, 0, 0, 0, 0, 0, 0],
            safe: [50, 50, 40],
            ..Default::default()
        };
        normalize(&mut data);
        assert_eq!(chart_sizes(&data).iter().sum::<u64>(), data.total);
        assert_eq!(data.sizes[1], 50);
        assert_eq!(data.safe, [50, 50, 40]);
    }
    fn fixture_file(path: &Path, size: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(path).unwrap().set_len(size).unwrap();
    }

    #[test]
    fn measures_safe_caches_system_payloads_and_apps_without_enabled_rules() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let local = home.join(".local/share");
        let system_flatpak = dir.path().join("var/lib/flatpak");
        let packages = dir.path().join("var/cache/pacman/pkg");
        let journal = dir.path().join("var/log/journal");
        let runtime = system_flatpak.join("runtime/unused/commit");
        let cache = home.join(".cache");
        let browser = home.join(".config/browser/Default/Cache");
        let dev = home.join(".npm/_cacache");
        let orphan = home.join(".config/removed");
        for (path, bytes) in [
            (cache.join("app/cache"), 10),
            (cache.join("thumbnails/image"), 20),
            (browser.join("code"), 30),
            (home.join(".config/browser/Default/Cookies"), 40),
            (dev.join("package"), 50),
            (packages.join("package.pkg.tar.zst"), 60),
            (journal.join("logs.journal"), 70),
            (runtime.join("files/payload"), 80),
            (home.join(".var/app/org.test.App/data/user-data"), 90),
            (home.join(".var/app/org.test.App/cache/cache"), 11),
            (
                system_flatpak.join("app/org.test.App/commit/files/payload"),
                100,
            ),
            (
                local.join("flatpak/app/org.test.App/commit/files/payload"),
                30,
            ),
            (home.join("Applications/Demo.AppImage"), 110),
            (local.join("Trash/files/deleted"), 120),
            (orphan.join("config"), 130),
            (home.join("Documents/video"), 6_000_000),
        ] {
            fixture_file(&path, bytes);
        }
        // Multiple roots and hard links must not inflate any byte totals.
        std::fs::hard_link(packages.join("package.pkg.tar.zst"), packages.join("copy")).unwrap();
        let data = measure(
            &home,
            &local,
            &system_flatpak,
            &[cache.clone(), cache.join("thumbnails"), browser],
            &[dev],
            &[orphan],
            &[packages, journal, runtime],
            &AtomicBool::new(false),
        );
        assert_eq!(data.sizes[..6], [71, 50, 210, 6_000_000, 330, 130]);
        assert_eq!(data.safe, [71, 50, 140]);
        assert_eq!(data.large_count, 1);
        assert_eq!(data.app_count, 2); // Same Flatpak in both scopes plus AppImage.
        assert_eq!(data.trash, 120);
    }

    #[test]
    fn installed_app_count_deduplicates_launchers_and_ignores_hidden_entries() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let local = home.join(".local/share");
        let system_flatpak = dir.path().join("system-flatpak");
        let image = home.join("Applications/Demo.AppImage");
        fixture_file(&image, 1);
        std::fs::create_dir_all(system_flatpak.join("app/org.test.App")).unwrap();
        std::fs::create_dir_all(local.join("flatpak/app/org.test.App")).unwrap();
        let desktop = local.join("applications");
        std::fs::create_dir_all(&desktop).unwrap();
        for (name, text) in [
            (
                "flatpak.desktop",
                "[Desktop Entry]\nType=Application\nX-Flatpak=org.test.App\n".to_owned(),
            ),
            (
                "image.desktop",
                format!(
                    "[Desktop Entry]\nType=Application\nExec=\"{}\" %U\n",
                    image.display()
                ),
            ),
            (
                "native.desktop",
                "[Desktop Entry]\nType=Application\nExec=native\n".to_owned(),
            ),
            (
                "hidden.desktop",
                "[Desktop Entry]\nType=Application\nHidden=true\n".to_owned(),
            ),
            (
                "link.desktop",
                "[Desktop Entry]\nType=Link\nURL=https://example.test\n".to_owned(),
            ),
        ] {
            std::fs::write(desktop.join(name), text).unwrap();
        }
        assert_eq!(installed_app_count(&home, &local, &system_flatpak), 3);
    }

    #[test]
    fn flatpak_cache_is_only_the_app_cache_directory() {
        let home = Path::new("/home/test");
        assert!(flatpak_cache(
            Path::new("/home/test/.var/app/org.test/cache/a"),
            home
        ));
        assert!(!flatpak_cache(
            Path::new("/home/test/.var/app/org.test/data/cache/a"),
            home
        ));
    }
}
