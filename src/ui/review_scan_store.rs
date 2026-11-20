//! Shared snapshots for Overview, sidebar and both review pages.
//! Visits reuse a completed scan; only explicit scans replace it.
use crate::services::{
    self,
    app_leftovers::{self, AppLeftover, Status},
    app_uninstall::{self, InstalledApp},
    removal_update::RemovalUpdate,
    review_items::{self, ReviewItem},
    FileKind, FoundFile,
};
use gio::prelude::*;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

pub const DEFAULT_MINIMUM: u64 = 200_000_000;
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    LargeFiles,
    Apps,
    Models,
    Games,
}
impl Kind {
    fn index(self) -> usize {
        match self {
            Self::LargeFiles => 0,
            Self::Apps => 1,
            Self::Models => 2,
            Self::Games => 3,
        }
    }
}
#[derive(Clone, Default)]
pub struct Thumbnail {
    pub pixels: Arc<[u8]>,
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub alpha: bool,
}
#[derive(Clone, Default)]
pub struct FilePreview {
    pub thumbnail: Option<Thumbnail>,
    pub icon: Option<String>,
}
#[derive(Clone, Default)]
pub struct LargeInventory {
    pub files: Vec<FoundFile>,
    pub duplicates: HashSet<PathBuf>,
    pub previews: HashMap<PathBuf, Arc<FilePreview>>,
}
#[derive(Clone, Default)]
pub struct ItemInventory {
    pub items: Vec<ReviewItem>,
    /// Decoded covers keyed by item location.
    pub covers: HashMap<PathBuf, Arc<Thumbnail>>,
}
impl ItemInventory {
    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }
}
#[derive(Clone, Default)]
pub struct AppInventory {
    pub apps: Vec<InstalledApp>,
    pub leftovers: Vec<AppLeftover>,
}
impl LargeInventory {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}
impl AppInventory {
    pub fn app_bytes(&self) -> u64 {
        self.apps.iter().map(InstalledApp::size).sum()
    }
    pub fn leftover_bytes(&self) -> u64 {
        self.leftovers.iter().map(AppLeftover::size).sum()
    }
}
#[derive(Clone)]
enum Inventory {
    Large(Arc<LargeInventory>),
    Apps(Arc<AppInventory>),
    Items(Arc<ItemInventory>),
}
#[derive(Default)]
struct Slot {
    inventory: Option<Inventory>,
    loading: bool,
    error: Option<String>,
    revision: u64,
    pending: RemovalUpdate,
}
type Job = Arc<dyn Fn(Kind, u64, &AtomicBool) -> Result<Inventory, String> + Send + Sync>;
struct State {
    summary_present: std::cell::Cell<bool>,
    slots: RefCell<[Slot; 4]>,
    minimum: Cell<u64>,
    listeners: RefCell<Vec<Rc<dyn Fn()>>>,
    job: Job,
    cancel: Arc<AtomicBool>,
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
#[derive(Clone)]
pub struct ReviewScanStore(Rc<State>);
#[derive(Clone)]
pub struct WeakReviewScanStore(std::rc::Weak<State>);
impl WeakReviewScanStore {
    pub fn upgrade(&self) -> Option<ReviewScanStore> {
        self.0.upgrade().map(ReviewScanStore)
    }
}
impl ReviewScanStore {
    pub fn new() -> Self {
        let media = std::sync::Mutex::new(MediaCache::default());
        Self::with_job(Arc::new(move |kind, minimum, cancel| {
            scan(kind, minimum, cancel, &media)
        }))
    }
    fn with_job(job: Job) -> Self {
        Self(Rc::new(State {
            summary_present: std::cell::Cell::new(false),
            slots: RefCell::new(Default::default()),
            minimum: Cell::new(DEFAULT_MINIMUM),
            listeners: RefCell::default(),
            job,
            cancel: Arc::new(AtomicBool::new(false)),
        }))
    }
    pub fn downgrade(&self) -> WeakReviewScanStore {
        WeakReviewScanStore(Rc::downgrade(&self.0))
    }
    pub fn large(&self) -> Option<Arc<LargeInventory>> {
        match &self.0.slots.borrow()[0].inventory {
            Some(Inventory::Large(v)) => Some(v.clone()),
            _ => None,
        }
    }
    pub fn apps(&self) -> Option<Arc<AppInventory>> {
        match &self.0.slots.borrow()[1].inventory {
            Some(Inventory::Apps(v)) => Some(v.clone()),
            _ => None,
        }
    }
    /// Models or games found by the last scan.
    pub fn items(&self, kind: Kind) -> Option<Arc<ItemInventory>> {
        match &self.0.slots.borrow()[kind.index()].inventory {
            Some(Inventory::Items(v)) => Some(v.clone()),
            _ => None,
        }
    }
    pub fn loading(&self, kind: Kind) -> bool {
        self.0.slots.borrow()[kind.index()].loading
    }
    pub fn error(&self, kind: Kind) -> Option<String> {
        self.0.slots.borrow()[kind.index()].error.clone()
    }
    pub fn busy(&self) -> bool {
        self.0.slots.borrow().iter().any(|s| s.loading)
    }
    pub fn minimum(&self) -> u64 {
        self.0.minimum.get()
    }
    pub fn subscribe(&self, listener: impl Fn() + 'static) {
        self.0.listeners.borrow_mut().push(Rc::new(listener));
    }
    fn notify(&self) {
        let listeners = self.0.listeners.borrow().clone();
        for listener in listeners {
            listener();
        }
    }
    pub fn forget_paths(&self, paths: &[PathBuf]) {
        self.apply_removed(&RemovalUpdate {
            paths: paths.iter().cloned().collect(),
            ..Default::default()
        });
    }
    pub fn apply_removed(&self, update: &RemovalUpdate) {
        if update.paths.is_empty() && update.uninstalled.is_empty() {
            return;
        }
        let started = std::time::Instant::now();
        let mut changed = false;
        {
            let mut slots = self.0.slots.borrow_mut();
            for slot in slots.iter_mut() {
                // An older scan must not put successfully deleted entries back.
                if slot.loading {
                    slot.revision += 1;
                    if slot.inventory.is_none() {
                        slot.pending.merge(update.clone());
                    }
                }
                if let Some(inventory) = &mut slot.inventory {
                    changed |= inventory.remove(update);
                }
            }
        }
        if changed {
            self.notify();
        }
        tracing::debug!(
            elapsed_us = started.elapsed().as_micros() as u64,
            changed,
            "forget_paths"
        );
    }
    pub fn set_summary_present(&self, present: bool) {
        if self.0.summary_present.replace(present) != present {
            self.notify();
        }
    }
    pub fn has_summary(&self) -> bool {
        self.0.summary_present.get()
    }
    pub fn request_all(&self, force: bool) {
        self.request(Kind::LargeFiles, force);
        self.request(Kind::Apps, force);
        self.request(Kind::Models, force);
        self.request(Kind::Games, force);
    }
    pub fn request_all_staggered(&self, force: bool) {
        self.request_delayed(
            Kind::LargeFiles,
            force,
            750 + Kind::LargeFiles.index() as u64 * 250,
        );
        self.request_delayed(Kind::Apps, force, 750 + Kind::Apps.index() as u64 * 250);
        self.request_delayed(Kind::Models, force, 750 + Kind::Models.index() as u64 * 250);
        self.request_delayed(Kind::Games, force, 750 + Kind::Games.index() as u64 * 250);
    }
    pub fn request_large(&self, minimum: u64, force: bool) {
        if self.loading(Kind::LargeFiles) {
            return;
        }
        let changed = self.0.minimum.replace(minimum) != minimum;
        self.request(Kind::LargeFiles, force || changed);
    }
    pub fn request(&self, kind: Kind, force: bool) {
        self.request_delayed(kind, force, 0);
    }
    fn request_delayed(&self, kind: Kind, force: bool, delay_ms: u64) {
        {
            let mut slots = self.0.slots.borrow_mut();
            let slot = &mut slots[kind.index()];
            if slot.loading || (!force && slot.inventory.is_some()) {
                return;
            }
            slot.loading = true;
            slot.pending = RemovalUpdate::default();
            slot.error = None;
        }
        self.notify();
        let revision = self.0.slots.borrow()[kind.index()].revision;
        let job = self.0.job.clone();
        let cancel = self.0.cancel.clone();
        let minimum = self.minimum();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let _permit = services::scan_jobs::acquire();
            let _ = tx.send_blocking(job(kind, minimum, &cancel));
        });
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = rx.recv().await;
            if let Some(store) = weak.upgrade() {
                {
                    let mut slots = store.0.slots.borrow_mut();
                    let slot = &mut slots[kind.index()];
                    slot.loading = false;
                    match result {
                        Ok(Ok(mut inventory)) => {
                            if slot.revision == revision || slot.inventory.is_none() {
                                if slot.revision != revision {
                                    inventory.remove(&slot.pending);
                                }
                                slot.inventory = Some(inventory);
                            }
                        }
                        Ok(Err(error)) => slot.error = Some(error),
                        Err(error) => slot.error = Some(error.to_string()),
                    }
                }
                store.notify();
            }
        });
    }
}
impl Inventory {
    fn remove(&mut self, update: &RemovalUpdate) -> bool {
        let affected =
            |path: &std::path::Path| update.removes(path) || update.bytes_under(path) > 0;
        match self {
            Self::Large(snapshot) => {
                if !snapshot.files.iter().any(|f| update.removes(&f.path)) {
                    return false;
                }
                let inventory = Arc::make_mut(snapshot);
                inventory.files.retain(|f| !update.removes(&f.path));
                inventory.duplicates.retain(|p| !update.removes(p));
                inventory.previews.retain(|p, _| !update.removes(p));
            }
            Self::Items(snapshot) => {
                if !snapshot.items.iter().any(|i| {
                    update.removes(&i.location) || i.targets.iter().any(|t| affected(&t.path))
                }) {
                    return false;
                }
                let inventory = Arc::make_mut(snapshot);
                let mut relocated = Vec::new();
                inventory.items.retain_mut(|item| {
                    if item.targets.is_empty() {
                        return !update.removes(&item.location);
                    }
                    remove_targets(&mut item.targets, update);
                    item.bytes = item.targets.iter().map(|t| t.size).sum();
                    if item.targets.is_empty() {
                        return false;
                    }
                    if update.removes(&item.location) {
                        let previous =
                            std::mem::replace(&mut item.location, item.targets[0].path.clone());
                        relocated.push((previous, item.location.clone()));
                    }
                    true
                });
                for (previous, location) in relocated {
                    if let Some(cover) = inventory.covers.remove(&previous) {
                        inventory.covers.insert(location, cover);
                    }
                }
                if !inventory.covers.is_empty() {
                    let locations: HashSet<_> =
                        inventory.items.iter().map(|i| &i.location).collect();
                    inventory.covers.retain(|path, _| locations.contains(path));
                }
            }
            Self::Apps(snapshot) => {
                if !snapshot.apps.iter().any(|a| {
                    update.uninstalled.contains(&a.id)
                        || a.targets
                            .iter()
                            .chain(&a.data_targets)
                            .any(|t| affected(&t.path))
                }) && !snapshot
                    .leftovers
                    .iter()
                    .any(|a| a.locations.iter().any(|l| affected(&l.target.path)))
                {
                    return false;
                }
                let inventory = Arc::make_mut(snapshot);
                let mut leftovers = Vec::new();
                inventory.apps.retain_mut(|app| {
                    let removed = remove_targets(&mut app.targets, update);
                    app.bytes = app.bytes.saturating_sub(removed);
                    remove_targets(&mut app.data_targets, update);
                    app.data_bytes = app.data_targets.iter().map(|t| t.size).sum();
                    if update.uninstalled.contains(&app.id)
                        || (matches!(app.removal, app_uninstall::Removal::Trash)
                            && app.targets.is_empty())
                    {
                        if !app.data_targets.is_empty() {
                            leftovers.push(AppLeftover {
                                key: app.id.clone(),
                                name: app.name.clone(),
                                icon: app.icon.clone(),
                                status: Status::NotInstalled,
                                confidence: app_leftovers::Confidence::High,
                                removed_on: Some(chrono::Utc::now().date_naive().to_string()),
                                locations: app
                                    .data_targets
                                    .iter()
                                    .cloned()
                                    .map(|target| app_leftovers::Location {
                                        target,
                                        modified: None,
                                        complete: true,
                                    })
                                    .collect(),
                            });
                        }
                        return false;
                    }
                    true
                });
                inventory.leftovers.iter_mut().for_each(|app| {
                    app.locations.retain_mut(|location| {
                        if update.removes(&location.target.path) {
                            return false;
                        }
                        location.target.size = location
                            .target
                            .size
                            .saturating_sub(update.bytes_under(&location.target.path));
                        true
                    });
                });
                inventory.leftovers.retain(|a| !a.locations.is_empty());
                inventory.leftovers.extend(leftovers);
            }
        }
        true
    }
}
fn remove_targets(targets: &mut Vec<services::TrashTarget>, update: &RemovalUpdate) -> u64 {
    let mut removed = 0;
    targets.retain_mut(|target| {
        if update.removes(&target.path) {
            removed += target.size;
            false
        } else {
            let bytes = if target.kind == services::StorageNodeKind::Directory {
                target.size.min(update.bytes_under(&target.path))
            } else {
                0
            };
            target.size -= bytes;
            removed += bytes;
            true
        }
    });
    removed
}

// Cached decoded pixels are shared across snapshots. Metadata checks and decoding
// both run on the scan worker; changed files replace only their own cache entry.
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    modified: std::time::SystemTime,
    bytes: u64,
}
impl Stamp {
    fn read(path: &std::path::Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata.modified().ok()?,
            bytes: metadata.len(),
        })
    }
}
type MediaEntries<T> = HashMap<PathBuf, (Option<Stamp>, Option<Arc<T>>)>;
struct DecodedCache<T> {
    entries: MediaEntries<T>,
}
impl<T> Default for DecodedCache<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}
impl<T> DecodedCache<T> {
    fn retain(&mut self, keep: impl FnMut(&PathBuf, &mut (Option<Stamp>, Option<Arc<T>>)) -> bool) {
        self.entries.retain(keep);
    }
    fn get_or_decode(
        &mut self,
        path: &std::path::Path,
        decode: impl FnOnce() -> Option<T>,
    ) -> Option<Arc<T>> {
        let stamp = Stamp::read(path);
        if let Some((previous, value)) = self.entries.get(path) {
            if stamp.is_some() && stamp == *previous {
                return value.clone();
            }
        }
        let value = decode().map(Arc::new);
        self.entries
            .insert(path.to_path_buf(), (stamp, value.clone()));
        value
    }
}
#[derive(Default)]
struct MediaCache {
    previews: DecodedCache<FilePreview>,
    covers: [DecodedCache<Thumbnail>; 2],
}

fn scan(
    kind: Kind,
    minimum: u64,
    cancel: &AtomicBool,
    media: &std::sync::Mutex<MediaCache>,
) -> Result<Inventory, String> {
    let home = dirs::home_dir()
        .ok_or_else(|| crate::i18n::tr("Scan could not finish. Try scanning again."))?;
    match kind {
        Kind::LargeFiles => {
            let mut files =
                services::find_large_files(&home, minimum, &AtomicUsize::new(0), cancel);
            let groups = services::find_duplicates(
                &home,
                minimum,
                &services::DuplicateProgress::default(),
                cancel,
            );
            let duplicates = groups
                .iter()
                .flat_map(|g| g.files.iter().map(|f| f.path.clone()))
                .collect();
            let mut seen: HashSet<_> = files.iter().map(|f| f.path.clone()).collect();
            for file in groups.into_iter().flat_map(|g| g.files) {
                if seen.insert(file.path.clone()) {
                    files.push(file);
                }
            }
            let mut media = media.lock().unwrap();
            media
                .previews
                .retain(|path, _| files.iter().any(|f| &f.path == path));
            let previews = files
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        media
                            .previews
                            .get_or_decode(&f.path, || Some(preview(f)))
                            .unwrap(),
                    )
                })
                .collect();
            Ok(Inventory::Large(Arc::new(LargeInventory {
                files,
                duplicates,
                previews,
            })))
        }
        Kind::Apps => {
            let all = app_leftovers::scan(&home, cancel)?;
            let apps = app_uninstall::scan(&home, &all, cancel);
            let leftovers = all
                .into_iter()
                .filter(|l| l.status == Status::NotInstalled)
                .collect();
            Ok(Inventory::Apps(Arc::new(AppInventory { apps, leftovers })))
        }
        Kind::Models | Kind::Games => {
            let items = match kind {
                Kind::Models => review_items::scan_models(&home, cancel),
                _ => review_items::scan_games(&home, cancel),
            };
            let mut media = media.lock().unwrap();
            let cache = &mut media.covers[usize::from(matches!(kind, Kind::Games))];
            cache.retain(|path, _| items.iter().any(|i| i.icon.as_ref() == Some(path)));
            let covers = items
                .iter()
                .filter_map(|item| {
                    let path = item.icon.as_deref()?;
                    let cover = cache.get_or_decode(path, || decode_cover(path))?;
                    Some((item.location.clone(), cover))
                })
                .collect();
            Ok(Inventory::Items(Arc::new(ItemInventory { items, covers })))
        }
    }
}
fn preview(file: &FoundFile) -> FilePreview {
    let Ok(info) = gio::File::for_path(&file.path).query_info(
        "standard::icon,thumbnail::path,thumbnail::is-valid",
        gio::FileQueryInfoFlags::NONE,
        None::<&gio::Cancellable>,
    ) else {
        return FilePreview::default();
    };
    FilePreview {
        thumbnail: (matches!(file.kind, FileKind::Image | FileKind::Video)
            && info.boolean("thumbnail::is-valid"))
        .then(|| {
            info.attribute_byte_string("thumbnail::path")
                .and_then(|p| decode_thumbnail(std::path::Path::new(p.as_str())))
        })
        .flatten(),
        icon: info
            .icon()
            .and_then(|i| i.to_string())
            .map(|s| s.to_string()),
    }
}

// Only small, decoded pixel buffers cross into the GTK thread. Neither disk
// access nor thumbnail decoding runs while constructing a file row.
fn decode_thumbnail(path: &std::path::Path) -> Option<Thumbnail> {
    let pixbuf = gtk4::gdk_pixbuf::Pixbuf::from_file_at_scale(path, 36, 36, true).ok()?;
    bounded_thumbnail(&pixbuf)
}
/// A centred 36×36 square from a cover of any shape (Steam headers are wide).
fn decode_cover(path: &std::path::Path) -> Option<Thumbnail> {
    let pixbuf = gtk4::gdk_pixbuf::Pixbuf::from_file(path).ok()?;
    let short = pixbuf.width().min(pixbuf.height()).max(1);
    let side = 36;
    let scaled = pixbuf.scale_simple(
        (pixbuf.width() * side / short).max(side),
        (pixbuf.height() * side / short).max(side),
        gtk4::gdk_pixbuf::InterpType::Bilinear,
    )?;
    let square = scaled.new_subpixbuf(
        (scaled.width() - side) / 2,
        (scaled.height() - side) / 2,
        side,
        side,
    );
    bounded_thumbnail(&square.copy()?)
}
fn bounded_thumbnail(pixbuf: &gtk4::gdk_pixbuf::Pixbuf) -> Option<Thumbnail> {
    let longest = pixbuf.width().max(pixbuf.height());
    let bounded = if longest > 36 {
        pixbuf.scale_simple(
            (pixbuf.width() * 36 / longest).max(1),
            (pixbuf.height() * 36 / longest).max(1),
            gtk4::gdk_pixbuf::InterpType::Bilinear,
        )?
    } else {
        pixbuf.clone()
    };
    Some(Thumbnail {
        pixels: bounded.read_pixel_bytes().to_vec().into(),
        width: bounded.width(),
        height: bounded.height(),
        stride: bounded.rowstride() as usize,
        alpha: bounded.has_alpha(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: &str, size: u64) -> services::TrashTarget {
        services::TrashTarget {
            path: path.into(),
            size,
            kind: services::StorageNodeKind::Directory,
            device: 0,
            inode: 0,
        }
    }
    fn leftover(path: &str, size: u64) -> AppLeftover {
        AppLeftover {
            key: path.into(),
            name: "leftover".into(),
            icon: None,
            status: Status::NotInstalled,
            confidence: app_leftovers::Confidence::High,
            removed_on: None,
            locations: vec![app_leftovers::Location {
                target: target(path, size),
                modified: None,
                complete: true,
            }],
        }
    }
    #[test]
    fn uninstall_and_partial_leftover_removals_discount_without_jobs_or_loading() {
        let store =
            ReviewScanStore::with_job(Arc::new(|_, _, _| panic!("deletion started a scan")));
        let app = InstalledApp {
            id: "test.app".into(),
            name: "Test".into(),
            icon: None,
            bytes: 100,
            data_bytes: 50,
            removal: app_uninstall::Removal::Unavailable,
            targets: vec![],
            data_targets: vec![target("/data/app", 50)],
        };
        store.0.slots.borrow_mut()[1].inventory = Some(Inventory::Apps(Arc::new(AppInventory {
            apps: vec![app],
            leftovers: vec![leftover("/data/old", 100)],
        })));
        let notifications = Rc::new(Cell::new(0));
        let weak = store.downgrade();
        let notified = notifications.clone();
        store.subscribe(move || {
            let store = weak.upgrade().unwrap();
            assert!(!store.busy(), "discount must never set loading");
            notified.set(notified.get() + 1);
        });
        store.apply_removed(&RemovalUpdate {
            paths: ["/data/old/removed".into()].into(),
            sizes: vec![("/data/old/removed".into(), 30)],
            uninstalled: vec!["test.app".into()],
            ..Default::default()
        });
        let inventory = store.apps().unwrap();
        assert!(inventory.apps.is_empty());
        assert_eq!(
            inventory.leftover_bytes(),
            120,
            "70 failed/remaining bytes + 50 retained app data"
        );
        assert_eq!(inventory.leftovers.len(), 2);
        assert_eq!(notifications.get(), 1);
        store.forget_paths(&["/data/old".into()]);
        assert_eq!(store.apps().unwrap().leftover_bytes(), 50);
        let remaining = store.apps().unwrap();
        store.forget_paths(&["/unrelated".into()]);
        assert!(Arc::ptr_eq(&remaining, &store.apps().unwrap()));
        assert_eq!(
            notifications.get(),
            2,
            "no-op removals do not notify or re-render"
        );
        assert_eq!(
            inventory.leftover_bytes(),
            120,
            "shared old snapshots remain unchanged"
        );
    }
    #[test]
    fn unique_inventory_is_mutated_in_place_and_partial_game_failures_remain() {
        let mut inventory = Inventory::Items(Arc::new(ItemInventory {
            items: vec![ReviewItem {
                title: "Game".into(),
                source: "test".into(),
                location: "/games/main".into(),
                targets: vec![target("/games/main", 100), target("/games/shaders", 20)],
                bytes: 120,
                last_used: None,
                icon: None,
                steam: false,
            }],
            covers: HashMap::new(),
        }));
        let pointer = match &inventory {
            Inventory::Items(items) => Arc::as_ptr(items),
            _ => unreachable!(),
        };
        inventory.remove(&RemovalUpdate {
            paths: ["/games/main".into()].into(),
            ..Default::default()
        });
        let Inventory::Items(items) = inventory else {
            unreachable!()
        };
        assert_eq!(Arc::as_ptr(&items), pointer);
        assert_eq!(items.items.len(), 1);
        assert_eq!(items.bytes(), 20);
        assert_eq!(
            items.items[0].targets[0].path,
            std::path::Path::new("/games/shaders")
        );
    }
    #[test]
    fn forget_paths_performance_with_large_snapshot() {
        let store =
            ReviewScanStore::with_job(Arc::new(|_, _, _| panic!("deletion started a scan")));
        let items = (0..50_000)
            .map(|n| ReviewItem {
                title: "item".into(),
                source: "test".into(),
                location: format!("/items/{n}").into(),
                targets: vec![],
                bytes: 100,
                last_used: None,
                icon: None,
                steam: false,
            })
            .collect();
        store.0.slots.borrow_mut()[2].inventory = Some(Inventory::Items(Arc::new(ItemInventory {
            items,
            covers: HashMap::new(),
        })));
        let started = std::time::Instant::now();
        store.forget_paths(&["/items/25000".into()]);
        println!(
            "forget_paths, 50,000 items: {} us",
            started.elapsed().as_micros()
        );
        assert_eq!(store.items(Kind::Models).unwrap().items.len(), 49_999);
        assert!(!store.busy());
    }
    #[test]
    fn decoded_media_is_shared_until_path_metadata_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cover");
        std::fs::write(&path, b"first").unwrap();
        let mut cache = DecodedCache::default();
        let first = cache
            .get_or_decode(&path, || Some(Thumbnail::default()))
            .unwrap();
        let second = cache
            .get_or_decode(&path, || panic!("decoded unchanged media twice"))
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        std::fs::write(&path, b"changed size").unwrap();
        let third = cache
            .get_or_decode(&path, || Some(Thumbnail::default()))
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &third));
    }
    #[test]
    fn finishing_an_old_scan_does_not_resurrect_removed_items() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let (release, wait) = std::sync::mpsc::channel();
                    let wait = std::sync::Mutex::new(wait);
                    let inventory = Arc::new(ItemInventory {
                        items: vec![ReviewItem {
                            title: "item".into(),
                            source: "test".into(),
                            location: "/gone".into(),
                            targets: vec![],
                            bytes: 100,
                            last_used: None,
                            icon: None,
                            steam: false,
                        }],
                        covers: HashMap::new(),
                    });
                    let scanned = inventory.clone();
                    let calls = Arc::new(AtomicUsize::new(0));
                    let called = calls.clone();
                    let store = ReviewScanStore::with_job(Arc::new(move |_, _, _| {
                        called.fetch_add(1, Ordering::SeqCst);
                        wait.lock().unwrap().recv().unwrap();
                        Ok(Inventory::Items(scanned.clone()))
                    }));
                    store.0.slots.borrow_mut()[3].inventory = Some(Inventory::Items(inventory));
                    let (tx, rx) = async_channel::unbounded();
                    store.subscribe(move || {
                        let _ = tx.try_send(());
                    });
                    store.request(Kind::Games, true);
                    store.forget_paths(&["/gone".into()]);
                    release.send(()).unwrap();
                    while store.busy() {
                        rx.recv().await.unwrap();
                    }
                    assert_eq!(store.items(Kind::Games).unwrap().bytes(), 0);
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        1,
                        "deletion did not schedule another scan"
                    );
                })
            })
            .unwrap();
    }

    #[test]
    fn forget_paths_updates_review_snapshots_without_scanning_or_touching_apps() {
        let directory = tempfile::tempdir().unwrap();
        let removed = directory.path().join("removed");
        let similar = directory.path().join("removed-other");
        std::fs::create_dir(&removed).unwrap();
        let deleted = removed.join("nested.bin");
        let exact = directory.path().join("exact.bin");
        for path in [&deleted, &exact, &similar] {
            std::fs::write(path, [1; 10]).unwrap();
        }
        let files = services::find_large_files(
            directory.path(),
            0,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        assert_eq!(files.len(), 3);
        let large = Arc::new(LargeInventory {
            duplicates: files.iter().map(|file| file.path.clone()).collect(),
            previews: files
                .iter()
                .map(|file| {
                    (
                        file.path.clone(),
                        Arc::new(FilePreview {
                            icon: Some("icon".into()),
                            ..Default::default()
                        }),
                    )
                })
                .collect(),
            files,
        });
        let items = Arc::new(ItemInventory {
            items: [&removed, &deleted, &exact, &similar]
                .into_iter()
                .map(|location| ReviewItem {
                    title: "item".into(),
                    source: "source".into(),
                    location: location.clone(),
                    targets: vec![],
                    bytes: 10,
                    last_used: None,
                    icon: None,
                    steam: false,
                })
                .collect(),
            covers: [&removed, &deleted, &exact, &similar]
                .into_iter()
                .map(|path| {
                    (
                        path.clone(),
                        Arc::new(Thumbnail {
                            pixels: vec![1, 2, 3, 4].into(),
                            ..Default::default()
                        }),
                    )
                })
                .collect(),
        });
        let apps = Arc::new(AppInventory::default());
        let store = ReviewScanStore::with_job(Arc::new(|_, _, _| panic!("unexpected scan")));
        {
            let mut slots = store.0.slots.borrow_mut();
            slots[0].inventory = Some(Inventory::Large(large.clone()));
            slots[1].inventory = Some(Inventory::Apps(apps.clone()));
            slots[2].inventory = Some(Inventory::Items(items.clone()));
            slots[3].inventory = Some(Inventory::Items(items.clone()));
        }
        let notifications = Rc::new(Cell::new(0));
        let notified = notifications.clone();
        let weak = store.downgrade();
        store.subscribe(move || {
            let store = weak.upgrade().unwrap();
            assert_eq!(store.large().unwrap().files.len(), 1);
            for kind in [Kind::Models, Kind::Games] {
                assert_eq!(store.items(kind).unwrap().items.len(), 1);
            }
            notified.set(notified.get() + 1);
        });

        store.forget_paths(&[removed, deleted, exact, directory.path().join("missing")]);

        let remaining = store.large().unwrap();
        assert!(!Arc::ptr_eq(&large, &remaining));
        assert_eq!(remaining.files[0].path, similar);
        assert_eq!(remaining.bytes(), 10);
        assert_eq!(remaining.duplicates, HashSet::from([similar.clone()]));
        assert_eq!(remaining.previews.len(), 1);
        assert_eq!(remaining.previews[&similar].icon.as_deref(), Some("icon"));
        for kind in [Kind::Models, Kind::Games] {
            let remaining = store.items(kind).unwrap();
            assert!(!Arc::ptr_eq(&items, &remaining));
            assert_eq!(remaining.items[0].location, similar);
            assert_eq!(remaining.bytes(), 10);
            assert_eq!(remaining.covers.len(), 1);
            assert_eq!(remaining.covers[&similar].pixels.as_ref(), [1, 2, 3, 4]);
            assert!(!store.loading(kind));
        }
        assert!(Arc::ptr_eq(&apps, &store.apps().unwrap()));
        assert_eq!(large.files.len(), 3);
        assert_eq!(large.previews.len(), 3);
        assert_eq!(large.duplicates.len(), 3);
        assert_eq!(items.items.len(), 4);
        assert_eq!(items.covers.len(), 4);
        assert_eq!(notifications.get(), 1);
        assert!(!store.busy());
    }

    #[test]
    fn decoded_thumbnails_cannot_enlarge_file_rows() {
        let pixbuf =
            gtk4::gdk_pixbuf::Pixbuf::new(gtk4::gdk_pixbuf::Colorspace::Rgb, true, 8, 256, 128)
                .unwrap();
        pixbuf.fill(0x123456ff);
        let thumbnail = bounded_thumbnail(&pixbuf).unwrap();
        assert_eq!((thumbnail.width, thumbnail.height), (36, 18));
        assert!(thumbnail.pixels.len() <= 36 * 36 * 4);
    }
    #[test]
    fn pages_and_overview_share_snapshots_rescans_and_failures() {
        let context = glib::MainContext::new();
        let _guard = context.acquire().unwrap();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let calls = Arc::new(AtomicUsize::new(0));
                    let counted = calls.clone();
                    let store = ReviewScanStore::with_job(Arc::new(move |kind, minimum, _| {
                        counted.fetch_add(1, Ordering::SeqCst);
                        if minimum == 42 {
                            return Err("scan failed".into());
                        }
                        Ok(match kind {
                            Kind::LargeFiles => {
                                Inventory::Large(Arc::new(LargeInventory::default()))
                            }
                            Kind::Apps => Inventory::Apps(Arc::new(AppInventory::default())),
                            Kind::Models | Kind::Games => Inventory::Items(Arc::default()),
                        })
                    }));
                    let (tx, rx) = async_channel::unbounded();
                    let weak = store.downgrade();
                    store.subscribe(move || {
                        let s = weak.upgrade().unwrap();
                        if !s.busy() {
                            tx.try_send(()).unwrap();
                        }
                    });
                    store.request_all(false);
                    while store.busy() {
                        rx.recv().await.unwrap();
                    }
                    let large = store.large().unwrap();
                    let apps = store.apps().unwrap();
                    store.request_all(false);
                    assert_eq!(calls.load(Ordering::SeqCst), 4);
                    assert!(Arc::ptr_eq(&large, &store.large().unwrap()));
                    assert!(Arc::ptr_eq(&apps, &store.apps().unwrap()));
                    store.request(Kind::Apps, true);
                    rx.recv().await.unwrap();
                    assert!(!Arc::ptr_eq(&apps, &store.apps().unwrap()));
                    assert!(Arc::ptr_eq(&large, &store.large().unwrap()));
                    store.request_large(42, true);
                    rx.recv().await.unwrap();
                    assert_eq!(
                        store.error(Kind::LargeFiles).as_deref(),
                        Some("scan failed")
                    );
                    assert!(Arc::ptr_eq(&large, &store.large().unwrap()));
                    assert_eq!(calls.load(Ordering::SeqCst), 6);
                })
            })
            .unwrap();
    }
}
