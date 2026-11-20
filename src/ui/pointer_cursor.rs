//! Hand ("pointer") cursor over everything the user can click.
//!
//! GTK 4 has no CSS `cursor` property, and the app builds most of its
//! buttons and rows on the fly, so instead of tagging each widget we watch
//! the pointer at window level: the first time it rests on a clickable
//! widget, that widget gets the pointer cursor and keeps it from then on.
//!
//! Widgets with a custom `GestureClick` (treemaps, tick-anywhere rows) set
//! their cursor themselves where the gesture is added; GTK containers carry
//! internal click gestures too, so gestures alone are not a signal here.

use glib::translate::ToGlibPtr;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::ActionRowExt;

/// Install the hover watcher on a toplevel window.
pub fn install(window: &impl IsA<gtk4::Widget>) {
    let motion = gtk4::EventControllerMotion::new();
    motion.set_propagation_phase(gtk4::PropagationPhase::Capture);
    motion.connect_motion(|controller, x, y| {
        let Some(root) = controller.widget() else {
            return;
        };
        if let Some(target) = root.pick(x, y, gtk4::PickFlags::DEFAULT) {
            mark_clickable(&target);
        }
    });
    window.add_controller(motion);
}

/// Walk up from `target` and give the first clickable ancestor the
/// pointer cursor. Stops at any widget that already has its own cursor
/// (text fields, previously marked widgets).
fn mark_clickable(target: &gtk4::Widget) {
    let mut current = Some(target.clone());
    while let Some(widget) = current {
        if widget.cursor().is_some() {
            return;
        }
        if is_clickable(&widget) {
            widget.set_cursor_from_name(Some("pointer"));
            return;
        }
        current = widget.parent();
    }
}

fn is_clickable(widget: &gtk4::Widget) -> bool {
    if !widget.is_sensitive() {
        return false;
    }
    if widget.is::<gtk4::Button>()
        || widget.is::<gtk4::Switch>()
        || widget.is::<gtk4::CheckButton>()
        || widget.is::<gtk4::DropDown>()
    {
        return true;
    }
    // Menu items inside popover menus.
    if widget.css_name() == "modelbutton" {
        return true;
    }
    if let Some(row) = widget.downcast_ref::<gtk4::ListBoxRow>() {
        return row_does_something(row);
    }
    false
}

/// Rows are activatable by default even when nothing listens, so only
/// count the ones whose activation actually does something.
fn row_does_something(row: &gtk4::ListBoxRow) -> bool {
    if !row.is_activatable() {
        return false;
    }
    if row.is::<adw::ComboRow>() {
        return true;
    }
    if let Some(action_row) = row.downcast_ref::<adw::ActionRow>() {
        if action_row.activatable_widget().is_some() || has_handler(action_row, "activated") {
            return true;
        }
        // The header row that expands/collapses an AdwExpanderRow.
        if row.ancestor(adw::ExpanderRow::static_type()).is_some() && row.has_css_class("header") {
            return true;
        }
    }
    row.parent()
        .and_downcast::<gtk4::ListBox>()
        .is_some_and(|list| has_handler(&list, "row-activated"))
}

fn has_handler(object: &impl IsA<glib::Object>, signal: &str) -> bool {
    let Some(id) = glib::subclass::SignalId::lookup(signal, object.type_()) else {
        return false;
    };
    let obj: &glib::Object = object.upcast_ref();
    // SAFETY: valid object pointer and a signal id looked up on its type.
    unsafe {
        glib::gobject_ffi::g_signal_has_handler_pending(
            obj.to_glib_none().0,
            glib::translate::IntoGlib::into_glib(id),
            0,
            glib::ffi::GFALSE,
        ) != glib::ffi::GFALSE
    }
}
