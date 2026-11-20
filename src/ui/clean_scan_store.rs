//! One in-memory inventory shared by Overview, sidebar and clean pages.
//! Only explicit scan actions replace a completed inventory.
use crate::{
    services::clean_sources::{self, CleanItem, CleanPageKind},
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
}
struct State {
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
    pub fn request_all(&self, force: bool) {
        for kind in [
            CleanPageKind::AppCaches,
            CleanPageKind::DevTools,
            CleanPageKind::System,
        ] {
            self.request(kind, force);
        }
    }
    pub fn request(&self, kind: CleanPageKind, force: bool) {
        {
            let mut slots = self.0.slots.borrow_mut();
            let slot = &mut slots[kind.index()];
            if slot.loading || (!force && slot.items.is_some()) {
                return;
            }
            slot.loading = true;
        }
        self.notify(kind);
        let job = self.0.job.clone();
        let cancel = self.0.cancel.clone();
        let (send, recv) = async_channel::bounded(1);
        std::thread::spawn(move || {
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
                        slot.items = Some(items);
                        slot.loading = false;
                    }
                    Self(state).notify(kind);
                }
            }
        });
    }
    // Filter and clone file inventories on a worker, then publish snapshots on GTK.
    pub async fn apply_cleaned(&self, result: crate::models::CleanResult, commands: Vec<String>) {
        let snapshots: Vec<_> = [
            CleanPageKind::AppCaches,
            CleanPageKind::DevTools,
            CleanPageKind::System,
        ]
        .into_iter()
        .filter_map(|kind| self.items(kind).map(|items| (kind, items)))
        .collect();
        let originals = snapshots.clone();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let paths = result
                .deleted_files
                .iter()
                .chain(&result.deleted_directories)
                .cloned()
                .collect();
            let updated: Vec<_> = snapshots
                .into_iter()
                .map(|(kind, items)| {
                    let remaining = items
                        .iter()
                        .filter_map(|original| {
                            if original.command.is_some() {
                                return (!commands.contains(&original.title))
                                    .then(|| original.clone());
                            }
                            let mut item = original.as_ref().clone();
                            item.scan.remove_cleaned(&paths);
                            item.bytes = item.scan.total_size;
                            (item.bytes > 0).then(|| Arc::new(item))
                        })
                        .collect::<Vec<_>>();
                    (kind, Arc::new(remaining))
                })
                .collect();
            let _ = tx.send_blocking(updated);
        });
        if let Ok(updated) = rx.recv().await {
            for ((kind, items), (_, original)) in updated.into_iter().zip(originals.iter()) {
                if self
                    .items(kind)
                    .is_some_and(|current| Arc::ptr_eq(&current, original))
                {
                    self.0.slots.borrow_mut()[kind.index()].items = Some(items);
                    self.notify(kind);
                }
            }
        }
        std::thread::spawn(move || drop(originals));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
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
