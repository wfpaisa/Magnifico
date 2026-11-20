use crate::models::ColorScheme;
use gtk4::{gdk, prelude::*};
use libadwaita as adw;

const LIGHT_WINDOW_BG: gdk::RGBA = gdk::RGBA::new(250.0 / 255.0, 250.0 / 255.0, 251.0 / 255.0, 1.0);
const LIGHT_WINDOW_FG: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 6.0 / 255.0, 0.8);
const LIGHT_VIEW_BG: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
const LIGHT_HEADERBAR_BG: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
const LIGHT_SIDEBAR_BG: gdk::RGBA =
    gdk::RGBA::new(235.0 / 255.0, 235.0 / 255.0, 237.0 / 255.0, 1.0);
const LIGHT_SECONDARY_SIDEBAR_BG: gdk::RGBA =
    gdk::RGBA::new(243.0 / 255.0, 243.0 / 255.0, 245.0 / 255.0, 1.0);
const LIGHT_CARD_BG: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
const LIGHT_DIALOG_BG: gdk::RGBA = gdk::RGBA::new(250.0 / 255.0, 250.0 / 255.0, 251.0 / 255.0, 1.0);
const LIGHT_POPOVER_BG: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
const DARK_WINDOW_BG: gdk::RGBA = gdk::RGBA::new(34.0 / 255.0, 34.0 / 255.0, 38.0 / 255.0, 1.0);
const DARK_WINDOW_FG: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
const DARK_VIEW_BG: gdk::RGBA = gdk::RGBA::new(29.0 / 255.0, 29.0 / 255.0, 32.0 / 255.0, 1.0);
const DARK_HEADERBAR_BG: gdk::RGBA = gdk::RGBA::new(46.0 / 255.0, 46.0 / 255.0, 50.0 / 255.0, 1.0);
const DARK_SIDEBAR_BG: gdk::RGBA = gdk::RGBA::new(46.0 / 255.0, 46.0 / 255.0, 50.0 / 255.0, 1.0);
const DARK_SECONDARY_SIDEBAR_BG: gdk::RGBA =
    gdk::RGBA::new(40.0 / 255.0, 40.0 / 255.0, 44.0 / 255.0, 1.0);
const DARK_CARD_BG: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 0.08);
const DARK_DIALOG_BG: gdk::RGBA = gdk::RGBA::new(54.0 / 255.0, 54.0 / 255.0, 58.0 / 255.0, 1.0);
const DARK_POPOVER_BG: gdk::RGBA = gdk::RGBA::new(54.0 / 255.0, 54.0 / 255.0, 58.0 / 255.0, 1.0);
const DEFAULT_ACCENT_BG: gdk::RGBA =
    gdk::RGBA::new(53.0 / 255.0, 132.0 / 255.0, 228.0 / 255.0, 1.0);
const DEFAULT_ACCENT_FG: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ThemeSnapshot {
    pub window_bg: gdk::RGBA,
    pub window_fg: gdk::RGBA,
    pub view_bg: gdk::RGBA,
    pub view_fg: gdk::RGBA,
    pub headerbar_bg: gdk::RGBA,
    pub headerbar_fg: gdk::RGBA,
    pub sidebar_bg: gdk::RGBA,
    pub sidebar_fg: gdk::RGBA,
    pub secondary_sidebar_bg: gdk::RGBA,
    pub secondary_sidebar_fg: gdk::RGBA,
    pub card_bg: gdk::RGBA,
    pub card_fg: gdk::RGBA,
    pub dialog_bg: gdk::RGBA,
    pub dialog_fg: gdk::RGBA,
    pub popover_bg: gdk::RGBA,
    pub popover_fg: gdk::RGBA,
    pub tooltip_bg: gdk::RGBA,
    pub tooltip_fg: gdk::RGBA,
    pub accent_bg: gdk::RGBA,
    pub accent_fg: gdk::RGBA,
    pub accent_standalone: gdk::RGBA,
    pub success: gdk::RGBA,
    pub warning: gdk::RGBA,
    pub error: gdk::RGBA,
    pub destructive: gdk::RGBA,
    pub primary_text: gdk::RGBA,
    pub secondary_text: gdk::RGBA,
    pub border: gdk::RGBA,
    pub divider: gdk::RGBA,
    pub shade: gdk::RGBA,
    pub selection_bg: gdk::RGBA,
    pub selection_fg: gdk::RGBA,
    pub button_bg: gdk::RGBA,
    pub button_fg: gdk::RGBA,
    pub button_border: gdk::RGBA,
    pub flat_button_hover: gdk::RGBA,
    pub flat_button_pressed: gdk::RGBA,
    pub icon_button_fg: gdk::RGBA,
    pub text_button_fg: gdk::RGBA,
    pub link_fg: gdk::RGBA,
    pub entry_bg: gdk::RGBA,
    pub entry_fg: gdk::RGBA,
    pub entry_border: gdk::RGBA,
    pub focus_ring: gdk::RGBA,
    pub active_state: gdk::RGBA,
    pub hover_state: gdk::RGBA,
    pub pressed_state: gdk::RGBA,
    pub overlay_bg: gdk::RGBA,
    pub is_dark: bool,
    pub is_high_contrast: bool,
    pub reduced_motion: bool,
}

impl ThemeSnapshot {
    pub fn from_widget(widget: &impl IsA<gtk4::Widget>) -> Self {
        let widget = widget.as_ref();
        let style_manager = adw::StyleManager::default();
        let is_dark = style_manager.is_dark();
        let is_high_contrast = style_manager.is_high_contrast();
        let reduced_motion = !adw::is_animations_enabled(widget);

        let window_fg = lookup_color(widget, "window_fg_color", fallback_window_fg(is_dark));
        let view_fg = lookup_color(widget, "view_fg_color", fallback_view_fg(is_dark));
        let accent_bg = lookup_color(widget, "accent_bg_color", DEFAULT_ACCENT_BG);
        let error = lookup_color(widget, "error_bg_color", accent_bg);
        let primary_text = view_fg;
        let secondary_text = with_alpha(&view_fg, if is_high_contrast { 0.82 } else { 0.72 });
        let border = lookup_color(widget, "border_color", with_alpha(&window_fg, 0.12));
        let shade = lookup_color(
            widget,
            "shade_color",
            if is_dark {
                gdk::RGBA::new(0.0, 0.0, 0.0, 0.22)
            } else {
                gdk::RGBA::new(0.0, 0.0, 0.0, 0.08)
            },
        );
        let hover_state = with_alpha(&window_fg, if is_high_contrast { 0.14 } else { 0.08 });
        let pressed_state = with_alpha(&window_fg, if is_high_contrast { 0.20 } else { 0.12 });

        Self {
            window_bg: lookup_color(widget, "window_bg_color", fallback_window_bg(is_dark)),
            window_fg,
            view_bg: lookup_color(widget, "view_bg_color", fallback_view_bg(is_dark)),
            view_fg,
            headerbar_bg: lookup_color(
                widget,
                "headerbar_bg_color",
                fallback_headerbar_bg(is_dark),
            ),
            headerbar_fg: lookup_color(
                widget,
                "headerbar_fg_color",
                fallback_headerbar_fg(is_dark),
            ),
            sidebar_bg: lookup_color(widget, "sidebar_bg_color", fallback_sidebar_bg(is_dark)),
            sidebar_fg: lookup_color(widget, "sidebar_fg_color", fallback_sidebar_fg(is_dark)),
            secondary_sidebar_bg: lookup_color(
                widget,
                "secondary_sidebar_bg_color",
                fallback_secondary_sidebar_bg(is_dark),
            ),
            secondary_sidebar_fg: lookup_color(
                widget,
                "secondary_sidebar_fg_color",
                fallback_secondary_sidebar_fg(is_dark),
            ),
            card_bg: lookup_color(widget, "card_bg_color", fallback_card_bg(is_dark)),
            card_fg: lookup_color(widget, "card_fg_color", fallback_card_fg(is_dark)),
            dialog_bg: lookup_color(widget, "dialog_bg_color", fallback_dialog_bg(is_dark)),
            dialog_fg: lookup_color(widget, "dialog_fg_color", fallback_dialog_fg(is_dark)),
            popover_bg: lookup_color(widget, "popover_bg_color", fallback_popover_bg(is_dark)),
            popover_fg: lookup_color(widget, "popover_fg_color", fallback_popover_fg(is_dark)),
            tooltip_bg: lookup_color(widget, "popover_bg_color", fallback_popover_bg(is_dark)),
            tooltip_fg: lookup_color(widget, "popover_fg_color", fallback_popover_fg(is_dark)),
            accent_bg,
            accent_fg: lookup_color(widget, "accent_fg_color", DEFAULT_ACCENT_FG),
            // Custom Cairo drawings cannot inherit the CSS foreground like a
            // symbolic GtkImage. Prefer libadwaita's live accent RGBA when the
            // runtime provides it (1.6+), with the CSS value as a fallback for
            // older supported runtimes.
            accent_standalone: runtime_accent_color(&style_manager)
                .unwrap_or_else(|| accent_standalone_color(&accent_bg)),
            success: lookup_color(widget, "success_bg_color", accent_bg),
            warning: lookup_color(widget, "warning_bg_color", accent_bg),
            error,
            destructive: lookup_color(widget, "destructive_bg_color", error),
            primary_text,
            secondary_text,
            border,
            divider: with_alpha(&border, if is_high_contrast { 1.0 } else { 0.8 }),
            shade,
            selection_bg: with_alpha(&accent_bg, if is_high_contrast { 0.28 } else { 0.18 }),
            selection_fg: lookup_color(widget, "accent_fg_color", DEFAULT_ACCENT_FG),
            button_bg: lookup_color(widget, "card_bg_color", fallback_card_bg(is_dark)),
            button_fg: lookup_color(widget, "card_fg_color", fallback_card_fg(is_dark)),
            button_border: with_alpha(&border, if is_high_contrast { 1.0 } else { 0.85 }),
            flat_button_hover: hover_state,
            flat_button_pressed: pressed_state,
            icon_button_fg: window_fg,
            text_button_fg: accent_bg,
            link_fg: accent_bg,
            entry_bg: lookup_color(widget, "view_bg_color", fallback_view_bg(is_dark)),
            entry_fg: view_fg,
            entry_border: with_alpha(&window_fg, if is_high_contrast { 0.28 } else { 0.14 }),
            focus_ring: with_alpha(&accent_bg, 0.95),
            active_state: with_alpha(&accent_bg, if is_high_contrast { 0.26 } else { 0.16 }),
            hover_state,
            pressed_state,
            overlay_bg: with_alpha(
                &lookup_color(widget, "dialog_bg_color", fallback_dialog_bg(is_dark)),
                if is_dark { 0.92 } else { 0.96 },
            ),
            is_dark,
            is_high_contrast,
            reduced_motion,
        }
    }
}

pub fn apply_color_scheme(style_manager: &adw::StyleManager, scheme: ColorScheme) {
    let color_scheme = match scheme {
        ColorScheme::System => adw::ColorScheme::Default,
        ColorScheme::Light => adw::ColorScheme::ForceLight,
        ColorScheme::Dark => adw::ColorScheme::ForceDark,
    };

    style_manager.set_color_scheme(color_scheme);
}

/// GTK 4.24 passes `backdrop-filter` blur to the compositor, and only GNOME
/// Shell 51 or newer on Wayland actually blurs it. Anywhere else the window
/// would just turn see-through, so the blur is offered only when GTK, the
/// session and the shell all support it. A bundled (AppImage) or newer GTK
/// on an older shell therefore keeps the blur off. Checked once.
pub fn blur_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(detect_blur_support)
}

fn gtk_has_backdrop_blur(major: u32, minor: u32) -> bool {
    major > 4 || (major == 4 && minor >= 24)
}

fn shell_has_backdrop_blur(version: &str) -> bool {
    version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 51)
}

fn detect_blur_support() -> bool {
    if !gtk_has_backdrop_blur(gtk4::major_version(), gtk4::minor_version()) {
        return false;
    }
    let on_wayland = gdk::Display::default()
        .is_some_and(|display| display.type_().name() == "GdkWaylandDisplay");
    let on_gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
        desktop
            .split(':')
            .any(|name| name.eq_ignore_ascii_case("GNOME"))
    });
    if !on_wayland || !on_gnome {
        return false;
    }
    gnome_shell_version().is_some_and(|version| shell_has_backdrop_blur(&version))
}

/// GNOME Shell's version string (for example "51.1") from the session bus.
fn gnome_shell_version() -> Option<String> {
    let proxy = gio::DBusProxy::for_bus_sync(
        gio::BusType::Session,
        gio::DBusProxyFlags::DO_NOT_CONNECT_SIGNALS | gio::DBusProxyFlags::DO_NOT_AUTO_START,
        None,
        "org.gnome.Shell",
        "/org/gnome/Shell",
        "org.gnome.Shell",
        gio::Cancellable::NONE,
    )
    .ok()?;
    proxy.set_default_timeout(1000);
    proxy
        .cached_property("ShellVersion")?
        .str()
        .map(str::to_owned)
}

/// The whole window turns translucent and the desktop behind it is
/// blurred: the sidebar shows it plainly, the content side through a tint
/// that keeps text readable.
const BLUR_CSS: &str = "
window.magnifico-blur,
window.magnifico-blur.background {
    background-color: transparent;
    backdrop-filter: blur(10px);
}
window.magnifico-blur .magnifico-window-layout,
window.magnifico-blur .magnifico-sidebar,
window.magnifico-blur headerbar.magnifico-sidebar-header,
window.magnifico-blur .magnifico-sidebar .navigation-sidebar {
    background: transparent;
}
/* One tint for the whole content side. The header, pages and status bar
   sit inside it, so they stay clear instead of stacking the tint. */
window.magnifico-blur .magnifico-content.magnifico-view {
    background-color: rgba(20, 20, 20, 0.6);
}
window.magnifico-blur.magnifico-light .magnifico-content.magnifico-view {
    background-color: rgba(250, 250, 251, 0.92);
}
/* A light sidebar over a dark desktop would hide its dark icons. */
window.magnifico-blur.magnifico-light .magnifico-sidebar,
window.magnifico-blur.magnifico-light headerbar.magnifico-sidebar-header {
    background-color: rgba(236, 236, 240, 0.72);
}
window.magnifico-blur headerbar.magnifico-content-header,
window.magnifico-blur .magnifico-content .magnifico-view {
    background-color: transparent;
}
";

/// Register the blur styles once at start-up. They only take effect on a
/// window with the `magnifico-blur` class.
pub fn install_blur_css(display: &gdk::Display) {
    if !blur_supported() {
        return;
    }
    let provider = gtk4::CssProvider::new();
    #[allow(deprecated)]
    provider.load_from_data(BLUR_CSS);
    gtk4::style_context_add_provider_for_display(
        display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
}

/// libadwaita's light palette. A user stylesheet (~/.config/gtk-4.0/gtk.css)
/// that hard-codes dark colours would otherwise leave white text on the
/// light theme, so these are re-applied above it while the app is light.
const LIGHT_PALETTE: &[(&str, &str)] = &[
    ("window-bg-color", "#fafafb"),
    ("window-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("view-bg-color", "#ffffff"),
    ("view-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("headerbar-bg-color", "#ffffff"),
    ("headerbar-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("headerbar-border-color", "rgba(0, 0, 6, 0.8)"),
    ("headerbar-backdrop-color", "#fafafb"),
    ("headerbar-shade-color", "rgba(0, 0, 6, 0.12)"),
    ("headerbar-darker-shade-color", "rgba(0, 0, 6, 0.12)"),
    ("sidebar-bg-color", "#ebebed"),
    ("sidebar-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("sidebar-backdrop-color", "#f2f2f4"),
    ("sidebar-border-color", "rgba(0, 0, 6, 0.07)"),
    ("sidebar-shade-color", "rgba(0, 0, 6, 0.07)"),
    ("secondary-sidebar-bg-color", "#f3f3f5"),
    ("secondary-sidebar-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("secondary-sidebar-backdrop-color", "#f6f6fa"),
    ("secondary-sidebar-border-color", "rgba(0, 0, 6, 0.07)"),
    ("secondary-sidebar-shade-color", "rgba(0, 0, 6, 0.07)"),
    ("card-bg-color", "#ffffff"),
    ("card-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("card-shade-color", "rgba(0, 0, 6, 0.07)"),
    ("dialog-bg-color", "#fafafb"),
    ("dialog-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("popover-bg-color", "#ffffff"),
    ("popover-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("popover-shade-color", "rgba(0, 0, 6, 0.07)"),
    ("thumbnail-bg-color", "#ffffff"),
    ("thumbnail-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("overview-bg-color", "#f3f3f5"),
    ("overview-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("active-toggle-bg-color", "#ffffff"),
    ("active-toggle-fg-color", "rgba(0, 0, 6, 0.8)"),
    ("shade-color", "rgba(0, 0, 6, 0.07)"),
    ("scrollbar-outline-color", "#ffffff"),
    ("destructive-color", "#c30000"),
    ("success-color", "#007c3d"),
    ("warning-color", "#905400"),
    ("error-color", "#c30000"),
];

fn light_palette_css() -> String {
    let mut css = String::from(":root {\n");
    for (name, value) in LIGHT_PALETTE {
        css.push_str(&format!("  --{name}: {value};\n"));
    }
    css.push_str("}\n");
    for (name, value) in LIGHT_PALETTE {
        css.push_str(&format!(
            "@define-color {} {value};\n",
            name.replace('-', "_")
        ));
    }
    // A user stylesheet may paint every boxed list for a dark desktop; keep
    // the light theme's cards white. This provider sits above user styles.
    css.push_str(
        ".pc-settings list.boxed-list { background-color: #FFFFFF; }\n\
         .pc-settings list.boxed-list > row { border-color: #E6E3DF; }\n",
    );
    css
}

thread_local! {
    static LIGHT_PROVIDER: std::cell::RefCell<Option<gtk4::CssProvider>> =
        const { std::cell::RefCell::new(None) };
}

/// Keep the light palette above user stylesheets only while the app is light.
pub fn sync_light_palette() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let light = !adw::StyleManager::default().is_dark();
    LIGHT_PROVIDER.with(|slot| {
        let mut slot = slot.borrow_mut();
        match (light, slot.as_ref()) {
            (true, None) => {
                let provider = gtk4::CssProvider::new();
                #[allow(deprecated)]
                provider.load_from_data(&light_palette_css());
                gtk4::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk4::STYLE_PROVIDER_PRIORITY_USER + 1,
                );
                *slot = Some(provider);
            }
            (false, Some(provider)) => {
                gtk4::style_context_remove_provider_for_display(&display, provider);
                *slot = None;
            }
            _ => {}
        }
    });
}

/// Turn the blur on or off for one window.
pub fn apply_blur(window: &impl IsA<gtk4::Widget>, enabled: bool) {
    set_class(
        window.as_ref(),
        "magnifico-blur",
        enabled && blur_supported(),
    );
}

pub fn sync_runtime_classes(widget: &impl IsA<gtk4::Widget>) {
    let widget = widget.as_ref();
    let style_manager = adw::StyleManager::default();

    set_class(widget, "magnifico-dark", style_manager.is_dark());
    set_class(widget, "magnifico-light", !style_manager.is_dark());
    set_class(
        widget,
        "magnifico-high-contrast",
        style_manager.is_high_contrast(),
    );
    set_class(
        widget,
        "magnifico-reduced-motion",
        !adw::is_animations_enabled(widget),
    );
}

pub fn transition_duration(widget: &impl IsA<gtk4::Widget>, default_ms: u32) -> u32 {
    if adw::is_animations_enabled(widget) {
        default_ms
    } else {
        0
    }
}

pub fn rgba_to_cairo(color: &gdk::RGBA) -> (f64, f64, f64, f64) {
    (
        color.red() as f64,
        color.green() as f64,
        color.blue() as f64,
        color.alpha() as f64,
    )
}

/// Looks up a named color from the widget's style context.
///
/// **Deprecated API:** `gtk_style_context_lookup_color` was deprecated in GTK 4.10.
/// There is no direct GTK4 replacement for querying arbitrary named CSS colors from a widget.
/// `Widget::color()` only returns the foreground color. Until an equivalent replacement
/// is available in the Rust bindings, this function is retained with the deprecation allowed.
#[allow(deprecated)]
fn lookup_color(widget: &gtk4::Widget, name: &str, fallback: gdk::RGBA) -> gdk::RGBA {
    widget
        .style_context()
        .lookup_color(name)
        .unwrap_or(fallback)
}

fn with_alpha(color: &gdk::RGBA, alpha: f32) -> gdk::RGBA {
    color.with_alpha(alpha)
}

fn runtime_accent_color(style_manager: &adw::StyleManager) -> Option<gdk::RGBA> {
    style_manager
        .find_property("accent-color-rgba")
        .map(|_| style_manager.property::<gdk::RGBA>("accent-color-rgba"))
}

fn accent_standalone_color(color: &gdk::RGBA) -> gdk::RGBA {
    *color
}

fn set_class(widget: &gtk4::Widget, css_class: &str, enabled: bool) {
    if enabled {
        widget.add_css_class(css_class);
    } else {
        widget.remove_css_class(css_class);
    }
}

const fn fallback_window_bg(is_dark: bool) -> gdk::RGBA {
    if is_dark {
        DARK_WINDOW_BG
    } else {
        LIGHT_WINDOW_BG
    }
}

const fn fallback_window_fg(is_dark: bool) -> gdk::RGBA {
    if is_dark {
        DARK_WINDOW_FG
    } else {
        LIGHT_WINDOW_FG
    }
}

const fn fallback_view_bg(is_dark: bool) -> gdk::RGBA {
    if is_dark {
        DARK_VIEW_BG
    } else {
        LIGHT_VIEW_BG
    }
}

const fn fallback_view_fg(is_dark: bool) -> gdk::RGBA {
    fallback_window_fg(is_dark)
}

const fn fallback_headerbar_bg(is_dark: bool) -> gdk::RGBA {
    if is_dark {
        DARK_HEADERBAR_BG
    } else {
        LIGHT_HEADERBAR_BG
    }
}

const fn fallback_headerbar_fg(is_dark: bool) -> gdk::RGBA {
    fallback_window_fg(is_dark)
}

const fn fallback_sidebar_bg(is_dark: bool) -> gdk::RGBA {
    if is_dark {
        DARK_SIDEBAR_BG
    } else {
        LIGHT_SIDEBAR_BG
    }
}

const fn fallback_sidebar_fg(is_dark: bool) -> gdk::RGBA {
    fallback_window_fg(is_dark)
}

const fn fallback_secondary_sidebar_bg(is_dark: bool) -> gdk::RGBA {
    if is_dark {
        DARK_SECONDARY_SIDEBAR_BG
    } else {
        LIGHT_SECONDARY_SIDEBAR_BG
    }
}

const fn fallback_secondary_sidebar_fg(is_dark: bool) -> gdk::RGBA {
    fallback_window_fg(is_dark)
}

const fn fallback_card_bg(is_dark: bool) -> gdk::RGBA {
    if is_dark {
        DARK_CARD_BG
    } else {
        LIGHT_CARD_BG
    }
}

const fn fallback_card_fg(is_dark: bool) -> gdk::RGBA {
    fallback_window_fg(is_dark)
}

const fn fallback_dialog_bg(is_dark: bool) -> gdk::RGBA {
    if is_dark {
        DARK_DIALOG_BG
    } else {
        LIGHT_DIALOG_BG
    }
}

const fn fallback_dialog_fg(is_dark: bool) -> gdk::RGBA {
    fallback_window_fg(is_dark)
}

const fn fallback_popover_bg(is_dark: bool) -> gdk::RGBA {
    if is_dark {
        DARK_POPOVER_BG
    } else {
        LIGHT_POPOVER_BG
    }
}

const fn fallback_popover_fg(is_dark: bool) -> gdk::RGBA {
    fallback_window_fg(is_dark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_needs_gtk_4_24_or_newer() {
        assert!(!gtk_has_backdrop_blur(4, 20));
        assert!(gtk_has_backdrop_blur(4, 24));
        assert!(gtk_has_backdrop_blur(5, 0));
    }

    #[test]
    fn blur_needs_gnome_shell_51_or_newer() {
        assert!(!shell_has_backdrop_blur("50.2"));
        assert!(shell_has_backdrop_blur("51.1"));
        assert!(shell_has_backdrop_blur("52.beta"));
        assert!(!shell_has_backdrop_blur(""));
        assert!(!shell_has_backdrop_blur("unknown"));
    }
}
