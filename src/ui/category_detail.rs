//! Detail view of one scan category, laid out like the storage analyzer: a
//! block map of the biggest folders, breadcrumbs, and a list of folders and
//! files with size bars. Every entry has a check box, so the user can keep
//! anything before cleaning. A back button returns to the dashboard.

use super::scan_overview::{format_size, safety_pill};
use super::storage_analyzer_page::{
    layout_binary, rounded_rectangle, treemap_color, truncate_label, TreemapVisual,
};
use crate::i18n::{format_count, tr, tr_args};
use crate::models::{RemovalMethod, Safety, ScanResult};
use crate::theme::ThemeSnapshot;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const MAX_TREEMAP_ITEMS: usize = 10;
const MAX_ROWS: usize = 300;

/// Treemap block on screen: x, y, width, height and the folder it opens.
type Hit = (f64, f64, f64, f64, Option<PathBuf>);

/// A folder or file inside the category. Folders with a single sub-folder
/// and no files of their own are merged into one entry ("a/b/c").
#[derive(Debug, Clone)]
pub struct Node {
    pub path: PathBuf,
    pub label: String,
    pub size: u64,
    pub files: usize,
    pub is_dir: bool,
    pub children: Vec<Node>,
}

#[derive(Default)]
struct Builder {
    size: u64,
    files: usize,
    dirs: BTreeMap<OsString, Builder>,
    leaves: Vec<(OsString, u64)>,
}

impl Builder {
    fn insert(&mut self, parts: &[OsString], size: u64) {
        self.size += size;
        self.files += 1;
        match parts {
            [] => {}
            [name] => self.leaves.push((name.clone(), size)),
            [dir, rest @ ..] => self.dirs.entry(dir.clone()).or_default().insert(rest, size),
        }
    }

    fn into_node(self, path: PathBuf, label: String) -> Node {
        let mut children: Vec<Node> = Vec::new();
        for (name, builder) in self.dirs {
            let child_path = path.join(&name);
            let child_label = name.to_string_lossy().into_owned();
            children.push(builder.into_node(child_path, child_label).compressed());
        }
        for (name, size) in self.leaves {
            children.push(Node {
                path: path.join(&name),
                label: name.to_string_lossy().into_owned(),
                size,
                files: 1,
                is_dir: false,
                children: Vec::new(),
            });
        }
        children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.label.cmp(&b.label)));
        Node {
            path,
            label,
            size: self.size,
            files: self.files,
            is_dir: true,
            children,
        }
    }
}

impl Node {
    /// Merge chains like `.cache` → `foo` → `bar` into one entry.
    fn compressed(mut self) -> Node {
        while self.is_dir && self.children.len() == 1 && self.children[0].is_dir {
            let child = self.children.remove(0);
            self.label = format!("{}/{}", self.label, child.label);
            self.path = child.path;
            self.children = child.children;
        }
        self
    }

    /// Tree of the category's files. The root is the deepest folder that
    /// holds all of them.
    pub fn from_result(result: &ScanResult, root_label: &str) -> Node {
        let mut builder = Builder::default();
        for file in &result.files {
            let parts: Vec<OsString> = file
                .path
                .components()
                .map(|c| c.as_os_str().to_os_string())
                .collect();
            builder.insert(&parts, file.size);
        }
        let mut root = builder.into_node(PathBuf::new(), root_label.to_string());
        // Walk down while there is only one folder, so the first screen
        // shows something useful instead of "/" → "home" → "user".
        while root.children.len() == 1 && root.children[0].is_dir {
            let child = root.children.remove(0);
            root.path = child.path;
            root.children = child.children;
        }
        root.label = root_label.to_string();
        root
    }

    fn at(&self, trail: &[usize]) -> &Node {
        let mut node = self;
        for index in trail {
            node = &node.children[*index];
        }
        node
    }
}

/// True when the path, or a folder that holds it, was unticked.
pub fn is_excluded(path: &Path, excluded: &HashSet<PathBuf>) -> bool {
    path.ancestors().any(|p| excluded.contains(p))
}

/// Copy of `result` without the unticked paths. Folders that hold an
/// unticked path are kept too, so they are never removed as "empty".
pub fn without_excluded(result: &ScanResult, excluded: &HashSet<PathBuf>) -> ScanResult {
    if excluded.is_empty() {
        return result.clone();
    }
    let mut kept = ScanResult::new();
    for file in &result.files {
        if !is_excluded(&file.path, excluded) {
            kept.add_file(file.clone());
        }
    }
    for dir in &result.directories {
        let holds_excluded = excluded.iter().any(|p| p.starts_with(&dir.path));
        if !is_excluded(&dir.path, excluded) && !holds_excluded {
            kept.add_directory(dir.clone());
        }
    }
    kept.whole_dirs = result
        .whole_dirs
        .iter()
        .filter(|dir| !is_excluded(dir, excluded) && !excluded.iter().any(|p| p.starts_with(dir)))
        .cloned()
        .collect();
    kept.skipped = result.skipped.clone();
    kept.security_violations = result.security_violations.clone();
    kept.errors = result.errors.clone();
    kept.cancelled = result.cancelled;
    kept
}

pub struct DetailCallbacks {
    pub on_back: Box<dyn Fn()>,
    pub on_open_rules: Box<dyn Fn(&'static str)>,
    pub on_excluded_changed: Box<dyn Fn(HashSet<PathBuf>)>,
    pub on_clean: Box<dyn Fn(ScanResult)>,
}

/// What the header shows.
pub struct Heading {
    pub title: String,
    pub explanation: String,
    pub icon: &'static str,
    pub safety: Option<Safety>,
    pub rules_page: Option<&'static str>,
}

struct State {
    title: String,
    editable: bool,
    found: ScanResult,
    root: Node,
    trail: RefCell<Vec<usize>>,
    excluded: RefCell<HashSet<PathBuf>>,
    hits: RefCell<Vec<Hit>>,
    treemap: gtk4::DrawingArea,
    breadcrumbs: gtk4::Box,
    select_all: gtk4::CheckButton,
    syncing: Cell<bool>,
    list: gtk4::ListBox,
    selected_label: gtk4::Label,
    clean_button: gtk4::Button,
    callbacks: DetailCallbacks,
}

/// Read-only view of what one rule would remove; nothing can be cleaned
/// from here. `on_close` runs when the user presses Close.
pub fn build_preview(
    heading: Heading,
    found: ScanResult,
    on_close: impl Fn() + 'static,
) -> gtk4::Widget {
    let callbacks = DetailCallbacks {
        on_back: Box::new(on_close),
        on_open_rules: Box::new(|_| {}),
        on_excluded_changed: Box::new(|_| {}),
        on_clean: Box::new(|_| {}),
    };
    build_view(heading, found, HashSet::new(), None, callbacks)
}

fn build_view(
    heading: Heading,
    found: ScanResult,
    excluded: HashSet<PathBuf>,
    method: Option<RemovalMethod>,
    callbacks: DetailCallbacks,
) -> gtk4::Widget {
    let editable = method.is_some();
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    page.set_margin_top(18);

    // Header: back, title with safety label, link to the rules.
    let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    let back = gtk4::Button::new();
    let back_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    back_content.append(&gtk4::Image::from_icon_name("pc-go-previous-symbolic"));
    back_content.append(&gtk4::Label::new(Some(&if editable {
        tr("Back")
    } else {
        tr("Close")
    })));
    back.set_child(Some(&back_content));
    back.set_valign(gtk4::Align::Center);
    header.append(&back);

    let titles = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    titles.set_hexpand(true);
    let title_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    title_row.append(&crate::ui::icon_image(heading.icon));
    let title = gtk4::Label::new(Some(&heading.title));
    title.add_css_class("title-2");
    title.set_wrap(true);
    title.set_xalign(0.0);
    title_row.append(&title);
    if let Some(safety) = heading.safety {
        title_row.append(&safety_pill(safety));
    }
    titles.append(&title_row);
    let explanation = gtk4::Label::new(Some(&heading.explanation));
    explanation.add_css_class("dim-label");
    explanation.set_xalign(0.0);
    explanation.set_wrap(true);
    titles.append(&explanation);
    header.append(&titles);

    let rules_button = heading.rules_page.map(|name| {
        let button = gtk4::Button::with_label(&tr("Edit Rules"));
        button.add_css_class("flat");
        button.set_valign(gtk4::Align::Center);
        (button, name)
    });
    if let Some((button, _)) = &rules_button {
        header.append(button);
    }
    page.append(&header);

    let summary = gtk4::Label::new(Some(&tr_args(
        "{count} files · {size} found",
        &[
            ("{count}", &format_count(found.file_count)),
            ("{size}", &format_size(found.total_size)),
        ],
    )));
    summary.add_css_class("heading");
    summary.set_xalign(0.0);
    page.append(&summary);

    let treemap_frame = gtk4::Frame::new(None);
    treemap_frame.add_css_class("storage-treemap-frame");
    let treemap = gtk4::DrawingArea::new();
    treemap.set_size_request(-1, 170);
    treemap.set_hexpand(true);
    treemap.set_tooltip_text(Some(&tr("Click a folder block to inspect its contents")));
    treemap_frame.set_child(Some(&treemap));
    page.append(&treemap_frame);

    // "Select all" for the folder on screen, then where we are.
    let list_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    list_header.set_margin_start(8);
    let select_all = gtk4::CheckButton::with_label(&tr("Select all"));
    select_all.set_tooltip_text(Some(&tr("Tick or untick everything in this folder")));
    list_header.append(&select_all);
    let breadcrumbs = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
    breadcrumbs.add_css_class("storage-breadcrumbs");
    breadcrumbs.set_hexpand(true);
    list_header.append(&breadcrumbs);
    page.append(&list_header);

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.add_css_class("storage-results-list");
    let scrolled = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .min_content_height(160)
        .build();
    scrolled.add_css_class("storage-results-scroll");
    scrolled.set_child(Some(&list));
    page.append(&scrolled);

    // Bottom bar: what is ticked, how it will be removed, clean button.
    let bottom = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    bottom.add_css_class("storage-selection-bar");
    let copy = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    copy.set_hexpand(true);
    let selected_label = gtk4::Label::new(None);
    selected_label.add_css_class("heading");
    selected_label.set_xalign(0.0);
    let (icon, method_text) = method_summary(method.unwrap_or_default());
    let method_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    method_row.append(&gtk4::Image::from_icon_name(icon));
    let method_label = gtk4::Label::new(Some(&method_text));
    method_label.add_css_class("caption");
    method_label.set_xalign(0.0);
    method_label.set_wrap(true);
    method_row.append(&method_label);
    copy.append(&selected_label);
    copy.append(&method_row);
    bottom.append(&copy);
    let clean_button = gtk4::Button::new();
    clean_button.add_css_class("pc-primary");
    clean_button.set_valign(gtk4::Align::Center);
    let clean_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    clean_content.append(&super::cleanup_fx::bolt_icon(18));
    clean_content.append(&gtk4::Label::new(None));
    clean_button.set_child(Some(&clean_content));
    bottom.append(&clean_button);
    page.append(&bottom);
    bottom.set_visible(editable);
    select_all.set_visible(editable);

    let root = Node::from_result(&found, &heading.title);
    let state = Rc::new(State {
        title: heading.title.clone(),
        editable,
        found,
        root,
        trail: RefCell::new(Vec::new()),
        excluded: RefCell::new(excluded),
        hits: RefCell::new(Vec::new()),
        treemap: treemap.clone(),
        breadcrumbs,
        select_all: select_all.clone(),
        syncing: Cell::new(false),
        list,
        selected_label,
        clean_button: clean_button.clone(),
        callbacks,
    });

    let weak = Rc::downgrade(&state);
    back.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            (s.callbacks.on_back)();
        }
    });
    if let Some((button, name)) = rules_button {
        let weak = Rc::downgrade(&state);
        button.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                (s.callbacks.on_open_rules)(name);
            }
        });
    }
    let weak = Rc::downgrade(&state);
    select_all.connect_toggled(move |check| {
        let Some(s) = weak.upgrade() else { return };
        if s.syncing.get() {
            return;
        }
        {
            let trail = s.trail.borrow();
            let current = s.root.at(&trail);
            let mut excluded = s.excluded.borrow_mut();
            for child in &current.children {
                excluded.retain(|p| !p.starts_with(&child.path));
                if !check.is_active() {
                    excluded.insert(child.path.clone());
                }
            }
        }
        (s.callbacks.on_excluded_changed)(s.excluded.borrow().clone());
        refresh(&s);
    });
    let weak = Rc::downgrade(&state);
    clean_button.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            let chosen = without_excluded(&s.found, &s.excluded.borrow());
            (s.callbacks.on_clean)(chosen);
        }
    });

    let weak = Rc::downgrade(&state);
    treemap.set_draw_func(move |area, cr, width, height| {
        if let Some(s) = weak.upgrade() {
            draw_treemap(&s, area, cr, width, height);
        }
    });
    let click = gtk4::GestureClick::new();
    let weak = Rc::downgrade(&state);
    click.connect_released(move |_, _, x, y| {
        let Some(s) = weak.upgrade() else { return };
        let path = s
            .hits
            .borrow()
            .iter()
            .find(|(hx, hy, w, h, _)| x >= *hx && x <= hx + w && y >= *hy && y <= hy + h)
            .and_then(|hit| hit.4.clone());
        if let Some(path) = path {
            later(&s, move |s| open_child(s, &path));
        }
    });
    treemap.add_controller(click);
    treemap.set_cursor_from_name(Some("pointer"));

    // The page owns the state; every other closure holds a weak reference.
    let keep = state.clone();
    page.connect_destroy(move |_| {
        keep.hits.borrow_mut().clear();
    });

    refresh(&state);
    page.upcast()
}

/// Run `change` after the current input event is done. Clicking a button
/// that rebuilds its own container would otherwise free the button while
/// GTK is still delivering the click (and its tooltip), which crashes GTK.
fn later(state: &Rc<State>, change: impl FnOnce(&Rc<State>) + 'static) {
    let weak = Rc::downgrade(state);
    glib::idle_add_local_once(move || {
        if let Some(s) = weak.upgrade() {
            change(&s);
        }
    });
}

fn open_child(state: &Rc<State>, path: &Path) {
    let index = {
        let trail = state.trail.borrow();
        let current = state.root.at(&trail);
        current
            .children
            .iter()
            .position(|c| c.is_dir && c.path == path)
    };
    if let Some(index) = index {
        state.trail.borrow_mut().push(index);
        refresh(state);
    }
}

fn refresh(state: &Rc<State>) {
    refresh_breadcrumbs(state);
    refresh_rows(state);
    refresh_selection(state);
    state.treemap.queue_draw();
}

fn refresh_breadcrumbs(state: &Rc<State>) {
    let container = &state.breadcrumbs;
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    let trail = state.trail.borrow().clone();
    for depth in 0..=trail.len() {
        if depth > 0 {
            let separator = gtk4::Label::new(Some("›"));
            separator.add_css_class("dim-label");
            container.append(&separator);
        }
        let node = state.root.at(&trail[..depth]);
        let button = gtk4::Button::with_label(&node.label);
        button.add_css_class("flat");
        button.add_css_class("storage-breadcrumb");
        if depth == 0 {
            button.set_tooltip_text(Some(&home_relative(&node.path)));
        }
        let weak = Rc::downgrade(state);
        button.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                later(&s, move |s| {
                    s.trail.borrow_mut().truncate(depth);
                    refresh(s);
                });
            }
        });
        container.append(&button);
    }
}

fn refresh_rows(state: &Rc<State>) {
    let list = &state.list;
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let trail = state.trail.borrow().clone();
    let current = state.root.at(&trail);
    let parent_excluded = is_excluded(&current.path, &state.excluded.borrow());
    state.select_all.set_sensitive(!parent_excluded);
    for child in current.children.iter().take(MAX_ROWS) {
        list.append(&row_for(state, child, current.size, parent_excluded));
    }
    if current.children.len() > MAX_ROWS {
        let rest = &current.children[MAX_ROWS..];
        let size: u64 = rest.iter().map(|n| n.size).sum();
        let label = gtk4::Label::new(Some(&tr_args(
            "and {count} smaller items ({size})",
            &[
                ("{count}", &format_count(rest.len())),
                ("{size}", &format_size(size)),
            ],
        )));
        label.add_css_class("dim-label");
        label.set_margin_top(12);
        label.set_margin_bottom(12);
        let row = gtk4::ListBoxRow::new();
        row.set_activatable(false);
        row.set_child(Some(&label));
        list.append(&row);
    }
}

fn row_for(
    state: &Rc<State>,
    node: &Node,
    parent_size: u64,
    parent_excluded: bool,
) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_activatable(false);
    row.add_css_class("storage-result-row");

    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    content.set_margin_top(7);
    content.set_margin_bottom(7);
    content.set_margin_start(8);
    content.set_margin_end(8);

    let check = gtk4::CheckButton::new();
    check.set_active(!is_excluded(&node.path, &state.excluded.borrow()));
    check.set_sensitive(!parent_excluded);
    check.set_visible(state.editable);
    check.set_tooltip_text(Some(&tr("Untick to keep it")));
    let weak = Rc::downgrade(state);
    let path = node.path.clone();
    check.connect_toggled(move |check| {
        let Some(s) = weak.upgrade() else { return };
        {
            let mut excluded = s.excluded.borrow_mut();
            if check.is_active() {
                excluded.remove(&path);
            } else {
                // A folder entry replaces any choices made inside it.
                excluded.retain(|p| !p.starts_with(&path));
                excluded.insert(path.clone());
            }
        }
        refresh_selection(&s);
        (s.callbacks.on_excluded_changed)(s.excluded.borrow().clone());
    });
    content.append(&check);

    let icon = gtk4::Image::from_icon_name(if node.is_dir {
        "pc-folder-symbolic"
    } else {
        "pc-text-x-generic-symbolic"
    });
    icon.add_css_class("accent");
    content.append(&icon);

    let labels = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
    labels.set_hexpand(true);
    let name = gtk4::Label::new(Some(&node.label));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    name.add_css_class("heading");
    let details_text = if node.is_dir {
        tr_args(
            "Folder · {count} files · {path}",
            &[
                ("{count}", &format_count(node.files)),
                ("{path}", &home_relative(&node.path)),
            ],
        )
    } else {
        tr_args("File · {path}", &[("{path}", &home_relative(&node.path))])
    };
    let details = gtk4::Label::new(Some(&details_text));
    details.set_xalign(0.0);
    details.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    details.add_css_class("caption");
    details.set_tooltip_text(Some(&node.path.display().to_string()));
    let target = node.path.clone();
    super::file_actions::path_clicks(&labels, move || Some(target.clone()));
    labels.append(&name);
    labels.append(&details);
    content.append(&labels);

    let bar = gtk4::ProgressBar::new();
    bar.set_width_request(150);
    bar.set_fraction(if parent_size > 0 {
        node.size as f64 / parent_size as f64
    } else {
        0.0
    });
    bar.set_valign(gtk4::Align::Center);
    bar.add_css_class("storage-size-bar");
    content.append(&bar);

    let size = gtk4::Label::new(Some(&format_size(node.size)));
    size.set_width_chars(11);
    size.set_xalign(1.0);
    size.add_css_class("heading");
    content.append(&size);

    content.append(&super::file_actions::buttons(&node.path, node.is_dir));

    if node.is_dir {
        let open = gtk4::Button::from_icon_name("pc-go-next-symbolic");
        open.add_css_class("flat");
        open.set_tooltip_text(Some(&tr("Inspect this folder")));
        let weak = Rc::downgrade(state);
        let path = node.path.clone();
        open.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                let path = path.clone();
                later(&s, move |s| open_child(s, &path));
            }
        });
        content.append(&open);
    } else {
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_width_request(34);
        content.append(&spacer);
    }

    row.set_child(Some(&content));
    row
}

/// Tick "Select all" when nothing in the open folder is unticked.
fn sync_select_all(state: &Rc<State>) {
    let all = {
        let trail = state.trail.borrow();
        let current = state.root.at(&trail);
        let excluded = state.excluded.borrow();
        !is_excluded(&current.path, &excluded)
            && !excluded.iter().any(|p| p.starts_with(&current.path))
    };
    state.syncing.set(true);
    state.select_all.set_active(all);
    state.syncing.set(false);
}

fn refresh_selection(state: &Rc<State>) {
    sync_select_all(state);
    let chosen = without_excluded(&state.found, &state.excluded.borrow());
    let kept = state.found.file_count.saturating_sub(chosen.file_count);
    let mut text = tr_args(
        "{count} files selected · {size}",
        &[
            ("{count}", &format_count(chosen.file_count)),
            ("{size}", &format_size(chosen.total_size)),
        ],
    );
    if kept > 0 {
        text.push_str(" · ");
        text.push_str(&tr_args(
            "{count} kept",
            &[("{count}", &format_count(kept))],
        ));
    }
    state.selected_label.set_text(&text);
    let label = state
        .clean_button
        .child()
        .and_then(|content| content.last_child())
        .and_downcast::<gtk4::Label>();
    label
        .as_ref()
        .unwrap_or(&gtk4::Label::new(None))
        .set_text(&tr_args(
            "Clean {category} ({size})",
            &[
                ("{category}", &state.title),
                ("{size}", &format_size(chosen.total_size)),
            ],
        ));
    state.clean_button.set_sensitive(!chosen.is_empty());
}

fn draw_treemap(
    state: &Rc<State>,
    area: &gtk4::DrawingArea,
    cr: &gtk4::cairo::Context,
    width: i32,
    height: i32,
) {
    let theme = ThemeSnapshot::from_widget(area);
    let trail = state.trail.borrow().clone();
    let current = state.root.at(&trail);
    let keep = current.children.len().min(MAX_TREEMAP_ITEMS - 1);
    let mut visuals: Vec<TreemapVisual> = current
        .children
        .iter()
        .take(keep)
        .enumerate()
        .map(|(index, node)| TreemapVisual {
            label: node.label.clone(),
            path: node.is_dir.then(|| node.path.clone()),
            size: node.size,
            color_index: index,
        })
        .collect();
    let shown: u64 = visuals.iter().map(|v| v.size).sum();
    let other = current.size.saturating_sub(shown);
    if other > 0 {
        visuals.push(TreemapVisual {
            label: tr("Other"),
            path: None,
            size: other,
            color_index: usize::MAX,
        });
    }

    let mut layouts = Vec::new();
    layout_binary(
        &visuals,
        5.0,
        5.0,
        (width as f64 - 10.0).max(0.0),
        (height as f64 - 10.0).max(0.0),
        &mut layouts,
    );
    let (bg_r, bg_g, bg_b, bg_a) = crate::theme::rgba_to_cairo(&theme.shade);
    cr.set_source_rgba(bg_r, bg_g, bg_b, bg_a.max(0.16));
    cr.rectangle(0.0, 0.0, width as f64, height as f64);
    let _ = cr.fill();

    let excluded = state.excluded.borrow();
    let mut hits = Vec::new();
    for (visual, x, y, w, h) in layouts {
        let pad = 2.5;
        let (x, y, w, h) = (
            x + pad,
            y + pad,
            (w - pad * 2.0).max(0.0),
            (h - pad * 2.0).max(0.0),
        );
        if w < 2.0 || h < 2.0 {
            continue;
        }
        let (r, g, b) = treemap_color(visual.color_index, theme.is_dark);
        let kept = visual
            .path
            .as_ref()
            .is_some_and(|p| is_excluded(p, &excluded));
        rounded_rectangle(cr, x, y, w, h, 6.0);
        cr.set_source_rgba(r, g, b, if kept { 0.35 } else { 1.0 });
        let _ = cr.fill();
        hits.push((x, y, w, h, visual.path.clone()));

        if w >= 72.0 && h >= 38.0 {
            let _ = cr.save();
            rounded_rectangle(cr, x, y, w, h, 6.0);
            cr.clip();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.96);
            cr.select_font_face(
                crate::fonts::FAMILY,
                gtk4::cairo::FontSlant::Normal,
                gtk4::cairo::FontWeight::Bold,
            );
            cr.set_font_size(12.0);
            cr.move_to(x + 9.0, y + 18.0);
            let _ = cr.show_text(&truncate_label(&visual.label, w));
            if h >= 56.0 {
                cr.select_font_face(
                    crate::fonts::FAMILY,
                    gtk4::cairo::FontSlant::Normal,
                    gtk4::cairo::FontWeight::Normal,
                );
                cr.set_font_size(10.0);
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.78);
                cr.move_to(x + 9.0, y + 35.0);
                let _ = cr.show_text(&format_size(visual.size));
            }
            let _ = cr.restore();
        }
    }
    state.hits.replace(hits);
}

fn home_relative(path: &Path) -> String {
    match dirs::home_dir() {
        Some(home) if path.starts_with(&home) => {
            format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display())
        }
        _ => path.display().to_string(),
    }
}

/// Icon and one-line description of what the removal method does.
fn method_summary(method: RemovalMethod) -> (&'static str, String) {
    match method {
        RemovalMethod::Secure => (
            "pc-security-high-symbolic",
            tr("Secure delete: each file is overwritten before it is deleted. This cannot be undone."),
        ),
        RemovalMethod::Trash => (
            "pc-user-trash-symbolic",
            tr("Files go to the trash, so you can restore them until you empty it."),
        ),
        RemovalMethod::Permanent => (
            "pc-edit-delete-symbolic",
            tr("Files are deleted permanently. This cannot be undone."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FileEntry;

    fn result(paths: &[(&str, u64)]) -> ScanResult {
        let mut r = ScanResult::new();
        for (p, s) in paths {
            r.add_file(FileEntry::new(PathBuf::from(p), *s, false, false));
        }
        r
    }

    #[test]
    fn tree_starts_at_the_common_folder_and_merges_single_chains() {
        let r = result(&[
            ("/home/u/.cache/a/x/y/big", 50),
            ("/home/u/.cache/b/small", 5),
        ]);
        let root = Node::from_result(&r, "Junk");
        assert_eq!(root.path, PathBuf::from("/home/u/.cache"));
        assert_eq!(root.size, 55);
        assert_eq!(root.children[0].label, "a/x/y");
        assert_eq!(root.children[0].children[0].label, "big");
        assert_eq!(root.children[1].label, "b");
    }

    #[test]
    fn unticked_folders_drop_their_files_and_keep_parent_directories() {
        let mut r = result(&[("/c/a/1", 10), ("/c/a/2", 10), ("/c/b/3", 7)]);
        r.add_directory(FileEntry::new(PathBuf::from("/c"), 0, true, false));
        r.add_directory(FileEntry::new(PathBuf::from("/c/a"), 0, true, false));
        r.add_directory(FileEntry::new(PathBuf::from("/c/b"), 0, true, false));
        let excluded: HashSet<PathBuf> = [PathBuf::from("/c/a")].into();
        let kept = without_excluded(&r, &excluded);
        assert_eq!(kept.total_size, 7);
        assert_eq!(kept.file_count, 1);
        let dirs: Vec<_> = kept.directories.iter().map(|d| d.path.clone()).collect();
        assert_eq!(dirs, vec![PathBuf::from("/c/b")]);
    }
}
