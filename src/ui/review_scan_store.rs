//! Shared snapshots for Overview, sidebar and both review pages.
//! Visits reuse a completed scan; only explicit scans replace it.
use crate::services::{
    self,
    app_leftovers::{self, AppLeftover, Status},
    app_uninstall::{self, InstalledApp},
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
#[derive(Default)]
pub struct Thumbnail {
    pub pixels: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub alpha: bool,
}
#[derive(Default)]
pub struct FilePreview {
    pub thumbnail: Option<Thumbnail>,
    pub icon: Option<String>,
}
#[derive(Default)]
pub struct LargeInventory {
    pub files: Vec<FoundFile>,
    pub duplicates: HashSet<PathBuf>,
    pub previews: HashMap<PathBuf, FilePreview>,
}
#[derive(Default)]
pub struct ItemInventory {
    pub items: Vec<ReviewItem>,
    /// Decoded covers keyed by item location.
    pub covers: HashMap<PathBuf, Thumbnail>,
}
impl ItemInventory {
    fn new(items: Vec<ReviewItem>) -> Self {
        let covers = items
            .iter()
            .filter_map(|i| {
                let cover = decode_cover(i.icon.as_deref()?)?;
                Some((i.location.clone(), cover))
            })
            .collect();
        Self { items, covers }
    }
    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }
}
#[derive(Default)]
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
}
type Job = Arc<dyn Fn(Kind, u64, &AtomicBool) -> Result<Inventory, String> + Send + Sync>;
struct State {
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
        Self::with_job(Arc::new(scan))
    }
    fn with_job(job: Job) -> Self {
        Self(Rc::new(State {
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
    pub fn request_all(&self, force: bool) {
        self.request(Kind::LargeFiles, force);
        self.request(Kind::Apps, force);
        self.request(Kind::Models, force);
        self.request(Kind::Games, force);
    }
    pub fn request_large(&self, minimum: u64, force: bool) {
        if self.loading(Kind::LargeFiles) {
            return;
        }
        let changed = self.0.minimum.replace(minimum) != minimum;
        self.request(Kind::LargeFiles, force || changed);
    }
    pub fn request(&self, kind: Kind, force: bool) {
        {
            let mut slots = self.0.slots.borrow_mut();
            let slot = &mut slots[kind.index()];
            if slot.loading || (!force && slot.inventory.is_some()) {
                return;
            }
            slot.loading = true;
            slot.error = None;
        }
        self.notify();
        let job = self.0.job.clone();
        let cancel = self.0.cancel.clone();
        let minimum = self.minimum();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
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
                        Ok(Ok(inventory)) => slot.inventory = Some(inventory),
                        Ok(Err(error)) => slot.error = Some(error),
                        Err(error) => slot.error = Some(error.to_string()),
                    }
                }
                store.notify();
            }
        });
    }
}
fn scan(kind: Kind, minimum: u64, cancel: &AtomicBool) -> Result<Inventory, String> {
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
            let previews = files.iter().map(|f| (f.path.clone(), preview(f))).collect();
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
        Kind::Models => Ok(Inventory::Items(Arc::new(ItemInventory::new(
            review_items::scan_models(&home, cancel),
        )))),
        Kind::Games => Ok(Inventory::Items(Arc::new(ItemInventory::new(
            review_items::scan_games(&home, cancel),
        )))),
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
        pixels: bounded.read_pixel_bytes().to_vec(),
        width: bounded.width(),
        height: bounded.height(),
        stride: bounded.rowstride() as usize,
        alpha: bounded.has_alpha(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
