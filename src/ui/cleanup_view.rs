//! The cleanup screen: how much of the disk is used, a thin bar coloured by
//! group, a legend per impact with a goal slider that picks the least
//! costly things first, a soft mosaic of what was found and an optional
//! tree of exactly what goes.
//! The header trash can owns the cleanup countdown.

use super::cleanup_fx::{draw_bolt, Burst, Particles, Spinner};
use super::mosaic::Mosaic;
use crate::i18n::{tr, tr_args};
use crate::models::{CleanupPlan, GroupKind, Impact, PlanGroup, ScanResult, Tick};
use crate::services::CleanProgress;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const COUNTDOWN_SECONDS: u32 = 5;
/// Height of the clean pill; the bolt sits in a square of this size.
const CLEAN_HEIGHT: i32 = 56;
/// Hover delay before a tile explains itself.
const HOVER_DELAY: Duration = Duration::from_millis(250);
const OTHER_COLOR: (f64, f64, f64) = (
    0x52 as f64 / 255.0,
    0x52 as f64 / 255.0,
    0x5b as f64 / 255.0,
);

/// Stable group colours; impact remains a separate badge.
fn group_style(kind: GroupKind) -> (&'static str, (f64, f64, f64)) {
    let (class, hex) = match kind {
        GroupKind::System => ("group-blue", 0x2563eb_u32),
        GroupKind::AppCaches => ("group-green", 0x047857_u32),
        GroupKind::DevCaches => ("group-yellow", 0xa16207_u32),
        GroupKind::Logs => ("group-orange", 0xc2410c_u32),
        GroupKind::Browsers => ("group-purple", 0x7c3aed_u32),
        GroupKind::Custom => ("group-brown", 0x0e7490_u32),
        GroupKind::RustBuilds => ("group-red", 0xe11d48_u32),
        GroupKind::PythonNode => ("group-teal", 0x4d7c0f_u32),
        GroupKind::OtherBuilds => ("group-indigo", 0x4f46e5_u32),
        GroupKind::Games => ("group-pink", 0xdb2777_u32),
        GroupKind::AiModels => ("group-olive", 0xc026d3_u32),
    };
    (
        class,
        (
            ((hex >> 16) & 255) as f64 / 255.0,
            ((hex >> 8) & 255) as f64 / 255.0,
            (hex & 255) as f64 / 255.0,
        ),
    )
}

type CleanCallback = Box<dyn Fn(ScanResult, ScanResult)>;
/// Asked before the countdown starts; calls the closure to go ahead.
type ConfirmCallback = Box<dyn Fn(ScanResult, ScanResult, Box<dyn FnOnce()>)>;

/// Sizes the way people read them: "64 GB", "2.7 GB", "640 MB".
pub fn short_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 10.0 || unit == 0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// Width of the widest text `short_size` can produce, in `label`'s font:
/// the counter keeps this width so changing digits never resize the pill.
fn widest_size(label: &gtk4::Label) -> i32 {
    ["B", "KB", "MB", "GB", "TB"]
        .iter()
        .flat_map(|unit| {
            (0..=9).flat_map(move |d| [format!("{d}{d}{d} {unit}"), format!("{d}.{d} {unit}")])
        })
        .map(|text| label.create_pango_layout(Some(&text)).pixel_size().0)
        .max()
        .unwrap_or(0)
}

struct Widgets {
    stack: gtk4::Stack,
    status_label: gtk4::Label,
    spinner: Spinner,
    particles: Particles,
    used_label: gtk4::Label,
    disk_label: gtk4::Label,
    /// "−36 GB": what the selection takes off the disk.
    delta_label: gtk4::Label,
    free_label: gtk4::Label,
    bar: gtk4::DrawingArea,
    /// One usage line and bar per other disk the plan has items on.
    other_disks: gtk4::Box,
    /// Selected size per impact, in `Impact::ALL` order.
    impact_labels: Vec<gtk4::Label>,
    summary_label: gtk4::Label,
    goal: gtk4::Scale,
    goal_label: gtk4::Label,
    mosaic: Mosaic,
    details_switch: gtk4::Switch,
    details: gtk4::Revealer,
    tree_total: gtk4::Label,
    tree: gtk4::Box,
    clean_button: gtk4::Button,
    clean_label: gtk4::Label,
    /// While cleaning: what is gone so far, in a fixed-width slot so the
    /// pill does not twitch as the digits change.
    count_label: gtk4::Label,
    scan_button: gtk4::Button,
    ring: gtk4::DrawingArea,
}

struct Tile {
    button: gtk4::Button,
    selected: gtk4::Label,
    offered: gtk4::Label,
    /// "📌 47 GB": what the group holds but does not offer.
    locked: gtk4::Box,
    locked_label: gtk4::Label,
    popover: gtk4::Popover,
    popover_freed: gtk4::Label,
}

/// Usage line and bar of a disk other than the home one, such as `/home2`.
struct OtherDisk {
    mount: String,
    total: u64,
    used: u64,
    used_label: gtk4::Label,
    delta_label: gtk4::Label,
    free_label: gtk4::Label,
    bar: gtk4::DrawingArea,
}

/// Total and used bytes of the filesystem mounted at `mount`.
fn disk_space(mount: &str) -> Option<(u64, u64)> {
    let path = std::ffi::CString::new(mount).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let block = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * block;
    let available = (stat.f_bavail as u64 * block).min(total);
    Some((total, total - available))
}

struct State {
    plan: RefCell<CleanupPlan>,
    disk_total: Cell<u64>,
    disk_used: Cell<u64>,
    /// Set while code, not the user, changes a control.
    updating: Cell<bool>,
    /// Tree nodes the user opened ("t0", "gSystem"…).
    open: RefCell<HashSet<String>>,
    /// Groups listed in the tree. A group stays once listed, so unticking
    /// all of it to pick items one by one does not make it vanish.
    shown: RefCell<HashSet<GroupKind>>,
    tiles: RefCell<Vec<Tile>>,
    other_disks: RefCell<Vec<Rc<OtherDisk>>>,
    countdown: Cell<Option<u32>>,
    /// When the countdown began, so the ring can sweep smoothly.
    countdown_started: Cell<Option<Instant>>,
    ring_tick: RefCell<Option<gtk4::TickCallbackId>>,
    /// Bolt charge, 0–1, while the cleanup launches.
    lid: Cell<f64>,
    /// When the launch began, so the bolt's rays can spin.
    launch_started: Cell<Option<Instant>>,
    launch_tick: RefCell<Option<gtk4::TickCallbackId>>,
    lid_tick: RefCell<Option<gtk4::TickCallbackId>>,
    /// Tree node just opened ("*" = the whole tree): its rows slide in.
    fresh: RefCell<Option<String>>,
    /// A cleanup is running: particles stream into the button.
    launching: Cell<bool>,
    cleaning: RefCell<Option<Cleaning>>,
    /// Bytes each group already took off the bar, eased towards what is
    /// selected so blocks shrink instead of jumping.
    bar_gone: RefCell<Vec<f64>>,
    bar_tick: RefCell<Option<gtk4::TickCallbackId>>,
    countdown_source: RefCell<Option<glib::SourceId>>,
    hover_source: RefCell<Option<glib::SourceId>>,
    enabled: Cell<bool>,
    on_clean: CleanCallback,
    confirm: RefCell<Option<ConfirmCallback>>,
    w: Widgets,
}

/// The cleanup the button is counting.
struct Cleaning {
    total: u64,
    progress: Arc<CleanProgress>,
    cancel: Arc<AtomicBool>,
    timer: glib::SourceId,
}

#[derive(Clone)]
pub struct CleanupView {
    root: gtk4::Box,
    state: Rc<State>,
}

impl CleanupView {
    pub fn new(on_clean: impl Fn(ScanResult, ScanResult) + 'static) -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.add_css_class("cleanup-view");
        root.set_vexpand(true);

        let stack = gtk4::Stack::new();
        stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        stack.set_transition_duration(crate::theme::transition_duration(&root, 150));

        // Status page: scanning, nothing found, cancelled.
        // Centred in the space under the disk bar, large and frameless.
        let status = gtk4::Box::new(gtk4::Orientation::Vertical, 20);
        status.add_css_class("cleanup-status");
        status.set_valign(gtk4::Align::Center);
        status.set_vexpand(true);
        let spinner = Spinner::new(88);
        let status_label = gtk4::Label::new(None);
        status_label.add_css_class("cleanup-status-label");
        status_label.set_justify(gtk4::Justification::Center);
        status_label.set_wrap(true);
        status.append(spinner.widget());
        status.append(&status_label);
        stack.add_named(&status, Some("status"));

        let card = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        card.add_css_class("cleanup-card");

        let scan_button = gtk4::Button::new();
        scan_button.set_child(Some(&crate::ui::icon_image("ui/reload")));
        scan_button.set_valign(gtk4::Align::Center);

        // "402 GB used of 512 GB ……… 110 GB free", then the disk bar.
        let usage = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let used_label = gtk4::Label::new(Some("—"));
        used_label.add_css_class("usage-value");
        used_label.set_valign(gtk4::Align::Baseline);
        let disk_label = gtk4::Label::new(None);
        disk_label.add_css_class("dim-label");
        disk_label.set_valign(gtk4::Align::Baseline);
        let delta_label = gtk4::Label::new(None);
        delta_label.add_css_class("usage-delta");
        delta_label.set_valign(gtk4::Align::Baseline);
        let free_label = gtk4::Label::new(None);
        free_label.add_css_class("dim-label");
        free_label.set_hexpand(true);
        free_label.set_xalign(1.0);
        free_label.set_valign(gtk4::Align::Baseline);
        usage.append(&used_label);
        usage.append(&disk_label);
        usage.append(&delta_label);
        usage.append(&free_label);

        let bar = gtk4::DrawingArea::new();
        bar.set_content_height(12);

        // Impact legend on the left, goal slider on the right.
        let legend_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
        let mut impact_labels = Vec::new();
        for impact in Impact::ALL {
            let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            let badge = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            badge.add_css_class("impact-chip");
            badge.add_css_class(impact.css_class());
            badge.append(&gtk4::Image::from_icon_name(impact.icon_name()));
            badge.append(&gtk4::Label::new(Some(&tr(impact.title()))));
            let size = gtk4::Label::new(Some("—"));
            size.add_css_class("impact-chip-size");
            chip.append(&badge);
            chip.append(&size);
            legend_row.append(&chip);
            impact_labels.push(size);
        }
        let goal_caption = gtk4::Label::new(Some(&tr("I want to free")));
        goal_caption.add_css_class("dim-label");
        goal_caption.set_hexpand(true);
        goal_caption.set_xalign(1.0);
        let goal = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.1);
        goal.set_width_request(200);
        goal.set_draw_value(false);
        goal.add_css_class("goal-scale");
        let goal_label = gtk4::Label::new(Some("—"));
        goal_label.add_css_class("goal-value");
        goal_label.set_width_chars(7);
        goal_label.set_xalign(1.0);
        legend_row.append(&goal_caption);
        legend_row.append(&goal);
        legend_row.append(&goal_label);
        card.append(&legend_row);

        let mosaic = Mosaic::new();
        mosaic.set_margin_top(14);
        card.append(&mosaic);

        let switch_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        switch_row.set_margin_top(12);
        let summary_label = gtk4::Label::new(None);
        summary_label.add_css_class("dim-label");
        summary_label.set_hexpand(true);
        summary_label.set_xalign(0.0);
        summary_label.set_wrap(true);
        switch_row.append(&summary_label);
        let details_switch = gtk4::Switch::new();
        details_switch.set_valign(gtk4::Align::Center);
        let switch_label = gtk4::Label::new(Some(&tr("Show Details")));
        switch_label.add_css_class("dim-label");
        switch_row.append(&switch_label);
        switch_row.append(&details_switch);
        {
            let details_switch = details_switch.clone();
            let click = gtk4::GestureClick::new();
            click.connect_released(move |_, _, _, _| {
                details_switch.set_active(!details_switch.is_active());
            });
            switch_label.add_controller(click);
            switch_label.set_cursor_from_name(Some("pointer"));
        }
        card.append(&switch_row);

        let details_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let tree_head = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        tree_head.set_margin_top(14);
        let tree_title = gtk4::Label::new(Some(&tr("What Will Be Cleaned")));
        tree_title.add_css_class("heading");
        tree_title.set_hexpand(true);
        tree_title.set_xalign(0.0);
        let tree_total = gtk4::Label::new(None);
        tree_total.add_css_class("dim-label");
        tree_head.append(&tree_title);
        tree_head.append(&tree_total);
        details_box.append(&tree_head);
        let tree = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        tree.set_margin_top(10);
        details_box.append(&tree);
        let details = gtk4::Revealer::new();
        details.set_transition_type(gtk4::RevealerTransitionType::SlideDown);
        details.set_transition_duration(crate::theme::transition_duration(&root, 260));
        details.set_child(Some(&details_box));
        card.append(&details);

        let clean_button = gtk4::Button::new();
        clean_button.add_css_class("clean-go");
        clean_button.set_valign(gtk4::Align::Center);
        // A blue pill: the drawing paints the pill and the bolt on its
        // left, the label sits on top and decides how wide it is.
        let ring = gtk4::DrawingArea::new();
        ring.set_content_height(CLEAN_HEIGHT);
        let clean_label = gtk4::Label::new(Some(&tr("Clean")));
        clean_label.add_css_class("clean-go-label");
        let count_label = gtk4::Label::new(None);
        count_label.add_css_class("clean-go-label");
        count_label.set_xalign(1.0);
        count_label.set_visible(false);
        let labels = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        labels.append(&count_label);
        labels.append(&clean_label);
        labels.set_margin_start(CLEAN_HEIGHT - 4);
        labels.set_margin_end(24);
        labels.set_valign(gtk4::Align::Center);
        let pill = gtk4::Overlay::new();
        pill.set_child(Some(&ring));
        pill.add_overlay(&labels);
        pill.set_measure_overlay(&labels, true);
        clean_button.set_child(Some(&pill));

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        let title = gtk4::Label::new(Some(&tr("Overview")));
        title.add_css_class("title-2");
        title.add_css_class("welcome-title");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        header.append(&title);

        card.set_valign(gtk4::Align::Start);
        stack.add_named(&card, Some("plan"));
        stack.set_vexpand(true);
        let page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        page.add_css_class("cleanup-page");
        page.set_vexpand(true);
        header.set_margin_bottom(14);
        bar.set_margin_top(10);
        stack.set_margin_top(18);
        page.append(&header);
        let other_disks = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        page.append(&usage);
        page.append(&bar);
        page.append(&other_disks);
        page.append(&stack);
        let particles = Particles::new();
        let overlay = gtk4::Overlay::new();
        overlay.set_vexpand(true);
        overlay.set_child(Some(&page));
        overlay.add_overlay(particles.widget());
        root.append(&overlay);

        let state = Rc::new(State {
            plan: RefCell::default(),
            disk_total: Cell::new(0),
            disk_used: Cell::new(0),
            updating: Cell::new(false),
            open: RefCell::new(["t0", "t1", "t2"].map(String::from).into_iter().collect()),
            shown: RefCell::new(HashSet::new()),
            tiles: RefCell::default(),
            other_disks: RefCell::default(),
            countdown: Cell::new(None),
            countdown_started: Cell::new(None),
            ring_tick: RefCell::default(),
            lid: Cell::new(0.0),
            launch_started: Cell::new(None),
            launch_tick: RefCell::default(),
            lid_tick: RefCell::default(),
            fresh: RefCell::default(),
            launching: Cell::new(false),
            cleaning: RefCell::default(),
            bar_gone: RefCell::default(),
            bar_tick: RefCell::default(),
            countdown_source: RefCell::default(),
            hover_source: RefCell::default(),
            enabled: Cell::new(true),
            on_clean: Box::new(on_clean),
            confirm: RefCell::default(),
            w: Widgets {
                stack,
                status_label,
                spinner,
                particles,
                used_label,
                disk_label,
                delta_label,
                free_label,
                bar,
                other_disks,
                impact_labels,
                summary_label,
                goal,
                goal_label,
                mosaic,
                details_switch,
                details,
                tree_total,
                tree,
                clean_button,
                clean_label,
                count_label,
                scan_button,
                ring,
            },
        });
        let view = Self { root, state };
        view.connect_signals();
        view.show_status(&tr("Scan to see what can be cleaned"), false);
        view
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.root
    }

    pub fn scan_button(&self) -> &gtk4::Button {
        &self.state.w.scan_button
    }

    /// The trash button; the page floats it above its scrolled content.
    pub fn clean_button(&self) -> &gtk4::Button {
        &self.state.w.clean_button
    }

    /// Check the selection (safety limits…) before the countdown starts.
    pub fn set_confirm(
        &self,
        confirm: impl Fn(ScanResult, ScanResult, Box<dyn FnOnce()>) + 'static,
    ) {
        self.state.confirm.replace(Some(Box::new(confirm)));
    }

    fn connect_signals(&self) {
        let w = &self.state.w;

        let weak = Rc::downgrade(&self.state);
        w.bar.set_draw_func(move |_, cr, width, height| {
            if let Some(state) = weak.upgrade() {
                draw_bar(&state, cr, f64::from(width), f64::from(height));
            }
        });

        let view = self.downgrade();
        w.goal.connect_value_changed(move |scale| {
            let Some(view) = view.upgrade() else { return };
            if view.state.updating.get() {
                return;
            }
            let goal = (scale.value() * 1e9) as u64;
            view.state.plan.borrow_mut().apply_goal(goal);
            view.cancel_countdown();
            view.refresh();
        });

        let view = self.downgrade();
        w.details_switch.connect_active_notify(move |switch| {
            if let Some(view) = view.upgrade() {
                if switch.is_active() {
                    // Rows slide in one after another as the tree opens.
                    view.state.fresh.replace(Some("*".into()));
                    view.rebuild_tree();
                }
                view.state.w.details.set_reveal_child(switch.is_active());
            }
        });

        let view = self.downgrade();
        w.clean_button.connect_clicked(move |_| {
            if let Some(view) = view.upgrade() {
                if let Some(cleaning) = view.state.cleaning.borrow().as_ref() {
                    cleaning.cancel.store(true, Ordering::Relaxed);
                } else if view.state.countdown.get().is_some() {
                    view.cancel_countdown();
                } else {
                    view.confirm_then_countdown();
                }
                view.refresh_button();
            }
        });

        let weak = Rc::downgrade(&self.state);
        w.ring.set_draw_func(move |_, cr, width, height| {
            if let Some(state) = weak.upgrade() {
                draw_clean_button(&state, cr, f64::from(width), f64::from(height));
            }
        });
    }

    fn downgrade(&self) -> WeakView {
        WeakView {
            root: self.root.downgrade(),
            state: Rc::downgrade(&self.state),
        }
    }

    pub fn show_scanning(&self) {
        self.show_status(&tr("Looking for what can be cleaned…"), true);
    }

    pub fn show_status(&self, text: &str, busy: bool) {
        self.cancel_countdown();
        let w = &self.state.w;
        w.status_label.set_text(text);
        w.spinner.set_spinning(busy);
        w.stack.set_visible_child_name("status");
        self.refresh_figures();
        self.refresh_button();
    }

    pub fn is_launching(&self) -> bool {
        self.state.launching.get()
    }

    pub fn is_scanning(&self) -> bool {
        self.state.w.spinner.is_spinning()
    }

    /// Disk the plan is measured against.
    pub fn set_disk(&self, total: u64, used: u64) {
        self.state.disk_total.set(total);
        self.state.disk_used.set(used);
        self.refresh_figures();
    }

    /// Lock the button while another operation runs.
    pub fn set_enabled(&self, enabled: bool) {
        self.state.enabled.set(enabled);
        if !enabled {
            self.cancel_countdown();
        }
        self.refresh_button();
    }

    pub fn set_plan(&self, mut plan: CleanupPlan) {
        self.cancel_countdown();
        let w = &self.state.w;
        w.spinner.set_spinning(false);
        if plan.is_empty() {
            self.state.plan.replace(plan);
            self.build_other_disks();
            self.show_status(&tr("Nothing to clean right now"), false);
            return;
        }
        let goal = plan.default_goal();
        plan.apply_goal(goal);
        self.state.plan.replace(plan);
        self.state.bar_gone.borrow_mut().clear();
        self.state.shown.borrow_mut().clear();
        self.show_plan();
    }

    /// Show what the cleanup actually removed, keeping the selection and
    /// which rows are expanded.
    pub fn remove_cleaned(&self, cleaned_paths: &HashSet<PathBuf>) -> u64 {
        if cleaned_paths.is_empty() {
            return self.state.plan.borrow().total_size();
        }
        self.cancel_countdown();
        let before = self.state.plan.borrow().home_total_size();
        self.state.plan.borrow_mut().remove_cleaned(cleaned_paths);
        // Until the disk is measured again, count what went as freed so
        // the bar does not grow back for a moment.
        let removed = before.saturating_sub(self.state.plan.borrow().home_total_size());
        self.state
            .disk_used
            .set(self.state.disk_used.get().saturating_sub(removed));
        self.state.bar_gone.borrow_mut().clear();
        self.state.fresh.take();
        self.show_plan();
        self.state.plan.borrow().total_size()
    }

    fn show_plan(&self) {
        let w = &self.state.w;
        let plan = self.state.plan.borrow();
        let max = plan.offered_size().max(1) as f64 / 1e9;
        self.state.updating.set(true);
        w.goal.set_range(0.0, max);
        w.goal
            .set_increments(if max > 20.0 { 1.0 } else { 0.1 }, 10.0);
        w.goal.set_value(plan.selected_size() as f64 / 1e9);
        self.state.updating.set(false);
        let empty = plan.is_empty();
        drop(plan);
        self.cancel_hover();
        self.build_tiles();
        self.build_other_disks();
        if empty {
            self.show_status(&tr("Nothing to clean right now"), false);
        } else {
            w.spinner.set_spinning(false);
            w.stack.set_visible_child_name("plan");
        }
        self.refresh();
    }

    fn build_tiles(&self) {
        for tile in self.state.tiles.borrow_mut().drain(..) {
            tile.popover.unparent();
        }
        let plan = self.state.plan.borrow();
        let total = plan.total_size().max(1) as f64;
        let mut widgets = Vec::new();
        let mut tiles = Vec::new();
        for (index, group) in plan.groups.iter().enumerate() {
            let tile = self.build_tile(index, group);
            // Small groups still get a readable tile.
            let weight = (group.size() as f64 / total).max(0.06);
            widgets.push((tile.button.clone().upcast::<gtk4::Widget>(), weight));
            tiles.push(tile);
        }
        drop(plan);
        self.state.w.mosaic.set_tiles(widgets);
        self.state.tiles.replace(tiles);
    }

    fn build_tile(&self, index: usize, group: &PlanGroup) -> Tile {
        let impact = group.impact();
        let button = gtk4::Button::new();
        button.add_css_class("mosaic-tile");
        button.add_css_class(group_style(group.kind).0);
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        // Top: impact on the right; the tile's colour shows the tick.
        let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let impact_icon = gtk4::Image::from_icon_name(impact.icon_name());
        impact_icon.add_css_class("tile-impact");
        impact_icon.set_tooltip_text(Some(&tr(impact.title())));
        let disks = group.other_disks();
        if !disks.is_empty() {
            top.append(&disk_chip(
                &disks.join(", "),
                &if group.home_size() == 0 {
                    tr("On another disk. It frees space there, not on the home disk.")
                } else {
                    tr("Part of it is on another disk. That part frees space there, not on the home disk.")
                },
            ));
        }
        top.append(&spacer);
        top.append(&impact_icon);
        // Bottom: name, then "3.9 GB of 4.2 GB".
        let title = gtk4::Label::new(Some(&tr(group.kind.title())));
        title.add_css_class("tile-title");
        title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title.set_xalign(0.0);
        title.set_vexpand(true);
        title.set_valign(gtk4::Align::End);
        let bottom = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let selected = gtk4::Label::new(None);
        selected.add_css_class("tile-size");
        let offered = gtk4::Label::new(None);
        offered.add_css_class("tile-of");
        offered.set_valign(gtk4::Align::Baseline);
        selected.set_valign(gtk4::Align::Baseline);
        bottom.append(&selected);
        bottom.append(&offered);
        let locked = gtk4::Box::new(gtk4::Orientation::Horizontal, 3);
        locked.add_css_class("tile-locked");
        locked.set_valign(gtk4::Align::Baseline);
        locked.set_margin_start(6);
        let pin = gtk4::Image::from_icon_name("pc-view-pin-symbolic");
        pin.set_pixel_size(12);
        let locked_label = gtk4::Label::new(None);
        locked.append(&pin);
        locked.append(&locked_label);
        locked.set_visible(false);
        bottom.append(&locked);
        content.append(&top);
        content.append(&title);
        content.append(&bottom);
        button.set_child(Some(&content));

        let (popover, popover_freed) = explanation_popover(group);
        popover.set_parent(&button);

        let view = self.downgrade();
        button.connect_clicked(move |_| {
            let Some(view) = view.upgrade() else { return };
            {
                let mut plan = view.state.plan.borrow_mut();
                let group = &mut plan.groups[index];
                let on = group.state() != Tick::All;
                group.set_all(on);
            }
            view.after_manual_change();
        });

        let motion = gtk4::EventControllerMotion::new();
        let view = self.downgrade();
        let shown = popover.clone();
        motion.connect_enter(move |_, _, _| {
            let Some(view) = view.upgrade() else { return };
            view.cancel_hover();
            // A missed leave event must not leave another card open.
            for tile in view.state.tiles.borrow().iter() {
                if tile.popover != shown {
                    tile.popover.popdown();
                }
            }
            let popover = shown.clone();
            let fired = view.downgrade();
            let source = glib::timeout_add_local_once(HOVER_DELAY, move || {
                // The source is gone once it runs; forget its id so a later
                // leave does not try to remove it again.
                if let Some(view) = fired.upgrade() {
                    view.state.hover_source.take();
                }
                popover.popup();
            });
            view.state.hover_source.replace(Some(source));
        });
        let view = self.downgrade();
        let hidden = popover.clone();
        motion.connect_leave(move |_| {
            if let Some(view) = view.upgrade() {
                view.cancel_hover();
            }
            hidden.popdown();
        });
        button.add_controller(motion);

        Tile {
            button,
            selected,
            offered,
            locked,
            locked_label,
            popover,
            popover_freed,
        }
    }

    fn cancel_hover(&self) {
        if let Some(source) = self.state.hover_source.take() {
            source.remove();
        }
    }

    /// After a tick by hand: the slider follows what is selected.
    fn after_manual_change(&self) {
        self.cancel_countdown();
        let selected = self.state.plan.borrow().selected_size();
        self.state.updating.set(true);
        self.state.w.goal.set_value(selected as f64 / 1e9);
        self.state.updating.set(false);
        self.refresh();
    }

    fn refresh(&self) {
        self.refresh_figures();
        self.refresh_tiles();
        self.rebuild_tree();
        self.refresh_button();
    }

    fn refresh_figures(&self) {
        let w = &self.state.w;
        let total = self.state.disk_total.get();
        let used = self.state.disk_used.get();
        let plan = self.state.plan.borrow();
        let freed = plan.selected_size();
        // Only what lives on the home disk frees space there.
        let freed_home = plan.home_selected_size();
        let elsewhere = plan.other_disks_selected();
        for (label, impact) in w.impact_labels.iter().zip(Impact::ALL) {
            label.set_text(&short_size(plan.impact_selected_size(impact)));
        }
        drop(plan);
        self.ease_bar();
        let mut summary = tr_args("Selected {size}", &[("{size}", &short_size(freed))]);
        if total > 0 && freed_home > 0 {
            summary.push_str(" · ");
            summary.push_str(&tr_args(
                "{size} free after",
                &[(
                    "{size}",
                    &short_size(total.saturating_sub(used) + freed_home),
                )],
            ));
        }
        for (disk, size) in elsewhere {
            summary.push_str(" · ");
            summary.push_str(&tr_args(
                "{size} on {disk}",
                &[("{size}", &short_size(size)), ("{disk}", &disk)],
            ));
        }
        w.summary_label.set_text(&summary);
        w.goal_label.set_text(&short_size(freed));
        self.refresh_other_disks();
    }

    /// The bar and the usage line show the disk after the cleanup: what
    /// is selected leaves the bar and the figures follow, easing there.
    fn ease_bar(&self) {
        let targets: Vec<f64> = self
            .state
            .plan
            .borrow()
            .groups
            .iter()
            .map(|g| g.home_selected_size() as f64)
            .collect();
        let animate = crate::theme::transition_duration(&self.root, 1) > 0;
        {
            let mut gone = self.state.bar_gone.borrow_mut();
            if gone.len() != targets.len() || !animate {
                *gone = targets.clone();
            }
        }
        if self.settled(&targets) {
            if let Some(tick) = self.state.bar_tick.take() {
                tick.remove();
            }
            self.show_usage();
            return;
        }
        if self.state.bar_tick.borrow().is_some() {
            return;
        }
        let view = self.downgrade();
        let last = Rc::new(Cell::new(None::<i64>));
        let tick = self.state.w.bar.add_tick_callback(move |_, clock| {
            let Some(view) = view.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let now = clock.frame_time();
            let dt = last
                .replace(Some(now))
                .map_or(1.0 / 60.0, |before| (now - before) as f64 / 1e6);
            let k = (dt * 9.0).min(1.0);
            let targets: Vec<f64> = view
                .state
                .plan
                .borrow()
                .groups
                .iter()
                .map(|g| g.home_selected_size() as f64)
                .collect();
            {
                let mut gone = view.state.bar_gone.borrow_mut();
                if gone.len() != targets.len() {
                    *gone = targets.clone();
                }
                for (shown, target) in gone.iter_mut().zip(&targets) {
                    *shown += (target - *shown) * k;
                    if (target - *shown).abs() < 1e6 {
                        *shown = *target;
                    }
                }
            }
            let done = view.settled(&targets);
            view.show_usage();
            if done {
                view.state.bar_tick.take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        self.state.bar_tick.replace(Some(tick));
    }

    fn settled(&self, targets: &[f64]) -> bool {
        let gone = self.state.bar_gone.borrow();
        gone.len() == targets.len() && gone.iter().zip(targets).all(|(a, b)| a == b)
    }

    fn show_usage(&self) {
        let w = &self.state.w;
        w.bar.queue_draw();
        let total = self.state.disk_total.get();
        if total == 0 {
            return;
        }
        let used = self.state.disk_used.get();
        let gone = (self.state.bar_gone.borrow().iter().sum::<f64>() as u64).min(used);
        let after = used - gone;
        w.used_label
            .set_text(&tr_args("{size} used", &[("{size}", &short_size(after))]));
        w.disk_label
            .set_text(&tr_args("of {size}", &[("{size}", &short_size(total))]));
        w.delta_label.set_text(&format!("−{}", short_size(gone)));
        w.delta_label.set_visible(gone > 0);
        w.free_label.set_text(&tr_args(
            "{size} free",
            &[("{size}", &short_size(total.saturating_sub(after)))],
        ));
    }

    /// A usage line and bar under the main one for each other disk the
    /// plan has items on, measured now.
    fn build_other_disks(&self) {
        let container = &self.state.w.other_disks;
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }
        let mut rows = Vec::new();
        for mount in self.state.plan.borrow().other_disks() {
            let Some((total, used)) = disk_space(&mount) else {
                continue;
            };
            let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            line.add_css_class("other-disk");
            line.set_margin_top(14);
            let icon = gtk4::Image::from_icon_name("pc-drive-harddisk-symbolic");
            icon.set_valign(gtk4::Align::Center);
            let name = gtk4::Label::new(Some(&mount));
            name.add_css_class("other-disk-name");
            name.set_valign(gtk4::Align::Baseline);
            let used_label = gtk4::Label::new(None);
            used_label.add_css_class("other-disk-used");
            used_label.set_valign(gtk4::Align::Baseline);
            let of_label = gtk4::Label::new(Some(&tr_args(
                "of {size}",
                &[("{size}", &short_size(total))],
            )));
            of_label.add_css_class("dim-label");
            of_label.set_valign(gtk4::Align::Baseline);
            let delta_label = gtk4::Label::new(None);
            delta_label.add_css_class("usage-delta");
            delta_label.set_valign(gtk4::Align::Baseline);
            let free_label = gtk4::Label::new(None);
            free_label.add_css_class("dim-label");
            free_label.set_hexpand(true);
            free_label.set_xalign(1.0);
            free_label.set_valign(gtk4::Align::Baseline);
            line.append(&icon);
            line.append(&name);
            line.append(&used_label);
            line.append(&of_label);
            line.append(&delta_label);
            line.append(&free_label);
            let bar = gtk4::DrawingArea::new();
            bar.set_content_height(8);
            bar.set_margin_top(8);
            container.append(&line);
            container.append(&bar);
            let disk = Rc::new(OtherDisk {
                mount,
                total,
                used,
                used_label,
                delta_label,
                free_label,
                bar,
            });
            let weak = Rc::downgrade(&self.state);
            let drawn = Rc::downgrade(&disk);
            disk.bar.set_draw_func(move |_, cr, width, height| {
                if let (Some(state), Some(disk)) = (weak.upgrade(), drawn.upgrade()) {
                    draw_other_bar(&state, &disk, cr, f64::from(width), f64::from(height));
                }
            });
            rows.push(disk);
        }
        self.state.other_disks.replace(rows);
    }

    fn refresh_other_disks(&self) {
        let plan = self.state.plan.borrow();
        for disk in self.state.other_disks.borrow().iter() {
            let gone: u64 = plan
                .groups
                .iter()
                .map(|group| group.selected_on(Some(&disk.mount)))
                .sum::<u64>()
                .min(disk.used);
            let after = disk.used - gone;
            disk.used_label
                .set_text(&tr_args("{size} used", &[("{size}", &short_size(after))]));
            disk.delta_label.set_text(&format!("−{}", short_size(gone)));
            disk.delta_label.set_visible(gone > 0);
            disk.free_label.set_text(&tr_args(
                "{size} free",
                &[("{size}", &short_size(disk.total.saturating_sub(after)))],
            ));
            disk.bar.queue_draw();
        }
    }

    fn refresh_tiles(&self) {
        let plan = self.state.plan.borrow();
        for (tile, group) in self.state.tiles.borrow().iter().zip(&plan.groups) {
            // Picked tiles turn solid, partly picked ones half way.
            let class = match group.state() {
                Tick::None => "off",
                Tick::Some => "partial",
                Tick::All => "on",
            };
            for other in ["off", "partial", "on"] {
                if other == class {
                    tile.button.add_css_class(other);
                } else {
                    tile.button.remove_css_class(other);
                }
            }
            let selected = group.selected_size();
            tile.selected.set_text(&if selected == 0 {
                "0".to_string()
            } else {
                short_size(selected)
            });
            tile.offered.set_text(&tr_args(
                "of {size}",
                &[(
                    "{size}",
                    &short_size(group.offered_size().max(group.selected_size())),
                )],
            ));
            let locked = locked_size(group);
            tile.locked.set_visible(locked > 0);
            tile.locked_label.set_text(&short_size(locked));
            tile.locked
                .set_tooltip_text(Some(&tr(if group.items.iter().any(|i| i.busy) {
                    "A running program is using it"
                } else {
                    "Changed recently, so it is not picked for you"
                })));
            tile.popover_freed.set_text(&tr_args(
                "Selected: {selected} of {total}",
                &[
                    ("{selected}", &short_size(group.selected_size())),
                    ("{total}", &short_size(group.size())),
                ],
            ));
        }
    }

    fn refresh_button(&self) {
        let w = &self.state.w;
        if let Some(cleaning) = self.state.cleaning.borrow().as_ref() {
            // The button counts what is gone; a tap cancels.
            let cancelling = cleaning.cancel.load(Ordering::Relaxed);
            let done = cleaning
                .progress
                .bytes_done
                .load(Ordering::Relaxed)
                .min(cleaning.total);
            w.clean_button.set_sensitive(!cancelling);
            if cancelling {
                w.count_label.set_visible(false);
                w.clean_label.set_text(&tr("Cancelling…"));
            } else {
                if !w.count_label.is_visible() {
                    w.count_label
                        .set_size_request(widest_size(&w.count_label), -1);
                    w.count_label.set_visible(true);
                }
                w.count_label.set_text(&short_size(done));
                w.clean_label.set_text(&tr_args(
                    "of {size}",
                    &[("{size}", &short_size(cleaning.total))],
                ));
            }
            w.clean_button.set_tooltip_text(Some(&tr("Cancel")));
            w.ring.queue_draw();
            return;
        }
        let freed = self.state.plan.borrow().selected_size();
        w.clean_button.set_sensitive(
            self.state.enabled.get()
                && freed > 0
                && !self.state.launching.get()
                && w.stack.visible_child_name().as_deref() == Some("plan"),
        );
        w.count_label.set_visible(false);
        w.clean_label.set_text(&match self.state.countdown.get() {
            Some(_) => tr("Cancel"),
            None if freed > 0 => format!("{} {}", tr("Clean"), short_size(freed)),
            None => tr("Clean"),
        });
        match self.state.countdown.get() {
            Some(left) => {
                w.ring.queue_draw();
                w.clean_button.set_tooltip_text(Some(&tr_args(
                    "Cleaning {size} in {seconds}… tap to cancel",
                    &[
                        ("{size}", &short_size(freed)),
                        ("{seconds}", &left.to_string()),
                    ],
                )));
            }
            None => {
                w.clean_button.set_tooltip_text(Some(&if freed > 0 {
                    tr_args("Done, clean {size}", &[("{size}", &short_size(freed))])
                } else {
                    tr("Pick something to clean")
                }));
            }
        }
        w.ring.queue_draw();
    }

    /// Start the countdown as if the trash can was pressed, when there is
    /// something to clean and no countdown is running yet.
    pub fn start_clean_countdown(&self) {
        if self.can_start_countdown() {
            self.confirm_then_countdown();
        }
    }

    fn can_start_countdown(&self) -> bool {
        self.state.enabled.get()
            && !self.state.launching.get()
            && self.state.countdown.get().is_none()
            && self.state.plan.borrow().selected_size() > 0
            && self.state.w.stack.visible_child_name().as_deref() == Some("plan")
    }

    /// Warnings come first, so nothing animates for a cleanup that is
    /// then refused.
    fn confirm_then_countdown(&self) {
        let confirmed = self.state.confirm.borrow().is_some();
        let view = self.downgrade();
        let proceed: Box<dyn FnOnce()> = Box::new(move || {
            if let Some(view) = view.upgrade() {
                if view.can_start_countdown() {
                    if confirmed {
                        view.launch_cleanup();
                    } else {
                        view.start_countdown();
                        view.refresh_button();
                    }
                }
            }
        });
        let (private, bulk) = self.state.plan.borrow().selected_results();
        match self.state.confirm.borrow().as_ref() {
            Some(confirm) => confirm(private, bulk, proceed),
            None => proceed(),
        }
    }

    fn start_countdown(&self) {
        self.state.countdown.set(Some(COUNTDOWN_SECONDS));
        self.state.countdown_started.set(Some(Instant::now()));
        // Redraw the ring every frame so it sweeps instead of ticking.
        let started = self.state.countdown_started.get();
        let view = self.downgrade();
        let id = self.state.w.ring.add_tick_callback(move |ring, _| {
            let Some(view) = view.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if view.state.countdown_started.get() != started {
                return glib::ControlFlow::Break;
            }
            ring.queue_draw();
            glib::ControlFlow::Continue
        });
        if crate::theme::transition_duration(&self.root, 1) == 0 {
            id.remove();
        } else if let Some(old) = self.state.ring_tick.replace(Some(id)) {
            old.remove();
        }
        let view = self.downgrade();
        let source = glib::timeout_add_seconds_local(1, move || {
            let Some(view) = view.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let left = view.state.countdown.get().unwrap_or(0).saturating_sub(1);
            if left == 0 {
                view.stop_ring();
                view.state.countdown.set(None);
                view.state.countdown_source.replace(None);
                view.refresh_button();
                view.launch_cleanup();
                return glib::ControlFlow::Break;
            }
            view.state.countdown.set(Some(left));
            view.refresh_button();
            glib::ControlFlow::Continue
        });
        self.state.countdown_source.replace(Some(source));
    }

    fn stop_ring(&self) {
        self.state.countdown_started.set(None);
        if let Some(id) = self.state.ring_tick.take() {
            id.remove();
        }
    }

    /// The countdown ran out: hand the selection over. The page calls
    /// `track_cleanup` once the cleanup really starts.
    fn launch_cleanup(&self) {
        let (private, bulk) = self.state.plan.borrow().selected_results();
        (self.state.on_clean)(private, bulk);
    }

    /// A cleanup of `total` bytes is running: the button counts it and
    /// particles stream from the picked tiles into it until
    /// `finish_cleanup`. Tapping the button sets `cancel`.
    pub fn track_cleanup(&self, total: u64, progress: Arc<CleanProgress>, cancel: Arc<AtomicBool>) {
        self.finish_cleanup();
        self.cancel_countdown();
        let w = &self.state.w;
        let layer = w.particles.widget();
        let mut bursts = Vec::new();
        {
            let plan = self.state.plan.borrow();
            let selected = plan.selected_size().max(1) as f64;
            let tiles = self.state.tiles.borrow();
            for (group, tile) in plan.groups.iter().zip(tiles.iter()) {
                let size = group.selected_size();
                if size == 0 {
                    continue;
                }
                let Some(bounds) = tile.button.compute_bounds(layer) else {
                    continue;
                };
                tile.button.add_css_class("launching");
                bursts.push(Burst {
                    rect: (
                        f64::from(bounds.x()),
                        f64::from(bounds.y()),
                        f64::from(bounds.width()),
                        f64::from(bounds.height()),
                    ),
                    rgb: group_style(group.kind).1,
                    weight: size as f64 / selected,
                });
            }
        }
        let target = w
            .ring
            .compute_bounds(layer)
            .map(|b| {
                (
                    f64::from(b.x() + b.height() / 2.0),
                    f64::from(b.y() + b.height() / 2.0),
                )
            })
            .unwrap_or((
                f64::from(layer.width()) / 2.0,
                f64::from(layer.height()) - 60.0,
            ));
        self.state.launching.set(true);
        w.scan_button.set_sensitive(false);
        self.animate_lid(true);
        if crate::theme::transition_duration(&self.root, 1) > 0 {
            // Keep the charging bolt moving for as long as it runs.
            self.state.launch_started.set(Some(Instant::now()));
            let view = self.downgrade();
            let tick = w.ring.add_tick_callback(move |ring, _| {
                let Some(view) = view.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if !view.state.launching.get() {
                    view.state.launch_tick.take();
                    return glib::ControlFlow::Break;
                }
                ring.queue_draw();
                glib::ControlFlow::Continue
            });
            if let Some(old) = self.state.launch_tick.replace(Some(tick)) {
                old.remove();
            }
        }
        w.particles.stream(bursts, target);
        let view = self.downgrade();
        let timer =
            glib::timeout_add_local(Duration::from_millis(100), move || match view.upgrade() {
                Some(view) => {
                    view.refresh_button();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        self.state.cleaning.replace(Some(Cleaning {
            total,
            progress,
            cancel,
            timer,
        }));
        self.refresh_button();
    }

    /// The cleanup ended (or never started): stop the stream and give the
    /// button back.
    pub fn finish_cleanup(&self) {
        let Some(cleaning) = self.state.cleaning.take() else {
            return;
        };
        cleaning.timer.remove();
        let w = &self.state.w;
        w.particles.stop_stream();
        self.state.launching.set(false);
        self.state.launch_started.set(None);
        if let Some(tick) = self.state.launch_tick.take() {
            tick.remove();
        }
        w.scan_button.set_sensitive(true);
        self.animate_lid(false);
        for tile in self.state.tiles.borrow().iter() {
            tile.button.remove_css_class("launching");
        }
        self.refresh_button();
    }

    fn animate_lid(&self, open: bool) {
        if let Some(tick) = self.state.lid_tick.take() {
            tick.remove();
        }
        let target = if open { 1.0 } else { 0.0 };
        let duration = crate::theme::transition_duration(&self.root, 250);
        if duration == 0 {
            self.state.lid.set(target);
            self.state.w.ring.queue_draw();
            return;
        }
        let from = self.state.lid.get();
        let started = Instant::now();
        let view = self.downgrade();
        let tick = self.state.w.ring.add_tick_callback(move |area, _| {
            let Some(view) = view.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let t = (started.elapsed().as_secs_f64() * 1000.0 / f64::from(duration)).min(1.0);
            view.state.lid.set(from + (target - from) * t);
            area.queue_draw();
            if t >= 1.0 {
                view.state.lid_tick.take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        self.state.lid_tick.replace(Some(tick));
    }

    fn cancel_countdown(&self) {
        self.stop_ring();
        if let Some(source) = self.state.countdown_source.take() {
            source.remove();
        }
        if self.state.countdown.take().is_some() {
            self.refresh_button();
        }
    }

    /// Impact → group → item, each level with a tri-state tick and a
    /// chevron. Only groups with something ticked are listed.
    fn rebuild_tree(&self) {
        let w = &self.state.w;
        while let Some(child) = w.tree.first_child() {
            w.tree.remove(&child);
        }
        let plan = self.state.plan.borrow();
        w.tree_total.set_text(&short_size(plan.selected_size()));
        let open = self.state.open.borrow().clone();
        let fresh = self.state.fresh.take();
        let all_fresh = fresh.as_deref() == Some("*");
        let is_fresh = |key: &str| all_fresh || fresh.as_deref() == Some(key);
        let mut entering: Vec<gtk4::Box> = Vec::new();
        let mut shown = self.state.shown.borrow_mut();
        for group in &plan.groups {
            if group.selected_count() > 0 {
                shown.insert(group.kind);
            }
        }
        let shown = shown.clone();

        for (tier_index, impact) in Impact::ALL.into_iter().enumerate() {
            let groups: Vec<(usize, &PlanGroup)> = plan
                .groups
                .iter()
                .enumerate()
                .filter(|(_, g)| g.impact() == impact && shown.contains(&g.kind))
                .collect();
            if groups.is_empty() {
                continue;
            }
            let section = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            section.add_css_class("tier-section");
            section.add_css_class(impact.css_class());

            let tier_key = format!("t{tier_index}");
            let tier_open = open.contains(&tier_key);
            let row = self.tree_row(
                0,
                Some((tier_key.clone(), tier_open)),
                plan.impact_state(impact),
                impact.icon_name(),
                &tr(impact.title()),
                Some(&tr(impact.hint())),
                plan.impact_selected_size(impact),
                {
                    let view = self.downgrade();
                    move |on| {
                        if let Some(view) = view.upgrade() {
                            view.state.plan.borrow_mut().set_impact(impact, on);
                            view.after_manual_change();
                        }
                    }
                },
            );
            row.add_css_class("tier-row");
            if all_fresh {
                entering.push(row.clone());
            }
            section.append(&row);

            if tier_open {
                for (group_index, group) in groups {
                    let group_key = format!("g{:?}", group.kind);
                    let group_open = open.contains(&group_key);
                    let count = tr_args(
                        "{selected} of {total}",
                        &[
                            (
                                "{selected}",
                                &crate::i18n::format_count(group.selected_count()),
                            ),
                            (
                                "{total}",
                                &group
                                    .offered_count()
                                    .max(group.selected_count())
                                    .to_string(),
                            ),
                        ],
                    );
                    let row = self.tree_row(
                        1,
                        Some((group_key.clone(), group_open)),
                        group.state(),
                        group.kind.icon_name(),
                        &tr(group.kind.title()),
                        Some(&count),
                        group.selected_size(),
                        {
                            let view = self.downgrade();
                            move |on| {
                                if let Some(view) = view.upgrade() {
                                    view.state.plan.borrow_mut().groups[group_index].set_all(on);
                                    view.after_manual_change();
                                }
                            }
                        },
                    );
                    row.add_css_class(group_style(group.kind).0);
                    row.add_css_class("group-accent");
                    if is_fresh(&tier_key) {
                        entering.push(row.clone());
                    }
                    section.append(&row);
                    if group_open {
                        for (item_index, item) in group.items.iter().enumerate() {
                            let row = self.tree_row(
                                2,
                                None,
                                if item.selected { Tick::All } else { Tick::None },
                                "",
                                &if item.translatable {
                                    tr(&item.label)
                                } else {
                                    item.label.clone()
                                },
                                None,
                                item.size(),
                                {
                                    let view = self.downgrade();
                                    move |on| {
                                        if let Some(view) = view.upgrade() {
                                            view.state.plan.borrow_mut().groups[group_index]
                                                .items[item_index]
                                                .selected = on;
                                            view.after_manual_change();
                                        }
                                    }
                                },
                            );
                            row.add_css_class(group_style(group.kind).0);
                            row.add_css_class("group-accent");
                            if !item.detail.is_empty() {
                                row.set_tooltip_text(Some(&item.detail));
                            }
                            if let Some(disk) = &item.disk {
                                let chip = disk_chip(
                                    disk,
                                    &tr("On another disk. It frees space there, not on the home disk."),
                                );
                                if let Some(size) = row.last_child() {
                                    chip.insert_before(&row, Some(&size));
                                }
                            }
                            if item.pinned || item.busy {
                                let pin = gtk4::Image::from_icon_name("pc-view-pin-symbolic");
                                pin.set_tooltip_text(Some(&tr(if item.busy {
                                    "A running program is using it"
                                } else {
                                    "Changed recently, so it is not picked for you"
                                })));
                                // Before the size label.
                                if let Some(size) = row.last_child() {
                                    pin.insert_before(&row, Some(&size));
                                }
                                if !item.selected {
                                    row.add_css_class("pinned");
                                }
                            }
                            if !item.can_select() {
                                row.set_sensitive(false);
                            }
                            if is_fresh(&tier_key) || is_fresh(&group_key) {
                                entering.push(row.clone());
                            }
                            section.append(&row);
                        }
                    }
                }
            }
            w.tree.append(&section);
        }
        // Staggered entrance; CSS skips it when animations are off.
        for (index, row) in entering.iter().enumerate() {
            row.add_css_class("enter");
            row.add_css_class(&format!("enter-{}", index.min(15)));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tree_row(
        &self,
        depth: i32,
        expander: Option<(String, bool)>,
        tick: Tick,
        icon: &str,
        title: &str,
        note: Option<&str>,
        size: u64,
        on_toggle: impl Fn(bool) + 'static,
    ) -> gtk4::Box {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        row.add_css_class("tree-row");
        row.set_margin_start(depth * 24 + if expander.is_none() { 30 } else { 0 });

        if let Some((key, open)) = expander {
            let chevron = gtk4::Button::from_icon_name(if open {
                "pc-pan-down-symbolic"
            } else {
                "pc-pan-end-symbolic"
            });
            chevron.add_css_class("flat");
            chevron.add_css_class("tree-chevron");
            let view = self.downgrade();
            chevron.connect_clicked(move |_| {
                let Some(view) = view.upgrade() else { return };
                {
                    let mut set = view.state.open.borrow_mut();
                    if !set.remove(&key) {
                        set.insert(key.clone());
                        view.state.fresh.replace(Some(key.clone()));
                    }
                }
                let view = view.clone();
                glib::idle_add_local_once(move || view.rebuild_tree());
            });
            row.append(&chevron);
        }

        let check = gtk4::CheckButton::new();
        check.add_css_class("tree-check");
        check.set_active(tick == Tick::All);
        check.set_inconsistent(tick == Tick::Some);
        let on_toggle = Rc::new(on_toggle);
        {
            let on_toggle = on_toggle.clone();
            check.connect_toggled(move |check| {
                // Some → All, All → None, None → All.
                let on = check.is_active() || check.is_inconsistent();
                let on_toggle = on_toggle.clone();
                glib::idle_add_local_once(move || on_toggle(on));
            });
        }
        row.append(&check);

        if !icon.is_empty() {
            let image = crate::ui::icon_image(icon);
            image.add_css_class("tree-icon");
            row.append(&image);
        }
        let label = gtk4::Label::new(Some(title));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        if depth == 0 {
            label.add_css_class("heading");
        }
        row.append(&label);
        if let Some(note) = note {
            let note = gtk4::Label::new(Some(note));
            note.add_css_class("dim-label");
            note.add_css_class("caption-text");
            note.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            row.append(&note);
        }
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        row.append(&spacer);
        let size_label = gtk4::Label::new(Some(&short_size(size)));
        size_label.add_css_class("numeric");
        row.append(&size_label);

        // A click anywhere on the row ticks it, like the checkbox.
        let click = gtk4::GestureClick::new();
        let check_weak = check.downgrade();
        click.connect_released(move |gesture, _, x, y| {
            let Some(check) = check_weak.upgrade() else {
                return;
            };
            let Some(row) = gesture.widget() else { return };
            // Buttons inside the row handle their own clicks.
            if let Some(target) = row.pick(x, y, gtk4::PickFlags::DEFAULT) {
                if target.ancestor(gtk4::Button::static_type()).is_some()
                    || target.ancestor(gtk4::CheckButton::static_type()).is_some()
                {
                    return;
                }
            }
            if check.is_sensitive() {
                check.set_active(!check.is_active());
            }
        });
        row.add_controller(click);
        row.set_cursor_from_name(Some("pointer"));
        row
    }
}

#[derive(Clone)]
struct WeakView {
    root: glib::WeakRef<gtk4::Box>,
    state: std::rc::Weak<State>,
}

impl WeakView {
    fn upgrade(&self) -> Option<CleanupView> {
        Some(CleanupView {
            root: self.root.upgrade()?,
            state: self.state.upgrade()?,
        })
    }
}

/// Hover card: what it is, what deleting it means, how it comes back.
fn explanation_popover(group: &PlanGroup) -> (gtk4::Popover, gtk4::Label) {
    let impact = group.impact();
    let popover = gtk4::Popover::new();
    popover.set_autohide(false);
    popover.set_can_focus(false);
    popover.set_can_target(false);
    popover.set_position(gtk4::PositionType::Bottom);
    popover.add_css_class("tile-explainer");

    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
    body.set_margin_top(6);
    body.set_margin_bottom(6);
    body.set_margin_start(6);
    body.set_margin_end(6);
    body.set_width_request(300);

    let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let icon = crate::ui::icon_image(group.kind.icon_name());
    let title = gtk4::Label::new(Some(&tr(group.kind.title())));
    title.add_css_class("heading");
    title.set_hexpand(true);
    title.set_xalign(0.0);
    let badge = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    badge.add_css_class("impact-badge");
    badge.add_css_class(impact.css_class());
    badge.append(&gtk4::Image::from_icon_name(impact.icon_name()));
    badge.append(&gtk4::Label::new(Some(&tr(impact.title()))));
    head.add_css_class(group_style(group.kind).0);
    head.add_css_class("group-accent");
    head.append(&icon);
    head.append(&title);
    head.append(&badge);
    body.append(&head);

    let [what, if_deleted, comes_back] = group.kind.explanation();
    for (icon, caption, text) in [
        ("pc-dialog-information-symbolic", "What it is", what),
        ("pc-user-trash-symbolic", "If you delete it", if_deleted),
        ("pc-view-refresh-symbolic", "How it comes back", comes_back),
    ] {
        let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        let image = gtk4::Image::from_icon_name(icon);
        image.set_valign(gtk4::Align::Start);
        image.add_css_class("dim-label");
        let column = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        let caption = gtk4::Label::new(Some(&tr(caption)));
        caption.add_css_class("caption-heading");
        caption.set_xalign(0.0);
        let text = gtk4::Label::new(Some(&tr(text)));
        text.set_wrap(true);
        text.set_xalign(0.0);
        text.set_max_width_chars(40);
        column.append(&caption);
        column.append(&text);
        line.append(&image);
        line.append(&column);
        body.append(&line);
    }
    let freed = gtk4::Label::new(None);
    freed.add_css_class("dim-label");
    freed.set_xalign(0.0);
    body.append(&freed);
    popover.set_child(Some(&body));
    (popover, freed)
}

/// Bytes of a group nobody picked that it does not offer: changed
/// recently or in use.
fn locked_size(group: &PlanGroup) -> u64 {
    group
        .items
        .iter()
        .filter(|item| !item.selected && !item.auto_selectable())
        .map(|item| item.size())
        .sum()
}

fn set_rgb(cr: &gtk4::cairo::Context, (r, g, b): (f64, f64, f64), alpha: f64) {
    cr.set_source_rgba(r, g, b, alpha);
}

fn rounded(cr: &gtk4::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// "🖴 /home2": the disk an item lives on when it is not the home one.
fn disk_chip(disk: &str, tooltip: &str) -> gtk4::Box {
    let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    chip.add_css_class("disk-chip");
    chip.set_valign(gtk4::Align::Center);
    let icon = gtk4::Image::from_icon_name("pc-drive-harddisk-symbolic");
    icon.set_pixel_size(12);
    chip.append(&icon);
    chip.append(&gtk4::Label::new(Some(disk)));
    chip.set_tooltip_text(Some(tooltip));
    chip
}

/// Another disk after the cleanup, drawn like the home one: other files,
/// then what each group keeps there, then free space.
fn draw_other_bar(
    state: &State,
    disk: &OtherDisk,
    cr: &gtk4::cairo::Context,
    width: f64,
    height: f64,
) {
    if disk.total == 0 {
        return;
    }
    let plan = state.plan.borrow();
    let on = Some(disk.mount.as_str());
    let found: u64 = plan
        .groups
        .iter()
        .map(|g| g.size_on(on))
        .sum::<u64>()
        .min(disk.used);
    let scale = width / disk.total as f64;
    rounded(cr, 0.0, 0.0, width, height, height / 2.0);
    cr.clip();
    set_rgb(cr, (0.5, 0.5, 0.5), 0.12);
    let _ = cr.paint();
    let mut x = disk.used.saturating_sub(found) as f64 * scale;
    set_rgb(cr, OTHER_COLOR, 1.0);
    cr.rectangle(0.0, 0.0, x, height);
    let _ = cr.fill();
    for group in &plan.groups {
        let kept = group.size_on(on).saturating_sub(group.selected_on(on)) as f64 * scale;
        if kept <= 0.0 {
            continue;
        }
        set_rgb(cr, group_style(group.kind).1, 1.0);
        cr.rectangle(x, 0.0, kept, height);
        let _ = cr.fill();
        if kept >= 6.0 {
            cr.save().ok();
            cr.set_operator(gtk4::cairo::Operator::Clear);
            cr.rectangle(x, 0.0, 2.0, height);
            let _ = cr.fill();
            cr.restore().ok();
        }
        x += kept;
    }
}

/// The whole disk as it will be after the cleanup: your files, then what
/// each group keeps, then free space. Selected bytes leave the bar.
fn draw_bar(state: &State, cr: &gtk4::cairo::Context, width: f64, height: f64) {
    let total = state.disk_total.get();
    if total == 0 {
        return;
    }
    let plan = state.plan.borrow();
    let gone = state.bar_gone.borrow();
    let used = state.disk_used.get();
    // The bar is the home disk: items on other disks are not in it.
    let found = plan.home_total_size().min(used);
    let scale = width / total as f64;

    rounded(cr, 0.0, 0.0, width, height, (height / 2.0).min(10.0));
    cr.clip();
    set_rgb(cr, (0.5, 0.5, 0.5), 0.12);
    let _ = cr.paint();

    let mut x = 0.0;
    let other = used.saturating_sub(found) as f64 * scale;
    set_rgb(cr, OTHER_COLOR, 1.0);
    cr.rectangle(x, 0.0, other, height);
    let _ = cr.fill();
    if other > 160.0 && height >= 24.0 {
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
        cr.select_font_face(
            crate::fonts::FAMILY,
            gtk4::cairo::FontSlant::Normal,
            gtk4::cairo::FontWeight::Normal,
        );
        cr.set_font_size(12.0);
        cr.move_to(x + 12.0, height / 2.0 + 4.0);
        let _ = cr.show_text(&tr("Your files and apps"));
    }
    x += other;

    // Each group is one block of what it keeps; a thin gap of track
    // separates blocks.
    for (index, group) in plan.groups.iter().enumerate() {
        let removed = gone
            .get(index)
            .copied()
            .unwrap_or(group.home_selected_size() as f64);
        let kept = (group.home_size() as f64 - removed).max(0.0) * scale;
        if kept <= 0.0 {
            continue;
        }
        // What the group does not offer (recent or in use) stays, faint
        // and striped, after what you could still pick.
        let locked = (locked_size(group) as f64 * scale).min(kept);
        let open = kept - locked;
        let rgb = group_style(group.kind).1;
        set_rgb(cr, rgb, 1.0);
        cr.rectangle(x, 0.0, open, height);
        let _ = cr.fill();
        if locked > 0.0 {
            let lx = x + open;
            set_rgb(cr, rgb, 0.35);
            cr.rectangle(lx, 0.0, locked, height);
            let _ = cr.fill();
            cr.save().ok();
            cr.rectangle(lx, 0.0, locked, height);
            cr.clip();
            set_rgb(cr, rgb, 0.8);
            cr.set_line_width(2.0);
            let mut stripe = lx - height;
            while stripe < lx + locked {
                cr.move_to(stripe, height);
                cr.line_to(stripe + height, 0.0);
                stripe += 6.0;
            }
            let _ = cr.stroke();
            cr.restore().ok();
        }
        if kept >= 6.0 {
            cr.save().ok();
            cr.set_operator(gtk4::cairo::Operator::Clear);
            cr.rectangle(x, 0.0, 2.0, height);
            let _ = cr.fill();
            cr.restore().ok();
        }
        x += kept;
    }
}

/// The clean button wears the app icon: a blue pill with a top
/// shine, a glassy rim and the white bolt. While counting down a white
/// ring sweeps around the seconds left, a sheen crosses the tile every
/// second and the glow pulses; while the cleanup launches the bolt
/// charges up and throws out rays.
fn draw_clean_button(state: &State, cr: &gtk4::cairo::Context, width: f64, height: f64) {
    use std::f64::consts::PI;
    let counting = state.countdown.get().is_some();
    let total = f64::from(COUNTDOWN_SECONDS);
    let elapsed = state
        .countdown_started
        .get()
        .map(|started| started.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    let remaining = match state.countdown_started.get() {
        Some(_) => (total - elapsed).clamp(0.0, total),
        None => f64::from(state.countdown.get().unwrap_or(0)),
    };
    let fraction = remaining / total;
    // The pill fills the area; the bolt and the ring live in the square
    // on its left end.
    let (x, y, pill_w, side) = (1.0, 1.0, width - 2.0, height - 2.0);
    let radius = side / 2.0;
    let (cx, cy) = (x + side / 2.0 + 2.0, y + side / 2.0);
    let mid = x + pill_w / 2.0;
    let active = state.w.clean_button.is_sensitive() || state.launching.get();
    let charge = state.lid.get();
    let launch_time = state
        .launch_started
        .get()
        .map(|started| started.elapsed().as_secs_f64())
        .unwrap_or(0.0);

    // Tile: the icon's sky-to-deep blue, brighter as the end nears.
    rounded(cr, x, y, pill_w, side, radius);
    if active {
        let heat = if counting { 1.0 - fraction } else { charge };
        let lift = |v: f64| v + (1.0 - v) * 0.18 * heat;
        let tile = gtk4::cairo::LinearGradient::new(mid, y, mid, y + side);
        tile.add_color_stop_rgb(0.0, lift(0.361), lift(0.722), lift(1.0));
        tile.add_color_stop_rgb(0.55, lift(0.118), lift(0.565), lift(1.0));
        tile.add_color_stop_rgb(1.0, lift(0.039), lift(0.424), lift(0.941));
        let _ = cr.set_source(&tile);
    } else {
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.22);
    }
    let _ = cr.fill_preserve();
    cr.save().ok();
    cr.clip();
    // While cleaning, a lighter band fills the pill as bytes go.
    if let Some(cleaning) = state.cleaning.borrow().as_ref() {
        let done = cleaning.progress.bytes_done.load(Ordering::Relaxed) as f64;
        let share = (done / cleaning.total.max(1) as f64).clamp(0.0, 1.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.2);
        cr.rectangle(x, y, pill_w * share, side);
        let _ = cr.fill();
    }
    if active {
        // Top shine, as on the icon.
        let shine = gtk4::cairo::LinearGradient::new(mid, y, mid, y + side * 0.6);
        shine.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.34);
        shine.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
        let _ = cr.set_source(&shine);
        let _ = cr.paint();
        // A sheen sweeps across once per second of the countdown.
        if counting {
            let pass = elapsed.fract();
            let sx = x - side + pass * (pill_w + side * 1.5);
            let sheen = gtk4::cairo::LinearGradient::new(sx, y, sx + side * 0.5, y + side);
            sheen.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.0);
            sheen.add_color_stop_rgba(0.5, 1.0, 1.0, 1.0, 0.32);
            sheen.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
            let _ = cr.set_source(&sheen);
            let _ = cr.paint();
        }
    }
    cr.restore().ok();
    // Glassy rim: bright on top, faint in the middle, soft below.
    if active {
        rounded(
            cr,
            x + 0.75,
            y + 0.75,
            pill_w - 1.5,
            side - 1.5,
            radius - 0.75,
        );
        let rim = gtk4::cairo::LinearGradient::new(mid, y, mid, y + side);
        rim.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.85);
        rim.add_color_stop_rgba(0.5, 1.0, 1.0, 1.0, 0.15);
        rim.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.45);
        let _ = cr.set_source(&rim);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    }

    if !counting {
        if !active {
            cr.push_group();
            draw_bolt(cr, cx, cy, side * 0.5, 0.0);
            let _ = cr.pop_group_to_source();
            let _ = cr.paint_with_alpha(0.55);
            return;
        }
        if charge > 0.0 {
            // Rays spin out of the bolt while it charges.
            let spin = launch_time * 2.4;
            cr.set_line_cap(gtk4::cairo::LineCap::Round);
            for k in 0..8 {
                let angle = spin + f64::from(k) * PI / 4.0;
                let flicker = 0.6 + 0.4 * (launch_time * 9.0 + f64::from(k) * 1.7).sin();
                let (inner, outer) = (side * 0.28, side * (0.36 + 0.07 * flicker));
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.7 * charge * flicker);
                cr.set_line_width(2.0);
                cr.move_to(cx + angle.cos() * inner, cy + angle.sin() * inner);
                cr.line_to(cx + angle.cos() * outer, cy + angle.sin() * outer);
                let _ = cr.stroke();
            }
        }
        let pulse = if charge > 0.0 {
            0.06 * (launch_time * 12.0).sin()
        } else {
            0.0
        };
        draw_bolt(
            cr,
            cx,
            cy,
            side * (0.5 + 0.1 * charge + pulse * charge),
            charge,
        );
        return;
    }
    // Countdown: the glow pulses with every second.
    let beat = 1.0 - elapsed.fract();
    let glow = gtk4::cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, side * 0.5);
    glow.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.28 * beat);
    glow.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
    let _ = cr.set_source(&glow);
    cr.arc(cx, cy, side * 0.5, 0.0, 2.0 * PI);
    let _ = cr.fill();
    let ring = side / 2.0 - 8.0;
    cr.set_line_cap(gtk4::cairo::LineCap::Round);
    cr.set_line_width(3.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.3);
    cr.arc(cx, cy, ring, 0.0, 2.0 * PI);
    let _ = cr.stroke();
    cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
    if fraction > 0.0 {
        cr.arc(cx, cy, ring, -PI / 2.0, -PI / 2.0 + 2.0 * PI * fraction);
        let _ = cr.stroke();
        // A bright spark rides the end of the ring.
        let tip = -PI / 2.0 + 2.0 * PI * fraction;
        let (sx, sy) = (cx + tip.cos() * ring, cy + tip.sin() * ring);
        let spark = gtk4::cairo::RadialGradient::new(sx, sy, 0.0, sx, sy, 6.0);
        spark.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 1.0);
        spark.add_color_stop_rgba(1.0, 0.75, 0.9, 1.0, 0.0);
        let _ = cr.set_source(&spark);
        cr.arc(sx, sy, 6.0, 0.0, 2.0 * PI);
        let _ = cr.fill();
    }
    cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
    cr.select_font_face(
        crate::fonts::FAMILY,
        gtk4::cairo::FontSlant::Normal,
        gtk4::cairo::FontWeight::Bold,
    );
    cr.set_font_size(17.0 + 3.0 * beat * beat);
    let text = (remaining.ceil() as u32).max(1).to_string();
    // Centre the glyph's ink, not its advance, so every digit sits in the
    // middle of the ring and nothing shifts as the number changes.
    if let Ok(ext) = cr.text_extents(&text) {
        cr.move_to(
            cx - ext.x_bearing() - ext.width() / 2.0,
            cy - ext.y_bearing() - ext.height() / 2.0,
        );
        let _ = cr.show_text(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::short_size;

    #[test]
    fn group_palette_has_distinct_colours() {
        use super::*;
        let kinds = [
            GroupKind::System,
            GroupKind::AppCaches,
            GroupKind::DevCaches,
            GroupKind::Logs,
            GroupKind::Browsers,
            GroupKind::Custom,
            GroupKind::RustBuilds,
            GroupKind::PythonNode,
            GroupKind::OtherBuilds,
            GroupKind::Games,
            GroupKind::AiModels,
        ];
        let classes: HashSet<_> = kinds.iter().map(|kind| group_style(*kind).0).collect();
        let colours: HashSet<_> = kinds
            .iter()
            .map(|kind| {
                let (_, rgb) = group_style(*kind);
                (
                    (rgb.0 * 255.0) as u8,
                    (rgb.1 * 255.0) as u8,
                    (rgb.2 * 255.0) as u8,
                )
            })
            .collect();
        assert_eq!(classes.len(), kinds.len());
        assert_eq!(colours.len(), kinds.len());
        let css = include_str!("../../data/style.css");
        for kind in kinds {
            let (class, rgb) = group_style(kind);
            let linear = |value: f64| {
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            let luminance =
                0.2126 * linear(rgb.0) + 0.7152 * linear(rgb.1) + 0.0722 * linear(rgb.2);
            assert!(
                1.05 / (luminance + 0.05) >= 4.5,
                "{class} needs readable white text"
            );
            let declaration = format!(
                "@define-color {} #{:02x}{:02x}{:02x};",
                class.replace('-', "_"),
                (rgb.0 * 255.0).round() as u8,
                (rgb.1 * 255.0).round() as u8,
                (rgb.2 * 255.0).round() as u8
            );
            assert!(css.contains(&declaration), "{class} must match Cairo");
        }
    }

    #[test]
    #[ignore = "Requires a GTK display; run under xvfb-run with --ignored"]
    fn header_controls_keep_selection_and_countdown_behaviour() {
        use super::*;
        use crate::models::{FileEntry, PlanItem};
        libadwaita::init().unwrap();
        gtk4::Settings::default()
            .unwrap()
            .set_property("gtk-enable-animations", false);
        let style = gtk4::CssProvider::new();
        style.connect_parsing_error(|_, _, error| panic!("CSS error: {error}"));
        style.load_from_data(include_str!("../../data/style.css"));
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &style,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        crate::i18n::set_language(crate::models::AppLanguage::Spanish);
        let calls = Rc::new(Cell::new(0));
        let cleaned = calls.clone();
        let view = CleanupView::new(move |private, bulk| {
            assert!(!private.is_empty());
            assert!(!bulk.is_empty());
            cleaned.set(cleaned.get() + 1);
        });
        let kinds = [
            GroupKind::System,
            GroupKind::AppCaches,
            GroupKind::DevCaches,
            GroupKind::Logs,
            GroupKind::Browsers,
            GroupKind::Custom,
            GroupKind::RustBuilds,
            GroupKind::PythonNode,
            GroupKind::OtherBuilds,
            GroupKind::Games,
            GroupKind::AiModels,
        ];
        let plan = CleanupPlan::new(
            kinds
                .into_iter()
                .enumerate()
                .map(|(index, kind)| {
                    let mut result = ScanResult::new();
                    result.add_file(FileEntry::new(
                        PathBuf::from(format!("/tmp/cleanup-ui-fixture/{index}")),
                        (index as u64 + 1) * 100_000_000,
                        false,
                        false,
                    ));
                    PlanGroup {
                        kind,
                        items: vec![PlanItem::new(kind.title(), "", result)],
                    }
                })
                .collect(),
        );
        view.set_disk(20_000_000_000, 15_000_000_000);
        view.set_plan(plan);
        view.state.plan.borrow_mut().apply_goal(u64::MAX);
        view.refresh();
        let selected = view.state.plan.borrow().selected_size();
        assert!(view.state.w.clean_button.is_sensitive());
        view.start_clean_countdown();
        assert!(view.state.countdown.get().is_some());
        view.state.w.clean_button.emit_clicked();
        assert!(view.state.countdown.get().is_none());
        assert_eq!(calls.get(), 0);
        view.set_enabled(false);
        view.start_clean_countdown();
        assert!(view.state.countdown.get().is_none());
        assert!(!view.state.w.clean_button.is_sensitive());
        view.set_enabled(true);
        assert_eq!(view.state.plan.borrow().selected_size(), selected);
        let window = gtk4::Window::new();
        window.set_default_size(760, 830);
        window.set_child(Some(view.widget()));
        window.present();
        let context = glib::MainContext::default();
        let until = Instant::now() + Duration::from_millis(200);
        while Instant::now() < until {
            context.iteration(false);
        }
        let layer = view.state.w.particles.widget();
        let target = view.state.w.clean_button.compute_bounds(layer).unwrap();
        let tile = view.state.tiles.borrow()[0]
            .button
            .compute_bounds(layer)
            .unwrap();
        assert!(target.y() < tile.y());
        assert!(target.x() >= 0.0 && target.x() + target.width() <= layer.width() as f32);
        if let Ok(path) = std::env::var("MAGNIFICO_SCREENSHOT") {
            let paintable = gtk4::WidgetPaintable::new(Some(&window));
            let snapshot = gtk4::Snapshot::new();
            paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
            let node = snapshot.to_node().unwrap();
            window
                .renderer()
                .unwrap()
                .render_texture(&node, None)
                .save_to_png(path)
                .unwrap();
        }
        // The countdown hands the selection over right away.
        view.launch_cleanup();
        assert_eq!(calls.get(), 1);
        let progress = Arc::new(CleanProgress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        view.track_cleanup(selected, progress, cancel.clone());
        assert!(view.is_launching());
        view.state.w.clean_button.emit_clicked();
        assert!(cancel.load(Ordering::Relaxed));
        view.finish_cleanup();
        assert!(!view.is_launching());
        assert_eq!(view.state.lid.get(), 0.0);
        view.set_plan(CleanupPlan::default());
        assert!(!view.state.w.clean_button.is_sensitive());
        view.start_clean_countdown();
        assert!(view.state.countdown.get().is_none());
        assert!(view.scan_button().is_visible());
        window.close();
    }

    #[test]
    fn sizes_read_like_the_mockup() {
        assert_eq!(short_size(64_000_000_000), "64 GB");
        assert_eq!(short_size(2_700_000_000), "2.7 GB");
        assert_eq!(short_size(640_000_000), "640 MB");
        assert_eq!(short_size(512), "512 B");
    }
}
