//! One in-memory inventory shared by Overview, sidebar and clean pages.
//! Only explicit scan actions replace a completed inventory.
use crate::{
    services::clean_sources::{self, CleanItem, CleanPageKind},
    services::removal_update::RemovalUpdate,
    storage::Storage,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub type Inventory = Arc<Vec<Arc<CleanItem>>>;
type Listener = Rc<dyn Fn(CleanPageKind)>;
type ScanJob = Arc<dyn Fn(CleanPageKind, Arc<AtomicBool>) -> Vec<CleanItem> + Send + Sync>;
#[derive(Default)]
struct Slot {
    items: Option<Inventory>,
    loading: bool,
    revision: u64,
    pending: RemovalUpdate,
}
struct State {
    summary_present: std::cell::Cell<bool>,
    slots: RefCell<[Slot; 3]>,
    listeners: RefCell<Vec<Listener>>,
    waiters: RefCell<Vec<async_channel::Sender<()>>>,
    job: ScanJob,
    cancel: Arc<AtomicBool>,
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
#[derive(Clone)]
pub struct CleanScanStore(Rc<State>);
#[derive(Clone)]
pub struct WeakCleanScanStore(std::rc::Weak<State>);
impl WeakCleanScanStore {
    pub fn upgrade(&self) -> Option<CleanScanStore> {
        self.0.upgrade().map(CleanScanStore)
    }
}
impl CleanScanStore {
    pub fn downgrade(&self) -> WeakCleanScanStore {
        WeakCleanScanStore(Rc::downgrade(&self.0))
    }
    pub fn new(storage: Arc<Storage>) -> Self {
        Self::with_job(Arc::new(move |kind, cancel| {
            clean_sources::scan(kind, &storage, cancel)
        }))
    }
    fn with_job(job: ScanJob) -> Self {
        Self(Rc::new(State {
            summary_present: std::cell::Cell::new(false),
            slots: RefCell::new(Default::default()),
            listeners: RefCell::new(Vec::new()),
            waiters: RefCell::new(Vec::new()),
            job,
            cancel: Arc::new(AtomicBool::new(false)),
        }))
    }
    pub fn items(&self, kind: CleanPageKind) -> Option<Inventory> {
        self.0.slots.borrow()[kind.index()].items.clone()
    }
    pub fn loading(&self, kind: CleanPageKind) -> bool {
        self.0.slots.borrow()[kind.index()].loading
    }
    pub fn busy(&self) -> bool {
        self.0.slots.borrow().iter().any(|s| s.loading)
    }
    pub fn subscribe(&self, listener: impl Fn(CleanPageKind) + 'static) {
        self.0.listeners.borrow_mut().push(Rc::new(listener));
    }
    fn notify(&self, kind: CleanPageKind) {
        let listeners = self.0.listeners.borrow().clone();
        for listener in listeners {
            listener(kind);
        }
        if !self.busy() {
            for waiter in self.0.waiters.borrow_mut().drain(..) {
                let _ = waiter.try_send(());
            }
        }
    }
    pub async fn wait_ready(&self) {
        if self.busy() {
            let (tx, rx) = async_channel::bounded(1);
            self.0.waiters.borrow_mut().push(tx);
            let _ = rx.recv().await;
        }
    }
    pub fn set_summary_present(&self, present: bool) {
        if self.0.summary_present.replace(present) != present {
            for kind in [
                CleanPageKind::AppCaches,
                CleanPageKind::DevTools,
                CleanPageKind::System,
            ] {
                self.notify(kind);
            }
        }
    }
    pub fn has_summary(&self) -> bool {
        self.0.summary_present.get()
    }
    pub fn request_all(&self, force: bool) {
        for kind in [
            CleanPageKind::AppCaches,
            CleanPageKind::DevTools,
            CleanPageKind::System,
        ] {
            self.request(kind, force);
        }
    }
    pub fn request_all_staggered(&self, force: bool) {
        for kind in [
            CleanPageKind::AppCaches,
            CleanPageKind::DevTools,
            CleanPageKind::System,
        ] {
            self.request_delayed(kind, force, kind.index() as u64 * 250);
        }
    }
    pub fn request(&self, kind: CleanPageKind, force: bool) {
        self.request_delayed(kind, force, 0);
    }
    fn request_delayed(&self, kind: CleanPageKind, force: bool, delay_ms: u64) {
        {
            let mut slots = self.0.slots.borrow_mut();
            let slot = &mut slots[kind.index()];
            if slot.loading || (!force && slot.items.is_some()) {
                return;
            }
            slot.loading = true;
            slot.pending = RemovalUpdate::default();
        }
        self.notify(kind);
        let revision = self.0.slots.borrow()[kind.index()].revision;
        let job = self.0.job.clone();
        let cancel = self.0.cancel.clone();
        let (send, recv) = async_channel::bounded(1);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let _permit = crate::services::scan_jobs::acquire();
            let _ = send.send_blocking(Arc::new(
                job(kind, cancel)
                    .into_iter()
                    .map(Arc::new)
                    .collect::<Vec<_>>(),
            ));
        });
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            if let Ok(items) = recv.recv().await {
                if let Some(state) = weak.upgrade() {
                    {
                        let mut slots = state.slots.borrow_mut();
                        let slot = &mut slots[kind.index()];
                        if slot.revision == revision || slot.items.is_none() {
                            slot.items = Some(filter_removed(items, &slot.pending));
                        }
                        slot.loading = false;
                    }
                    Self(state).notify(kind);
                }
            }
        });
    }
    pub async fn apply_cleaned(&self, result: crate::models::CleanResult, commands: Vec<String>) {
        let mut update = RemovalUpdate::from(&result);
        update.commands = commands;
        self.apply_removed(Arc::new(update)).await;
    }
    /// Copy only affected items on a worker; keep unchanged snapshot identities.
    pub async fn apply_removed(&self, update: Arc<RemovalUpdate>) {
        if update.paths.is_empty() && update.commands.is_empty() {
            return;
        }
        let started = std::time::Instant::now();
        for slot in self.0.slots.borrow_mut().iter_mut() {
            slot.revision += 1;
            if slot.loading && slot.items.is_none() {
                slot.pending.merge(update.as_ref().clone());
            }
        }
        loop {
            let snapshots: Vec<_> = [
                CleanPageKind::AppCaches,
                CleanPageKind::DevTools,
                CleanPageKind::System,
            ]
            .into_iter()
            .filter_map(|kind| self.items(kind).map(|items| (kind, items)))
            .collect();
            let originals = snapshots.clone();
            let update = update.clone();
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let updated: Vec<_> = snapshots
                    .into_iter()
                    .map(|(kind, items)| (kind, filter_removed(items, &update)))
                    .collect();
                let _ = tx.send_blocking(updated);
            });
            let mut retry = false;
            if let Ok(updated) = rx.recv().await {
                for ((kind, items), (_, original)) in updated.into_iter().zip(&originals) {
                    let current = self.items(kind);
                    if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, original)) {
                        if !Arc::ptr_eq(&items, original) {
                            self.0.slots.borrow_mut()[kind.index()].items = Some(items);
                            self.notify(kind);
                        }
                    } else {
                        retry = true;
                    }
                }
            }
            std::thread::spawn(move || drop(originals));
            if !retry {
                break;
            }
        }
        tracing::debug!(
            elapsed_us = started.elapsed().as_micros() as u64,
            "apply_cleaned"
        );
    }
}

fn filter_removed(mut items: Inventory, update: &RemovalUpdate) -> Inventory {
    let affected = |item: &CleanItem| {
        (item.command.is_some() && update.commands.contains(&item.title))
            || item
                .scan
                .files
                .iter()
                .chain(&item.scan.directories)
                .any(|f| update.removes(&f.path))
    };
    if !items.iter().any(|i| affected(i)) {
        return items;
    }
    Arc::make_mut(&mut items).retain_mut(|item| {
        if !affected(item) {
            return true;
        }
        if item.command.is_some() && update.commands.contains(&item.title) {
            return false;
        }
        let item = Arc::make_mut(item);
        item.scan.remove_cleaned(&update.paths);
        item.bytes = item.scan.total_size;
        item.bytes > 0
    });
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[test]
    fn deletion_preserves_unchanged_arcs_and_never_starts_jobs_or_loading() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    use crate::models::{CleanResult, FileEntry, Safety, ScanResult};
                    let item = |path: &str| {
                        let mut scan = ScanResult::new();
                        scan.add_file(FileEntry::new(path.into(), 100, false, false));
                        Arc::new(CleanItem {
                            key: "generic-cache",
                            title: path.into(),
                            explanation: "",
                            icon: "",
                            app_icons: Vec::new(),
                            safety: Safety::Safe,
                            bytes: 100,
                            scan,
                            command: None,
                        })
                    };
                    let kept = item("/kept");
                    let store = CleanScanStore::with_job(Arc::new(|_, _| {
                        panic!("deletion started a scan")
                    }));
                    store.0.slots.borrow_mut()[0].items =
                        Some(Arc::new(vec![item("/removed"), kept.clone()]));
                    let weak = store.downgrade();
                    let notifications = Rc::new(std::cell::Cell::new(0));
                    let notified = notifications.clone();
                    store.subscribe(move |_| {
                        assert!(
                            !weak.upgrade().unwrap().busy(),
                            "discount must never set loading"
                        );
                        notified.set(notified.get() + 1);
                    });
                    let mut result = CleanResult::new();
                    result.add_deleted_file("/removed".into(), 100);
                    result.add_failed("/kept".into(), "busy");
                    store.apply_cleaned(result.clone(), Vec::new()).await;
                    let snapshot = store.items(CleanPageKind::AppCaches).unwrap();
                    assert_eq!(snapshot.len(), 1);
                    assert!(Arc::ptr_eq(&kept, &snapshot[0]));
                    store.apply_cleaned(result, Vec::new()).await;
                    assert!(Arc::ptr_eq(
                        &snapshot,
                        &store.items(CleanPageKind::AppCaches).unwrap()
                    ));
                    assert_eq!(notifications.get(), 1);
                    assert!(!store.busy());
                })
            })
            .unwrap();
    }

    #[test]
    fn removal_performance_with_large_snapshots() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    use crate::models::{CleanResult, FileEntry, Safety, ScanResult};
                    // Exercise realistic inventory sizes without a display or disk walk.
                    let mut scan = ScanResult::new();
                    for n in 0..50_000 {
                        scan.add_file(FileEntry::new(
                            format!("/cache/{n}").into(),
                            100,
                            false,
                            false,
                        ));
                    }
                    let store = CleanScanStore::with_job(Arc::new(|_, _| {
                        panic!("deletion started a scan")
                    }));
                    store.0.slots.borrow_mut()[0].items =
                        Some(Arc::new(vec![Arc::new(CleanItem {
                            key: "generic-cache",
                            title: "cache".into(),
                            explanation: "",
                            icon: "",
                            app_icons: Vec::new(),
                            safety: Safety::Safe,
                            bytes: scan.total_size,
                            scan,
                            command: None,
                        })]));
                    let mut result = CleanResult::new();
                    result.add_deleted_file("/cache/25000".into(), 100);
                    let started = std::time::Instant::now();
                    store.apply_cleaned(result, Vec::new()).await;
                    println!(
                        "apply_cleaned, 50,000 files: {} us",
                        started.elapsed().as_micros()
                    );
                    assert_eq!(
                        store.items(CleanPageKind::AppCaches).unwrap()[0]
                            .scan
                            .file_count,
                        49_999
                    );
                    assert!(!store.busy());
                })
            })
            .unwrap();
    }

    #[test]
    fn cached_summary_does_not_create_cleanable_items_or_skip_revalidation() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let store = CleanScanStore::with_job(Arc::new(|_, _| Vec::new()));
                    store.set_summary_present(true);
                    assert!(store.has_summary());
                    assert!(store.items(CleanPageKind::AppCaches).is_none());
                    assert!(super::super::clean_workflow::safe_items(&store).is_empty());
                    store.request_all_staggered(true);
                    assert!(store.loading(CleanPageKind::AppCaches));
                    assert!(store.loading(CleanPageKind::DevTools));
                    assert!(store.loading(CleanPageKind::System));
                    store.wait_ready().await;
                    assert!(!store.busy());
                    assert!(store.items(CleanPageKind::AppCaches).is_some());
                })
            })
            .unwrap();
    }
    #[test]
    fn overview_selects_only_safe_snapshots_and_cleanup_preserves_failed_files() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    use crate::models::{CleanResult, FileEntry, Safety, ScanResult};
                    let gtk_thread = std::thread::current().id();
                    let calls = Arc::new(AtomicUsize::new(0));
                    let counted = calls.clone();
                    let store = CleanScanStore::with_job(Arc::new(move |kind, _| {
                        assert_ne!(gtk_thread, std::thread::current().id());
                        counted.fetch_add(1, Ordering::SeqCst);
                        [Safety::Safe, Safety::ReviewFirst]
                            .into_iter()
                            .map(|safety| {
                                let title = format!("{kind:?}-{safety:?}");
                                let mut scan = ScanResult::new();
                                scan.add_file(FileEntry::new(
                                    std::path::PathBuf::from(&title),
                                    100,
                                    false,
                                    false,
                                ));
                                CleanItem {
                                    key: "generic-cache",
                                    title,
                                    explanation: "",
                                    icon: "pc-folder-symbolic",
                                    app_icons: Vec::new(),
                                    safety,
                                    bytes: 100,
                                    scan,
                                    command: None,
                                }
                            })
                            .collect()
                    }));
                    assert!(super::super::clean_workflow::safe_items(&store).is_empty());
                    store.request_all(false);
                    store.wait_ready().await;
                    assert!(!store.busy());
                    let selected = super::super::clean_workflow::safe_items(&store);
                    assert_eq!(selected.len(), 3);
                    assert!(selected.iter().all(|(_, i)| i.safety == Safety::Safe));
                    let mut result = CleanResult::new();
                    result.add_deleted_file(selected[0].1.scan.files[0].path.clone(), 100);
                    result.add_failed(selected[1].1.scan.files[0].path.clone(), "busy");
                    store.apply_cleaned(result, Vec::new()).await;
                    assert_eq!(store.items(CleanPageKind::AppCaches).unwrap().len(), 1);
                    assert_eq!(store.items(CleanPageKind::DevTools).unwrap().len(), 2);
                    assert_eq!(super::super::clean_workflow::safe_items(&store).len(), 2);
                    store.request_all(false);
                    store.wait_ready().await;
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        3,
                        "cleanup must reuse snapshots"
                    );
                })
            })
            .unwrap();
    }
    #[test]
    fn visits_reuse_the_same_inventory_and_only_explicit_scans_replace_it() {
        glib::MainContext::default().block_on(async {
            let calls = Arc::new(AtomicUsize::new(0));
            let counted = calls.clone();
            let store = CleanScanStore::with_job(Arc::new(move |_, _| {
                counted.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            }));
            let (tx, rx) = async_channel::unbounded();
            let weak = Rc::downgrade(&store.0);
            store.subscribe(move |kind| {
                let s = CleanScanStore(weak.upgrade().unwrap());
                if !s.loading(kind) {
                    tx.try_send(()).unwrap();
                }
            });
            let kind = CleanPageKind::AppCaches;
            store.request(kind, false);
            store.request(kind, false);
            rx.recv().await.unwrap();
            let inventory = store.items(kind).unwrap();
            store.request(kind, false);
            store.request_all(false);
            assert!(Arc::ptr_eq(&inventory, &store.items(kind).unwrap()));
            // Other pages may scan for the first time, but this page is cached.
            while store.busy() {
                rx.recv().await.unwrap();
            }
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            while rx.try_recv().is_ok() {}
            store.request(kind, true);
            rx.recv().await.unwrap();
            assert!(!Arc::ptr_eq(&inventory, &store.items(kind).unwrap()));
            assert_eq!(calls.load(Ordering::SeqCst), 4);
        });
    }
}
