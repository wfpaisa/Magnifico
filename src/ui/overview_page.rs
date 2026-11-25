// Adapted from Purge OverviewView.swift (MIT, © 2026 Jithin Sabu).
// https://github.com/jithin-sabu/purge-app — see THIRD_PARTY_LICENSES.md.
use super::clean_scan_store::CleanScanStore;
use super::flight_layer::FlightLayer;
use super::overview_grid::{Grid, GridSegment, Sweep};
use super::purge_widgets as widgets;
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
const TOKENS: [&str; 10] = [
    "pc_chart_caches",
    "pc_chart_dev",
    "pc_chart_system",
    "pc_chart_large",
    "pc_chart_apps",
    "pc_chart_leftovers",
    "pc_chart_models",
    "pc_chart_games",
    "pc_chart_else",
    "pc_chart_free",
];
const FREE: usize = 9;
/// Segment id of the grey, unselected used space on a category page.
const STRIP_USED: usize = 10;
/// Opacity of the cells of a category page that are not selected.
const STRIP_FADED: f64 = 0.28;

#[derive(Clone, Copy)]
enum Tag {
    Safe,
    Review,
    Careful,
}
/// How much care each category needs before anything in it is removed.
const TAGS: [Tag; 9] = [
    Tag::Safe,
    Tag::Safe,
    Tag::Safe,
    Tag::Careful,
    Tag::Review,
    Tag::Review,
    Tag::Review,
    Tag::Review,
    Tag::Careful,
];

/// State that outlives the page's re-renders: the grid keeps animating while
/// the rows and labels around it are rebuilt.
struct Fx {
    grid: Grid,
    /// The map shown on top of each category page, by category index.
    strips: RefCell<Vec<(usize, Grid)>>,
    flights: FlightLayer,
    trash: RefCell<glib::WeakRef<gtk4::Widget>>,
    /// A removal is being applied: its cells fly to the Trash.
    sweep_next: Cell<bool>,
    /// Hold renders until the removal has reached every store.
    hold: Cell<bool>,
    /// Categories (bit per index, bit 8 = everything else) still being
    /// measured when the page was last drawn.
    waiting: Cell<u16>,
    /// Categories in the order their size arrived while analysing: cells
    /// already placed never move until the final sort.
    arrival: RefCell<Vec<usize>>,
    /// `MAGNIFICO_SIM`: fake readings drive the page so the animations can be
    /// watched without scanning or deleting anything.
    sim: bool,
    sim_mask: Cell<u16>,
    sim_run: Cell<u32>,
}

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
    rendered: Rc<RefCell<Option<Readings>>>,
    disk_revision: Rc<Cell<u64>>,
    fx: Rc<Fx>,
}

/// Cells of the disk map, heaviest category first and the free space last.
fn build_segments(data: &Readings, waiting: u16, arrival: &[usize]) -> Vec<GridSegment> {
    // Space still being measured flickers; it is everything left over
    // once every category is known, so it is the last to settle.
    let is_waiting = |i: usize| waiting & (1 << i) != 0;
    let mut chart = chart_sizes(&Readings {
        sizes: {
            let mut sizes = data.sizes;
            for (i, size) in sizes.iter_mut().enumerate().take(8) {
                if is_waiting(i) {
                    *size = 0;
                }
            }
            sizes
        },
        ..data.clone()
    });
    if waiting != 0 {
        chart[8] = data
            .used
            .saturating_sub(chart[..8].iter().sum::<u64>())
            .min(data.used);
    }
    let mut by_weight: Vec<usize> = (0..8).filter(|&i| !is_waiting(i)).collect();
    if waiting == 0 {
        by_weight.push(8);
    }
    if waiting != 0 && !arrival.is_empty() {
        // Still analysing: keep each category where it landed.
        let mut placed: Vec<usize> = arrival.iter().copied().filter(|&i| !is_waiting(i)).collect();
        let missing: Vec<usize> = by_weight.iter().copied().filter(|i| !placed.contains(i)).collect();
        placed.extend(missing);
        by_weight = placed;
    } else {
        by_weight.sort_by_key(|&i| std::cmp::Reverse(chart[i]));
    }
    let describe = |i: usize, bytes: u64| {
        tr_args(
            "{category} · {size}",
            &[("{category}", &tr(TITLES[i])), ("{size}", &size(bytes))],
        )
    };
    let mut segments: Vec<GridSegment> = by_weight
        .iter()
        .map(|&i| GridSegment {
            id: i,
            fraction: fraction(chart[i], data.total),
            color: TOKENS[i],
            alpha: 1.0,
            label: describe(i, data.sizes[i]),
        })
        .collect();
    if waiting != 0 {
        segments.push(GridSegment {
            id: super::overview_grid::PENDING,
            fraction: fraction(chart[8], data.total),
            color: TOKENS[8],
            alpha: 1.0,
            label: tr("Scanning…"),
        });
    }
    segments.push(GridSegment {
        id: FREE,
        fraction: fraction(chart[FREE], data.total),
        color: TOKENS[FREE],
        alpha: 1.0,
        label: describe(FREE, data.free),
    });
segments
}

/// Map of a category page: every cell is that category, the whole map being
/// everything the page lists. What is selected shows in full colour, the rest
/// faded like an unticked item.
fn strip_segments(data: &Readings, category: usize, selected: u64, total: u64) -> Vec<GridSegment> {
    let total = if total > 0 { total } else { data.sizes[category] };
    let selected = selected.min(total);
    let picked = if total > 0 { selected as f64 / total as f64 } else { 0.0 };
    let mut segments = Vec::new();
    if selected > 0 {
        segments.push(GridSegment {
            id: category,
            fraction: picked,
            color: TOKENS[category],
            alpha: 1.0,
            label: tr_args(
                "{category} · {size}",
                &[("{category}", &tr(TITLES[category])), ("{size}", &size(selected))],
            ),
        });
    }
    segments.push(GridSegment {
        id: STRIP_USED,
        fraction: 1.0 - picked,
        color: TOKENS[category],
        alpha: STRIP_FADED,
        label: tr_args("{size} used", &[("{size}", &size(total - selected))]),
    });
    segments
}

impl OverviewPage {
    pub fn new(
        storage: Arc<Storage>,
        store: CleanScanStore,
        review: ReviewScanStore,
        summary: Option<crate::services::scan_cache::ScanSummary>,
        on_sizes: impl Fn([u64; 6], u64) + 'static,
        flights: FlightLayer,
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
        let simulating = std::env::var_os("MAGNIFICO_SIM").is_some();
        let sim_clean = gtk4::Button::with_label("Simular limpieza");
        if simulating {
            // Nothing real is scanned or deleted in the simulation.
            clean.set_visible(false);
            actions.append(&sim_clean);
        }
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
            latest: Rc::new(RefCell::new(summary.map(Readings::from))),
            rendered: Rc::new(RefCell::new(None)),
            disk_revision: Rc::new(Cell::new(0)),
            fx: Rc::new(Fx {
                grid: Grid::new(),
                strips: RefCell::new(Vec::new()),
                flights,
                trash: RefCell::new(glib::WeakRef::new()),
                sweep_next: Cell::new(false),
                hold: Cell::new(false),
                waiting: Cell::new(0),
                arrival: RefCell::new(Vec::new()),
                sim: std::env::var_os("MAGNIFICO_SIM").is_some(),
                sim_mask: Cell::new(0),
                sim_run: Cell::new(0),
            }),
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
        let rendered = page.rendered.clone();
        let disk_revision = page.disk_revision.clone();
        let fx = page.fx.clone();
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
                    rendered: rendered.clone(),
                    disk_revision: disk_revision.clone(),
                    fx: fx.clone(),
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
        page.render_cached();
        if page.fx.sim {
            let sim = page.clone();
            sim_clean.connect_clicked(move |_| sim.simulate_clean());
            page.simulate_load();
        } else {
            page.store.request_all_staggered(true);
            page.review.request_all_staggered(true);
            page.refresh();
        }
        let rendered = page.rendered.clone();
        page.widget.add_tick_callback(move |_, _| {
            if rendered.borrow().is_some() {
                tracing::debug!(
                    elapsed_ms = crate::STARTED.elapsed().as_millis() as u64,
                    "overview_values_visible"
                );
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
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
        let rendered = self.rendered.clone();
        let disk_revision = self.disk_revision.clone();
        let fx = self.fx.clone();
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
                    rendered: rendered.clone(),
                    disk_revision: disk_revision.clone(),
                    fx: fx.clone(),
                    store,
                    review,
                }
                .render_cached();
            }
        }
    }
    pub fn scan_everything(&self) {
        if self.fx.sim {
            self.simulate_load();
            return;
        }
        self.store.request_all(true);
        self.review.request_all(true);
        self.refresh();
    }
    pub fn refresh(&self) {
        if self.fx.sim {
            return;
        }
        if self.loading.replace(true) {
            return;
        }
        self.cancel.store(false, Ordering::Relaxed);
        self.scan.set_sensitive(false);
        self.subtitle.set_text(&tr("Scanning…"));
        self.update_loader();
        let (tx, rx) = async_channel::bounded(1);
        let storage = self.storage.clone();
        let cancel = self.cancel.clone();
        crate::runtime().spawn_blocking(move || {
            let _permit = crate::services::scan_jobs::acquire();
            let _ = tx.send_blocking(readings(&storage, &cancel));
        });
        let page = self.clone();
        let revision = self.disk_revision.get();
        glib::spawn_future_local(async move {
            if let Ok(data) = rx.recv().await {
                let mut latest = page.latest.borrow_mut();
                if revision != page.disk_revision.get() {
                    // Keep the local delta when an older disk reading finishes.
                } else if let Some(previous) = latest.as_mut() {
                    previous.total = data.total;
                    previous.used = data.used;
                    previous.free = data.free;
                    previous.trash = data.trash;
                } else {
                    *latest = Some(data);
                }
            } else {
                // Keep the last successful readings on refresh failure.
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
            self.latest.borrow().is_none() && (self.loading.get() || phase.is_some()),
            phase.unwrap_or("Disk usage"),
        );
    }
    pub fn render_cached(&self) {
        if self.fx.hold.get() {
            return;
        }
        self.update_loader();
        self.scan
            .set_sensitive(!self.loading.get() && !self.store.busy() && !self.review.busy());
        let mut data = match self.latest.borrow().clone() {
            Some(data) => data,
            None if self
                .store
                .items(crate::services::clean_sources::CleanPageKind::AppCaches)
                .is_some()
                || self
                    .store
                    .items(crate::services::clean_sources::CleanPageKind::DevTools)
                    .is_some()
                || self
                    .store
                    .items(crate::services::clean_sources::CleanPageKind::System)
                    .is_some()
                || self.review.large().is_some()
                || self.review.apps().is_some()
                || self
                    .review
                    .items(super::review_scan_store::Kind::Models)
                    .is_some()
                || self
                    .review
                    .items(super::review_scan_store::Kind::Games)
                    .is_some() =>
            {
                Readings::default()
            }
            None => return,
        };
        if !self.fx.sim {
            self.merge_stores(&mut data);
        }
        normalize(&mut data);
        if !self.fx.sim {
            crate::services::scan_cache::update(data.summary());
        }
        *self.latest.borrow_mut() = Some(data.clone());
        self.update_loader();
        let waiting = self.waiting_mask();
        if self.rendered.borrow().as_ref() != Some(&data) || self.fx.waiting.get() != waiting {
            let previous = self.rendered.borrow().clone();
            let was_waiting = self.fx.waiting.replace(waiting);
            let sweep = self.fx.sweep_next.replace(false) && previous.is_some();
            self.clear_readings();
            self.render(&data, previous.as_ref(), (was_waiting, waiting), sweep);
            let sizes: [u64; 6] = data.sizes[..6].try_into().unwrap();
            let trash = data.trash;
            if sweep {
                // The sidebar numbers change once the cells have landed.
                let on_sizes = self.on_sizes.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(1700), move || {
                    on_sizes(sizes, trash)
                });
            } else {
                (self.on_sizes)(sizes, trash);
            }
            *self.rendered.borrow_mut() = Some(data.clone());
        }
        self.subtitle.set_text(&tr(
            if self.store.busy() || self.review.busy() || self.loading.get() {
                "Scanning…"
            } else {
                "Scanned just now"
            },
        ));
    }

    /// Fold the stores' exact snapshots into the readings.
    fn merge_stores(&self, data: &mut Readings) {
        for kind in [
            crate::services::clean_sources::CleanPageKind::AppCaches,
            crate::services::clean_sources::CleanPageKind::DevTools,
            crate::services::clean_sources::CleanPageKind::System,
        ] {
            let Some(items) = self.store.items(kind) else {
                continue;
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
        if let Some(large) = self.review.large() {
            data.sizes[3] = large.bytes();
            data.large_count = large.files.len();
        }
        if let Some(apps) = self.review.apps() {
            data.sizes[4] = apps.app_bytes();
            data.sizes[5] = apps.leftover_bytes();
            data.app_count = apps.apps.len();
            data.leftover_count = apps.leftovers.len();
        }
        if let Some(models) = self.review.items(super::review_scan_store::Kind::Models) {
            data.sizes[6] = models.bytes();
            data.model_count = models.items.len();
        }
        if let Some(games) = self.review.items(super::review_scan_store::Kind::Games) {
            data.sizes[7] = games.bytes();
            data.game_count = games.items.len();
        }
    }

    /// Bit per category still being measured; bit 8 (everything else) is
    /// unknown whenever anything else is.
    fn waiting_mask(&self) -> u16 {
        if self.fx.sim {
            return self.fx.sim_mask.get();
        }
        use super::review_scan_store::Kind;
        use crate::services::clean_sources::CleanPageKind as Clean;
        let mut mask = 0u16;
        for (bit, kind) in [Clean::AppCaches, Clean::DevTools, Clean::System]
            .into_iter()
            .enumerate()
        {
            if self.store.loading(kind) {
                mask |= 1 << bit;
            }
        }
        let first_scan = !self.review.has_summary();
        if self.review.loading(Kind::LargeFiles) || (first_scan && self.review.large().is_none()) {
            mask |= 1 << 3;
        }
        if self.review.loading(Kind::Apps) || (first_scan && self.review.apps().is_none()) {
            mask |= 3 << 4;
        }
        if self.review.loading(Kind::Models) {
            mask |= 1 << 6;
        }
        if self.review.loading(Kind::Games) {
            mask |= 1 << 7;
        }
        if mask != 0 {
            mask |= 1 << 8;
        }
        mask
    }

    fn sim_readings() -> Readings {
        const GB: u64 = 1_000_000_000;
        let gb = |tenths: u64| tenths * GB / 10;
        let mut data = Readings {
            total: gb(7318),
            used: gb(6730),
            free: gb(588),
            sizes: [gb(260), gb(149), gb(63), gb(930), gb(505), gb(94), gb(1018), gb(1623), 0, 0],
            safe: [gb(234), gb(90), gb(62)],
            trash: gb(48),
            large_count: 166,
            model_count: 22,
            game_count: 10,
            leftover_count: 103,
            app_count: 134,
        };
        normalize(&mut data);
        data
    }

    /// Simulation: the categories arrive one by one, like a real scan.
    fn simulate_load(&self) {
        let full = Self::sim_readings();
        let run = self.fx.sim_run.get() + 1;
        self.fx.sim_run.set(run);
        let mut start = full.clone();
        start.sizes = [0; 10];
        start.safe = [0; 3];
        *self.latest.borrow_mut() = Some(start);
        self.fx.sim_mask.set(0x1FF);
        self.render_cached();
        for (n, i) in [0usize, 3, 1, 4, 2, 5, 6, 7].into_iter().enumerate() {
            let page = self.clone();
            let full = full.clone();
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(900 + 800 * n as u64),
                move || {
                    if page.fx.sim_run.get() != run {
                        return;
                    }
                    if let Some(data) = page.latest.borrow_mut().as_mut() {
                        data.sizes[i] = full.sizes[i];
                        if i < 3 {
                            data.safe[i] = full.safe[i];
                        }
                    }
                    page.fx.sim_mask.set(page.fx.sim_mask.get() & !(1 << i));
                    page.render_cached();
                },
            );
        }
        let page = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(900 + 800 * 8 + 300), move || {
            if page.fx.sim_run.get() != run {
                return;
            }
            page.fx.sim_mask.set(0);
            page.render_cached();
        });
    }

    /// Simulation: first the safe items go, then a quarter of the games.
    fn simulate_clean(&self) {
        if let Some(data) = self.latest.borrow_mut().as_mut() {
            let safe: u64 = data.safe.iter().sum();
            if safe > 0 {
                for i in 0..3 {
                    data.sizes[i] = data.sizes[i].saturating_sub(data.safe[i]);
                    data.safe[i] = 0;
                }
                data.apply_disk_delta(safe, 0);
            } else {
                let cut = data.sizes[7] / 4;
                data.sizes[7] -= cut;
                data.apply_disk_delta(cut, 0);
            }
        }
        self.fx.sweep_next.set(true);
        self.render_cached();
        self.fx.sweep_next.set(false);
    }

    /// Simulation: part of one category goes, as if a selection was removed.
    fn simulate_category(&self, category: usize) {
        let (picked, listed) = super::selection_bus::state(category);
        let listed = if listed > 0 {
            listed
        } else {
            self.latest.borrow().as_ref().map_or(0, |data| data.sizes[category])
        };
        if picked == 0 {
            // First the page "selects" 40%, then it removes it.
            super::selection_bus::report(category, listed * 2 / 5, listed);
            return;
        }
        let cut = picked;
        let page = self.clone();
        if let Some(data) = self.latest.borrow_mut().as_mut() {
            data.sizes[category] -= cut.min(data.sizes[category]);
            if category < 3 {
                data.safe[category] = data.safe[category].min(data.sizes[category]);
            }
            data.apply_disk_delta(cut, 0);
        }
        self.fx.sweep_next.set(true);
        super::selection_bus::report(category, 0, listed.saturating_sub(cut));
        page.render_cached();
        self.fx.sweep_next.set(false);
    }

    pub fn apply_removed(&self, update: &crate::services::removal_update::RemovalUpdate) {
        self.disk_revision.set(self.disk_revision.get() + 1);
        {
            let mut latest = self.latest.borrow_mut();
            let data = latest.get_or_insert_with(Readings::default);
            data.apply_disk_delta(update.freed_bytes, update.trashed_bytes);
        }
        self.fx.hold.set(false);
        self.render_cached();
        self.fx.sweep_next.set(false);
    }

    /// Put a copy of the disk map under the header of a category page, with
    /// that category in colour. Its cells fly to the Trash on removals.
    pub fn attach_strip(&self, page: &gtk4::Widget, category: usize) {
        // Pages that show a loading state keep their content inside a stack.
        let content = page
            .first_child()
            .and_then(|child| child.downcast::<gtk4::Stack>().ok())
            .and_then(|stack| stack.child_by_name("content"));
        let Some(page) = content
            .as_ref()
            .unwrap_or(page)
            .downcast_ref::<gtk4::Box>()
        else {
            return;
        };
        let strip = Grid::with_rows(super::overview_grid::STRIP_ROWS);
        strip.set_instant_when_hidden(true);
        match page.first_child() {
            Some(header) => page.insert_child_after(strip.widget(), Some(&header)),
            None => page.append(strip.widget()),
        }
        let data = self.rendered.borrow().clone();
        if let Some(data) = data {
            let (selected, total) = super::selection_bus::state(category);
            strip.update(
                strip_segments(&data, category, selected, total),
                FREE,
                None,
                false,
            );
        }
        // The uninstaller has two maps and shows the one of its active tab.
        let shown = move || !matches!(category, 4 | 5) || super::selection_bus::apps_view() == category;
        strip.widget().set_visible(shown());
        let page = self.clone();
        let widget = strip.widget().clone();
        super::selection_bus::subscribe(move |changed| {
            widget.set_visible(shown());
            if changed == category {
                page.refresh_strips(category);
            }
        });
        if self.fx.sim {
            // Simulation: clicking the map removes 40% of that category.
            let click = gtk4::GestureClick::new();
            click.set_button(0);
            click.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let page = self.clone();
            click.connect_pressed(move |_, _, _, _| page.simulate_category(category));
            strip.widget().add_controller(click);
        }
        self.fx.strips.borrow_mut().push((category, strip));
    }

    /// The selection of a category page changed: repaint its map. While a
    /// removal is on its way the next render does it, flying the cells.
    fn refresh_strips(&self, category: usize) {
        if self.fx.hold.get() || self.fx.sweep_next.get() {
            return;
        }
        let Some(data) = self.rendered.borrow().clone() else {
            return;
        };
        let (selected, total) = super::selection_bus::state(category);
        for (_, strip) in self.fx.strips.borrow().iter().filter(|(c, _)| *c == category) {
            strip.update(
                strip_segments(&data, category, selected, total),
                FREE,
                None,
                false,
            );
        }
    }

    /// The Trash button the cleaned cells fly to.
    pub fn set_trash_target(&self, target: &impl IsA<gtk4::Widget>) {
        self.fx.trash.borrow().set(Some(target.upcast_ref()));
    }

    /// A removal is about to reach the stores: wait for the final numbers and
    /// show the cells leaving for the Trash instead of reflowing them.
    pub fn begin_removal(&self) {
        self.fx.hold.set(true);
        self.fx.sweep_next.set(true);
        let page = self.clone();
        // Never stay frozen if the removal path ends without reaching us.
        glib::timeout_add_local_once(std::time::Duration::from_secs(4), move || {
            if page.fx.hold.replace(false) {
                page.fx.sweep_next.set(false);
                page.render_cached();
            }
        });
    }
    pub fn empty_trash(&self, bytes: u64, remaining: u64) {
        self.disk_revision.set(self.disk_revision.get() + 1);
        {
            let mut latest = self.latest.borrow_mut();
            let data = latest.get_or_insert_with(Readings::default);
            data.trash = remaining;
            data.apply_disk_delta(bytes, 0);
        }
        self.render_cached();
    }

    pub fn save_summary(&self) {
        if self.fx.sim {
            return;
        }
        if let Some(data) = self.latest.borrow().as_ref() {
            crate::services::scan_cache::update(data.summary());
        }
    }

    fn clear_readings(&self) {
        let Some(header) = self.body.first_child() else {
            return;
        };
        // The grid stays in place: its allocation is needed to launch the
        // cells towards the Trash.
        let grid: gtk4::Widget = self.fx.grid.widget().clone().upcast();
        let mut next = header.next_sibling();
        while let Some(child) = next {
            next = child.next_sibling();
            if child != grid {
                self.body.remove(&child);
            }
        }
    }

    fn render(
        &self,
        data: &Readings,
        prev: Option<&Readings>,
        (was_waiting, waiting): (u16, u16),
        sweep: bool,
    ) {
        let animated = libadwaita::is_animations_enabled(&self.body);
        // Numbers count towards their new value while the cells fly away.
        let (delay, span) = if sweep { (0.6, 1.1) } else { (0.0, 0.5) };
        let tween_from = |old: Option<u64>, new: u64| if animated { old.unwrap_or(new) } else { new };

        let summary = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let used = gtk4::Label::new(None);
        used.add_css_class("pc-disk-used");
        used.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        tween(
            &used,
            tween_from(prev.map(|p| p.used), data.used),
            data.used,
            (delay, span),
            |bytes| tr_args("{size} used", &[("{size}", &size(bytes))]),
        );
        summary.append(&used);
        let total = gtk4::Label::new(Some(&tr_args(
            "of {size}",
            &[("{size}", &size(data.total))],
        )));
        total.add_css_class("pc-secondary");
        total.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        summary.append(&total);
        let free = gtk4::Label::new(None);
        free.set_hexpand(true);
        free.set_xalign(1.0);
        free.add_css_class("pc-secondary");
        free.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        tween(
            &free,
            tween_from(prev.map(|p| p.free), data.free),
            data.free,
            (delay, span),
            |bytes| tr_args("{size} free", &[("{size}", &size(bytes))]),
        );
        summary.append(&free);
        match self.body.first_child() {
            Some(header) => self.body.insert_child_after(&summary, Some(&header)),
            None => self.body.append(&summary),
        }
        if data.total == 0 {
            total.set_text(&tr("Disk usage unavailable"));
        }

        // The grid outlives this render so its animations keep running.
        let is_waiting = |i: usize| waiting & (1 << i) != 0;
        let (order, wave) = {
            let mut arrival = self.fx.arrival.borrow_mut();
            if waiting != 0 {
                for i in (0..8).filter(|&i| !is_waiting(i)) {
                    if !arrival.contains(&i) {
                        arrival.push(i);
                    }
                }
                (arrival.clone(), false)
            } else {
                // Analysis is over: the final sort sweeps across the map.
                let wave = !arrival.is_empty();
                arrival.clear();
                (Vec::new(), wave)
            }
        };
        let segments = build_segments(data, waiting, &order);
        let trash = if sweep {
            self.fx.trash.borrow().upgrade()
        } else {
            None
        };
        // Category pages carry their own copy of the map; only the one on
        // screen sends cells to the Trash.
        for (category, strip) in self.fx.strips.borrow().iter() {
            let target = trash.as_ref().filter(|_| strip.widget().is_mapped());
            let (selected, total) = super::selection_bus::state(*category);
            strip.update(
                strip_segments(data, *category, selected, total),
                FREE,
                target.map(|target| Sweep {
                    flights: &self.fx.flights,
                    target,
                }),
                false,
            );
        }
        let target = trash.as_ref().filter(|_| self.fx.grid.widget().is_mapped());
        self.fx.grid.update(
            segments,
            FREE,
            target.map(|target| Sweep {
                flights: &self.fx.flights,
                target,
            }),
            wave,
        );
        if self.fx.grid.widget().parent().is_none() {
            self.body.append(self.fx.grid.widget());
        }

        // Rows: heaviest first; categories still being measured wait at the end.
        let pending = is_waiting;
        let mut order: Vec<usize> = (0..9)
            // AI models and games appear only when the scan found some.
            .filter(|&i| !((6..8).contains(&i) && data.sizes[i] == 0 && !pending(i)))
            .collect();
        order.sort_by_key(|&i| (pending(i), std::cmp::Reverse(data.sizes[i])));
        let largest = order
            .iter()
            .filter(|&&i| !pending(i))
            .map(|&i| data.sizes[i])
            .max()
            .unwrap_or(0);
        // Every column shares its width between the header and all rows.
        let names = gtk4::SizeGroup::new(gtk4::SizeGroupMode::Horizontal);
        let details = gtk4::SizeGroup::new(gtk4::SizeGroupMode::Horizontal);
        let tags = gtk4::SizeGroup::new(gtk4::SizeGroupMode::Horizontal);
        let sizes = gtk4::SizeGroup::new(gtk4::SizeGroupMode::Horizontal);

        let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        head.add_css_class("pc-overview-head");
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_size_request(26, -1);
        head.append(&spacer);
        for (text, group, align, grow) in [
            ("Name", Some(&names), 0.0, false),
            ("Details", Some(&details), 0.0, true),
            ("Safety", Some(&tags), 0.0, false),
            ("Size", Some(&sizes), 1.0, false),
        ] {
            let label = gtk4::Label::new(Some(&tr(text)));
            label.add_css_class("pc-tertiary");
            label.set_xalign(align);
            label.set_hexpand(grow);
            if let Some(group) = group {
                group.add_widget(&label);
            }
            head.append(&label);
        }
        self.body.append(&head);

        let mut buttons: Vec<Option<glib::WeakRef<gtk4::Button>>> = vec![None; 10];
        let card = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        card.add_css_class("pc-card");
        for &i in &order {
            let waiting_row = pending(i);
            let button = gtk4::Button::new();
            button.add_css_class("pc-item-row");
            button.add_css_class("pc-overview-row");
            button.set_action_name(Some("win.open-page"));
            button.set_action_target_value(Some(&PAGES[i].to_variant()));
            let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
            line.append(&small_tile(COLORS[i], ICONS[i]));
            let title = gtk4::Label::new(Some(&tr(TITLES[i])));
            title.add_css_class("pc-row-title");
            title.set_xalign(0.0);
            // Long names give way first, so the details keep their room.
            title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            title.set_max_width_chars(32);
            names.add_widget(&title);
            line.append(&title);

            let detail = gtk4::Label::new(Some(&if waiting_row {
                tr("Scanning…")
            } else {
                row_detail(self, data, i)
            }));
            detail.add_css_class("pc-secondary");
            detail.add_css_class("pc-row-detail");
            detail.set_xalign(0.0);
            // Takes the room the text needs; it only shortens in a narrow window.
            detail.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            detail.set_hexpand(true);
            details.add_widget(&detail);
            line.append(&detail);

            let (tag_text, tag_class) = match TAGS[i] {
                Tag::Safe => ("Safe", "pc-safe"),
                Tag::Review => ("Review", "pc-check"),
                Tag::Careful => ("Careful", "pc-careful"),
            };
            let tag = gtk4::Label::new(Some(&tr(tag_text)));
            tag.add_css_class("pc-badge");
            tag.add_css_class(tag_class);
            tag.set_valign(gtk4::Align::Center);
            tag.set_halign(gtk4::Align::Start);
            tags.add_widget(&tag);
            line.append(&tag);

            // "28.5% - 208.8 GB" on top, the fourteen-square bar under it.
            let column = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
            column.set_valign(gtk4::Align::Center);
            let filled = if waiting_row || largest == 0 || data.sizes[i] == 0 {
                0
            } else {
                ((data.sizes[i] as f64 / largest as f64 * MINI_CELLS as f64).ceil() as usize)
                    .clamp(1, MINI_CELLS)
            };
            if waiting_row {
                let skeleton = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
                skeleton.add_css_class("pc-skeleton");
                skeleton.set_size_request(64, 16);
                skeleton.set_halign(gtk4::Align::End);
                column.append(&skeleton);
            } else {
                let value = gtk4::Label::new(None);
                value.add_css_class("pc-row-size");
                value.set_xalign(1.0);
                value.set_halign(gtk4::Align::End);
                let share = gtk4::Label::new(Some(&format!(
                    "{} - ",
                    tr_args(
                        "{percent}%",
                        &[(
                            "{percent}",
                            &format!("{:.1}", fraction(data.sizes[i], data.total) * 100.0),
                        )],
                    )
                )));
                share.add_css_class("pc-tertiary");
                share.add_css_class("pc-row-share");
                let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
                top.set_halign(gtk4::Align::End);
                top.append(&share);
                let from = if was_waiting & (1 << i) != 0 {
                    0
                } else {
                    tween_from(prev.map(|p| p.sizes[i]), data.sizes[i])
                };
                tween(&value, from, data.sizes[i], (delay, span), size);
                top.append(&value);
                column.append(&top);
            }
            let bar = mini_bar(TOKENS[i], filled);
            bar.set_halign(gtk4::Align::Fill);
            column.append(&bar);
            sizes.add_widget(&column);
            line.append(&column);

            button.set_child(Some(&line));
            button.set_tooltip_text(Some(&row_tooltip(self, data, i)));
            buttons[i] = Some(button.downgrade());
            card.append(&button);
        }
        self.body.append(&card);

        // One hover handler for rows and cells: dim everything else.
        let grid = self.fx.grid.clone();
        let hover: Rc<dyn Fn(Option<usize>)> = Rc::new(move |id| {
            for (i, button) in buttons.iter().enumerate() {
                let Some(button) = button.as_ref().and_then(|b| b.upgrade()) else {
                    continue;
                };
                if id.is_some_and(|selected| selected != i) {
                    button.add_css_class("pc-dimmed");
                } else {
                    button.remove_css_class("pc-dimmed");
                }
                if id == Some(i) {
                    button.add_css_class("pc-highlighted");
                } else {
                    button.remove_css_class("pc-highlighted");
                }
            }
            grid.set_highlighted(id);
        });
        self.fx.grid.set_hover(hover.clone());
        let mut child = card.first_child();
        for &i in &order {
            let Some(button) = child else { break };
            child = button.next_sibling();
            let motion = gtk4::EventControllerMotion::new();
            let callback = hover.clone();
            motion.connect_enter(move |_, _, _| callback(Some(i)));
            let callback = hover.clone();
            motion.connect_leave(move |_| callback(None));
            button.add_controller(motion);
        }

        let foot = gtk4::Label::new(Some(&tr_args("{size} of the used space is already in the Trash. Empty it in Files to free the space.", &[("{size}", &size(data.trash))])));
        foot.set_xalign(0.0);
        foot.set_wrap(true);
        foot.add_css_class("pc-tertiary");
        self.body.append(&foot);
    }
}

const MINI_CELLS: usize = 14;

fn small_tile(color: &str, icon: &str) -> gtk4::Box {
    let tile = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    tile.add_css_class("pc-icon-tile");
    tile.add_css_class("pc-small");
    tile.add_css_class(&format!("pc-{color}"));
    tile.set_size_request(26, 26);
    tile.set_hexpand(false);
    tile.set_halign(gtk4::Align::Start);
    tile.set_valign(gtk4::Align::Center);
    let image = gtk4::Image::from_icon_name(icon);
    image.set_pixel_size(14);
    image.set_hexpand(true);
    image.set_halign(gtk4::Align::Center);
    image.set_valign(gtk4::Align::Center);
    tile.append(&image);
    tile
}

/// Fourteen flat squares: how much of the largest category this one holds.
fn mini_bar(token: &'static str, filled: usize) -> gtk4::DrawingArea {
    const CELL: f64 = 7.0;
    const CELL_H: f64 = 5.0;
    const GAP: f64 = 1.0;
    let area = gtk4::DrawingArea::new();
    area.set_content_width((MINI_CELLS as f64 * (CELL + GAP) - GAP) as i32);
    area.set_content_height(CELL_H as i32);
    area.set_valign(gtk4::Align::Center);
    area.set_hexpand(false);
    area.set_draw_func(move |area, cr, width, _| {
        let cell = ((width as f64 + GAP) / MINI_CELLS as f64 - GAP).max(CELL);
        let color = super::overview_grid::lookup(area, token);
        let empty = super::overview_grid::lookup(area, "pc_chart_free");
        for i in 0..MINI_CELLS {
            let (c, alpha) = if i < filled { (&color, 1.0) } else { (&empty, 0.45) };
            cr.set_source_rgba(
                c.red() as f64,
                c.green() as f64,
                c.blue() as f64,
                alpha,
            );
            cr.rectangle(i as f64 * (cell + GAP), 0.0, cell, CELL_H);
            let _ = cr.fill();
        }
    });
    let weak = area.downgrade();
    let signal = libadwaita::StyleManager::default().connect_dark_notify(move |_| {
        if let Some(area) = weak.upgrade() {
            area.queue_draw();
        }
    });
    let signal = RefCell::new(Some(signal));
    area.connect_destroy(move |_| {
        if let Some(signal) = signal.borrow_mut().take() {
            libadwaita::StyleManager::default().disconnect(signal);
        }
    });
    area
}

/// Count a label from `from` to `to` after `delay` seconds, over `span` seconds.
fn tween(
    label: &gtk4::Label,
    from: u64,
    to: u64,
    (delay, span): (f64, f64),
    format: impl Fn(u64) -> String + 'static,
) {
    label.set_text(&format(from));
    if from == to {
        return;
    }
    let weak = label.downgrade();
    let start = std::time::Instant::now();
    glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
        let Some(label) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let t = ((start.elapsed().as_secs_f64() - delay) / span).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let value = from as f64 + (to as f64 - from as f64) * eased;
        label.set_text(&format(if t >= 1.0 { to } else { value as u64 }));
        if t >= 1.0 {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

/// The row's detail column: counts, or how much of it is safe to clean.
fn row_detail(page: &OverviewPage, data: &Readings, i: usize) -> String {
    if i < 3 {
        return size(data.safe[i]);
    }
    row_tooltip(page, data, i)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Full explanation of the row, shown on hover.
fn row_tooltip(page: &OverviewPage, data: &Readings, i: usize) -> String {
    let detail = match i {
        0..=2 => tr_args("{size} safe to clean", &[("{size}", &size(data.safe[i]))]),
        3 => tr_args(
            "{count} files over {size}",
            &[
                ("{count}", &crate::i18n::format_count(data.large_count)),
                ("{size}", &size(page.review.minimum())),
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
        _ => tr("Linux, your documents and photos, and other files"),
    };
    match i {
        0 | 1 => format!(
            "{detail}\n{}",
            tr("Regenerable caches measured independently of cleanup selections.")
        ),
        2 => format!("{detail}\n{}", tr("Package caches, journal files and unused Flatpak runtimes. Safe amount excludes journal files; review them in System.")),
        _ => detail,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

impl From<crate::services::scan_cache::ScanSummary> for Readings {
    fn from(summary: crate::services::scan_cache::ScanSummary) -> Self {
        let mut sizes = [0; 10];
        sizes[..8].copy_from_slice(&summary.bytes);
        let [large_count, app_count, leftover_count, model_count, game_count] = summary.counts;
        let mut data = Self {
            sizes,
            total: summary.total,
            free: summary.free,
            used: summary.total.saturating_sub(summary.free),
            safe: summary.safe,
            trash: summary.trash_bytes,
            large_count,
            app_count,
            leftover_count,
            model_count,
            game_count,
        };
        normalize(&mut data);
        data
    }
}
impl Readings {
    fn apply_disk_delta(&mut self, freed: u64, trashed: u64) {
        self.free = self.free.saturating_add(freed).min(self.total);
        self.used = self.total.saturating_sub(self.free);
        self.trash = self.trash.saturating_add(trashed);
    }
    fn summary(&self) -> crate::services::scan_cache::ScanSummary {
        crate::services::scan_cache::ScanSummary {
            bytes: self.sizes[..8].try_into().unwrap(),
            counts: [
                self.large_count,
                self.app_count,
                self.leftover_count,
                self.model_count,
                self.game_count,
            ],
            trash_bytes: self.trash,
            saved_at: chrono::Utc::now().timestamp(),
            total: self.total,
            free: self.free,
            safe: self.safe,
            ..Default::default()
        }
    }
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

    fn disk() -> Readings {
        Readings {
            total: 1_000,
            used: 800,
            free: 200,
            sizes: [10, 20, 30, 40, 50, 60, 70, 80, 440, 200],
            ..Default::default()
        }
    }

    #[test]
    fn analysing_keeps_categories_where_they_landed() {
        let ids = |waiting, arrival: &[usize]| -> Vec<usize> {
            build_segments(&disk(), waiting, arrival)
                .iter()
                .map(|s| s.id)
                .collect()
        };
        // Category 5 landed first, then 1: the heavier 7 and 6 wait after them.
        let early = ids(0b1100_0100, &[5, 1]);
        assert_eq!(&early[..2], &[5, 1]);
        // Category 0 arriving later goes after, nothing moves.
        let later = ids(0b1100_0000, &[5, 1, 0, 2, 3, 4]);
        assert_eq!(&later[..6], &[5, 1, 0, 2, 3, 4]);
        // Done: sorted by weight, the largest first.
        assert_eq!(ids(0, &[])[0], 8);
    }

    #[test]
    fn category_map_is_all_one_colour_with_the_selection_in_full() {
        let ids = |segments: &[GridSegment]| segments.iter().map(|s| s.id).collect::<Vec<_>>();
        let none = strip_segments(&disk(), 0, 0, 1_000);
        assert_eq!(ids(&none), [STRIP_USED]);
        assert!((none[0].fraction - 1.0).abs() < 1e-9 && none[0].alpha < 1.0);
        let some = strip_segments(&disk(), 0, 250, 1_000);
        assert_eq!(ids(&some), [0, STRIP_USED]);
        assert!((some[0].fraction - 0.25).abs() < 1e-9 && some[0].alpha == 1.0);
        // Everything selected leaves nothing faded.
        let all = strip_segments(&disk(), 0, u64::MAX, 1_000);
        assert!((all[0].fraction - 1.0).abs() < 1e-9);
        assert!(all[1].fraction.abs() < 1e-9);
    }

    #[test]
    fn local_disk_deltas_preserve_categories_and_round_trip_the_summary() {
        let mut data = Readings {
            total: 1_000,
            used: 800,
            free: 200,
            trash: 100,
            sizes: [10, 20, 30, 40, 50, 60, 70, 80, 0, 200],
            large_count: 4,
            ..Default::default()
        };
        data.apply_disk_delta(50, 20);
        assert_eq!((data.free, data.used, data.trash), (250, 750, 120));
        assert_eq!(data.sizes[3], 40);
        normalize(&mut data);
        assert_eq!(Readings::from(data.summary()), data);
        data.trash = 0;
        data.apply_disk_delta(120, 0);
        assert_eq!((data.free, data.used, data.trash), (370, 630, 0));
    }

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
