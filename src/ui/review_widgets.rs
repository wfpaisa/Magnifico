// Adapted from Purge CapsuleSearchField.swift and AppDropdown.swift (MIT,
// © 2026 Jithin Sabu). See THIRD_PARTY_LICENSES.md.
use crate::i18n::tr;
use gtk4::prelude::*;
use std::path::Path;

pub fn action(icon: &str, title: &str, primary: bool) -> gtk4::Button {
    let button = gtk4::Button::new();
    button.add_css_class(if primary {
        "pc-primary"
    } else {
        "pc-secondary-button"
    });
    // Header buttons keep their own height instead of stretching to the title.
    button.set_valign(gtk4::Align::Center);
    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    if primary {
        content.append(&super::cleanup_fx::bolt_icon(18));
    } else {
        content.append(&gtk4::Image::from_icon_name(icon));
    }
    content.append(&gtk4::Label::new(Some(&tr(title))));
    button.set_child(Some(&content));
    button
}
pub fn dropdown(labels: &[&str]) -> gtk4::DropDown {
    let labels: Vec<_> = labels.iter().map(|s| tr(s)).collect();
    let refs: Vec<_> = labels.iter().map(String::as_str).collect();
    let dropdown = gtk4::DropDown::from_strings(&refs);
    let factory = gtk4::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let label = gtk4::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        label.set_max_width_chars(20);
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        if let (Some(label), Some(value)) = (
            item.child().and_downcast::<gtk4::Label>(),
            item.item().and_downcast::<gtk4::StringObject>(),
        ) {
            label.set_text(&value.string());
            label.set_tooltip_text(Some(&value.string()));
        }
    });
    dropdown.set_factory(Some(&factory));
    dropdown.add_css_class("pc-dropdown");
    dropdown
}
// EXTERNAL permits wheel/touchpad scrolling without reserving scrollbar space.
pub fn horizontal_strip(child: &impl IsA<gtk4::Widget>) -> gtk4::ScrolledWindow {
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_policy(gtk4::PolicyType::External, gtk4::PolicyType::Never);
    scroll.set_propagate_natural_width(false);
    scroll.set_min_content_width(0);
    scroll.set_child(Some(child));
    scroll
}
pub fn search(prompt: &str) -> gtk4::SearchEntry {
    let entry = gtk4::SearchEntry::new();
    entry.set_placeholder_text(Some(&tr(prompt)));
    entry.add_css_class("pc-search");
    entry.set_hexpand(true);
    entry.set_width_chars(1);
    entry.set_max_width_chars(18);
    entry
}
pub fn home_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(relative) = path.strip_prefix(home) {
            return if relative.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", relative.display())
            };
        }
    }
    path.display().to_string()
}
pub fn size(bytes: u64) -> String {
    bytesize::ByteSize(bytes).to_string()
}
pub fn report(widget: &impl IsA<gtk4::Widget>, text: &str) {
    let parent = widget.root().and_downcast::<gtk4::Window>();
    if let Some(window) = parent.and_downcast::<super::MainWindow>() {
        window.record_cleanup_report(text);
        window.show_cleanup_report(text, false);
        window.save_scan_summary();
    }
}

/// Apply a delta to all views when attached to the main window.
pub async fn removed(
    widget: &impl IsA<gtk4::Widget>,
    update: crate::services::removal_update::RemovalUpdate,
) -> bool {
    if let Some(window) = widget
        .root()
        .and_downcast::<gtk4::Window>()
        .and_downcast::<super::MainWindow>()
    {
        window.apply_removed(update).await;
        true
    } else {
        false
    }
}
