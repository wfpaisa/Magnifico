// Inspired by Purge AboutView.swift (MIT, © 2026 Jithin Sabu).
// See THIRD_PARTY_LICENSES.md.
use crate::i18n::{tr, tr_args};
use gtk4::prelude::*;
use libadwaita::{self as adw, prelude::*};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone)]
pub struct AboutPage {
    pub widget: gtk4::ScrolledWindow,
    history: gtk4::Box,
    logs: Rc<RefCell<Vec<String>>>,
}
impl AboutPage {
    pub fn new() -> Self {
        let widget = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .build();
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 24);
        content.add_css_class("pc-overview");
        widget.set_child(Some(&content));
        let icon = gtk4::Image::from_icon_name(crate::APP_ID);
        icon.set_pixel_size(72);
        content.append(&icon);
        let name = gtk4::Label::new(Some(crate::APP_NAME));
        name.add_css_class("pc-page-title");
        content.append(&name);
        let version = gtk4::Label::new(Some(&tr_args(
            "Version {version}",
            &[("{version}", crate::DISPLAY_VERSION)],
        )));
        version.add_css_class("pc-secondary");
        content.append(&version);
        let links = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
        links.set_halign(gtk4::Align::Center);
        links.append(&gtk4::LinkButton::with_label(
            "https://github.com/wfpaisa/Magnifico",
            &tr("Website"),
        ));
        links.append(&gtk4::LinkButton::with_label(
            "https://github.com/wfpaisa/Magnifico/issues",
            &tr("Report an Issue"),
        ));
        content.append(&links);
        let licenses = adw::PreferencesGroup::new();
        licenses.set_title(&tr("Licenses"));
        for (title, text) in [
            ("Magnífico · MIT", include_str!("../../LICENSE")),
            (
                "Purge · MIT · © 2026 Jithin Sabu",
                include_str!("../../THIRD_PARTY_LICENSES.md"),
            ),
        ] {
            let row = adw::ExpanderRow::new();
            row.set_title(title);
            let label = gtk4::Label::new(Some(text));
            label.set_wrap(true);
            label.set_xalign(0.0);
            label.set_selectable(true);
            label.set_margin_top(16);
            label.set_margin_bottom(16);
            label.set_margin_start(16);
            label.set_margin_end(16);
            row.add_row(&label);
            licenses.add(&row);
        }
        content.append(&licenses);
        let history = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        let title = gtk4::Label::new(Some(&tr("Cleaning History")));
        title.set_xalign(0.0);
        title.add_css_class("heading");
        content.append(&title);
        content.append(&history);
        let logs = std::fs::read(Self::path())
            .ok()
            .filter(|b| b.len() <= 8 * 1024 * 1024)
            .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
            .unwrap_or_default();
        let page = Self {
            widget,
            history,
            logs: Rc::new(RefCell::new(logs)),
        };
        page.render();
        page
    }
    fn path() -> std::path::PathBuf {
        dirs::data_local_dir()
            .unwrap_or_default()
            .join("magnifico/cleanup-history.json")
    }
    pub fn record(&self, text: String) {
        {
            let mut logs = self.logs.borrow_mut();
            logs.insert(0, text.chars().take(200_000).collect());
            logs.truncate(20);
            static WRITER: std::sync::LazyLock<async_channel::Sender<Vec<String>>> =
                std::sync::LazyLock::new(|| {
                    let (tx, rx) = async_channel::unbounded::<Vec<String>>();
                    std::thread::spawn(move || {
                        while let Ok(logs) = rx.recv_blocking() {
                            let path = AboutPage::path();
                            let saved = (|| -> Result<(), Box<dyn std::error::Error>> {
                                std::fs::create_dir_all(path.parent().unwrap())?;
                                let tmp = path.with_extension("json.tmp");
                                use std::io::Write;
                                use std::os::unix::fs::OpenOptionsExt;
                                let mut file = std::fs::OpenOptions::new()
                                    .write(true)
                                    .create(true)
                                    .truncate(true)
                                    .mode(0o600)
                                    .open(&tmp)?;
                                file.write_all(&serde_json::to_vec(&logs)?)?;
                                std::fs::rename(tmp, path)?;
                                Ok(())
                            })();
                            if let Err(e) = saved {
                                tracing::warn!("Could not save cleanup history: {e}");
                            }
                        }
                    });
                    tx
                });
            let _ = WRITER.try_send(logs.clone());
        }
        self.render();
    }
    pub fn append_latest(&self, text: &str) {
        let latest = if self.logs.borrow().is_empty() {
            String::new()
        } else {
            self.logs.borrow_mut().remove(0)
        };
        self.record(format!("{latest}\n\n{text}"));
    }

    fn render(&self) {
        while let Some(child) = self.history.first_child() {
            self.history.remove(&child);
        }
        if self.logs.borrow().is_empty() {
            self.history
                .append(&gtk4::Label::new(Some(&tr("No cleanups yet"))));
        }
        for log in self.logs.borrow().iter().take(20) {
            let group = adw::PreferencesGroup::new();
            let row = adw::ExpanderRow::new();
            row.set_title(
                log.lines()
                    .find(|s| {
                        !s.is_empty()
                            && !s.contains("CLEANUP LOG")
                            && !s.contains("REGISTRO DE LIMPIEZA")
                    })
                    .unwrap_or(&tr("Cleanup Log")),
            );
            row.set_use_markup(false);
            let page = super::CleanupLogPage::new();
            page.restore_text(log);
            page.set_size_request(-1, 320);
            row.add_row(&page);
            group.add(&row);
            self.history.append(&group);
        }
    }
}
