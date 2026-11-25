//! Settings group for the project scan: on or off, how recent a change
//! pins a project, and which folders hold projects.

use crate::i18n::tr;
use crate::storage::Storage;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

/// Choices for "Pin projects changed in the last", in days. 0 turns it off.
const AGES: [(u32, &str); 6] = [
    (0, "Off"),
    (1, "1 day"),
    (3, "3 days"),
    (7, "1 week"),
    (14, "2 weeks"),
    (30, "1 month"),
];

/// Redraws the folder rows after a change.
type Refresh = Rc<dyn Fn()>;

pub fn build(storage: Arc<Storage>) -> gtk4::Widget {
    let settings = storage.get_settings();

    let group = adw::PreferencesGroup::new();
    group.set_title(&tr("Projects"));
    group.set_description(Some(&tr(
        "The cleanup screen lists the build and dependency folders of your projects. They are never removed by scheduled cleanup.",
    )));

    let enable_row = adw::ActionRow::new();
    enable_row.set_title(&tr("Look for Project Build Folders"));
    enable_row.set_subtitle(&tr(
        "Projects with a program working inside them are always skipped",
    ));
    let enable_switch = gtk4::Switch::new();
    enable_switch.set_active(settings.inactive_projects_enabled);
    enable_switch.set_valign(gtk4::Align::Center);
    enable_row.add_suffix(&enable_switch);
    enable_row.set_activatable_widget(Some(&enable_switch));
    group.add(&enable_row);

    let age_row = adw::ComboRow::new();
    age_row.set_title(&tr("Pin Projects Changed In The Last"));
    age_row.set_subtitle(&tr("Pinned projects are shown but never picked for you"));
    let age_names: Vec<String> = AGES.iter().map(|(_, label)| tr(label)).collect();
    let age_refs: Vec<&str> = age_names.iter().map(String::as_str).collect();
    age_row.set_model(Some(&gtk4::StringList::new(&age_refs)));
    let selected = AGES
        .iter()
        .position(|(days, _)| *days >= settings.pin_recent_days)
        .unwrap_or(AGES.len() - 1);
    age_row.set_selected(selected as u32);
    age_row.set_sensitive(settings.inactive_projects_enabled);
    group.add(&age_row);

    let add_button = gtk4::Button::from_icon_name("pc-list-add-symbolic");
    add_button.add_css_class("flat");
    add_button.set_valign(gtk4::Align::Center);
    add_button.set_tooltip_text(Some(&tr("Add a folder with projects")));
    add_button.set_sensitive(settings.inactive_projects_enabled);
    group.set_header_suffix(Some(&add_button));

    let folder_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    let refresh: Rc<RefCell<Option<Refresh>>> = Rc::default();
    let refresh_fn: Refresh = {
        let storage = storage.clone();
        let group = group.clone();
        let folder_rows = folder_rows.clone();
        let refresh = refresh.clone();
        Rc::new(move || {
            for row in folder_rows.borrow_mut().drain(..) {
                group.remove(&row);
            }
            let settings = storage.get_settings();
            for folder in &settings.project_folders {
                let row = adw::ActionRow::new();
                row.set_title(&display_path(folder));
                row.add_prefix(&gtk4::Image::from_icon_name("pc-folder-symbolic"));
                row.set_sensitive(settings.inactive_projects_enabled);
                let remove = gtk4::Button::from_icon_name("pc-user-trash-symbolic");
                remove.add_css_class("flat");
                remove.set_valign(gtk4::Align::Center);
                remove.set_tooltip_text(Some(&tr("Stop searching this folder")));
                let storage = storage.clone();
                let refresh = refresh.clone();
                let folder = folder.clone();
                remove.connect_clicked(move |_| {
                    update(&storage, |s| s.project_folders.retain(|f| *f != folder));
                    // After the click is done: the refresh removes this button.
                    if let Some(refresh) = refresh.borrow().clone() {
                        glib::idle_add_local_once(move || refresh());
                    }
                });
                row.add_suffix(&remove);
                group.add(&row);
                folder_rows.borrow_mut().push(row);
            }
            if settings.project_folders.is_empty() {
                let row = adw::ActionRow::new();
                row.set_title(&tr("No folders. Add one to look for projects."));
                row.add_css_class("dim-label");
                group.add(&row);
                folder_rows.borrow_mut().push(row);
            }
        })
    };
    refresh.replace(Some(refresh_fn.clone()));
    refresh_fn();

    {
        let storage = storage.clone();
        let age_row = age_row.clone();
        let add_button = add_button.clone();
        let refresh = refresh_fn.clone();
        enable_switch.connect_active_notify(move |switch| {
            let enabled = switch.is_active();
            update(&storage, |s| s.inactive_projects_enabled = enabled);
            age_row.set_sensitive(enabled);
            add_button.set_sensitive(enabled);
            refresh();
        });
    }
    {
        let storage = storage.clone();
        age_row.connect_selected_notify(move |row| {
            if let Some((days, _)) = AGES.get(row.selected() as usize) {
                update(&storage, |s| s.pin_recent_days = *days);
            }
        });
    }
    add_button.connect_clicked(move |button| {
        let window = button
            .root()
            .and_then(|root| root.downcast::<gtk4::Window>().ok());
        let chooser = gtk4::FileChooserNative::new(
            Some(&tr("Choose a Folder with Projects")),
            window.as_ref(),
            gtk4::FileChooserAction::SelectFolder,
            Some(&tr("Add")),
            Some(&tr("Cancel")),
        );
        if let Some(home) = dirs::home_dir() {
            let _ = chooser.set_current_folder(Some(&gio::File::for_path(home)));
        }
        let storage = storage.clone();
        let refresh = refresh_fn.clone();
        chooser.run_async(move |chooser, response| {
            if response == gtk4::ResponseType::Accept {
                if let Some(path) = chooser.file().and_then(|file| file.path()) {
                    update(&storage, |s| {
                        if !s.project_folders.contains(&path) {
                            s.project_folders.push(path);
                        }
                    });
                    refresh();
                }
            }
            chooser.destroy();
        });
    });

    group.upcast()
}

fn update(storage: &Storage, change: impl FnOnce(&mut crate::models::AppSettings)) {
    if let Err(error) = storage.update_settings(change) {
        tracing::warn!("Failed to update inactive project settings: {error}");
    }
}

/// `~/code` instead of `/home/name/code`.
fn display_path(path: &Path) -> String {
    match dirs::home_dir() {
        Some(home) if path == home => tr("Home folder"),
        Some(home) => match path.strip_prefix(&home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}
