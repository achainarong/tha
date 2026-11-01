//! The download manager's state, free of I/O so its behavior can be tested directly: a file tree
//! filled in the background, a dialog, and the session's downloads. `main` runs what it requests.
use crate::files::{self, Entry, Header, Kind, Listing, Source, Totals};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::PathBuf,
    time::{Duration, Instant},
};

/// A tree nobody has used for this long is dropped; opening the manager again rebuilds it.
pub const CACHE_IDLE: Duration = Duration::from_secs(10 * 60);
/// How far the background walk goes by itself. Folders opened by hand are always read.
pub const MAX_DEPTH: usize = 6;
pub const MAX_FOLDERS: usize = 2000;
pub const MAX_ENTRIES: usize = 100_000;
pub const MAX_RUNNING: usize = 4;
const SKIPPED: [&str; 3] = ["/proc", "/sys", "/dev"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Folder {
    Queued,
    Loading,
    Loaded(Listing),
    Failed(String),
}

pub struct Tree {
    pub source: Source,
    pub folders: BTreeMap<String, Folder>,
    pub expanded: BTreeSet<String>,
    queue: VecDeque<String>,
    in_flight: Option<String>,
    entries: usize,
    /// The background walk reached a limit, so the tree is incomplete.
    pub limited: bool,
    /// Browsing cannot work here at all, such as an image without `ls`.
    pub stopped: Option<String>,
    last_used: Instant,
}

fn depth(path: &str) -> usize {
    path.split('/').filter(|part| !part.is_empty()).count()
}

impl Tree {
    fn new(source: Source, now: Instant) -> Self {
        let mut tree = Self {
            source,
            folders: BTreeMap::new(),
            expanded: BTreeSet::from(["/".to_owned()]),
            queue: VecDeque::new(),
            in_flight: None,
            entries: 0,
            limited: false,
            stopped: None,
            last_used: now,
        };
        tree.folders.insert("/".into(), Folder::Queued);
        tree.queue.push_back("/".into());
        tree
    }

    /// The one next folder to read: reading stays sequential, so the load stays small.
    fn next(&mut self) -> Option<String> {
        if self.in_flight.is_some() || self.stopped.is_some() {
            return None;
        }
        let path = self.queue.pop_front()?;
        self.folders.insert(path.clone(), Folder::Loading);
        self.in_flight = Some(path.clone());
        Some(path)
    }

    fn listed(&mut self, path: &str, result: Result<Listing, String>) {
        if self.in_flight.as_deref() == Some(path) {
            self.in_flight = None;
        }
        match result {
            Ok(listing) => {
                self.entries += listing.entries.len();
                for entry in &listing.entries {
                    if entry.kind != Kind::Folder {
                        continue;
                    }
                    let child = files::join(path, &entry.name);
                    if self.folders.contains_key(&child) || SKIPPED.contains(&child.as_str()) {
                        continue;
                    }
                    if depth(&child) > MAX_DEPTH
                        || self.folders.len() >= MAX_FOLDERS
                        || self.entries >= MAX_ENTRIES
                    {
                        self.limited = true;
                        continue;
                    }
                    self.folders.insert(child.clone(), Folder::Queued);
                    self.queue.push_back(child);
                }
                self.folders
                    .insert(path.to_owned(), Folder::Loaded(listing));
            }
            Err(error) => {
                // A failure at the very top means browsing does not work here, not one folder.
                if path == "/" {
                    self.stopped = Some(error.clone());
                    self.queue.clear();
                }
                self.folders.insert(path.to_owned(), Folder::Failed(error));
            }
        }
    }

    /// Whether the background walk is still going.
    pub fn building(&self) -> bool {
        self.stopped.is_none() && (self.in_flight.is_some() || !self.queue.is_empty())
    }

    pub fn listed_folders(&self) -> usize {
        self.folders
            .values()
            .filter(|folder| matches!(folder, Folder::Loaded(_)))
            .count()
    }

    /// Reads a folder next, ahead of the background walk, including ones the walk left out.
    fn prioritize(&mut self, path: &str) {
        let waiting = match self.folders.get(path) {
            None => true,
            Some(Folder::Queued) => true,
            Some(_) => false,
        };
        if waiting && self.stopped.is_none() {
            self.queue.retain(|queued| queued != path);
            self.queue.push_front(path.to_owned());
            self.folders.insert(path.to_owned(), Folder::Queued);
        }
    }

    /// What is on screen, in order, following only open folders.
    pub fn rows(&self) -> Vec<Row> {
        fn walk(tree: &Tree, path: &str, level: usize, rows: &mut Vec<Row>) {
            let Some(Folder::Loaded(listing)) = tree.folders.get(path) else {
                return;
            };
            for entry in &listing.entries {
                let child = files::join(path, &entry.name);
                let open = entry.kind == Kind::Folder && tree.expanded.contains(&child);
                rows.push(Row {
                    path: child.clone(),
                    entry: entry.clone(),
                    level,
                    open,
                    state: tree.folders.get(&child).cloned(),
                });
                if open {
                    walk(tree, &child, level + 1, rows);
                }
            }
        }
        let mut rows = Vec::new();
        walk(self, "/", 0, &mut rows);
        rows
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: String,
    pub entry: Entry,
    pub level: usize,
    pub open: bool,
    pub state: Option<Folder>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Loading,
    Ready(Header),
    Failed(String),
}

pub struct Dialog {
    pub id: u64,
    pub source: Source,
    pub path: String,
    pub preview: Preview,
    pub totals: Option<Totals>,
    pub count_failed: Option<String>,
    pub save_to: String,
    /// Why the last attempt to start did not.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

/// Review before writing into a container, because anything with the same name is replaced.
pub struct UploadDialog {
    pub source: Source,
    pub local: PathBuf,
    pub folder: String,
    pub size: Option<u64>,
    pub error: Option<String>,
}

pub struct Job {
    pub id: u64,
    pub label: String,
    pub path: String,
    pub destination: PathBuf,
    pub total: Option<u64>,
    pub bytes: u64,
    pub started: Instant,
    pub finished: Option<Instant>,
    pub status: JobStatus,
}

impl Job {
    pub fn speed(&self) -> u64 {
        let elapsed = self
            .finished
            .unwrap_or_else(Instant::now)
            .duration_since(self.started)
            .as_secs_f64();
        if elapsed < 0.25 {
            0
        } else {
            (self.bytes as f64 / elapsed) as u64
        }
    }

    pub fn percent(&self) -> Option<u64> {
        self.total
            .filter(|total| *total > 0)
            .map(|total| (self.bytes.saturating_mul(100) / total).min(100))
    }
}

pub enum Request {
    List {
        key: String,
        source: Source,
        path: String,
    },
    Inspect {
        dialog: u64,
        source: Source,
        path: String,
    },
    Count {
        dialog: u64,
        source: Source,
        path: String,
    },
    Start {
        job: u64,
        source: Source,
        path: String,
        destination: PathBuf,
        folder: bool,
    },
    Upload {
        job: u64,
        source: Source,
        local: PathBuf,
        folder: String,
    },
    Cancel {
        job: u64,
    },
    /// The dialog closed, so anything still measuring for it should stop.
    CloseDialog,
}

pub enum Event {
    Listed {
        key: String,
        path: String,
        result: Result<Listing, String>,
    },
    Inspected {
        dialog: u64,
        result: Result<Header, String>,
    },
    Counted {
        dialog: u64,
        result: Result<Totals, String>,
    },
    Progress {
        job: u64,
        bytes: u64,
    },
    Finished {
        job: u64,
        result: Result<u64, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Browse,
    Downloads,
}

pub struct Manager {
    /// The tree on screen, by key. `None` while the panel is closed.
    pub open: Option<String>,
    pub pane: Pane,
    pub selected: usize,
    pub job_selected: usize,
    pub trees: HashMap<String, Tree>,
    pub dialog: Option<Dialog>,
    pub upload: Option<UploadDialog>,
    pub jobs: Vec<Job>,
    pub requests: Vec<Request>,
    pub next_id: u64,
    pub demo: bool,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            open: None,
            pane: Pane::Browse,
            selected: 0,
            job_selected: 0,
            trees: HashMap::new(),
            dialog: None,
            upload: None,
            jobs: Vec::new(),
            requests: Vec::new(),
            next_id: 0,
            demo: false,
        }
    }
}

/// `~/Downloads/NAME`, or the home folder when that does not exist.
pub fn default_destination(name: &str) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return name.to_owned();
    };
    let downloads = home.join("Downloads");
    if downloads.is_dir() {
        format!("~/Downloads/{name}")
    } else {
        format!("~/{name}")
    }
}

pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

impl Manager {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn tree(&self) -> Option<&Tree> {
        self.open.as_ref().and_then(|key| self.trees.get(key))
    }

    fn tree_mut(&mut self) -> Option<&mut Tree> {
        let key = self.open.clone()?;
        self.trees.get_mut(&key)
    }

    /// Opens the panel and starts the background walk, reusing a tree that is still cached.
    pub fn open_source(&mut self, source: Source, now: Instant) {
        let key = source.key();
        self.trees
            .entry(key.clone())
            .or_insert_with(|| Tree::new(source, now))
            .last_used = now;
        if self.open.as_deref() != Some(&key) {
            self.selected = 0;
        }
        self.open = Some(key);
        self.pane = Pane::Browse;
        self.pump();
    }

    /// Leaves downloads running. The walk pauses, since nobody is looking at it.
    pub fn close(&mut self) {
        self.open = None;
    }

    /// Queues the next folder for the open tree only.
    pub fn pump(&mut self) {
        let Some(key) = self.open.clone() else {
            return;
        };
        let Some(tree) = self.trees.get_mut(&key) else {
            return;
        };
        if let Some(path) = tree.next() {
            let source = tree.source.clone();
            self.requests.push(Request::List { key, source, path });
        }
    }

    /// Frees trees that have not been used for a while. The open one counts as in use.
    pub fn release_idle(&mut self, now: Instant) {
        let open = self.open.clone();
        self.trees.retain(|key, tree| {
            Some(key) == open.as_ref() || now.duration_since(tree.last_used) < CACHE_IDLE
        });
    }

    fn touch(&mut self) {
        if let Some(tree) = self.tree_mut() {
            tree.last_used = Instant::now();
        }
    }

    pub fn current_row(&self) -> Option<Row> {
        self.tree()?.rows().into_iter().nth(self.selected)
    }

    pub fn move_selection(&mut self, amount: isize) {
        self.touch();
        match self.pane {
            Pane::Browse => {
                let count = self.tree().map_or(0, |tree| tree.rows().len());
                self.selected = self
                    .selected
                    .saturating_add_signed(amount)
                    .min(count.saturating_sub(1));
            }
            Pane::Downloads => {
                self.job_selected = self
                    .job_selected
                    .saturating_add_signed(amount)
                    .min(self.jobs.len().saturating_sub(1));
            }
        }
    }

    /// Opens or closes a folder. A closed folder keeps what was read.
    pub fn toggle_selected(&mut self) {
        self.touch();
        let Some(row) = self.current_row() else {
            return;
        };
        if row.entry.kind != Kind::Folder {
            return;
        }
        let Some(tree) = self.tree_mut() else {
            return;
        };
        if !tree.expanded.remove(&row.path) {
            tree.expanded.insert(row.path.clone());
            tree.prioritize(&row.path);
        }
        self.pump();
    }

    /// Opens the review dialog. Nothing downloads until it is confirmed.
    pub fn open_dialog(&mut self, source: Source, path: String) {
        self.touch();
        let id = self.id();
        self.requests.push(Request::CloseDialog);
        self.requests.push(Request::Inspect {
            dialog: id,
            source: source.clone(),
            path: path.clone(),
        });
        self.dialog = Some(Dialog {
            id,
            source,
            save_to: default_destination(files::file_name(&path)),
            path,
            preview: Preview::Loading,
            totals: None,
            count_failed: None,
            error: None,
        });
    }

    pub fn dialog_selected(&mut self) {
        let Some(row) = self.current_row() else {
            return;
        };
        if let Some(source) = self.tree().map(|tree| tree.source.clone()) {
            self.open_dialog(source, row.path);
        }
    }

    pub fn close_dialog(&mut self) {
        self.dialog = None;
        self.requests.push(Request::CloseDialog);
    }

    /// Checks what can be known here, then starts. A size is an estimate, so it does not block.
    pub fn confirm_dialog(&mut self, now: Instant) {
        let running = self
            .jobs
            .iter()
            .filter(|job| job.status == JobStatus::Running)
            .count();
        let id = self.id();
        let demo = self.demo;
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        if demo {
            dialog.error = Some(crate::i18n::t("Demo mode: files are not downloaded").into());
            return;
        }
        let Preview::Ready(header) = &dialog.preview else {
            dialog.error = Some(crate::i18n::t("Wait until the file has been checked").into());
            return;
        };
        if running >= MAX_RUNNING {
            dialog.error =
                Some(crate::i18n::t("Four downloads are already running; wait for one").into());
            return;
        }
        let destination = expand_home(dialog.save_to.trim());
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        if dialog.save_to.trim().is_empty() || !parent.is_dir() {
            dialog.error = Some(crate::i18n::t("The destination folder does not exist").into());
            return;
        }
        if destination.symlink_metadata().is_ok() {
            dialog.error = Some(crate::i18n::t("A file with this name already exists").into());
            return;
        }
        let folder = header.kind == Kind::Folder;
        let total = if folder {
            dialog.totals.map(|totals| totals.bytes)
        } else {
            Some(header.size)
        };
        let request = Request::Start {
            job: id,
            source: dialog.source.clone(),
            path: dialog.path.clone(),
            destination: destination.clone(),
            folder,
        };
        self.jobs.insert(
            0,
            Job {
                id,
                label: dialog.source.label().to_owned(),
                path: dialog.path.clone(),
                destination,
                total,
                bytes: 0,
                started: now,
                finished: None,
                status: JobStatus::Running,
            },
        );
        self.requests.push(request);
        self.job_selected = 0;
        self.pane = Pane::Downloads;
        self.close_dialog();
    }

    pub fn open_upload(
        &mut self,
        source: Source,
        local: &str,
        folder: String,
    ) -> Result<(), String> {
        let local = expand_home(local.trim());
        let metadata = std::fs::symlink_metadata(&local).map_err(|_| {
            crate::tf!(
                "Not found on this computer: {path}",
                path = crate::model::safe_text(&local.to_string_lossy())
            )
        })?;
        self.upload = Some(UploadDialog {
            source,
            size: metadata.is_file().then_some(metadata.len()),
            local,
            folder,
            error: None,
        });
        Ok(())
    }

    pub fn confirm_upload(&mut self, now: Instant) {
        let running = self
            .jobs
            .iter()
            .filter(|job| job.status == JobStatus::Running)
            .count();
        let id = self.id();
        let demo = self.demo;
        let Some(dialog) = &mut self.upload else {
            return;
        };
        if demo {
            dialog.error = Some(crate::i18n::t("Demo mode: files are not uploaded").into());
            return;
        }
        if running >= MAX_RUNNING {
            dialog.error =
                Some(crate::i18n::t("Four downloads are already running; wait for one").into());
            return;
        }
        let Some(dialog) = self.upload.take() else {
            return;
        };
        self.jobs.insert(
            0,
            Job {
                id,
                label: dialog.source.label().to_owned(),
                path: dialog.local.to_string_lossy().into_owned(),
                destination: PathBuf::from(&dialog.folder),
                total: None,
                bytes: 0,
                started: now,
                finished: None,
                status: JobStatus::Running,
            },
        );
        self.requests.push(Request::Upload {
            job: id,
            source: dialog.source,
            local: dialog.local,
            folder: dialog.folder,
        });
        self.job_selected = 0;
        self.pane = Pane::Downloads;
    }

    pub fn cancel_selected(&mut self) {
        let Some(job) = self.jobs.get_mut(self.job_selected) else {
            return;
        };
        if job.status == JobStatus::Running {
            job.status = JobStatus::Cancelled;
            job.finished = Some(Instant::now());
            self.requests.push(Request::Cancel { job: job.id });
        }
    }

    pub fn apply(&mut self, event: Event, now: Instant) {
        match event {
            Event::Listed { key, path, result } => {
                if let Some(tree) = self.trees.get_mut(&key) {
                    tree.listed(&path, result);
                }
                self.pump();
            }
            Event::Inspected { dialog, result } => {
                let Some(open) = self.dialog.as_mut().filter(|open| open.id == dialog) else {
                    return;
                };
                match result {
                    Ok(header) => {
                        // Only folders need counting; a file's header already has its size.
                        if header.kind == Kind::Folder {
                            let (source, path) = (open.source.clone(), open.path.clone());
                            open.save_to = format!("{}.tar", open.save_to);
                            open.totals = Some(Totals::default());
                            self.requests.push(Request::Count {
                                dialog,
                                source,
                                path,
                            });
                        }
                        if let Some(open) = self.dialog.as_mut() {
                            open.preview = Preview::Ready(header);
                        }
                    }
                    Err(error) => open.preview = Preview::Failed(error),
                }
            }
            Event::Counted { dialog, result } => {
                let Some(open) = self.dialog.as_mut().filter(|open| open.id == dialog) else {
                    return;
                };
                match result {
                    Ok(totals) => open.totals = Some(totals),
                    Err(error) => open.count_failed = Some(error),
                }
            }
            Event::Progress { job, bytes } => {
                if let Some(job) = self.jobs.iter_mut().find(|item| item.id == job)
                    && job.status == JobStatus::Running
                {
                    job.bytes = bytes;
                }
            }
            Event::Finished { job, result } => {
                let Some(job) = self.jobs.iter_mut().find(|item| item.id == job) else {
                    return;
                };
                // A cancelled job stays cancelled, whatever its task managed to report.
                if job.status != JobStatus::Running {
                    return;
                }
                job.finished = Some(now);
                match result {
                    Ok(bytes) => {
                        job.bytes = bytes;
                        job.status = JobStatus::Done;
                    }
                    Err(error) => job.status = JobStatus::Failed(error),
                }
            }
        }
    }

    /// For the status line: how many run, and how far along they are together.
    pub fn activity(&self) -> Option<(usize, Option<u64>)> {
        let running: Vec<_> = self
            .jobs
            .iter()
            .filter(|job| job.status == JobStatus::Running)
            .collect();
        if running.is_empty() {
            return None;
        }
        let known = running.iter().all(|job| job.total.is_some_and(|t| t > 0));
        let percent = known.then(|| {
            let total: u64 = running.iter().filter_map(|job| job.total).sum();
            let bytes: u64 = running.iter().map(|job| job.bytes).sum();
            (bytes.saturating_mul(100) / total.max(1)).min(100)
        });
        Some((running.len(), percent))
    }
}

/// `4.7 GB`, for sizes shown to people.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Fixture files for `--demo`, which reaches no engine.
pub fn demo_listing(path: &str) -> Listing {
    let file = |name: &str, size| Entry {
        name: name.into(),
        kind: Kind::File,
        size: Some(size),
    };
    let folder = |name: &str| Entry {
        name: name.into(),
        kind: Kind::Folder,
        size: None,
    };
    let entries = match path {
        "/" => vec![folder("etc"), folder("tmp"), folder("var")],
        "/etc" => vec![file("hostname", 12), file("os-release", 267)],
        "/tmp" => vec![file("mydb.dump", 2_400_000_000)],
        "/var" => vec![folder("backups")],
        "/var/backups" => vec![
            file("monday.dump", 1_100_000_000),
            file("tuesday.dump", 1_200_000_000),
        ],
        _ => Vec::new(),
    };
    Listing {
        entries,
        truncated: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{BackendKind, Connection},
        files::Totals,
    };

    fn source(id: &str) -> Source {
        Source::Container {
            connection: Connection {
                name: "local".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///fixture".into(),
            },
            id: id.into(),
            label: format!("container {id}"),
        }
    }

    fn folder(name: &str) -> Entry {
        Entry {
            name: name.into(),
            kind: Kind::Folder,
            size: None,
        }
    }

    fn file(name: &str, size: u64) -> Entry {
        Entry {
            name: name.into(),
            kind: Kind::File,
            size: Some(size),
        }
    }

    fn listing(entries: Vec<Entry>) -> Result<Listing, String> {
        Ok(Listing {
            entries,
            truncated: false,
        })
    }

    /// The folders requested so far, clearing the list.
    fn lists(manager: &mut Manager) -> Vec<String> {
        manager
            .requests
            .drain(..)
            .filter_map(|request| match request {
                Request::List { path, .. } => Some(path),
                _ => None,
            })
            .collect()
    }

    fn answer(manager: &mut Manager, key: &str, path: &str, result: Result<Listing, String>) {
        manager.apply(
            Event::Listed {
                key: key.into(),
                path: path.into(),
                result,
            },
            Instant::now(),
        );
    }

    #[test]
    fn opening_builds_the_tree_in_the_background_one_folder_at_a_time() {
        let mut manager = Manager::default();
        let key = source("a").key();
        manager.open_source(source("a"), Instant::now());
        assert_eq!(lists(&mut manager), ["/"]);
        // Another pump cannot start a second listing while one is in flight.
        manager.pump();
        assert!(lists(&mut manager).is_empty());
        answer(
            &mut manager,
            &key,
            "/",
            listing(vec![
                folder("etc"),
                folder("proc"),
                folder("sys"),
                folder("dev"),
                folder("var"),
                file("x", 1),
            ]),
        );
        assert_eq!(lists(&mut manager), ["/etc"]);
        answer(&mut manager, &key, "/etc", listing(vec![]));
        assert_eq!(lists(&mut manager), ["/var"]);
        answer(&mut manager, &key, "/var", listing(vec![]));
        assert!(lists(&mut manager).is_empty());
        assert!(!manager.tree().unwrap().building());
    }

    #[test]
    fn a_folder_the_user_opens_goes_ahead_of_the_background_walk() {
        let mut manager = Manager::default();
        let key = source("a").key();
        manager.open_source(source("a"), Instant::now());
        lists(&mut manager);
        answer(
            &mut manager,
            &key,
            "/",
            listing(vec![folder("a"), folder("b"), folder("c")]),
        );
        assert_eq!(lists(&mut manager), ["/a"]);
        manager.selected = 2;
        manager.toggle_selected();
        assert!(lists(&mut manager).is_empty());
        answer(&mut manager, &key, "/a", listing(vec![]));
        assert_eq!(lists(&mut manager), ["/c"]);
    }

    #[test]
    fn the_walk_respects_its_limits_and_says_the_tree_is_incomplete() {
        let mut manager = Manager::default();
        let key = source("a").key();
        manager.open_source(source("a"), Instant::now());
        lists(&mut manager);
        let mut path = "/".to_owned();
        for level in 0..=MAX_DEPTH {
            answer(&mut manager, &key, &path, listing(vec![folder("deep")]));
            let next = lists(&mut manager);
            if level == MAX_DEPTH - 1 {
                assert_eq!(depth(&next[0]), MAX_DEPTH);
            }
            if next.is_empty() {
                break;
            }
            path = next[0].clone();
        }
        assert!(manager.tree().unwrap().limited);
    }

    #[test]
    fn a_missing_ls_stops_browsing_but_one_unreadable_folder_does_not() {
        let mut manager = Manager::default();
        let key = source("a").key();
        manager.open_source(source("a"), Instant::now());
        lists(&mut manager);
        answer(
            &mut manager,
            &key,
            "/",
            listing(vec![folder("a"), folder("b")]),
        );
        lists(&mut manager);
        answer(&mut manager, &key, "/a", Err("Permission denied".into()));
        assert_eq!(lists(&mut manager), ["/b"]);
        assert!(manager.tree().unwrap().stopped.is_none());

        let key = source("no-ls").key();
        manager.open_source(source("no-ls"), Instant::now());
        lists(&mut manager);
        answer(&mut manager, &key, "/", Err("no ls".into()));
        assert_eq!(manager.tree().unwrap().stopped.as_deref(), Some("no ls"));
        manager.pump();
        assert!(lists(&mut manager).is_empty());
    }

    #[test]
    fn the_cache_is_reused_and_released_after_it_goes_unused() {
        let start = Instant::now();
        let mut manager = Manager::default();
        let key = source("a").key();
        manager.open_source(source("a"), start);
        lists(&mut manager);
        answer(&mut manager, &key, "/", listing(vec![file("x", 1)]));

        // Reopening within the window keeps what was already read.
        manager.close();
        manager.open_source(source("a"), start + Duration::from_secs(60));
        assert!(lists(&mut manager).is_empty());
        assert_eq!(manager.tree().unwrap().rows().len(), 1);

        // The open tree is in use however long it stays open.
        let opened = start + Duration::from_secs(60);
        manager.release_idle(opened + CACHE_IDLE * 3);
        assert!(manager.trees.contains_key(&key));

        // Closed and idle, it goes, and opening it again rebuilds it.
        manager.close();
        manager.release_idle(opened + CACHE_IDLE - Duration::from_secs(1));
        assert!(manager.trees.contains_key(&key));
        manager.release_idle(opened + CACHE_IDLE);
        assert!(!manager.trees.contains_key(&key));
        manager.open_source(source("a"), opened + CACHE_IDLE);
        assert_eq!(lists(&mut manager), ["/"]);
    }

    #[test]
    fn closing_pauses_the_walk_and_a_late_answer_for_a_released_tree_is_ignored() {
        let mut manager = Manager::default();
        let key = source("a").key();
        manager.open_source(source("a"), Instant::now());
        lists(&mut manager);
        manager.close();
        answer(&mut manager, &key, "/", listing(vec![folder("a")]));
        assert!(lists(&mut manager).is_empty());
        manager.trees.clear();
        answer(&mut manager, &key, "/a", listing(vec![]));
        assert!(manager.trees.is_empty());
    }

    fn ready(manager: &mut Manager, kind: Kind, size: u64) -> u64 {
        let id = manager.dialog.as_ref().unwrap().id;
        manager.apply(
            Event::Inspected {
                dialog: id,
                result: Ok(Header {
                    name: "x".into(),
                    kind,
                    size,
                    modified: 0,
                }),
            },
            Instant::now(),
        );
        id
    }

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("tha-downloads-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn nothing_downloads_until_a_checked_dialog_is_confirmed() {
        let root = temp("confirm");
        let mut manager = Manager::default();
        manager.open_dialog(source("a"), "/tmp/mydb.dump".into());
        assert!(
            manager
                .dialog
                .as_ref()
                .unwrap()
                .save_to
                .ends_with("mydb.dump")
        );
        manager.confirm_dialog(Instant::now());
        assert!(manager.jobs.is_empty());
        assert!(manager.dialog.as_ref().unwrap().error.is_some());

        ready(&mut manager, Kind::File, 2400);
        manager.dialog.as_mut().unwrap().save_to =
            root.join("missing/out").to_string_lossy().into_owned();
        manager.confirm_dialog(Instant::now());
        assert!(manager.jobs.is_empty());
        assert!(
            manager
                .dialog
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("folder")
        );

        std::fs::write(root.join("exists"), b"keep").unwrap();
        manager.dialog.as_mut().unwrap().save_to =
            root.join("exists").to_string_lossy().into_owned();
        manager.confirm_dialog(Instant::now());
        assert!(manager.jobs.is_empty());
        assert!(
            manager
                .dialog
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("exists")
        );

        manager.dialog.as_mut().unwrap().save_to = root.join("out").to_string_lossy().into_owned();
        manager.requests.clear();
        manager.confirm_dialog(Instant::now());
        assert!(manager.dialog.is_none());
        assert_eq!(manager.jobs[0].total, Some(2400));
        assert_eq!(manager.pane, Pane::Downloads);
        assert!(
            manager
                .requests
                .iter()
                .any(|request| matches!(request, Request::Start { folder: false, .. }))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_folder_gets_counted_saved_as_tar_and_can_start_before_the_count_ends() {
        let root = temp("folder");
        let mut manager = Manager::default();
        manager.open_dialog(source("a"), "/var/backups".into());
        manager.requests.clear();
        let id = ready(&mut manager, Kind::Folder, 0);
        assert!(
            manager
                .requests
                .iter()
                .any(|request| matches!(request, Request::Count { .. }))
        );
        assert!(
            manager
                .dialog
                .as_ref()
                .unwrap()
                .save_to
                .ends_with("backups.tar")
        );
        let totals = Totals {
            bytes: 1200,
            files: 3400,
            done: false,
        };
        manager.apply(
            Event::Counted {
                dialog: id,
                result: Ok(totals),
            },
            Instant::now(),
        );
        assert_eq!(manager.dialog.as_ref().unwrap().totals, Some(totals));
        manager.apply(
            Event::Counted {
                dialog: id + 99,
                result: Err("late".into()),
            },
            Instant::now(),
        );
        assert!(manager.dialog.as_ref().unwrap().count_failed.is_none());

        manager.dialog.as_mut().unwrap().save_to =
            root.join("b.tar").to_string_lossy().into_owned();
        manager.requests.clear();
        manager.confirm_dialog(Instant::now());
        assert!(
            manager
                .requests
                .iter()
                .any(|request| matches!(request, Request::Start { folder: true, .. }))
        );
        assert_eq!(manager.jobs[0].total, Some(1200));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn jobs_report_progress_finish_and_stay_cancelled() {
        let now = Instant::now();
        let mut manager = Manager::default();
        for id in [1, 2] {
            manager.jobs.push(Job {
                id,
                label: "c".into(),
                path: "/x".into(),
                destination: "/out".into(),
                total: Some(1000),
                bytes: 0,
                started: now,
                finished: None,
                status: JobStatus::Running,
            });
        }
        manager.apply(Event::Progress { job: 1, bytes: 500 }, now);
        assert_eq!(manager.jobs[0].percent(), Some(50));
        assert_eq!(manager.activity(), Some((2, Some(25))));

        manager.job_selected = 1;
        manager.cancel_selected();
        assert!(matches!(manager.requests[0], Request::Cancel { job: 2 }));
        manager.apply(
            Event::Finished {
                job: 2,
                result: Ok(1000),
            },
            now,
        );
        assert_eq!(manager.jobs[1].status, JobStatus::Cancelled);

        manager.apply(
            Event::Finished {
                job: 1,
                result: Err("disk full".into()),
            },
            now,
        );
        assert_eq!(
            manager.jobs[0].status,
            JobStatus::Failed("disk full".into())
        );
        assert_eq!(manager.activity(), None);
    }

    #[test]
    fn the_running_limit_holds_and_closing_the_panel_leaves_downloads_alone() {
        let root = temp("limit");
        let now = Instant::now();
        let mut manager = Manager::default();
        for id in 0..MAX_RUNNING as u64 {
            manager.jobs.push(Job {
                id: 100 + id,
                label: "c".into(),
                path: "/x".into(),
                destination: "/out".into(),
                total: None,
                bytes: 0,
                started: now,
                finished: None,
                status: JobStatus::Running,
            });
        }
        manager.open_dialog(source("a"), "/f".into());
        ready(&mut manager, Kind::File, 1);
        manager.dialog.as_mut().unwrap().save_to = root.join("o").to_string_lossy().into_owned();
        manager.confirm_dialog(now);
        assert_eq!(manager.jobs.len(), MAX_RUNNING);
        manager.close();
        assert_eq!(manager.activity().unwrap().0, MAX_RUNNING);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_upload_is_reviewed_first_and_a_missing_file_is_refused() {
        let root = temp("upload");
        let file = root.join("seed.sql");
        std::fs::write(&file, b"12345").unwrap();
        let mut manager = Manager::default();
        assert!(
            manager
                .open_upload(source("a"), "/no/such/file", "/tmp".into())
                .is_err()
        );
        manager
            .open_upload(source("a"), &file.to_string_lossy(), "/tmp".into())
            .unwrap();
        assert_eq!(manager.upload.as_ref().unwrap().size, Some(5));
        assert!(
            manager.requests.is_empty(),
            "Nothing runs before it is confirmed"
        );
        manager.confirm_upload(Instant::now());
        assert!(manager.upload.is_none());
        assert!(matches!(manager.requests[0], Request::Upload { .. }));
        assert_eq!(manager.jobs[0].status, JobStatus::Running);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn demo_mode_browses_fixtures_but_never_downloads() {
        let mut manager = Manager {
            demo: true,
            ..Default::default()
        };
        manager.open_dialog(source("a"), "/tmp/mydb.dump".into());
        ready(&mut manager, Kind::File, 1);
        manager.confirm_dialog(Instant::now());
        assert!(manager.jobs.is_empty());
        assert_eq!(demo_listing("/tmp").entries[0].name, "mydb.dump");
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(4_700_000_000), "4.7 GB");
    }
}
