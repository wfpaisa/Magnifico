//! What each category page currently has selected, and how much it lists in
//! all, so the map on that page can show the selection against the whole.
use std::{cell::RefCell, rc::Rc};

type Listener = Rc<dyn Fn(usize)>;

struct Bus {
    /// Selected and total bytes by category.
    bytes: [(u64, u64); 10],
    /// Which of the two uninstaller maps (apps 4, leftovers 5) is on screen.
    apps_view: usize,
    listeners: Vec<Listener>,
}

thread_local! {
    static BUS: RefCell<Bus> = RefCell::new(Bus {
        bytes: [(0, 0); 10],
        apps_view: 4,
        listeners: Vec::new(),
    });
}

/// A page tells how many bytes of `category` are selected now, out of the
/// `total` it lists.
pub fn report(category: usize, selected: u64, total: u64) {
    let listeners = BUS.with(|bus| {
        let mut bus = bus.borrow_mut();
        let Some(slot) = bus.bytes.get_mut(category) else {
            return Vec::new();
        };
        if *slot == (selected, total) {
            return Vec::new();
        }
        *slot = (selected, total);
        bus.listeners.clone()
    });
    for listener in listeners {
        listener(category);
    }
}

/// Selected bytes and listed bytes of a category page.
pub fn state(category: usize) -> (u64, u64) {
    BUS.with(|bus| bus.borrow().bytes.get(category).copied().unwrap_or((0, 0)))
}

/// Called with the category whose numbers changed.
pub fn subscribe(listener: impl Fn(usize) + 'static) {
    BUS.with(|bus| bus.borrow_mut().listeners.push(Rc::new(listener)));
}

/// The uninstaller page shows `category` (4 apps or 5 leftovers) in its tab.
pub fn set_apps_view(category: usize) {
    let listeners = BUS.with(|bus| {
        let mut bus = bus.borrow_mut();
        if bus.apps_view == category {
            return Vec::new();
        }
        bus.apps_view = category;
        bus.listeners.clone()
    });
    for listener in listeners {
        listener(category);
    }
}

/// The uninstaller category whose map is on screen.
pub fn apps_view() -> usize {
    BUS.with(|bus| bus.borrow().apps_view)
}
