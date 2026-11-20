//! Dashboard card that splits a scan into categories: one coloured bar for
//! what can be freed, then a row per category with a safety label, a plain
//! explanation and its size. Clicking a row opens the category's details.

use crate::i18n::{tr, tr_args};
use crate::models::{Safety, ScanCategory, ScanResult};
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub type CategoryResults = Vec<(ScanCategory, ScanResult)>;

/// Bar segments: share of the total and RGB colour.
type Segments = Rc<RefCell<Vec<(f64, (f64, f64, f64))>>>;

#[derive(Clone)]
pub struct ScanOverview {
    root: gtk4::Frame,
    headline: gtk4::Label,
    subline: gtk4::Label,
    bar: gtk4::DrawingArea,
    segments: Segments,
    list: gtk4::ListBox,
    on_open: Rc<dyn Fn(ScanCategory)>,
    /// Keep the safety labels and sizes in straight columns.
    pill_column: gtk4::SizeGroup,
    size_column: gtk4::SizeGroup,
    /// Scan animation: running flag and sweep position (0..1, loops).
    scanning: Rc<Cell<bool>>,
    phase: Rc<Cell<f64>>,
}

impl ScanOverview {
    pub fn new(on_open: impl Fn(ScanCategory) + 'static) -> Self {
        let root = gtk4::Frame::new(None);
        root.add_css_class("dashboard-section");

        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        vbox.set_margin_top(20);
        vbox.set_margin_bottom(20);
        vbox.set_margin_start(20);
        vbox.set_margin_end(20);

        let headline = gtk4::Label::new(None);
        headline.add_css_class("title-2");
        headline.set_xalign(0.0);
        let subline = gtk4::Label::new(None);
        subline.add_css_class("dim-label");
        subline.set_xalign(0.0);
        subline.set_wrap(true);

        let segments: Segments = Rc::default();
        let bar = gtk4::DrawingArea::new();
        bar.set_content_height(14);
        bar.set_hexpand(true);
        let scanning = Rc::new(Cell::new(false));
        let phase = Rc::new(Cell::new(0.0));
        let draw_segments = segments.clone();
        let draw_scanning = scanning.clone();
        let draw_phase = phase.clone();
        bar.set_draw_func(move |_, cr, width, height| {
            let (w, h) = (width as f64, height as f64);
            let radius = h / 2.0;
            rounded_rect(cr, 0.0, 0.0, w, h, radius);
            cr.clip();
            // Neutral track that reads on light and dark backgrounds.
            cr.set_source_rgba(0.5, 0.5, 0.5, 0.25);
            let _ = cr.paint();
            if draw_scanning.get() {
                draw_sweep(cr, w, h, draw_phase.get());
                return;
            }
            let mut x = 0.0;
            for (fraction, (r, g, b)) in draw_segments.borrow().iter() {
                let seg = (fraction * w).max(3.0);
                cr.set_source_rgb(*r, *g, *b);
                cr.rectangle(x, 0.0, seg, h);
                let _ = cr.fill();
                x += seg + 2.0;
            }
        });

        let list = gtk4::ListBox::new();
        list.set_selection_mode(gtk4::SelectionMode::None);
        list.add_css_class("boxed-list");

        vbox.append(&headline);
        vbox.append(&subline);
        vbox.append(&bar);
        vbox.append(&list);
        root.set_child(Some(&vbox));

        let overview = Self {
            root,
            headline,
            subline,
            bar,
            segments,
            list,
            on_open: Rc::new(on_open),
            pill_column: gtk4::SizeGroup::new(gtk4::SizeGroupMode::Horizontal),
            size_column: gtk4::SizeGroup::new(gtk4::SizeGroupMode::Horizontal),
            scanning,
            phase,
        };
        overview.show_placeholder(&tr("Scan to see what can be cleaned"));
        overview
    }

    pub fn widget(&self) -> &gtk4::Frame {
        &self.root
    }

    /// Animated "looking for files" state: a coloured sweep runs along the
    /// bar and every category shows a spinner until results arrive.
    pub fn show_scanning(&self) {
        self.headline.set_text(&tr("Looking for files…"));
        self.subline
            .set_text(&tr("Checking each category. This can take a moment."));
        self.segments.borrow_mut().clear();
        self.rebuild_rows(&[], true);
        if self.scanning.replace(true) {
            return;
        }
        let scanning = self.scanning.clone();
        let phase = self.phase.clone();
        let start = Cell::new(None::<i64>);
        self.bar.add_tick_callback(move |bar, clock| {
            if !scanning.get() {
                bar.queue_draw();
                return glib::ControlFlow::Break;
            }
            let now = clock.frame_time();
            let first = *start.get().get_or_insert(now);
            start.set(Some(first));
            // One sweep every 1.6 seconds.
            phase.set(((now - first) as f64 / 1_600_000.0).fract());
            bar.queue_draw();
            glib::ControlFlow::Continue
        });
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning.get()
    }

    pub fn show_placeholder(&self, text: &str) {
        self.scanning.set(false);
        self.headline.set_text(text);
        self.subline.set_text(&tr(
            "Each category says what it holds and whether it is safe to clean.",
        ));
        self.segments.borrow_mut().clear();
        self.bar.queue_draw();
        self.rebuild_rows(&[], false);
    }

    pub fn set_results(&self, results: &CategoryResults) {
        self.scanning.set(false);
        let total: u64 = results.iter().map(|(_, r)| r.total_size).sum();
        let safe: u64 = results
            .iter()
            .filter(|(c, _)| c.safety() == Safety::Safe)
            .map(|(_, r)| r.total_size)
            .sum();

        self.headline.set_text(&tr_args(
            "{size} can be freed",
            &[("{size}", &format_size(total))],
        ));
        self.subline.set_text(&tr_args(
            "{safe} is safe to clean. Review the rest before cleaning.",
            &[("{safe}", &format_size(safe))],
        ));

        let mut segments = self.segments.borrow_mut();
        segments.clear();
        if total > 0 {
            for (category, result) in results {
                if result.total_size > 0 {
                    segments.push((result.total_size as f64 / total as f64, category.color()));
                }
            }
        }
        drop(segments);
        self.bar.queue_draw();
        self.rebuild_rows(results, false);
    }

    fn rebuild_rows(&self, results: &[(ScanCategory, ScanResult)], scanning: bool) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for category in ScanCategory::ALL {
            let result = results.iter().find(|(c, _)| *c == category).map(|(_, r)| r);
            // Once results are in, hide categories that are turned off or found
            // nothing: an empty row only raises the question "why zero?".
            if !results.is_empty() && result.is_none_or(|r| r.total_size == 0 && r.is_empty()) {
                continue;
            }
            self.list
                .append(&self.category_row(category, result, scanning));
        }
    }

    fn category_row(
        &self,
        category: ScanCategory,
        result: Option<&ScanResult>,
        scanning: bool,
    ) -> adw::ActionRow {
        let row = adw::ActionRow::new();
        row.set_title(&tr(category.title()));
        row.set_subtitle(&tr(category.explanation()));
        row.set_subtitle_lines(2);

        let dot = gtk4::DrawingArea::new();
        dot.set_content_width(10);
        dot.set_content_height(10);
        dot.set_valign(gtk4::Align::Center);
        let (r, g, b) = category.color();
        dot.set_draw_func(move |_, cr, w, h| {
            cr.set_source_rgb(r, g, b);
            cr.arc(
                w as f64 / 2.0,
                h as f64 / 2.0,
                w.min(h) as f64 / 2.0,
                0.0,
                std::f64::consts::TAU,
            );
            let _ = cr.fill();
        });
        row.add_prefix(&dot);
        row.add_prefix(&crate::ui::icon_image(category.icon_name()));

        let pill_cell = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        let pill = safety_pill(category.safety());
        // Never expand: an expanding suffix steals space from the title and
        // shifts the column differently on every row.
        pill.set_halign(gtk4::Align::Start);
        pill_cell.set_hexpand(false);
        pill_cell.append(&pill);
        pill_cell.set_margin_start(12);
        self.pill_column.add_widget(&pill_cell);
        row.add_suffix(&pill_cell);

        let size_cell = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        size_cell.set_margin_start(12);
        size_cell.set_hexpand(false);
        self.size_column.add_widget(&size_cell);
        let size = gtk4::Label::new(None);
        size.set_xalign(1.0);
        size.set_hexpand(true);
        match result {
            _ if scanning => {
                let spinner = super::cleanup_fx::Spinner::spinning_widget(18);
                spinner.set_halign(gtk4::Align::End);
                spinner.set_hexpand(true);
                size_cell.append(&spinner);
            }
            Some(r) if r.total_size > 0 || !r.is_empty() => {
                size.set_text(&format_size(r.total_size));
                size.add_css_class("heading");
            }
            Some(_) => {
                size.set_text(&tr("Nothing found"));
                size.add_css_class("dim-label");
            }
            None => {
                size.set_text("—");
                size.add_css_class("dim-label");
            }
        }
        if !scanning {
            size_cell.append(&size);
        }
        row.add_suffix(&size_cell);

        row.add_suffix(&gtk4::Image::from_icon_name("pc-go-next-symbolic"));
        row.set_activatable(!scanning);
        let open = self.on_open.clone();
        row.connect_activated(move |_| open(category));
        row
    }
}

pub fn safety_pill(safety: Safety) -> gtk4::Label {
    let (text, class) = match safety {
        Safety::Safe => ("Safe to clean", "safety-safe"),
        Safety::ReviewFirst => ("Review first", "safety-review"),
    };
    let pill = gtk4::Label::new(Some(&tr(text)));
    pill.add_css_class("safety-pill");
    pill.add_css_class(class);
    pill.set_valign(gtk4::Align::Center);
    pill
}

pub fn format_size(bytes: u64) -> String {
    bytesize::ByteSize(bytes).to_string()
}

/// A soft band of the category colours sliding left to right.
fn draw_sweep(cr: &gtk4::cairo::Context, w: f64, h: f64, phase: f64) {
    let band = (w * 0.35).max(60.0);
    // Ease in and out so the band speeds up mid-way, like GNOME's progress.
    let eased = 0.5 - (phase * std::f64::consts::PI).cos() / 2.0;
    let x = -band + eased * (w + band);
    let gradient = gtk4::cairo::LinearGradient::new(x, 0.0, x + band, 0.0);
    let colors: Vec<_> = ScanCategory::ALL.iter().map(|c| c.color()).collect();
    let last = colors.len().saturating_sub(1).max(1) as f64;
    gradient.add_color_stop_rgba(0.0, colors[0].0, colors[0].1, colors[0].2, 0.0);
    for (i, (r, g, b)) in colors.iter().enumerate() {
        let at = 0.15 + 0.7 * i as f64 / last;
        gradient.add_color_stop_rgba(at, *r, *g, *b, 1.0);
    }
    let (r, g, b) = colors[colors.len() - 1];
    gradient.add_color_stop_rgba(1.0, r, g, b, 0.0);
    let _ = cr.set_source(&gradient);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
}

fn rounded_rect(cr: &gtk4::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
    cr.close_path();
}
