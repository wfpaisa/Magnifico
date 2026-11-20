//! Per-row file actions: a quick preview and "Show in Files". Both only
//! look at the file; neither changes it.

use crate::i18n::{tr, tr_args};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Largest image the preview decodes, so a huge file cannot stall the UI.
const MAX_PREVIEW_IMAGE: u64 = 40 * 1024 * 1024;
/// Bytes of a text file shown in the preview.
const TEXT_PREVIEW_BYTES: usize = 4096;

/// Preview button (files only) and "Show in Files" button for one row.
pub fn buttons(path: &Path, is_dir: bool) -> gtk4::Box {
    let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    buttons.set_valign(gtk4::Align::Center);

    if !is_dir {
        let preview = gtk4::MenuButton::new();
        preview.set_icon_name("pc-view-reveal-symbolic");
        preview.add_css_class("flat");
        preview.set_tooltip_text(Some(&tr("Preview")));
        let popover = gtk4::Popover::new();
        preview.set_popover(Some(&popover));
        let path = path.to_path_buf();
        // Built on open: rows are many, previews are rare.
        popover.connect_show(move |popover| {
            popover.set_child(Some(&preview_content(&path)));
        });
        buttons.append(&preview);
    }

    let reveal = gtk4::Button::from_icon_name("pc-folder-open-symbolic");
    reveal.add_css_class("flat");
    reveal.set_tooltip_text(Some(&tr("Show in Files")));
    let path = path.to_path_buf();
    reveal.connect_clicked(move |_| show_in_files(&path));
    buttons.append(&reveal);

    buttons
}

/// Middle click shows the item in the file manager (selected when the file
/// manager supports it); right click copies its full path. `path` is asked
/// for on click, so rows can work it out only when needed.
pub fn path_clicks(widget: &impl IsA<gtk4::Widget>, path: impl Fn() -> Option<PathBuf> + 'static) {
    let widget = widget.as_ref();
    let hint = tr("Middle-click to show in Files · Right-click to copy the path");
    let tooltip = match widget.tooltip_text() {
        Some(text) if !text.is_empty() => format!("{text}\n{hint}"),
        _ => hint,
    };
    widget.set_tooltip_text(Some(&tooltip));
    let click = gtk4::GestureClick::new();
    click.set_button(0);
    click.connect_pressed(move |gesture, _, _, _| {
        let button = gesture.current_button();
        if button != gtk4::gdk::BUTTON_MIDDLE && button != gtk4::gdk::BUTTON_SECONDARY {
            return;
        }
        gesture.set_state(gtk4::EventSequenceState::Claimed);
        let Some(path) = path() else {
            return;
        };
        let Some(widget) = gesture.widget() else {
            return;
        };
        if button == gtk4::gdk::BUTTON_MIDDLE {
            show_in_files(&path);
            return;
        }
        widget.clipboard().set_text(&path.to_string_lossy());
        widget.set_tooltip_text(Some(&tr("Path copied")));
        let weak = widget.downgrade();
        let tooltip = tooltip.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            if let Some(widget) = weak.upgrade() {
                widget.set_tooltip_text(Some(&tooltip));
            }
        });
    });
    widget.add_controller(click);
}

/// Deepest folder holding every path, for items made of many files.
pub fn common_folder<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Option<PathBuf> {
    let mut common: Option<PathBuf> = None;
    for path in paths {
        common = Some(match common {
            None => path.to_path_buf(),
            Some(current) => current
                .ancestors()
                .find(|a| path.starts_with(a))
                .map(Path::to_path_buf)?,
        });
    }
    common
}

/// Open the file manager with `path` selected. Falls back to opening the
/// containing folder when no file manager answers on D-Bus.
pub fn show_in_files(path: &Path) {
    let uri = gio::File::for_path(path).uri().to_string();
    let parent = path.parent().map(Path::to_path_buf);
    gio::bus_get(
        gio::BusType::Session,
        None::<&gio::Cancellable>,
        move |bus| {
            let Ok(bus) = bus else {
                open_folder(parent.as_deref());
                return;
            };
            let args = (vec![uri], String::new()).to_variant();
            bus.call(
                Some("org.freedesktop.FileManager1"),
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1",
                "ShowItems",
                Some(&args),
                None,
                gio::DBusCallFlags::NONE,
                5000,
                None::<&gio::Cancellable>,
                move |reply| {
                    if reply.is_err() {
                        open_folder(parent.as_deref());
                    }
                },
            );
        },
    );
}

fn open_folder(folder: Option<&Path>) {
    let Some(folder) = folder else { return };
    let uri = gio::File::for_path(folder).uri();
    if let Err(error) = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
        tracing::warn!("Cannot open {}: {error}", folder.display());
    }
}

fn open_with_default_app(path: &Path) {
    let uri = gio::File::for_path(path).uri();
    if let Err(error) = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
        tracing::warn!("Cannot open {}: {error}", path.display());
    }
}

fn preview_content(path: &Path) -> gtk4::Widget {
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(6);
    content.set_margin_end(6);
    content.set_width_request(320);

    let name = gtk4::Label::new(Some(
        &path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ));
    name.add_css_class("heading");
    name.set_xalign(0.0);
    name.set_wrap(true);
    name.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    content.append(&name);

    let meta = std::fs::symlink_metadata(path).ok();
    let (content_type, _) = gio::content_type_guess(Some(path), &[]);
    let content_type = content_type.to_string();

    if let Some(body) = preview_body(path, &content_type, meta.as_ref()) {
        content.append(&body);
    }

    let mut facts = vec![gio::content_type_get_description(&content_type).to_string()];
    if let Some(meta) = &meta {
        facts.push(super::scan_overview::format_size(meta.len()));
        if let Ok(modified) = meta.modified() {
            let date = glib::DateTime::from_unix_local(
                modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            )
            .ok()
            .and_then(|d| d.format("%x").ok())
            .map(|d| d.to_string())
            .unwrap_or_default();
            facts.push(tr_args("Modified {date}", &[("{date}", &date)]));
        }
    }
    let facts = gtk4::Label::new(Some(&facts.join(" · ")));
    facts.add_css_class("caption");
    facts.add_css_class("dim-label");
    facts.set_xalign(0.0);
    facts.set_wrap(true);
    content.append(&facts);

    let open = gtk4::Button::with_label(&tr("Open"));
    open.set_halign(gtk4::Align::End);
    let path_buf: PathBuf = path.to_path_buf();
    open.connect_clicked(move |_| open_with_default_app(&path_buf));
    content.append(&open);

    content.upcast()
}

fn preview_body(
    path: &Path,
    content_type: &str,
    meta: Option<&std::fs::Metadata>,
) -> Option<gtk4::Widget> {
    let size = meta.map(|m| m.len()).unwrap_or(0);
    if content_type.starts_with("image/") && size <= MAX_PREVIEW_IMAGE {
        let picture = gtk4::Picture::for_filename(path);
        picture.set_can_shrink(true);
        picture.set_size_request(300, 220);
        return Some(picture.upcast());
    }
    if gio::content_type_is_a(content_type, "text/plain") {
        let mut buffer = vec![0; TEXT_PREVIEW_BYTES];
        let read = std::fs::File::open(path)
            .and_then(|mut file| file.read(&mut buffer))
            .ok()?;
        buffer.truncate(read);
        let text = String::from_utf8_lossy(&buffer).into_owned();
        let view = gtk4::TextView::new();
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        view.set_wrap_mode(gtk4::WrapMode::WordChar);
        view.buffer().set_text(&text);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        scroll.set_size_request(300, 200);
        return Some(scroll.upcast());
    }
    let icon = gtk4::Image::from_gicon(&gio::content_type_get_icon(content_type));
    icon.set_pixel_size(64);
    Some(icon.upcast())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    #[test]
    fn common_folder_of_paths() {
        let paths = [
            Path::new("/h/.cache/app/a/x"),
            Path::new("/h/.cache/app/b/y"),
        ];
        assert_eq!(
            super::common_folder(paths),
            Some(PathBuf::from("/h/.cache/app"))
        );
        assert_eq!(
            super::common_folder([Path::new("/a/b")]),
            Some(PathBuf::from("/a/b"))
        );
        assert_eq!(super::common_folder(std::iter::empty::<&Path>()), None);
    }
}
