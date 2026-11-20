//! A common loading state for disk inventories.
use crate::i18n::tr;
use gtk4::prelude::*;

#[derive(Clone)]
pub struct ScanLoading {
    stack: glib::WeakRef<gtk4::Stack>,
    phase: glib::WeakRef<gtk4::Label>,
    spinner: super::cleanup_fx::Spinner,
}
impl ScanLoading {
    pub fn new(content: &impl IsA<gtk4::Widget>, phase: &str) -> (Self, gtk4::Stack) {
        let stack = gtk4::Stack::new();
        stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        stack.set_transition_duration(200);
        stack.set_vexpand(true);
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        let loading = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        loading.set_valign(gtk4::Align::Center);
        loading.set_halign(gtk4::Align::Center);
        // Drawn on the frame clock, so it turns even when the icon theme's
        // spinner animation does not.
        let spinner = super::cleanup_fx::Spinner::new(48);
        spinner.set_spinning(true);
        loading.append(spinner.widget());
        loading.append(&gtk4::Label::new(Some(&tr("Analyzing your disk…"))));
        let phase = gtk4::Label::new(Some(&tr(phase)));
        phase.add_css_class("pc-secondary");
        loading.append(&phase);
        stack.add_named(&loading, Some("loading"));
        stack.add_named(content, Some("content"));
        (
            Self {
                stack: stack.downgrade(),
                phase: phase.downgrade(),
                spinner,
            },
            stack,
        )
    }
    pub fn wrap(container: &gtk4::Box, phase: &str) -> Self {
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, container.spacing());
        while let Some(child) = container.first_child() {
            container.remove(&child);
            content.append(&child);
        }
        let (view, stack) = Self::new(&content, phase);
        container.append(&stack);
        view
    }
    pub fn set_loading(&self, loading: bool, phase: &str) {
        if let (Some(label), Some(stack)) = (self.phase.upgrade(), self.stack.upgrade()) {
            label.set_text(&tr(phase));
            self.spinner.set_spinning(loading);
            stack.set_visible_child_name(if loading { "loading" } else { "content" });
        }
    }
}
