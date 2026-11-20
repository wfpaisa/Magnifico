//! What the cleanup screen offers: everything found, grouped by concept
//! (system junk, Rust builds, games…) and ranked by what deleting it costs
//! the user. The screen only flips `selected` flags; this module does the
//! arithmetic.

use super::ScanResult;
use std::collections::HashSet;
use std::path::PathBuf;

/// What happens after deleting something, from harmless to costly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Impact {
    /// Rebuilt on its own; nothing to notice.
    Free,
    /// Comes back the next time a project is built; that build is slower.
    Rebuild,
    /// Has to be downloaded again, using time and data.
    Redownload,
}

impl Impact {
    pub const ALL: [Impact; 3] = [Self::Free, Self::Rebuild, Self::Redownload];

    pub fn title(self) -> &'static str {
        match self {
            Self::Free => "No Impact",
            Self::Rebuild => "Rebuild",
            Self::Redownload => "Download Again",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Free => "Comes back on its own",
            Self::Rebuild => "The next build takes longer",
            Self::Redownload => "Uses your connection to get it back",
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Free => "pc-magnifico-free-symbolic",
            Self::Rebuild => "pc-magnifico-rebuild-symbolic",
            Self::Redownload => "pc-magnifico-redownload-symbolic",
        }
    }

    /// CSS class carrying the tier colour (see `style.css`).
    pub fn css_class(self) -> &'static str {
        match self {
            Self::Free => "impact-free",
            Self::Rebuild => "impact-rebuild",
            Self::Redownload => "impact-redownload",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    System,
    AppCaches,
    DevCaches,
    Logs,
    Browsers,
    Custom,
    RustBuilds,
    PythonNode,
    OtherBuilds,
    Games,
    AiModels,
}

impl GroupKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::AppCaches => "App Caches",
            Self::DevCaches => "Programming Caches",
            Self::Logs => "Old Logs",
            Self::Browsers => "Browsers",
            Self::Custom => "Your Folders",
            Self::RustBuilds => "Rust Builds",
            Self::PythonNode => "Python and Node",
            Self::OtherBuilds => "Other Builds",
            Self::Games => "Games",
            Self::AiModels => "AI Models",
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::System => "sidebar/user-system",
            Self::AppCaches => "sidebar/applications",
            Self::DevCaches => "pc-utilities-terminal-symbolic",
            Self::Logs => "pc-text-x-generic-symbolic",
            Self::Browsers => "sidebar/browsers",
            Self::Custom => "sidebar/custom-directories",
            Self::RustBuilds => "pc-package-x-generic-symbolic",
            Self::PythonNode => "pc-application-x-addon-symbolic",
            Self::OtherBuilds => "pc-applications-engineering-symbolic",
            Self::Games => "pc-input-gaming-symbolic",
            Self::AiModels => "pc-magnifico-ai-symbolic",
        }
    }

    pub fn impact(self) -> Impact {
        match self {
            Self::System
            | Self::AppCaches
            | Self::DevCaches
            | Self::Logs
            | Self::Browsers
            | Self::Custom => Impact::Free,
            Self::RustBuilds | Self::PythonNode | Self::OtherBuilds => Impact::Rebuild,
            Self::Games | Self::AiModels => Impact::Redownload,
        }
    }

    /// Plain-language answers for the hover card: what it is, what
    /// happens if it goes, and how it comes back.
    pub fn explanation(self) -> [&'static str; 3] {
        match self {
            Self::System => [
                "Temporary files, thumbnails, crash reports, package caches and the trash.",
                "Nothing you use changes. Files in the trash are gone for good.",
                "The system creates new ones as it needs them.",
            ],
            Self::AppCaches => [
                "Copies apps keep to open faster.",
                "The first launch of each app may be a little slower.",
                "Each app rebuilds its cache by itself.",
            ],
            Self::DevCaches => [
                "Packages that cargo, npm, pip and other tools downloaded before.",
                "Your projects keep working.",
                "Each tool downloads what a project needs on its next build.",
            ],
            Self::Logs => [
                "Old log files past the retention period you set.",
                "You lose old history that apps wrote for debugging.",
                "Apps keep writing new logs.",
            ],
            Self::Browsers => [
                "Browser caches, plus the cookies, history or sessions you chose.",
                "Pages load slower once. Cookies sign you out of websites.",
                "The browser fills its cache again as you browse.",
            ],
            Self::Custom => [
                "Folders you added yourself in Custom Directories.",
                "Whatever is inside is removed.",
                "Only if the program that made them creates them again.",
            ],
            Self::RustBuilds => [
                "The target folders where cargo keeps compiled code.",
                "Your source code is untouched. The next build compiles everything again.",
                "Run cargo build in the project.",
            ],
            Self::PythonNode => [
                "Virtual environments and node_modules folders of your projects.",
                "The project will not run until its dependencies are installed again.",
                "Run npm install, uv sync or pip install in the project.",
            ],
            Self::OtherBuilds => [
                "Build output of Gradle, Maven, Flutter, Zig and other tools.",
                "The next build starts from scratch.",
                "Build the project again.",
            ],
            Self::Games => [
                "Installed games and Steam's shader cache. Save games are kept.",
                "The game disappears from your library until you install it again.",
                "Install it again from Steam or Heroic.",
            ],
            Self::AiModels => [
                "AI models downloaded by Hugging Face, Ollama, LM Studio and similar tools.",
                "The next time a program needs the model, it waits for the download.",
                "The program downloads it again, or pull it yourself.",
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub label: String,
    /// Shown as a tooltip: where it lives.
    pub detail: String,
    pub result: ScanResult,
    /// Changed recently: the user is probably working on it, so it is never
    /// picked by the goal slider. It can still be ticked by hand.
    pub pinned: bool,
    /// A running program uses it: it cannot be ticked at all.
    pub busy: bool,
    pub selected: bool,
    /// The label is fixed English text (a rule name), not a file name, so
    /// the screen may translate it.
    pub translatable: bool,
    /// Where the item lives when that is not the home folder's disk, such
    /// as `/home2`. Freeing it does not free space on the home disk.
    pub disk: Option<String>,
}

impl PlanItem {
    pub fn new(label: impl Into<String>, detail: impl Into<String>, result: ScanResult) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            result,
            pinned: false,
            busy: false,
            selected: false,
            translatable: false,
            disk: None,
        }
    }

    pub fn translatable(mut self) -> Self {
        self.translatable = true;
        self
    }

    pub fn size(&self) -> u64 {
        self.result.total_size
    }

    /// Counted against the home disk.
    pub fn on_home_disk(&self) -> bool {
        self.disk.is_none()
    }

    pub fn can_select(&self) -> bool {
        !self.busy
    }

    /// Picked by the goal slider and by ticking a whole group.
    pub fn auto_selectable(&self) -> bool {
        !self.busy && !self.pinned
    }
}

#[derive(Debug, Clone)]
pub struct PlanGroup {
    pub kind: GroupKind,
    pub items: Vec<PlanItem>,
}

impl PlanGroup {
    pub fn impact(&self) -> Impact {
        self.kind.impact()
    }

    pub fn size(&self) -> u64 {
        self.items.iter().map(PlanItem::size).sum()
    }

    /// What ticking the whole group would free.
    pub fn offered_size(&self) -> u64 {
        self.items
            .iter()
            .filter(|item| item.auto_selectable())
            .map(PlanItem::size)
            .sum()
    }

    pub fn selected_size(&self) -> u64 {
        self.items
            .iter()
            .filter(|item| item.selected)
            .map(PlanItem::size)
            .sum()
    }

    /// What the group holds on the home disk.
    pub fn home_size(&self) -> u64 {
        self.items
            .iter()
            .filter(|item| item.on_home_disk())
            .map(PlanItem::size)
            .sum()
    }

    pub fn home_selected_size(&self) -> u64 {
        self.items
            .iter()
            .filter(|item| item.selected && item.on_home_disk())
            .map(PlanItem::size)
            .sum()
    }

    /// What the group holds on `disk` (`None`: the home disk).
    pub fn size_on(&self, disk: Option<&str>) -> u64 {
        self.items
            .iter()
            .filter(|item| item.disk.as_deref() == disk)
            .map(PlanItem::size)
            .sum()
    }

    pub fn selected_on(&self, disk: Option<&str>) -> u64 {
        self.items
            .iter()
            .filter(|item| item.selected && item.disk.as_deref() == disk)
            .map(PlanItem::size)
            .sum()
    }

    /// Other disks the group has items on, in first-seen order.
    pub fn other_disks(&self) -> Vec<&str> {
        let mut disks: Vec<&str> = Vec::new();
        for disk in self.items.iter().filter_map(|item| item.disk.as_deref()) {
            if !disks.contains(&disk) {
                disks.push(disk);
            }
        }
        disks
    }

    pub fn selected_count(&self) -> usize {
        self.items.iter().filter(|item| item.selected).count()
    }

    pub fn offered_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.auto_selectable())
            .count()
    }

    /// All, some or none of the offered items ticked.
    pub fn state(&self) -> Tick {
        tick(
            self.selected_size(),
            self.offered_size().max(1),
            self.selected_count(),
        )
    }

    /// Tick every offered item, or untick everything.
    pub fn set_all(&mut self, on: bool) {
        for item in &mut self.items {
            item.selected = on && item.auto_selectable();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    None,
    Some,
    All,
}

fn tick(selected: u64, offered: u64, selected_count: usize) -> Tick {
    if selected_count == 0 {
        Tick::None
    } else if selected >= offered {
        Tick::All
    } else {
        Tick::Some
    }
}

#[derive(Debug, Clone, Default)]
pub struct CleanupPlan {
    pub groups: Vec<PlanGroup>,
}

impl CleanupPlan {
    /// Keep groups with something in them, items largest first, groups in
    /// impact order then largest first.
    pub fn new(groups: Vec<PlanGroup>) -> Self {
        let mut groups: Vec<PlanGroup> = groups
            .into_iter()
            .map(|mut group| {
                group.items.retain(|item| item.size() > 0);
                group
                    .items
                    .sort_by_key(|item| std::cmp::Reverse(item.size()));
                group
            })
            .filter(|group| !group.items.is_empty())
            .collect();
        groups.sort_by(|a, b| a.impact().cmp(&b.impact()).then(b.size().cmp(&a.size())));
        Self { groups }
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn total_size(&self) -> u64 {
        self.groups.iter().map(PlanGroup::size).sum()
    }

    /// Drop cleaned paths, keeping the order and selection of what is left.
    pub fn remove_cleaned(&mut self, cleaned_paths: &HashSet<PathBuf>) {
        if cleaned_paths.is_empty() {
            return;
        }
        for group in &mut self.groups {
            for item in &mut group.items {
                item.result.remove_cleaned(cleaned_paths);
            }
            group.items.retain(|item| !item.result.is_empty());
        }
        self.groups.retain(|group| !group.items.is_empty());
    }

    pub fn offered_size(&self) -> u64 {
        self.groups.iter().map(PlanGroup::offered_size).sum()
    }

    pub fn home_total_size(&self) -> u64 {
        self.groups.iter().map(PlanGroup::home_size).sum()
    }

    pub fn home_selected_size(&self) -> u64 {
        self.groups.iter().map(PlanGroup::home_selected_size).sum()
    }

    /// Disks other than the home one that hold items, in first-seen order.
    pub fn other_disks(&self) -> Vec<String> {
        let mut disks: Vec<String> = Vec::new();
        for group in &self.groups {
            for disk in group.other_disks() {
                if !disks.iter().any(|known| known == disk) {
                    disks.push(disk.to_string());
                }
            }
        }
        disks
    }

    /// Selected bytes on each disk other than the home one.
    pub fn other_disks_selected(&self) -> Vec<(String, u64)> {
        let mut disks: Vec<(String, u64)> = Vec::new();
        for item in self.groups.iter().flat_map(|group| &group.items) {
            let (Some(disk), true) = (&item.disk, item.selected) else {
                continue;
            };
            match disks.iter_mut().find(|(name, _)| name == disk) {
                Some((_, size)) => *size += item.size(),
                None => disks.push((disk.clone(), item.size())),
            }
        }
        disks
    }

    pub fn selected_size(&self) -> u64 {
        self.groups.iter().map(PlanGroup::selected_size).sum()
    }

    /// Everything without impact: the starting goal.
    pub fn default_goal(&self) -> u64 {
        self.groups
            .iter()
            .filter(|group| group.impact() == Impact::Free)
            .map(PlanGroup::offered_size)
            .sum()
    }

    pub fn impact_selected_size(&self, impact: Impact) -> u64 {
        self.groups
            .iter()
            .filter(|group| group.impact() == impact)
            .map(PlanGroup::selected_size)
            .sum()
    }

    pub fn impact_offered_size(&self, impact: Impact) -> u64 {
        self.groups
            .iter()
            .filter(|group| group.impact() == impact)
            .map(PlanGroup::offered_size)
            .sum()
    }

    pub fn impact_state(&self, impact: Impact) -> Tick {
        let count = self
            .groups
            .iter()
            .filter(|group| group.impact() == impact)
            .map(PlanGroup::selected_count)
            .sum();
        tick(
            self.impact_selected_size(impact),
            self.impact_offered_size(impact).max(1),
            count,
        )
    }

    pub fn set_impact(&mut self, impact: Impact, on: bool) {
        for group in &mut self.groups {
            if group.impact() == impact {
                group.set_all(on);
            }
        }
    }

    /// Pick the least costly items first (by impact, then largest) until
    /// `goal` bytes are reached. Pinned and busy items are never picked.
    pub fn apply_goal(&mut self, goal: u64) {
        let mut order: Vec<(Impact, u64, usize, usize)> = Vec::new();
        for (g, group) in self.groups.iter_mut().enumerate() {
            for (i, item) in group.items.iter_mut().enumerate() {
                item.selected = false;
                if item.auto_selectable() {
                    order.push((group.kind.impact(), item.size(), g, i));
                }
            }
        }
        order.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let mut picked = 0u64;
        for (_, size, g, i) in order {
            if picked >= goal {
                break;
            }
            self.groups[g].items[i].selected = true;
            picked += size;
        }
    }

    /// Ticked items, split into what may hold private data (cleaned with
    /// the user's removal method) and regenerable bulk (builds, games,
    /// models) that is simply unlinked.
    pub fn selected_results(&self) -> (ScanResult, ScanResult) {
        let mut private = ScanResult::new();
        let mut bulk = ScanResult::new();
        for group in &self.groups {
            let target = if group.impact() == Impact::Free {
                &mut private
            } else {
                &mut bulk
            };
            for item in group.items.iter().filter(|item| item.selected) {
                target.merge(item.result.clone());
            }
        }
        (private, bulk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FileEntry;
    use std::path::PathBuf;

    fn item(name: &str, size: u64) -> PlanItem {
        let mut result = ScanResult::new();
        result.add_file(FileEntry::new(
            PathBuf::from(format!("/tmp/plan-test/{name}")),
            size,
            false,
            false,
        ));
        PlanItem::new(name, name, result)
    }

    #[test]
    fn items_on_another_disk_do_not_count_for_the_home_disk() {
        let mut away = item("game", 100);
        away.disk = Some("/home2".into());
        away.selected = true;
        let mut home = item("cache", 10);
        home.selected = true;
        let plan = CleanupPlan::new(vec![PlanGroup {
            kind: GroupKind::Games,
            items: vec![away, home],
        }]);
        assert_eq!(plan.selected_size(), 110);
        assert_eq!(plan.home_selected_size(), 10);
        assert_eq!(plan.home_total_size(), 10);
        assert_eq!(
            plan.other_disks_selected(),
            vec![("/home2".to_string(), 100)]
        );
        assert_eq!(plan.groups[0].other_disks(), vec!["/home2"]);
    }

    fn plan() -> CleanupPlan {
        let mut pinned = item("active", 500);
        pinned.pinned = true;
        CleanupPlan::new(vec![
            PlanGroup {
                kind: GroupKind::RustBuilds,
                items: vec![item("old", 300), pinned, item("empty", 0)],
            },
            PlanGroup {
                kind: GroupKind::System,
                items: vec![item("tmp", 100), item("thumbs", 50)],
            },
            PlanGroup {
                kind: GroupKind::Games,
                items: vec![item("game", 1000)],
            },
        ])
    }

    #[test]
    fn orders_groups_by_impact_and_drops_empty_items() {
        let plan = plan();
        let kinds: Vec<GroupKind> = plan.groups.iter().map(|g| g.kind).collect();
        assert_eq!(
            kinds,
            vec![GroupKind::System, GroupKind::RustBuilds, GroupKind::Games]
        );
        assert_eq!(plan.groups[1].items.len(), 2);
        assert_eq!(plan.default_goal(), 150);
    }

    #[test]
    fn goal_takes_cheapest_first_and_skips_pinned() {
        let mut plan = plan();
        plan.apply_goal(200);
        assert_eq!(plan.selected_size(), 150 + 300);
        assert!(plan.groups[1]
            .items
            .iter()
            .all(|i| !i.pinned || !i.selected));
        assert_eq!(plan.groups[2].selected_size(), 0);

        plan.apply_goal(u64::MAX);
        assert_eq!(plan.selected_size(), 150 + 300 + 1000);
    }

    #[test]
    fn group_state_and_split_results() {
        let mut plan = plan();
        plan.groups[1].set_all(true);
        assert_eq!(plan.groups[1].state(), Tick::All);
        plan.groups[0].items[0].selected = true;
        assert_eq!(plan.groups[0].state(), Tick::Some);
        let (private, bulk) = plan.selected_results();
        assert_eq!(private.total_size, 100);
        assert_eq!(bulk.total_size, 300);
    }

    #[test]
    fn cleaned_items_and_empty_groups_disappear_without_reselecting() {
        let mut plan = plan();
        plan.apply_goal(200);
        let cleaned = HashSet::from([
            PathBuf::from("/tmp/plan-test/tmp"),
            PathBuf::from("/tmp/plan-test/thumbs"),
            PathBuf::from("/tmp/plan-test/old"),
        ]);

        plan.remove_cleaned(&cleaned);

        assert_eq!(plan.groups.len(), 2);
        assert_eq!(plan.groups[0].kind, GroupKind::RustBuilds);
        assert_eq!(plan.groups[0].items.len(), 1);
        assert!(plan.groups[0].items[0].pinned);
        assert!(!plan.groups[0].items[0].selected);
        assert!(!plan.groups[1].items[0].selected);
        assert_eq!(plan.total_size(), 1500);
        assert_eq!(plan.offered_size(), 1000);
        assert_eq!(plan.selected_size(), 0);
    }

    #[test]
    fn partial_cleanup_keeps_failed_and_skipped_paths_and_recomputes_counts() {
        let mut remaining = item("removed", 100);
        remaining.selected = true;
        remaining.result.merge(item("failed", 20).result);
        remaining.result.merge(item("in-use", 30).result);
        remaining.result.add_directory(FileEntry::new(
            PathBuf::from("/tmp/plan-test/empty-dir"),
            0,
            true,
            false,
        ));
        remaining.result.add_directory(FileEntry::new(
            PathBuf::from("/tmp/plan-test/kept-dir"),
            0,
            true,
            false,
        ));
        let mut plan = CleanupPlan::new(vec![PlanGroup {
            kind: GroupKind::System,
            items: vec![remaining, item("not-selected", 5)],
        }]);
        let mut outcome = crate::models::CleanResult::new();
        outcome.add_deleted_file(PathBuf::from("/tmp/plan-test/removed"), 99);
        outcome.add_deleted_directory(PathBuf::from("/tmp/plan-test/empty-dir"));
        outcome.add_failed(PathBuf::from("/tmp/plan-test/failed"), "Permission denied");
        outcome.add_skipped(PathBuf::from("/tmp/plan-test/in-use"), "In use");
        outcome.cancelled = true;
        let paths = outcome
            .deleted_files
            .into_iter()
            .chain(outcome.deleted_directories)
            .collect();

        plan.remove_cleaned(&paths);

        let remaining = &plan.groups[0].items[0];
        assert!(remaining.selected);
        assert_eq!(remaining.result.file_count, 2);
        assert_eq!(remaining.result.dir_count, 1);
        assert_eq!(remaining.result.total_size, 50);
        assert!(remaining.result.files[0].path.ends_with("failed"));
        assert!(remaining.result.files[1].path.ends_with("in-use"));
        assert!(!plan.groups[0].items[1].selected);
        assert_eq!(plan.groups[0].size(), 55);
        assert_eq!(plan.total_size(), 55);
        let (private, bulk) = plan.selected_results();
        assert_eq!(private.total_size, 50);
        assert_eq!(private.file_count, 2);
        assert_eq!(private.dir_count, 1);
        assert!(bulk.is_empty());
    }

    #[test]
    fn cleaned_parent_removes_descendants_without_matching_sibling_prefixes() {
        let mut plan = CleanupPlan::new(vec![PlanGroup {
            kind: GroupKind::Custom,
            items: vec![item("cache/a", 100), item("cache-old/a", 50)],
        }]);
        let paths = HashSet::from([PathBuf::from("/tmp/plan-test/cache")]);
        plan.remove_cleaned(&paths);
        plan.remove_cleaned(&paths);
        plan.remove_cleaned(&HashSet::new());
        assert_eq!(plan.groups[0].items.len(), 1);
        assert_eq!(plan.total_size(), 50);

        plan.remove_cleaned(&HashSet::from([PathBuf::from(
            "/tmp/plan-test/cache-old/a",
        )]));
        assert!(plan.is_empty());
        assert_eq!(plan.total_size(), 0);
        assert_eq!(plan.selected_size(), 0);
    }

    #[test]
    fn failed_empty_directory_stays_in_the_plan() {
        let mut remaining = item("removed", 100);
        remaining.selected = true;
        remaining.result.add_directory(FileEntry::new(
            PathBuf::from("/tmp/plan-test/failed-dir"),
            0,
            true,
            false,
        ));
        let mut plan = CleanupPlan::new(vec![PlanGroup {
            kind: GroupKind::System,
            items: vec![remaining],
        }]);
        plan.remove_cleaned(&HashSet::from([PathBuf::from("/tmp/plan-test/removed")]));
        assert!(!plan.is_empty());
        assert_eq!(plan.total_size(), 0);
        assert_eq!(plan.groups[0].items[0].result.dir_count, 1);
        assert!(plan.groups[0].items[0].selected);
    }
}
