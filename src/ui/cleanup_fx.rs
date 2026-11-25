//! Motion for the cleanup screen, drawn with Cairo on the frame clock:
//! a spinner that does not depend on animated icons, and the particles
//! that stream from the mosaic into the clean button while a cleanup runs.

use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;

pub type Rgb = (f64, f64, f64);

const FREE: Rgb = (0.0, 0xA8 as f64 / 255.0, 0x96 as f64 / 255.0);
const REBUILD: Rgb = (0x6D as f64 / 255.0, 0x4A as f64 / 255.0, 1.0);
const REDOWNLOAD: Rgb = (
    0xE8 as f64 / 255.0,
    0x33 as f64 / 255.0,
    0x7A as f64 / 255.0,
);

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

fn ease_in_out(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Seconds on the widget's frame clock.
fn frame_seconds(clock: &gtk4::gdk::FrameClock) -> f64 {
    clock.frame_time() as f64 / 1e6
}

/// Small xorshift generator: the particles only need to look scattered.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        Self(glib::monotonic_time() as u64 | 1)
    }

    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next()
    }
}

// ───────────────────────────── Spinner ─────────────────────────────

/// A small grid of pixels breathing in the three impact colours.
#[derive(Clone)]
pub struct Spinner {
    area: gtk4::DrawingArea,
    spinning: Rc<Cell<bool>>,
    /// Frame-clock seconds of the last frame; the angle follows from it.
    phase: Rc<Cell<f64>>,
    tick: Rc<RefCell<Option<gtk4::TickCallbackId>>>,
}

impl Spinner {
    pub fn new(size: i32) -> Self {
        let area = gtk4::DrawingArea::new();
        area.set_content_width(size);
        area.set_content_height(size);
        area.set_halign(gtk4::Align::Center);
        let spinner = Self {
            area,
            spinning: Rc::default(),
            phase: Rc::default(),
            tick: Rc::default(),
        };
        let phase = spinner.phase.clone();
        spinner.area.set_draw_func(move |_, cr, width, height| {
            draw_spinner(cr, f64::from(width), f64::from(height), phase.get());
        });
        // Ticks only run while the spinner is on screen.
        // Only the shared cells go in, not the area, so there is no cycle.
        let (spinning, phase, slot) = (
            spinner.spinning.clone(),
            spinner.phase.clone(),
            spinner.tick.clone(),
        );
        spinner.area.connect_map(move |area| {
            if spinning.get() {
                start_spin(area, &phase, &slot);
            }
        });
        let slot = spinner.tick.clone();
        spinner.area.connect_unmap(move |_| {
            if let Some(id) = slot.take() {
                id.remove();
            }
        });
        spinner
    }

    /// A spinner that is already turning, for places that only show it.
    pub fn spinning_widget(size: i32) -> gtk4::DrawingArea {
        let spinner = Self::new(size);
        spinner.set_spinning(true);
        spinner.area
    }

    pub fn widget(&self) -> &gtk4::DrawingArea {
        &self.area
    }

    pub fn is_spinning(&self) -> bool {
        self.spinning.get()
    }

    pub fn set_spinning(&self, on: bool) {
        self.spinning.set(on);
        self.area.set_visible(on);
        if !on {
            if let Some(id) = self.tick.take() {
                id.remove();
            }
        } else if self.area.is_mapped() {
            self.start_ticks();
        }
    }

    fn start_ticks(&self) {
        start_spin(&self.area, &self.phase, &self.tick);
    }
}

fn start_spin(
    area: &gtk4::DrawingArea,
    phase: &Rc<Cell<f64>>,
    slot: &Rc<RefCell<Option<gtk4::TickCallbackId>>>,
) {
    if slot.borrow().is_some() {
        return;
    }
    let phase = phase.clone();
    let id = area.add_tick_callback(move |area, clock| {
        phase.set(frame_seconds(clock));
        area.queue_draw();
        glib::ControlFlow::Continue
    });
    slot.replace(Some(id));
}

fn draw_spinner(cr: &gtk4::cairo::Context, width: f64, height: f64, seconds: f64) {
    // A 3x3 grid of pixels breathing in a diagonal wave, one impact colour
    // per diagonal.
    const GRID: usize = 3;
    let side = width.min(height);
    let cell = (side / GRID as f64).floor().max(1.0);
    let pad = (cell * 0.16).max(0.5);
    let (left, top) = (
        (width - cell * GRID as f64) / 2.0,
        (height - cell * GRID as f64) / 2.0,
    );
    for row in 0..GRID {
        for col in 0..GRID {
            let (x, y) = (
                left + col as f64 * cell + pad,
                top + row as f64 * cell + pad,
            );
            let size = cell - 2.0 * pad;
            cr.set_source_rgba(0.5, 0.5, 0.5, 0.2);
            cr.rectangle(x, y, size, size);
            let _ = cr.fill();
            let wave = (seconds * 3.0 - (row + col) as f64 * 0.9).sin().max(0.0);
            let rgb = [FREE, REBUILD, REDOWNLOAD][(row + col) % 3];
            cr.set_source_rgba(rgb.0, rgb.1, rgb.2, 0.15 + 0.85 * wave);
            cr.rectangle(x, y, size, size);
            let _ = cr.fill();
        }
    }
}

// ──────────────────────────── Particles ────────────────────────────

/// One tile's share of the particles.
pub struct Burst {
    /// Tile bounds in the particle layer's coordinates.
    pub rect: (f64, f64, f64, f64),
    pub rgb: Rgb,
    /// Share of everything being cleaned, 0–1: bigger tiles shed more.
    pub weight: f64,
}

struct Particle {
    from: (f64, f64),
    via: (f64, f64),
    to: (f64, f64),
    /// When it leaves its tile, in seconds since the stream began.
    delay: f64,
    duration: f64,
    size: f64,
    spin: f64,
    rgb: Rgb,
}

/// Particles per second for the whole stream.
const STREAM_RATE: f64 = 70.0;

struct Flock {
    particles: Vec<Particle>,
    emitters: Vec<Burst>,
    /// New particles keep leaving the tiles while this is set.
    streaming: bool,
    /// Fractional particles owed to the next frame.
    owed: f64,
    started: Option<f64>,
    now: f64,
    /// Where the particles land; shock rings ripple there.
    target: (f64, f64),
    /// When the first particle lands, so the rings start with it.
    first_landing: f64,
    rng: Rng,
}

impl Default for Flock {
    fn default() -> Self {
        Self {
            particles: Vec::new(),
            emitters: Vec::new(),
            streaming: false,
            owed: 0.0,
            started: None,
            now: 0.0,
            target: (0.0, 0.0),
            first_landing: f64::MAX,
            rng: Rng::new(),
        }
    }
}

impl Flock {
    fn spawn(&mut self, burst_index: usize) {
        let burst = &self.emitters[burst_index];
        let (x, y, w, h) = burst.rect;
        let rgb = burst.rgb;
        let rng = &mut self.rng;
        let from = (
            rng.range(x + 6.0, x + (w - 6.0).max(7.0)),
            rng.range(y + 6.0, y + (h - 6.0).max(7.0)),
        );
        let to = (
            self.target.0 + rng.range(-5.0, 5.0),
            self.target.1 + rng.range(-3.0, 3.0),
        );
        let via = (
            (from.0 + to.0) / 2.0 + rng.range(-60.0, 60.0),
            from.1.min(to.1) - rng.range(60.0, 160.0),
        );
        let particle = Particle {
            from,
            via,
            to,
            delay: self.now,
            duration: rng.range(0.85, 1.25),
            size: rng.range(2.5, 6.0),
            spin: rng.range(-6.0, 6.0),
            rgb: mix(rgb, (1.0, 1.0, 1.0), rng.range(0.0, 0.35)),
        };
        self.first_landing = self.first_landing.min(particle.delay + particle.duration);
        self.particles.push(particle);
    }

    /// Emit what this frame owes, by tile weight, and drop landed ones.
    fn step(&mut self, elapsed: f64) {
        if self.streaming && !self.emitters.is_empty() {
            self.owed += STREAM_RATE * elapsed.min(0.1);
            let total: f64 = self.emitters.iter().map(|b| b.weight.max(0.05)).sum();
            while self.owed >= 1.0 {
                self.owed -= 1.0;
                let mut pick = self.rng.range(0.0, total);
                let mut index = 0;
                for (i, burst) in self.emitters.iter().enumerate() {
                    index = i;
                    pick -= burst.weight.max(0.05);
                    if pick <= 0.0 {
                        break;
                    }
                }
                self.spawn(index);
            }
        }
        let now = self.now;
        self.particles.retain(|p| now < p.delay + p.duration);
    }
}

/// A transparent layer laid over the cleanup card.
#[derive(Clone)]
pub struct Particles {
    area: gtk4::DrawingArea,
    flock: Rc<RefCell<Flock>>,
    tick: Rc<RefCell<Option<gtk4::TickCallbackId>>>,
}

impl Particles {
    pub fn new() -> Self {
        let area = gtk4::DrawingArea::new();
        area.set_can_target(false);
        area.set_hexpand(true);
        area.set_vexpand(true);
        let flock: Rc<RefCell<Flock>> = Rc::default();
        let drawn = flock.clone();
        area.set_draw_func(move |_, cr, _, _| {
            draw_flock(&drawn.borrow(), cr);
        });
        Self {
            area,
            flock,
            tick: Rc::default(),
        }
    }

    pub fn widget(&self) -> &gtk4::DrawingArea {
        &self.area
    }

    /// Keep particles flowing from the tiles into `target` until
    /// `stop_stream`. Does nothing without animations.
    pub fn stream(&self, bursts: Vec<Burst>, target: (f64, f64)) {
        if bursts.is_empty() || !adw::is_animations_enabled(&self.area) {
            return;
        }
        {
            let mut flock = self.flock.borrow_mut();
            flock.emitters = bursts;
            flock.target = target;
            flock.streaming = true;
            if self.tick.borrow().is_none() {
                flock.particles.clear();
                flock.started = None;
                flock.now = 0.0;
                flock.owed = 0.0;
                flock.first_landing = f64::MAX;
            }
        }
        if self.tick.borrow().is_some() {
            return;
        }
        let flock = self.flock.clone();
        let slot = self.tick.clone();
        let id = self.area.add_tick_callback(move |area, clock| {
            let now = frame_seconds(clock);
            let finished = {
                let mut flock = flock.borrow_mut();
                let started = *flock.started.get_or_insert(now);
                let before = flock.now;
                flock.now = now - started;
                let elapsed = flock.now - before;
                flock.step(elapsed);
                !flock.streaming && flock.particles.is_empty()
            };
            area.queue_draw();
            if finished {
                slot.replace(None);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        self.tick.replace(Some(id));
    }

    /// No new particles; those in flight still land.
    pub fn stop_stream(&self) {
        let mut flock = self.flock.borrow_mut();
        flock.streaming = false;
        flock.emitters.clear();
    }
}

fn bezier(a: (f64, f64), b: (f64, f64), c: (f64, f64), t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    (
        u * u * a.0 + 2.0 * u * t * b.0 + t * t * c.0,
        u * u * a.1 + 2.0 * u * t * b.1 + t * t * c.1,
    )
}

/// Electric blue the particles turn into as they reach the bolt.
const SPARK: Rgb = (0.36, 0.72, 1.0);

fn draw_flock(flock: &Flock, cr: &gtk4::cairo::Context) {
    if flock.particles.is_empty() && !flock.streaming {
        return;
    }
    let now = flock.now;
    cr.set_line_cap(gtk4::cairo::LineCap::Round);
    for p in &flock.particles {
        if now < p.delay {
            continue;
        }
        let t = ((now - p.delay) / p.duration).clamp(0.0, 1.0);
        if t >= 1.0 {
            continue;
        }
        let e = ease_in_out(t);
        let (x, y) = bezier(p.from, p.via, p.to, e);
        let size = p.size * (1.0 - 0.55 * e);
        let alpha = 1.0 - ((t - 0.88) / 0.12).max(0.0);
        // The group's colour charges into electric blue on the way in.
        let rgb = mix(p.rgb, SPARK, (e * 1.2).min(1.0).powi(2));
        // A light trail behind it that grows as it speeds up.
        let (tx, ty) = bezier(p.from, p.via, p.to, ease_in_out((t - 0.07).max(0.0)));
        cr.set_source_rgba(rgb.0, rgb.1, rgb.2, 0.35 * alpha);
        cr.set_line_width(size * 0.9);
        cr.move_to(tx, ty);
        cr.line_to(x, y);
        let _ = cr.stroke();
        // Soft glow, then the bit itself as a tiny rotated square.
        cr.set_source_rgba(rgb.0, rgb.1, rgb.2, 0.22 * alpha);
        cr.arc(x, y, size * 2.0, 0.0, 2.0 * PI);
        let _ = cr.fill();
        cr.save().ok();
        cr.translate(x, y);
        cr.rotate(p.spin * t);
        let core = mix(rgb, (1.0, 1.0, 1.0), e * 0.5);
        cr.set_source_rgba(core.0, core.1, core.2, alpha);
        cr.rectangle(-size / 2.0, -size / 2.0, size, size);
        let _ = cr.fill();
        cr.restore().ok();
    }
    // Shock rings ripple out of the bolt while the energy arrives.
    // While the stream runs they keep coming, one wave after another.
    let mut since = now - flock.first_landing;
    if flock.streaming && since > 0.0 {
        since %= 0.84;
    }
    if since > 0.0 {
        let (cx, cy) = flock.target;
        for k in 0..3 {
            let local = since - f64::from(k) * 0.28;
            if !(0.0..0.7).contains(&local) {
                continue;
            }
            let t = local / 0.7;
            cr.set_source_rgba(SPARK.0, SPARK.1, SPARK.2, 0.55 * (1.0 - t));
            cr.set_line_width(3.0 * (1.0 - t) + 0.5);
            cr.arc(cx, cy, 30.0 + 46.0 * ease_in_out(t), 0.0, 2.0 * PI);
            let _ = cr.stroke();
        }
    }
}

/// The app's bolt (24×24 icon grid) centred on (cx, cy) at `size`
/// pixels: a soft drop shadow, a white-to-ice-blue fill and a white edge,
/// like the app icon. `glow` (0..1) adds a white halo behind it.
/// The app's bolt as a small icon, for the blue cleaning buttons.
pub fn bolt_icon(size: i32) -> gtk4::DrawingArea {
    let bolt = gtk4::DrawingArea::new();
    bolt.set_content_width(size);
    bolt.set_content_height(size);
    bolt.set_valign(gtk4::Align::Center);
    bolt.set_draw_func(move |_, cr, width, height| {
        draw_bolt(
            cr,
            f64::from(width) / 2.0,
            f64::from(height) / 2.0,
            f64::from(size),
            0.0,
        );
    });
    bolt
}

pub fn draw_bolt(cr: &gtk4::cairo::Context, cx: f64, cy: f64, size: f64, glow: f64) {
    let path = |cr: &gtk4::cairo::Context| {
        cr.move_to(5.22576, 11.3294);
        cr.line_to(12.224, 2.34651);
        cr.curve_to(12.7713, 1.64397, 13.7972, 2.08124, 13.7972, 3.01707);
        cr.line_to(13.7972, 9.96994);
        cr.curve_to(13.7972, 10.5305, 14.1995, 10.985, 14.6958, 10.985);
        cr.line_to(18.0996, 10.985);
        cr.curve_to(18.8729, 10.985, 19.2851, 12.0149, 18.7742, 12.6706);
        cr.line_to(11.776, 21.6535);
        cr.curve_to(11.2287, 22.356, 10.2028, 21.9188, 10.2028, 20.9829);
        cr.line_to(10.2028, 14.0301);
        cr.curve_to(10.2028, 13.4695, 9.80048, 13.015, 9.3042, 13.015);
        cr.line_to(5.90035, 13.015);
        cr.curve_to(5.12711, 13.015, 4.71494, 11.9851, 5.22576, 11.3294);
        cr.close_path();
    };
    if glow > 0.0 {
        let halo = gtk4::cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, size * 0.75);
        halo.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.75 * glow);
        halo.add_color_stop_rgba(1.0, 0.75, 0.88, 1.0, 0.0);
        let _ = cr.set_source(&halo);
        cr.arc(cx, cy, size * 0.75, 0.0, 2.0 * PI);
        let _ = cr.fill();
    }
    cr.save().ok();
    cr.translate(cx - size / 2.0, cy - size / 2.0);
    cr.scale(size / 24.0, size / 24.0);
    cr.set_line_join(gtk4::cairo::LineJoin::Round);
    // Drop shadow in the icon's deep blue.
    cr.save().ok();
    cr.translate(0.0, 0.9);
    path(cr);
    cr.set_source_rgba(0.0, 0.227, 0.549, 0.45);
    let _ = cr.fill();
    cr.restore().ok();
    path(cr);
    let fill = gtk4::cairo::LinearGradient::new(7.0, 2.0, 17.0, 22.0);
    fill.add_color_stop_rgb(0.0, 1.0, 1.0, 1.0);
    fill.add_color_stop_rgb(0.6, 0.918, 0.961, 1.0);
    fill.add_color_stop_rgb(1.0, 0.725, 0.863, 1.0);
    let _ = cr.set_source(&fill);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
    cr.set_line_width(0.35);
    let _ = cr.stroke();
    cr.restore().ok();
}
