//! Transparent layer over the whole window where cells fly in a straight
//! line from the overview grid to a target widget (the sidebar Trash button).
use gtk4::{glib, prelude::*};
use std::{cell::RefCell, rc::Rc};

fn now() -> f64 {
    glib::monotonic_time() as f64 / 1e6
}

struct Flight {
    from: (f64, f64),
    to: (f64, f64),
    start: f64,
    duration: f64,
    size: f64,
    token: &'static str,
    target: glib::WeakRef<gtk4::Widget>,
    landed: bool,
}

#[derive(Default)]
struct State {
    flights: Vec<Flight>,
    tick: Option<gtk4::TickCallbackId>,
}

#[derive(Clone)]
pub struct FlightLayer {
    area: gtk4::DrawingArea,
    state: Rc<RefCell<State>>,
}

impl FlightLayer {
    pub fn new() -> Self {
        let area = gtk4::DrawingArea::new();
        area.set_can_target(false);
        area.set_hexpand(true);
        area.set_vexpand(true);
        let state: Rc<RefCell<State>> = Rc::default();
        let drawn = state.clone();
        area.set_draw_func(move |area, cr, _, _| {
            let t = now();
            let dark = area
                .root()
                .is_some_and(|r| r.has_css_class("magnifico-dark"));
            for f in &drawn.borrow().flights {
                if t < f.start {
                    continue;
                }
                let p = ((t - f.start) / f.duration).clamp(0.0, 1.0);
                if p >= 1.0 {
                    continue;
                }
                // Accelerates towards the Trash and shrinks on the way.
                let e = p * p;
                let x = f.from.0 + (f.to.0 - f.from.0) * e;
                let y = f.from.1 + (f.to.1 - f.from.1) * e;
                let token = if dark {
                    f.token.replacen("pc_", "pc_dark_", 1)
                } else {
                    f.token.to_string()
                };
                let c = area
                    .style_context()
                    .lookup_color(&token)
                    .unwrap_or_else(|| gtk4::gdk::RGBA::new(0.6, 0.6, 0.6, 1.0));
                let size = (f.size + 2.0) * (1.0 - 0.6 * e);
                let alpha = 1.0 - ((p - 0.85) / 0.15).max(0.0);
                // Short streak behind the cell, then the cell itself.
                let back = (p - 0.12).max(0.0).powi(2);
                cr.set_source_rgba(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), 0.3 * alpha);
                cr.set_line_width(size * 0.8);
                cr.move_to(f.from.0 + (f.to.0 - f.from.0) * back, f.from.1 + (f.to.1 - f.from.1) * back);
                cr.line_to(x, y);
                let _ = cr.stroke();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.35 * alpha);
                cr.rectangle(x - size / 2.0 - 1.0, y - size / 2.0 - 1.0, size + 2.0, size + 2.0);
                let _ = cr.fill();
                cr.set_source_rgba(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), alpha);
                cr.rectangle(x - size / 2.0, y - size / 2.0, size, size);
                let _ = cr.fill();
            }
        });
        Self { area, state }
    }

    pub fn widget(&self) -> &gtk4::DrawingArea {
        &self.area
    }

    /// Centre of `widget` in the layer's coordinates.
    pub fn target_point(&self, widget: &gtk4::Widget) -> (f64, f64) {
        let centre = gtk4::graphene::Point::new(
            widget.width() as f32 / 2.0,
            widget.height() as f32 / 2.0,
        );
        widget
            .compute_point(&self.area, &centre)
            .map_or((0.0, 0.0), |p| (f64::from(p.x()), f64::from(p.y())))
    }

    /// Queue one cell: `origin` is (x, y, size) in the layer's coordinates.
    pub fn launch(
        &self,
        origin: (f64, f64, f64),
        to: (f64, f64),
        token: &'static str,
        start: f64,
        duration: f64,
        target: &gtk4::Widget,
    ) {
        self.state.borrow_mut().flights.push(Flight {
            from: (origin.0, origin.1),
            to,
            start,
            duration,
            size: origin.2,
            token,
            target: target.downgrade(),
            landed: false,
        });
        self.ensure_tick();
    }

    fn ensure_tick(&self) {
        if self.state.borrow().tick.is_some() {
            return;
        }
        let state = self.state.clone();
        let id = self.area.add_tick_callback(move |area, _| {
            let t = now();
            let mut hits = Vec::new();
            let busy = {
                let mut state = state.borrow_mut();
                for f in &mut state.flights {
                    if !f.landed && t >= f.start + f.duration {
                        f.landed = true;
                        hits.push(f.target.clone());
                    }
                }
                state.flights.retain(|f| t < f.start + f.duration);
                !state.flights.is_empty()
            };
            // The Trash button lights up for a moment with every landing.
            for target in hits {
                if let Some(widget) = target.upgrade() {
                    widget.add_css_class("pc-trash-hit");
                    glib::timeout_add_local_once(std::time::Duration::from_millis(140), move || {
                        widget.remove_css_class("pc-trash-hit");
                    });
                }
            }
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
