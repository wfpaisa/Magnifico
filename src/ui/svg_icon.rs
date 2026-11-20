//! Paintable that draws an SVG source in the color GTK asks for.
//!
//! GTK's symbolic icons are recolored by filling every shape, which turns
//! line icons (stroke="currentColor", fill="none") into solid blobs. This
//! paintable implements `GtkSymbolicPaintable` instead: GTK hands it the
//! widget's foreground color, `currentColor` is replaced by that color and the
//! SVG is rasterized at the requested size. Used in a `gtk::Image` or an
//! `adw::StatusPage`, it follows light/dark themes, the selected-row color
//! and dimmed styles exactly like a theme icon, while keeping line icons as
//! lines.

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene};
use std::cell::{Cell, RefCell};

include!(concat!(env!("OUT_DIR"), "/embedded_icons.rs"));

/// Rasterize at this multiple of the drawn size so HiDPI screens stay sharp.
const OVERSAMPLE: f64 = 2.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SvgIcon {
        pub source: Cell<&'static str>,
        /// Last rendered texture and the (color, pixel size) it was made for.
        pub cache: RefCell<Option<(String, i32, gdk::Texture)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SvgIcon {
        const NAME: &'static str = "MagnificoSvgIcon";
        type Type = super::SvgIcon;
        type Interfaces = (gdk::Paintable, gtk4::SymbolicPaintable);
    }

    impl ObjectImpl for SvgIcon {}

    impl PaintableImpl for SvgIcon {
        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            // Without a color from GTK, draw in plain black like a theme icon.
            self.draw(snapshot, width, height, &gdk::RGBA::BLACK);
        }
    }

    impl SymbolicPaintableImpl for SvgIcon {
        fn snapshot_symbolic(
            &self,
            snapshot: &gdk::Snapshot,
            width: f64,
            height: f64,
            colors: &[gdk::RGBA],
        ) {
            let color = colors.first().copied().unwrap_or(gdk::RGBA::BLACK);
            self.draw(snapshot, width, height, &color);
        }
    }

    impl SvgIcon {
        fn draw(&self, snapshot: &gdk::Snapshot, width: f64, height: f64, color: &gdk::RGBA) {
            let side = width.min(height);
            if side <= 0.0 {
                return;
            }
            let pixels = (side * OVERSAMPLE).ceil() as i32;
            let key = format!(
                "#{:02x}{:02x}{:02x}",
                (color.red() * 255.0).round() as u8,
                (color.green() * 255.0).round() as u8,
                (color.blue() * 255.0).round() as u8
            );

            let cached = self
                .cache
                .borrow()
                .as_ref()
                .filter(|(c, p, _)| *c == key && *p == pixels)
                .map(|(_, _, texture)| texture.clone());
            let texture = match cached {
                Some(texture) => texture,
                None => {
                    let svg = sized_svg(self.source.get(), pixels).replace("currentColor", &key);
                    match gdk::Texture::from_bytes(&glib::Bytes::from_owned(svg.into_bytes())) {
                        Ok(texture) => {
                            self.cache.replace(Some((key, pixels, texture.clone())));
                            texture
                        }
                        Err(error) => {
                            tracing::warn!("Failed to render SVG icon: {}", error);
                            return;
                        }
                    }
                }
            };

            let snapshot = snapshot
                .downcast_ref::<gtk4::Snapshot>()
                .expect("GTK paints with a GtkSnapshot");
            let x = ((width - side) / 2.0) as f32;
            let y = ((height - side) / 2.0) as f32;
            // Keep the color's transparency (dimmed or insensitive widgets).
            snapshot.push_opacity(f64::from(color.alpha()));
            snapshot.append_texture(
                &texture,
                &graphene::Rect::new(x, y, side as f32, side as f32),
            );
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct SvgIcon(ObjectSubclass<imp::SvgIcon>)
        @implements gdk::Paintable, gtk4::SymbolicPaintable;
}

impl SvgIcon {
    pub fn new(source: &'static str) -> Self {
        let icon: Self = glib::Object::new();
        icon.imp().source.set(source);
        icon
    }

    /// Paintable for `sidebar/<name>` or `ui/<name>`; bare names use the sidebar.
    pub fn named(name: &str) -> Option<Self> {
        embedded_svg(name)
            .or_else(|| embedded_svg(&format!("sidebar/{name}")))
            .map(Self::new)
    }
}

fn embedded_paintable(icon: &str) -> Option<SvgIcon> {
    embedded_svg(icon).map(SvgIcon::new)
}

/// Image for a theme icon name or a `sidebar/<name>` / `ui/<name>` icon.
pub fn icon_image(icon: &str) -> gtk4::Image {
    match embedded_paintable(icon) {
        Some(paintable) => gtk4::Image::from_paintable(Some(&paintable)),
        None => gtk4::Image::from_icon_name(icon),
    }
}

/// Set a status page icon from a theme icon name or a `sidebar/<name>` / `ui/<name>` icon.
pub fn set_status_icon(status: &libadwaita::StatusPage, icon: &str) {
    match embedded_paintable(icon) {
        Some(paintable) => status.set_paintable(Some(&paintable)),
        None => status.set_icon_name(Some(icon)),
    }
}

/// Set the root `<svg>` width and height so it rasterizes sharply at `pixels`.
fn sized_svg(source: &str, pixels: i32) -> String {
    let Some(start) = source.find("<svg") else {
        return source.to_string();
    };
    let Some(len) = source[start..].find('>') else {
        return source.to_string();
    };
    let tag = &source[start..start + len];
    let mut cleaned = String::new();
    let mut rest = tag;
    // Drop existing width/height attributes, keep everything else.
    while let Some(pos) = find_attr(rest) {
        cleaned.push_str(&rest[..pos]);
        let after = &rest[pos..];
        let quote_start = after.find(['"', '\'']).unwrap_or(after.len());
        let quote = after[quote_start..].chars().next().unwrap_or('"');
        let value_end = after[quote_start + 1..]
            .find(quote)
            .map(|i| quote_start + 2 + i)
            .unwrap_or(after.len());
        rest = &after[value_end..];
    }
    cleaned.push_str(rest);
    format!(
        "{}{} width=\"{pixels}\" height=\"{pixels}\"{}",
        &source[..start],
        cleaned,
        &source[start + len..]
    )
}

/// Byte offset of a ` width=` or ` height=` attribute (with its leading space).
fn find_attr(tag: &str) -> Option<usize> {
    [
        " width=",
        " height=",
        "\twidth=",
        "\theight=",
        "\nwidth=",
        "\nheight=",
    ]
    .iter()
    .filter_map(|needle| tag.find(needle))
    .min()
}

#[cfg(test)]
mod tests {
    use super::sized_svg;

    #[test]
    fn replaces_root_size_only() {
        let svg = r#"<svg xmlns="x" viewBox="0 0 24 24" width="20" height="20"><rect width="5" height="5"/></svg>"#;
        assert_eq!(
            sized_svg(svg, 40),
            r#"<svg xmlns="x" viewBox="0 0 24 24" width="40" height="40"><rect width="5" height="5"/></svg>"#
        );
    }

    #[test]
    fn adds_size_when_missing() {
        let svg = r#"<svg viewBox="0 0 24 24"><path/></svg>"#;
        assert_eq!(
            sized_svg(svg, 20),
            r#"<svg viewBox="0 0 24 24" width="20" height="20"><path/></svg>"#
        );
    }
}
