// Adapted from Purge's SwiftUI components (MIT, © 2026 Jithin Sabu).
// https://github.com/jithin-sabu/purge-app — see THIRD_PARTY_LICENSES.md.
use crate::i18n::{format_count, tr, tr_args};
use gtk4::prelude::*;

pub fn page_header(title: &str, subtitle: &gtk4::Label, actions: &gtk4::Box) -> gtk4::Box {
    let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
    header.add_css_class("pc-page-header");
    let text = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let title = gtk4::Label::new(Some(&tr(title)));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(26);
    title.add_css_class("pc-page-title");
    subtitle.set_xalign(0.0);
    subtitle.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    subtitle.set_max_width_chars(30);
    actions.set_hexpand(false);
    subtitle.add_css_class("pc-secondary");
    text.append(&title);
    text.append(subtitle);
    header.append(&text);
    header.append(actions);
    header
}

pub fn filter_chip(icon: &str, label: &str, count: usize, active: bool) -> gtk4::ToggleButton {
    let button = gtk4::ToggleButton::new();
    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    content.append(&gtk4::Image::from_icon_name(icon));
    content.append(&gtk4::Label::new(Some(&tr_args(
        "{label} {count}",
        &[("{label}", &tr(label)), ("{count}", &format_count(count))],
    ))));
    button.set_child(Some(&content));
    button.add_css_class("pc-chip");
    if label == "Safe to Clean" {
        button.add_css_class("pc-safe");
    }
    button.set_active(active);
    button
}

#[derive(Clone, Copy)]
pub enum BadgeKind {
    Safe,
    Review,
    Check,
}

pub fn badge(kind: BadgeKind) -> gtk4::Label {
    let (text, class) = match kind {
        BadgeKind::Safe => ("Safe to Clean", "pc-safe"),
        BadgeKind::Review => ("Review first", "pc-check"),
        BadgeKind::Check => ("Check First", "pc-check"),
    };
    let label = gtk4::Label::new(Some(&tr(text)));
    label.add_css_class("pc-badge");
    label.add_css_class(class);
    label.set_valign(gtk4::Align::Center);
    label
}

pub fn icon_tile(color: &str, icon: &str) -> gtk4::Box {
    let tile = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    tile.add_css_class("pc-icon-tile");
    tile.add_css_class(color);
    tile.set_size_request(34, 34);
    tile.set_hexpand(false);
    tile.set_halign(gtk4::Align::Start);
    tile.set_valign(gtk4::Align::Center);
    let image = gtk4::Image::from_icon_name(icon);
    image.set_pixel_size(18);
    image.set_hexpand(false);
    image.set_halign(gtk4::Align::Center);
    image.set_valign(gtk4::Align::Center);
    image.set_margin_start(8);
    image.set_margin_end(8);
    image.set_margin_top(8);
    image.set_margin_bottom(8);
    tile.append(&image);
    tile
}

pub struct ItemCard {
    pub button: gtk4::Button,
    pub description: gtk4::Label,
    pub size: gtk4::Label,
    pub percent: gtk4::Label,
}

pub fn item_card(
    checkbox: Option<&gtk4::CheckButton>,
    icon: &gtk4::Box,
    title: &str,
    description: &str,
    size: &str,
    status: Option<BadgeKind>,
) -> ItemCard {
    let button = gtk4::Button::new();
    button.add_css_class("pc-item-row");
    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 14);
    if let Some(checkbox) = checkbox {
        content.append(checkbox);
    }
    content.append(icon);
    let text = gtk4::Box::new(gtk4::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let title_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let title = gtk4::Label::new(Some(&tr(title)));
    title.add_css_class("pc-row-title");
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(60);
    title.set_hexpand(true);
    title_box.set_hexpand(true);
    title.set_xalign(0.0);
    title_box.append(&title);
    if let Some(status) = status {
        title_box.append(&badge(status));
    }
    text.append(&title_box);
    let description = gtk4::Label::new(Some(&tr(description)));
    description.set_xalign(0.0);
    description.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    description.add_css_class("pc-secondary");
    text.append(&description);
    content.append(&text);
    let percent = gtk4::Label::new(None);
    percent.add_css_class("pc-tertiary");
    content.append(&percent);
    let size = gtk4::Label::new(Some(size));
    size.set_width_chars(10);
    size.set_xalign(1.0);
    size.add_css_class("pc-row-size");
    content.append(&size);
    button.set_child(Some(&content));
    ItemCard {
        button,
        description,
        size,
        percent,
    }
}
