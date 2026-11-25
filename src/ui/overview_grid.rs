//! Disk overview as a waffle grid: 100 × 8 flat square cells, one colour per
//! category, sorted by weight with the free space last.
//!
//! The grid keeps its own state between renders, so the page can hand it a
//! new layout at any time and it animates the difference: cells that change
//! owner flash white and take the new colour in random order ("pixels"), and
//! after a cleanup the cells that were removed fly in a straight line to the
//! Trash button, nearest first, leaving a hole until they land.
use gtk4::{glib, prelude::*};
use std::{cell::RefCell, rc::Rc};

pub const COLS: usize = 100;
pub const ROWS: usize = 8;
/// Category pages carry a flatter copy of the map.
pub const STRIP_ROWS: usize = 5;
/// Segment id for space whose owner is still being measured: its cells flicker.
pub const PENDING: usize = usize::MAX;
const GAP: f64 = 1.0;
const CORNER: f64 = 4.0;
/// Seconds a cell takes to fade from its white flash to its colour.
const FLASH: f64 = 0.35;
/// Window in which the cells of one change pick their random moment.
const SCATTER: f64 = 0.7;
const BLINK: f64 = 0.6;
const FLIGHT: f64 = 0.5;

/// Small xorshift generator: cells only need to look scattered.
struct Rng(u64);
impl Rng {
    fn double_range(&mut self, low: f64, high: f64) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        low + (self.0 % 10_000) as f64 / 10_000.0 * (high - low)
    }
}

fn now() -> f64 {
    glib::monotonic_time() as f64 / 1e6
}

/// Cells per segment by largest remainder: always sums to `cells`, and any
/// segment with something in it keeps at least one cell.
pub fn allocate(fractions: &[f64], cells: usize) -> Vec<usize> {
    let total: f64 = fractions.iter().map(|f| f.max(0.0)).sum();
    if total <= 0.0 {
        return vec![0; fractions.len()];
    }
    let exact: Vec<f64> = fractions
        .iter()
        .map(|f| f.max(0.0) / total * cells as f64)
        .collect();
    let mut counts: Vec<usize> = exact.iter().map(|v| v.floor() as usize).collect();
    let missing = cells - counts.iter().sum::<usize>();
    let mut order: Vec<usize> = (0..exact.len()).collect();
    order.sort_by(|&a, &b| {
        (exact[b] - exact[b].floor())
            .partial_cmp(&(exact[a] - exact[a].floor()))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for &i in order.iter().cycle().take(missing) {
        counts[i] += 1;
    }
    for i in 0..counts.len() {
        if counts[i] == 0 && exact[i] > 0.0 {
            if let Some(big) = (0..counts.len()).max_by_key(|&j| counts[j]) {
                if counts[big] > 1 {
                    counts[big] -= 1;
                    counts[i] = 1;
                }
            }
        }
    }
    counts
}

/// Owner of every cell, left to right then top to bottom within a column.
pub fn layout(fractions: &[f64], cells: usize) -> Vec<usize> {
    let mut owner = Vec::with_capacity(cells);
    for (segment, count) in allocate(fractions, cells).into_iter().enumerate() {
        owner.extend(std::iter::repeat_n(segment, count));
    }
    owner
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Settled,
    /// Becomes `to` at `at`, with a white flash.
    Change { at: f64, to: usize },
    /// Blinks until `launch`, is a hole until `land`, then turns `free`
    /// (and afterwards `after`, when the new layout wants something else).
    Sweep {
        flash_from: f64,
        launch: f64,
        land: f64,
        free: usize,
        after: Option<(f64, usize)>,
    },
}

#[derive(Clone, Copy)]
struct Cell {
    /// Category shown now; `None` while its value has not arrived.
    owner: Option<usize>,
    phase: Phase,
    flashed_at: f64,
}

#[derive(Clone)]
pub struct GridSegment {
    /// Stable category id: cells keep it across layouts.
    pub id: usize,
    pub fraction: f64,
    pub color: &'static str,
    /// 1.0 is the full colour; less shows it faded.
    pub alpha: f64,
    pub label: String,
}

struct State {
    cells: Vec<Cell>,
    /// Colour token and tooltip by category id.
    info: Vec<Option<(&'static str, String, f64)>>,
    rows: usize,
    highlighted: Option<usize>,
    /// Hidden grids never tick, so they take every change at once.
    instant_when_hidden: bool,
    tick: Option<gtk4::TickCallbackId>,
}

mod ratio {
    use gtk4::{glib, prelude::*, subclass::prelude::*};
    use std::cell::Cell;

    #[derive(Default)]
    pub struct RatioBox {
        pub rows: Cell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RatioBox {
        const NAME: &'static str = "MagnificoRatioBox";
        type Type = super::RatioBox;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for RatioBox {
        fn dispose(&self) {
            if let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for RatioBox {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            gtk4::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk4::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().first_child() else {
                return (0, 0, -1, -1);
            };
            let rows = self.rows.get().max(1) as f64;
            if orientation == gtk4::Orientation::Horizontal {
                let (min, nat, _, _) = child.measure(orientation, -1);
                return (min, nat, -1, -1);
            }
            // The height comes from the width in the same pass, so the first
            // frame is already right.
            let width = if for_size > 0 {
                for_size
            } else {
                child.measure(gtk4::Orientation::Horizontal, -1).1
            };
            let height = (f64::from(width) * rows / super::COLS as f64).ceil() as i32;
            (height, height, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                child.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    /// Holds the grid and keeps its height proportional to its width.
    pub struct RatioBox(ObjectSubclass<ratio::RatioBox>)
        @extends gtk4::Widget;
}

impl RatioBox {
    fn new(child: &gtk4::DrawingArea, rows: usize) -> Self {
        use gtk4::subclass::prelude::ObjectSubclassIsExt;
        let frame: Self = glib::Object::new();
        frame.imp().rows.set(rows);
        frame.set_hexpand(true);
        child.set_parent(&frame);
        frame
    }
}

#[derive(Clone)]
pub struct Grid {
    /// Gives the area its height from the width it gets, in the same pass.
    frame: RatioBox,
    area: gtk4::DrawingArea,
    state: Rc<RefCell<State>>,
    hover: Rc<RefCell<Rc<dyn Fn(Option<usize>)>>>,
}

/// Where swept cells go.
pub struct Sweep<'a> {
    pub flights: &'a super::flight_layer::FlightLayer,
    pub target: &'a gtk4::Widget,
}

impl Grid {
    pub fn new() -> Self {
        Self::with_rows(ROWS)
    }

    pub fn with_rows(rows: usize) -> Self {
        let area = gtk4::DrawingArea::new();
        area.set_hexpand(true);
        area.set_content_width(COLS as i32 * 2);
        area.set_content_height(rows as i32 * 2);
        area.add_css_class("pc-segmented-bar");
        // Square cells: the height follows the width it is given.
        let frame = RatioBox::new(&area, rows);
        let state = Rc::new(RefCell::new(State {
            cells: vec![
                Cell {
                    owner: None,
                    phase: Phase::Settled,
                    flashed_at: f64::MIN,
                };
                COLS * rows
            ],
            info: Vec::new(),
            rows,
            highlighted: None,
            instant_when_hidden: false,
            tick: None,
        }));
        let hover: Rc<RefCell<Rc<dyn Fn(Option<usize>)>>> =
            Rc::new(RefCell::new(Rc::new(|_| {})));
        let drawn = state.clone();
        area.set_draw_func(move |area, cr, width, height| {
            draw(area, cr, width, height, &drawn.borrow());
        });
        let motion = gtk4::EventControllerMotion::new();
        let (weak, st, cb) = (area.downgrade(), state.clone(), hover.clone());
        motion.connect_motion(move |_, x, y| {
            let Some(area) = weak.upgrade() else { return };
            let id = cell_at(area.width(), area.height(), st.borrow().rows, x, y)
                .and_then(|i| st.borrow().cells[i].owner);
            let label = id.and_then(|id| {
                st.borrow()
                    .info
                    .get(id)
                    .and_then(|i| i.as_ref().map(|i| i.1.clone()))
            });
            area.set_tooltip_text(label.as_deref());
            let handler = cb.borrow().clone();
            handler(id);
        });
        let cb = hover.clone();
        motion.connect_leave(move |_| {
            let handler = cb.borrow().clone();
            handler(None);
        });
        area.add_controller(motion);
        // A theme switch changes the Cairo colours as well as the CSS ones.
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
        Self {
            frame,
            area,
            state,
            hover,
        }
    }

    pub fn widget(&self) -> &RatioBox {
        &self.frame
    }

    /// Called with the category id under the pointer, or `None` when it leaves.
    pub fn set_hover(&self, handler: Rc<dyn Fn(Option<usize>)>) {
        *self.hover.borrow_mut() = handler;
    }

    pub fn set_instant_when_hidden(&self, on: bool) {
        self.state.borrow_mut().instant_when_hidden = on;
    }

    pub fn set_highlighted(&self, id: Option<usize>) {
        self.state.borrow_mut().highlighted = id;
        self.area.queue_draw();
    }

    /// Show a new layout; `segments` are in display order. Cells that change
    /// owner animate in random order. With `sweep`, cells that left a
    /// shrinking category fly to the target first and reflow after landing.
    /// With `wave`, the cells that change do so in a sweep from left to right.
    pub fn update(&self, segments: Vec<GridSegment>, free_id: usize, sweep: Option<Sweep>, wave: bool) {
        let fractions: Vec<f64> = segments.iter().map(|s| s.fraction).collect();
        let rows = self.state.borrow().rows;
        let cells = COLS * rows;
        let target: Vec<Option<usize>> = layout(&fractions, cells)
            .into_iter()
            .map(|i| Some(segments[i].id).filter(|&id| id != PENDING))
            .collect();
        let hidden = self.state.borrow().instant_when_hidden && !self.area.is_mapped();
        let animated = libadwaita::is_animations_enabled(&self.area) && !hidden;
        let t0 = now();
        let mut rng = Rng((t0 * 1e6) as u64 | 1);
        let mut state = self.state.borrow_mut();
        let slots = segments
            .iter()
            .filter(|s| s.id != PENDING)
            .map(|s| s.id)
            .max()
            .map_or(0, |m| m + 1);
        state.info = vec![None; slots];
        for s in segments.iter().filter(|s| s.id != PENDING) {
            state.info[s.id] = Some((s.color, s.label.clone(), s.alpha));
        }
        state.highlighted = None;
        let first = state.cells.iter().all(|c| c.owner.is_none());

        // Cells leaving the categories that shrank, nearest to the Trash first.
        let mut flying: Vec<(usize, f64)> = Vec::new();
        let mut layer_target = None;
        if let (Some(sweep), true, false) = (&sweep, animated, first) {
            let layer = sweep.flights;
            let point = layer.target_point(sweep.target);
            layer_target = Some(point);
            let mut old = vec![0usize; slots.max(free_id + 1)];
            let mut new = old.clone();
            for c in &state.cells {
                if let Some(o) = c.owner.filter(|&o| o < old.len()) {
                    old[o] += 1;
                }
            }
            for &o in target.iter().flatten() {
                new[o] += 1;
            }
            for id in (0..old.len()).filter(|&id| id != free_id && new[id] < old[id]) {
                let mut mine: Vec<(usize, f64)> = (0..cells)
                    .filter(|&i| state.cells[i].owner == Some(id))
                    .map(|i| (i, distance(&self.area, layer, i, rows, point)))
                    .collect();
                mine.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                flying.extend(mine.into_iter().take(old[id] - new[id]));
            }
            flying.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        }
        let step = (1.0 / flying.len().max(1) as f64).min(0.03);
        let mut swept_until = t0;
        let mut swept = vec![None; cells];
        for (rank, &(i, _)) in flying.iter().enumerate() {
            let launch = t0 + BLINK + rank as f64 * step;
            swept_until = swept_until.max(launch + FLIGHT);
            swept[i] = Some(launch);
        }
        // Everything else flips to its final owner at a random moment, after
        // the sweep when there is one.
        let base = if swept_until > t0 { swept_until + 0.25 } else { t0 };
        for i in 0..cells {
            let to = target.get(i).copied().flatten();
            if let Some(launch) = swept[i] {
                let land = launch + FLIGHT;
                let after = to
                    .filter(|&to| to != free_id)
                    .map(|to| (base + rng.double_range(0.0, SCATTER), to));
                let from = state.cells[i]
                    .owner
                    .and_then(|o| state.info.get(o).cloned().flatten())
                    .map_or("pc_chart_else", |i| i.0);
                state.cells[i].phase = Phase::Sweep {
                    flash_from: t0,
                    launch,
                    land,
                    free: free_id,
                    after,
                };
                if let (Some(sweep), Some(point)) = (&sweep, layer_target) {
                    if let Some(origin) = cell_center(&self.area, sweep.flights, i, rows) {
                        sweep
                            .flights
                            .launch(origin, point, from, launch, FLIGHT, sweep.target);
                    }
                }
                continue;
            }
            if let Phase::Sweep { flash_from, launch, land, free, .. } = state.cells[i].phase {
                // A sweep still in flight: only its follow-up can change.
                let after = to
                    .filter(|&to| to != free)
                    .map(|to| (base + rng.double_range(0.0, SCATTER), to));
                state.cells[i].phase = Phase::Sweep { flash_from, launch, land, free, after };
                continue;
            }
            if let Phase::Change { to: planned, .. } = state.cells[i].phase {
                if Some(planned) == to {
                    continue;
                }
            }
            if state.cells[i].owner == to {
                state.cells[i].phase = Phase::Settled;
                continue;
            }
            match to {
                Some(to) if animated => {
                    // A wave moves left to right; otherwise cells pick any moment.
                    let at = base
                        + if wave {
                            (i / rows) as f64 / COLS as f64 * SCATTER
                        } else {
                            rng.double_range(0.0, SCATTER)
                        };
                    state.cells[i].phase = Phase::Change { at, to };
                }
                to => {
                    state.cells[i].owner = to;
                    state.cells[i].phase = Phase::Settled;
                }
            }
        }
        let busy = state
            .cells
            .iter()
            .any(|c| c.phase != Phase::Settled || c.owner.is_none());
        drop(state);
        self.area.queue_draw();
        if busy {
            self.start_tick();
        }
    }

    fn start_tick(&self) {
        if self.state.borrow().tick.is_some() {
            return;
        }
        let state = self.state.clone();
        let id = self.area.add_tick_callback(move |area, _| {
            let t = now();
            let busy = {
                let mut state = state.borrow_mut();
                for c in &mut state.cells {
                    match c.phase {
                        Phase::Change { at, to } if t >= at => {
                            c.owner = Some(to);
                            c.flashed_at = at;
                            c.phase = Phase::Settled;
                        }
                        Phase::Sweep { land, free, after, .. } if t >= land => {
                            c.owner = Some(free);
                            c.flashed_at = land;
                            c.phase = after.map_or(Phase::Settled, |(at, to)| Phase::Change { at, to });
                        }
                        _ => {}
                    }
                }
                state
                    .cells
                    .iter()
                    .any(|c| {
                        c.phase != Phase::Settled || c.owner.is_none() || t - c.flashed_at < FLASH
                    })
            };
            area.queue_draw();
            if busy {
                glib::ControlFlow::Continue
            } else {
                state.borrow_mut().tick = None;
                glib::ControlFlow::Break
            }
        });
        self.state.borrow_mut().tick = Some(id);
    }
}

fn cell_geometry(width: i32, height: i32, rows: usize) -> (f64, f64, f64) {
    // The width decides the size. The height catches up one frame later, so
    // using it here would draw a tiny grid for an instant at start-up.
    let size = ((f64::from(width) - GAP * (COLS as f64 - 1.0)) / COLS as f64).max(1.0);
    let used_h = size * rows as f64 + GAP * (rows as f64 - 1.0);
    (size, 0.0, ((f64::from(height) - used_h) / 2.0).max(0.0))
}

fn cell_at(width: i32, height: i32, rows: usize, x: f64, y: f64) -> Option<usize> {
    let (size, ox, oy) = cell_geometry(width, height, rows);
    let col = ((x - ox) / (size + GAP)).floor();
    let row = ((y - oy) / (size + GAP)).floor();
    ((0.0..COLS as f64).contains(&col) && (0.0..rows as f64).contains(&row))
        .then(|| col as usize * rows + row as usize)
}

fn cell_center(
    area: &gtk4::DrawingArea,
    layer: &super::flight_layer::FlightLayer,
    index: usize,
    rows: usize,
) -> Option<(f64, f64, f64)> {
    let (size, ox, oy) = cell_geometry(area.width(), area.height(), rows);
    let (col, row) = (index / rows, index % rows);
    let x = ox + col as f64 * (size + GAP) + size / 2.0;
    let y = oy + row as f64 * (size + GAP) + size / 2.0;
    let point = area.compute_point(layer.widget(), &gtk4::graphene::Point::new(x as f32, y as f32))?;
    Some((f64::from(point.x()), f64::from(point.y()), size))
}

fn distance(
    area: &gtk4::DrawingArea,
    layer: &super::flight_layer::FlightLayer,
    index: usize,
    rows: usize,
    target: (f64, f64),
) -> f64 {
    cell_center(area, layer, index, rows)
        .map(|(x, y, _)| (x - target.0).hypot(y - target.1))
        .unwrap_or(f64::MAX)
}

pub(super) fn lookup(area: &gtk4::DrawingArea, token: &str) -> gtk4::gdk::RGBA {
    let dark = area
        .root()
        .is_some_and(|r| r.has_css_class("magnifico-dark"));
    let token = if dark {
        token.replacen("pc_", "pc_dark_", 1)
    } else {
        token.to_string()
    };
    area.style_context()
        .lookup_color(&token)
        .unwrap_or_else(|| gtk4::gdk::RGBA::new(0.5, 0.5, 0.5, 1.0))
}

fn hash(a: usize, b: u64) -> f64 {
    let mut x = (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 32;
    (x % 1000) as f64 / 1000.0
}

fn draw(area: &gtk4::DrawingArea, cr: &gtk4::cairo::Context, width: i32, height: i32, state: &State) {
    let t = now();
    let rows = state.rows;
    let (size, ox, oy) = cell_geometry(width, height, rows);
    // Only the four outer corners are rounded.
    let (w, h) = (
        size * COLS as f64 + GAP * (COLS as f64 - 1.0),
        size * rows as f64 + GAP * (rows as f64 - 1.0),
    );
    let r = CORNER;
    cr.new_sub_path();
    cr.arc(ox + w - r, oy + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(ox + w - r, oy + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(ox + r, oy + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(ox + r, oy + r, r, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
    cr.clip();
    let colors: Vec<_> = state
        .info
        .iter()
        .map(|i| i.as_ref().map(|i| (lookup(area, i.0), i.2)))
        .collect();
    let pending = lookup(area, "pc_chart_free");
    let step = (t * 11.0) as u64;
    for (i, cell) in state.cells.iter().enumerate() {
        let x = ox + (i / rows) as f64 * (size + GAP);
        let y = oy + (i % rows) as f64 * (size + GAP);
        let mut white = (1.0 - (t - cell.flashed_at) / FLASH).clamp(0.0, 1.0);
        let shown = match cell.phase {
            Phase::Sweep { flash_from, launch, .. } => {
                if t >= launch {
                    continue; // a hole until the cell lands
                }
                // Blinks three times before it leaves.
                let blink = ((t - flash_from) / BLINK * 3.0 * std::f64::consts::TAU).sin();
                white = white.max(if blink > 0.0 { 0.85 } else { 0.0 });
                cell.owner
            }
            _ => cell.owner,
        };
        let (color, mut alpha) = match shown.and_then(|o| colors.get(o)).and_then(Option::as_ref) {
            Some((color, alpha)) => (color, *alpha),
            None => (&pending, 0.2 + 0.45 * hash(i, step)),
        };
        if shown.is_some_and(|o| state.highlighted.is_some_and(|h| h != o)) {
            alpha = 0.28;
        }
        let mix = |c: f32| f64::from(c) * (1.0 - white) + white;
        cr.set_source_rgba(
            mix(color.red()),
            mix(color.green()),
            mix(color.blue()),
            alpha,
        );
        cr.rectangle(x, y, size, size);
        let _ = cr.fill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CELLS: usize = COLS * ROWS;

    #[test]
    fn allocation_always_fills_the_grid() {
        for fractions in [
            vec![0.285, 0.222, 0.139, 0.127, 0.069, 0.035, 0.02, 0.013, 0.009, 0.081],
            vec![1.0],
            vec![0.0001, 0.9999],
            vec![0.5, 0.5, 0.0],
        ] {
            let counts = allocate(&fractions, CELLS);
            assert_eq!(counts.iter().sum::<usize>(), CELLS, "{fractions:?}");
        }
        assert_eq!(allocate(&[0.0, 0.0], CELLS), vec![0, 0]);
    }

    #[test]
    fn tiny_segments_keep_one_cell() {
        let counts = allocate(&[0.0001, 0.9999], CELLS);
        assert_eq!(counts, vec![1, CELLS - 1]);
    }

    #[test]
    fn layout_orders_cells_by_segment() {
        let owner = layout(&[0.5, 0.25, 0.25], CELLS);
        assert_eq!(owner.len(), CELLS);
        assert_eq!(owner[0], 0);
        assert_eq!(owner[CELLS - 1], 2);
        assert!(owner.windows(2).all(|w| w[0] <= w[1]));
    }
}
