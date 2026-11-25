//! Shared review and execution for cached cleanup inventories.
use super::{
    clean_progress::CleanProgressWindow, clean_scan_store::CleanScanStore, removal_dialog,
};
use crate::{
    i18n::{tr, tr_args},
    models::{Safety, ScanResult},
    services::{
        clean_sources::{self, CleanItem, CleanPageKind},
        CleanOptions, CleanProgress, Cleaner,
    },
    storage::Storage,
};
use gtk4::prelude::*;
use libadwaita::prelude::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

type Selection = Vec<(CleanPageKind, Arc<CleanItem>)>;
const KINDS: [CleanPageKind; 3] = [
    CleanPageKind::AppCaches,
    CleanPageKind::DevTools,
    CleanPageKind::System,
];

pub fn safe_items(store: &CleanScanStore) -> Selection {
    KINDS
        .into_iter()
        .flat_map(|kind| {
            store.items(kind).into_iter().flat_map(move |items| {
                items
                    .iter()
                    .filter(|i| i.safety == Safety::Safe)
                    .map(|i| (kind, i.clone()))
                    .collect::<Vec<_>>()
            })
        })
        .collect()
}

pub fn confirm(
    window: &super::MainWindow,
    storage: Arc<Storage>,
    store: CleanScanStore,
    items: Selection,
    finished: impl FnOnce() + 'static,
) {
    if items.is_empty() {
        let dialog = super::dialogs::message_dialog(
            Some(window.upcast_ref()),
            &tr("Nothing to Clean"),
            &tr("No safe items to clean."),
        );
        dialog.add_response("close", &tr("Close"));
        dialog.present();
        finished();
        return;
    }
    let window = window.clone();
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let settings = storage.get_settings();
        let trash = settings.removal_method == crate::models::RemovalMethod::Trash
            && items.iter().all(|(_, i)| i.command.is_none())
            && items
                .iter()
                .map(|(_, i)| {
                    // A whole folder is one entry in the trash.
                    let scan = &i.scan;
                    scan.whole_dirs.len()
                        + scan
                            .files
                            .iter()
                            .filter(|f| !scan.whole_dirs.iter().any(|dir| f.path.starts_with(dir)))
                            .count()
                })
                .sum::<usize>()
                <= crate::services::TRASH_MAX_FILES;
        let cards = items
            .iter()
            .map(|(_, item)| removal_dialog::RemovalItem {
                name: tr(&item.title),
                source: item
                    .command
                    .as_ref()
                    .map(|c| {
                        format!(
                            "{} {}\n{}",
                            c.program,
                            c.args.join(" "),
                            if c.admin {
                                tr("Administrator approval is required.")
                            } else {
                                String::new()
                            }
                        )
                    })
                    .unwrap_or_else(|| {
                        item.scan
                            .files
                            .iter()
                            .filter_map(|f| f.path.parent())
                            .collect::<std::collections::BTreeSet<_>>()
                            .iter()
                            .take(8)
                            .map(|p| super::review_widgets::home_path(p))
                            .collect::<Vec<_>>()
                            .join("\n")
                    }),
                bytes: item.bytes,
                safe: item.safety == Safety::Safe,
            })
            .collect();
        let notice = tr(if items.iter().any(|(_, i)| i.command.is_some()) {
            "Commands remove data directly. Administrator approval may be required."
        } else if trash {
            "Files can be restored from the trash. Space is freed only when you empty it"
        } else {
            "Files will be permanently deleted. This cannot be undone."
        });
        let options = CleanOptions {
            max_files_per_operation: settings.max_files_per_operation,
            max_size_per_operation: settings.max_size_per_operation,
            removal_method: settings.removal_method,
            secure_browser_data: settings.secure_browser_data,
        };
        let _ = tx.send_blocking((items, cards, trash, notice, options));
    });
    glib::spawn_future_local(async move {
        let Ok((items, cards, trash, notice, options)) = rx.recv().await else {
            finished();
            return;
        };
        removal_dialog::present(
            &window.clone(),
            "Confirm Cleanup",
            cards,
            trash,
            &notice,
            move |picked| {
                if let Some(picked) = picked {
                    let items = removal_dialog::pick(items, &picked);
                    execute(window, store, items, options, finished);
                } else {
                    finished();
                }
            },
        );
    });
}

fn execute(
    window: super::MainWindow,
    store: CleanScanStore,
    items: Selection,
    options: CleanOptions,
    finished: impl FnOnce() + 'static,
) {
    run(window, store, items, options, Box::new(finished));
}

/// Not generic: "Allow" runs it again, and a generic callback would make
/// the compiler instantiate it forever.
fn run(
    window: super::MainWindow,
    store: CleanScanStore,
    items: Selection,
    options: CleanOptions,
    finished: Box<dyn FnOnce()>,
) {
    let retry_items = items.clone();
    let progress = Arc::new(CleanProgress::default());
    let progress_window = CleanProgressWindow::show(
        Some(window.upcast_ref()),
        items.iter().map(|(_, i)| i.bytes).sum(),
        items
            .iter()
            .map(|(_, i)| i.scan.file_count + usize::from(i.command.is_some()))
            .sum(),
        progress.clone(),
    );
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        // Merging de-duplicates every file, so it must also run off the GTK thread.
        let mut scan = ScanResult::new();
        for (_, item) in &items {
            scan.merge(item.scan.clone());
        }
        clean_sources::revalidate_versions(&mut scan);
        let cancel = Arc::new(AtomicBool::new(false));
        let result = Cleaner::with_options_and_cancellation(options, cancel.clone())
            .with_progress(progress.clone())
            .clean(&scan);
        let mut completed = Vec::new();
        let mut errors = Vec::new();
        for (_, item) in &items {
            if result.blocked_reason.is_some() || cancel.load(Ordering::Relaxed) {
                break;
            }
            if let Some(command) = &item.command {
                match clean_sources::run_command(command) {
                    Ok(_) => {
                        progress.bytes_done.fetch_add(item.bytes, Ordering::Relaxed);
                        progress.files_done.fetch_add(1, Ordering::Relaxed);
                        completed.push(item.title.clone());
                    }
                    Err(error) => errors.push(format!("{}: {}", item.title, error)),
                }
            }
        }
        let mut report = tr_args(
            "Freed {size} from selected files.",
            &[(
                "{size}",
                &bytesize::ByteSize(result.bytes_freed).to_string(),
            )],
        );
        for title in &completed {
            report.push_str(&format!(
                "\n{}",
                tr_args("Completed: {title}", &[("{title}", &tr(title))])
            ));
        }
        if result.cancelled {
            report.push_str(&format!("\n{}", tr("Cleaning cancelled.")));
        }
        if let Some(reason) = &result.blocked_reason {
            report.push_str(&format!("\n{reason}"));
        }
        for (path, reason) in result.failed.iter().chain(result.skipped.iter()) {
            report.push_str(&format!("\n{}: {}", path.display(), reason));
        }
        for error in &errors {
            report.push_str(&format!("\n{error}"));
        }
        let warning =
            !errors.is_empty() || !result.failed.is_empty() || result.blocked_reason.is_some();
        let _ = tx.send_blocking((result, completed, report, warning));
    });
    glib::spawn_future_local(async move {
        if let Ok((result, completed, report, warning)) = rx.recv().await {
            let blocked = result.blocked_reason.clone();
            let mut update = crate::services::removal_update::RemovalUpdate::from(&result);
            update.commands = completed.clone();
            if window.root().is_some() {
                window.apply_removed(update).await;
            } else {
                store.apply_cleaned(result, completed).await;
            }
            window.record_cleanup_report(&report);
            window.show_cleanup_report(&report, warning);
            if let Some(reason) = &blocked {
                let retry_window = window.clone();
                let retry_store = store.clone();
                // Nothing was deleted when the limits stopped it, so it is
                // safe to run the same selection again without them.
                let allow: Box<dyn Fn()> = Box::new(move || {
                    let unlimited = CleanOptions {
                        max_files_per_operation: usize::MAX,
                        max_size_per_operation: u64::MAX,
                        ..options
                    };
                    run(
                        retry_window.clone(),
                        retry_store.clone(),
                        retry_items.clone(),
                        unlimited,
                        Box::new(|| {}),
                    );
                });
                window.show_blocked_dialog(reason, Some(allow));
            }
        }
        progress_window.close();
        window.save_scan_summary();
        finished();
    });
}
