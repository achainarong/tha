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
/// Larger files are not read just because the cursor passed over them.
pub const PREVIEW_LIMIT: usize = 256 * 1024;
const MAX_CACHED_PREVIEWS: usize = 32;
/// Moving quickly through the tree must not read every file along the way.
const PREVIEW_DELAY: Duration = Duration::from_millis(150);
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
    /// An explicit search has already replaced this outstanding ordinary listing.
    superseded_in_flight: bool,
    entries: usize,
    /// The background walk reached a limit, so the tree is incomplete.
    pub limited: bool,
    /// Browsing cannot work here at all, such as an image without `ls`.
    pub stopped: Option<String>,
    last_used: Instant,
    previews: HashMap<String, FilePreview>,
}

/// What the right side of the explorer shows for the selected row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePreview {
    Waiting,
    Loading,
    Text {
        text: String,
        truncated: bool,
    },
    Binary,
    /// Over the limit, so it was not read. `p` shows the start of it anyway.
    TooBig(u64),
    Folder,
    Special,
    Failed(String),
}

pub struct PreviewState {
    pub path: String,
    pub status: FilePreview,
    pub scroll: usize,
    since: Instant,
    generation: u64,
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
            superseded_in_flight: false,
            entries: 0,
            limited: false,
            stopped: None,
            last_used: now,
            previews: HashMap::new(),
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
            if std::mem::take(&mut self.superseded_in_flight) {
                return;
            }
        }
        match result {
            Ok(listing) => {
                let old = match self.folders.get(path) {
                    Some(Folder::Loaded(old)) => old.entries.len(),
                    _ => 0,
                };
                self.entries = self.entries.saturating_sub(old) + listing.entries.len();
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
    Search(crate::file_search::Request),
    CancelSearch,
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
    /// Replaces any preview still loading.
    Preview {
        generation: u64,
        source: Source,
        path: String,
    },
    Cancel {
        job: u64,
    },
    /// The dialog closed, so anything still measuring for it should stop.
    CloseDialog,
}

pub enum Event {
    SearchListed {
        key: String,
        generation: u64,
        path: String,
        listing: Listing,
    },
    SearchFinished {
        key: String,
        generation: u64,
        incomplete: bool,
        unreadable: usize,
        error: Option<String>,
    },
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
    Previewed {
        key: String,
        generation: u64,
        path: String,
        result: Result<files::FileStart, String>,
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

pub struct FileSearch {
    pub query: String,
    pub root: String,
    pub matches: Vec<String>,
    pub running: bool,
    pub incomplete: bool,
    pub unreadable: usize,
    pub error: Option<String>,
    generation: u64,
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
    pub preview: Option<PreviewState>,
    /// Text typed after `:` in the explorer, as in vim.
    pub command: Option<String>,
    pub search_input: Option<String>,
    pub search_root: String,
    pub search: Option<FileSearch>,
    /// Why the last key did nothing.
    pub message: Option<String>,
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
            preview: None,
            command: None,
            search_input: None,
            search_root: "/".into(),
            search: None,
            message: None,
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
            self.cancel_search();
            self.selected = 0;
            self.preview = None;
            self.command = None;
            self.search_input = None;
            self.search = None;
            self.message = None;
        }
        self.open = Some(key);
        self.pane = Pane::Browse;
        self.pump();
    }

    pub fn begin_search(&mut self, system: bool) {
        self.touch();
        self.search_root = if system {
            "/".to_owned()
        } else {
            self.current_row().map_or_else(
                || "/".to_owned(),
                |row| {
                    if row.entry.kind == Kind::Folder {
                        row.path
                    } else {
                        row.path
                            .rsplit_once('/')
                            .map_or(
                                "/",
                                |(parent, _)| if parent.is_empty() { "/" } else { parent },
                            )
                            .to_owned()
                    }
                },
            )
        };
        self.search_input = Some(
            self.search
                .as_ref()
                .map_or_else(String::new, |search| search.query.clone()),
        );
        self.message = None;
    }

    /// Shared by typed and pasted input. Search text never reaches a shell or engine.
    pub fn append_search(&mut self, text: &str) {
        if let Some(input) = &mut self.search_input {
            for character in crate::model::safe_text(text).chars() {
                if input.len() + character.len_utf8() > 1024 {
                    break;
                }
                input.push(character);
            }
        }
    }

    pub fn confirm_search(&mut self) {
        let Some(input) = self.search_input.take() else {
            return;
        };
        let query = input.trim().to_owned();
        self.cancel_search();
        self.search = None;
        if query.is_empty() {
            self.pump();
            return;
        }
        let Some(source) = self.tree().map(|tree| tree.source.clone()) else {
            return;
        };
        let generation = self.id();
        let root = self.search_root.clone();
        let selected = self.current_row().map(|row| row.path);
        // Drop old subtrees between searches, keeping the scope and current selection reachable.
        if let Some(tree) = self.tree_mut() {
            tree.folders.retain(|path, _| {
                path == "/"
                    || *path == root
                    || root.starts_with(&format!("{path}/"))
                    || selected
                        .as_ref()
                        .is_some_and(|selected| selected.starts_with(&format!("{path}/")))
            });
            tree.entries = tree
                .folders
                .values()
                .map(|folder| match folder {
                    Folder::Loaded(listing) => listing.entries.len(),
                    _ => 0,
                })
                .sum();
            tree.queue.clear();
            tree.limited = true;
            if let Some(path) = &selected
                && let Some(index) = tree.rows().iter().position(|row| &row.path == path)
            {
                self.selected = index;
            }
        }
        self.search = Some(FileSearch {
            query: query.clone(),
            root: root.clone(),
            matches: Vec::new(),
            running: true,
            incomplete: false,
            unreadable: 0,
            error: None,
            generation,
        });
        self.requests
            .push(Request::Search(crate::file_search::Request {
                generation,
                source,
                root,
                query,
                demo: self.demo,
            }));
    }

    fn cancel_search(&mut self) {
        if self.search.as_ref().is_some_and(|search| search.running) {
            self.requests.push(Request::CancelSearch);
        }
    }

    pub fn stop_search(&mut self) {
        self.cancel_search();
        if let Some(search) = &mut self.search {
            if search.running {
                search.incomplete = true;
            }
            search.running = false;
        }
        self.pump();
    }

    pub fn next_search_match(&mut self, backwards: bool) {
        self.touch();
        let current = self.current_row().map(|row| row.path);
        let Some(search) = &self.search else {
            return;
        };
        let count = search.matches.len();
        if count == 0 {
            return;
        }
        let position =
            current.and_then(|path| search.matches.iter().position(|found| *found == path));
        let next = match (position, backwards) {
            (Some(index), false) => (index + 1) % count,
            (Some(index), true) => (index + count - 1) % count,
            (None, false) => 0,
            (None, true) => count - 1,
        };
        self.select_search_result(next);
    }

    fn select_search_result(&mut self, index: usize) {
        let Some(path) = self
            .search
            .as_ref()
            .and_then(|search| search.matches.get(index))
            .cloned()
        else {
            return;
        };
        if let Some(tree) = self.tree_mut() {
            for (end, _) in path.match_indices('/').skip(1) {
                tree.expanded.insert(path[..end].to_owned());
            }
            if let Some(selected) = tree.rows().iter().position(|row| row.path == path) {
                self.selected = selected;
            }
        }
    }

    /// Follows the selection. Waits briefly so that moving through the tree reads nothing, and
    /// leaves large files alone, so browsing never costs more than a glance. Returns whether
    /// the view changed, including timer-driven transitions that need their own redraw.
    pub fn update_preview(&mut self, now: Instant) -> bool {
        if self.pane != Pane::Browse || self.open.is_none() {
            return false;
        }
        let Some(row) = self.current_row() else {
            return self.preview.take().is_some();
        };
        let changed = self
            .preview
            .as_ref()
            .is_none_or(|preview| preview.path != row.path);
        if changed {
            let cached = self
                .tree()
                .and_then(|tree| tree.previews.get(&row.path).cloned());
            self.preview = Some(PreviewState {
                path: row.path.clone(),
                status: cached.unwrap_or(FilePreview::Waiting),
                scroll: 0,
                since: now,
                generation: 0,
            });
            self.message = None;
        }
        let waiting = self
            .preview
            .as_ref()
            .is_some_and(|preview| preview.status == FilePreview::Waiting);
        if !waiting
            || self
                .preview
                .as_ref()
                .is_some_and(|p| now.duration_since(p.since) < PREVIEW_DELAY)
        {
            return changed;
        }
        let status = match row.entry.kind {
            Kind::Folder => FilePreview::Folder,
            Kind::Link | Kind::Other => FilePreview::Special,
            Kind::File
                if row
                    .entry
                    .size
                    .is_some_and(|size| size > PREVIEW_LIMIT as u64) =>
            {
                FilePreview::TooBig(row.entry.size.unwrap_or(0))
            }
            Kind::File => FilePreview::Loading,
        };
        if status == FilePreview::Loading {
            self.request_preview();
        } else if let Some(preview) = &mut self.preview {
            preview.status = status;
        }
        true
    }

    fn request_preview(&mut self) {
        let generation = self.id();
        let Some(source) = self.tree().map(|tree| tree.source.clone()) else {
            return;
        };
        let Some(preview) = &mut self.preview else {
            return;
        };
        preview.status = FilePreview::Loading;
        preview.generation = generation;
        let path = preview.path.clone();
        self.requests.push(Request::Preview {
            generation,
            source,
            path,
        });
    }

    /// Reads the selected folder again, or the folder holding the selected file. The tree shows
    /// each folder as it was when read, so this is how new files appear.
    pub fn refresh_selected(&mut self) {
        self.touch();
        let folder = self.current_row().map_or_else(
            || "/".to_owned(),
            |row| {
                if row.entry.kind == Kind::Folder {
                    row.path
                } else {
                    match row.path.rsplit_once('/') {
                        Some(("", _)) | None => "/".to_owned(),
                        Some((parent, _)) => parent.to_owned(),
                    }
                }
            },
        );
        let Some(tree) = self.tree_mut() else {
            return;
        };
        tree.stopped = None;
        let prefix = if folder == "/" {
            "/".to_owned()
        } else {
            format!("{folder}/")
        };
        tree.previews.retain(|path, _| !path.starts_with(&prefix));
        tree.folders.remove(&folder);
        tree.prioritize(&folder);
        self.preview = None;
        self.cancel_search();
        self.search = None;
        self.pump();
    }

    /// `p` on a big file: shows its start after all, still bounded by the limit.
    pub fn show_big_file(&mut self) {
        let too_big = self
            .preview
            .as_ref()
            .is_some_and(|preview| matches!(preview.status, FilePreview::TooBig(_)));
        if too_big {
            self.request_preview();
        }
    }

    pub fn scroll_preview(&mut self, amount: isize) {
        if let Some(preview) = &mut self.preview {
            preview.scroll = preview.scroll.saturating_add_signed(amount);
        }
    }

    pub fn preview_is_binary(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|preview| preview.status == FilePreview::Binary)
    }

    /// After an edit, what was shown is out of date.
    pub fn invalidate_preview(&mut self, key: &str, path: &str) {
        if let Some(tree) = self.trees.get_mut(key) {
            tree.previews.remove(path);
        }
        if self.open.as_deref() == Some(key)
            && self
                .preview
                .as_ref()
                .is_some_and(|preview| preview.path == path)
        {
            self.preview = None;
        }
    }

    /// Leaves downloads running. The walk pauses, since nobody is looking at it.
    pub fn close(&mut self) {
        self.cancel_search();
        self.open = None;
        self.preview = None;
        self.command = None;
        self.search_input = None;
        self.search = None;
        self.message = None;
    }

    /// Queues the next folder for the open tree only.
    pub fn pump(&mut self) {
        if self.search.as_ref().is_some_and(|search| search.running) {
            return;
        }
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
            Event::SearchListed {
                key,
                generation,
                path,
                listing,
            } => {
                if self.open.as_deref() != Some(&key)
                    || !self
                        .search
                        .as_ref()
                        .is_some_and(|search| search.running && search.generation == generation)
                {
                    return;
                }
                let selected = self.current_row().map(|row| row.path);
                let Some(search) = self.search.as_mut() else {
                    return;
                };
                let first = search.matches.is_empty();
                let query = search.query.to_lowercase();
                for entry in &listing.entries {
                    let child = files::join(&path, &entry.name);
                    if child.to_lowercase().contains(&query)
                        && search.matches.len() < crate::file_search::MAX_RESULTS
                    {
                        search.matches.push(child);
                    }
                }
                let first = first && !search.matches.is_empty();
                if let Some(tree) = self.tree_mut() {
                    if path == "/" {
                        tree.stopped = None;
                    }
                    if tree.in_flight.as_deref() == Some(&path) {
                        tree.superseded_in_flight = true;
                    }
                    let old = match tree.folders.get(&path) {
                        Some(Folder::Loaded(old)) => old.entries.len(),
                        _ => 0,
                    };
                    tree.entries = tree.entries.saturating_sub(old) + listing.entries.len();
                    tree.queue.retain(|queued| queued != &path);
                    tree.folders.insert(path, Folder::Loaded(listing));
                }
                if first {
                    self.select_search_result(0);
                } else if let Some(path) = selected
                    && let Some(index) = self
                        .tree()
                        .and_then(|tree| tree.rows().iter().position(|row| row.path == path))
                {
                    self.selected = index;
                }
            }
            Event::SearchFinished {
                key,
                generation,
                incomplete,
                unreadable,
                error,
            } => {
                if self.open.as_deref() != Some(&key) {
                    return;
                }
                if let Some(search) = self
                    .search
                    .as_mut()
                    .filter(|search| search.running && search.generation == generation)
                {
                    search.running = false;
                    search.incomplete = incomplete;
                    search.unreadable = unreadable;
                    search.error = error;
                }
                self.pump();
            }
            Event::Previewed {
                key,
                generation,
                path,
                result,
            } => {
                if self.open.as_deref() != Some(&key) {
                    return;
                }
                let Some(preview) = self
                    .preview
                    .as_mut()
                    .filter(|preview| preview.generation == generation && preview.path == path)
                else {
                    return;
                };
                let status = match result {
                    Err(error) => FilePreview::Failed(error),
                    Ok(start) if files::looks_binary(&start.bytes, start.truncated) => {
                        FilePreview::Binary
                    }
                    Ok(start) => FilePreview::Text {
                        text: String::from_utf8_lossy(&start.bytes).into_owned(),
                        truncated: start.truncated,
                    },
                };
                preview.status = status.clone();
                // A transient read/configuration failure must not remain cached after recovery.
                if !matches!(status, FilePreview::Failed(_))
                    && let Some(tree) = self.tree_mut()
                {
                    if tree.previews.len() >= MAX_CACHED_PREVIEWS {
                        tree.previews.clear();
                    }
                    tree.previews.insert(path, status);
                }
            }
            Event::Listed { key, path, result } => {
                let selected = self
                    .search
                    .as_ref()
                    .and_then(|_| self.current_row())
                    .map(|row| row.path);
                if let Some(tree) = self.trees.get_mut(&key) {
                    tree.listed(&path, result);
                }
                if self.open.as_deref() == Some(&key)
                    && let Some(path) = selected
                    && let Some(index) = self
                        .tree()
                        .and_then(|tree| tree.rows().iter().position(|row| row.path == path))
                {
                    self.selected = index;
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

    fn previews(manager: &mut Manager) -> Vec<String> {
        manager
            .requests
            .drain(..)
            .filter_map(|request| match request {
                Request::Preview { path, .. } => Some(path),
                _ => None,
            })
            .collect()
    }

    fn explorer_with(entries: Vec<Entry>) -> (Manager, Instant) {
        let start = Instant::now();
        let mut manager = Manager::default();
        let key = source("a").key();
        manager.open_source(source("a"), start);
        answer(&mut manager, &key, "/", listing(entries));
        manager.requests.clear();
        (manager, start)
    }

    fn answer_preview(manager: &mut Manager, bytes: &[u8], truncated: bool) {
        let preview = manager.preview.as_ref().unwrap();
        let (generation, path) = (preview.generation, preview.path.clone());
        manager.apply(
            Event::Previewed {
                key: manager.open.clone().unwrap(),
                generation,
                path: path.clone(),
                result: Ok(files::FileStart {
                    header: Header {
                        name: path,
                        kind: Kind::File,
                        size: bytes.len() as u64,
                        modified: 0,
                        mode: 0o644,
                        uid: 0,
                        gid: 0,
                    },
                    bytes: bytes.to_vec(),
                    truncated,
                }),
            },
            Instant::now(),
        );
    }

    fn search_answer(manager: &mut Manager, path: &str, entries: Vec<Entry>) {
        let generation = manager.search.as_ref().unwrap().generation;
        manager.apply(
            Event::SearchListed {
                key: manager.open.clone().unwrap(),
                generation,
                path: path.into(),
                listing: Listing {
                    entries,
                    truncated: false,
                },
            },
            Instant::now(),
        );
    }

    #[test]
    fn folder_and_system_search_have_distinct_scopes_and_reveal_collapsed_matches() {
        let (mut manager, start) = explorer_with(vec![folder("etc"), folder("var")]);
        manager.begin_search(false);
        assert_eq!(manager.search_root, "/etc");
        manager.append_search("CONF");
        manager.confirm_search();
        assert!(
            manager
                .requests
                .iter()
                .any(|request| matches!(request, Request::Search(target) if target.root == "/etc"))
        );
        search_answer(
            &mut manager,
            "/etc",
            vec![file("a.conf", 1), file("b.conf", 2)],
        );
        assert_eq!(manager.current_row().unwrap().path, "/etc/a.conf");
        manager.next_search_match(false);
        assert_eq!(manager.current_row().unwrap().path, "/etc/b.conf");
        manager.next_search_match(false);
        assert_eq!(manager.current_row().unwrap().path, "/etc/a.conf");
        manager.next_search_match(true);
        assert_eq!(manager.current_row().unwrap().path, "/etc/b.conf");

        manager.begin_search(true);
        assert_eq!(manager.search_root, "/");
        manager.search_input = Some("/var/log".into());
        manager.confirm_search();
        search_answer(&mut manager, "/", vec![folder("etc"), folder("var")]);
        search_answer(&mut manager, "/var", vec![folder("log")]);
        search_answer(&mut manager, "/var/log", vec![file("app.log", 2)]);
        assert!(
            manager
                .search
                .as_ref()
                .unwrap()
                .matches
                .contains(&"/var/log/app.log".into())
        );
        manager.next_search_match(false);
        assert_eq!(manager.current_row().unwrap().path, "/var/log/app.log");
        manager.dialog_selected();
        let dialog = manager.dialog.as_ref().unwrap();
        assert_eq!(dialog.path, "/var/log/app.log");
        assert_eq!(dialog.save_to, default_destination("app.log"));
        assert_eq!(dialog.source, source("a"));
        manager.close_dialog();
        manager.update_preview(start);
    }

    #[test]
    fn search_input_is_bounded_and_clearing_it_cancels_without_changing_the_source() {
        let (mut manager, _) = explorer_with(vec![file("test.txt", 1)]);
        manager.begin_search(true);
        manager.append_search(&"ü".repeat(1024));
        assert_eq!(manager.search_input.as_ref().unwrap().len(), 1024);
        manager.confirm_search();
        let key = manager.open.clone();
        manager.begin_search(false);
        manager.search_input = Some(String::new());
        manager.confirm_search();
        assert!(manager.search.is_none());
        assert_eq!(manager.open, key);
        assert!(
            manager
                .requests
                .iter()
                .any(|request| matches!(request, Request::CancelSearch))
        );
    }

    #[test]
    fn cancelled_and_replaced_searches_cannot_deliver_results_into_the_current_tree() {
        let (mut manager, _) = explorer_with(vec![file("a.txt", 1)]);
        manager.begin_search(true);
        manager.append_search("old");
        manager.confirm_search();
        let old = manager.search.as_ref().unwrap().generation;
        manager.begin_search(true);
        manager.search_input = Some("new".into());
        manager.confirm_search();
        manager.apply(
            Event::SearchListed {
                key: source("a").key(),
                generation: old,
                path: "/".into(),
                listing: Listing {
                    entries: vec![file("old.txt", 1)],
                    truncated: false,
                },
            },
            Instant::now(),
        );
        assert!(manager.search.as_ref().unwrap().matches.is_empty());
        assert_eq!(manager.current_row().unwrap().path, "/a.txt");
        let current = manager.search.as_ref().unwrap().generation;
        manager.stop_search();
        manager.apply(
            Event::SearchFinished {
                key: source("a").key(),
                generation: current,
                incomplete: false,
                unreadable: 0,
                error: None,
            },
            Instant::now(),
        );
        assert!(manager.search.as_ref().unwrap().incomplete);
        manager.close();
        assert!(manager.search.is_none());
    }

    #[test]
    fn search_matches_literal_unicode_paths_and_does_not_treat_queries_as_regexes() {
        let (mut manager, _) = explorer_with(vec![file("ÜBER[1].txt", 1)]);
        manager.begin_search(true);
        manager.append_search("über[1]");
        manager.confirm_search();
        search_answer(
            &mut manager,
            "/",
            vec![file("ÜBER[1].txt", 1), file("über1.txt", 1)],
        );
        assert_eq!(manager.search.as_ref().unwrap().matches, ["/ÜBER[1].txt"]);
    }

    #[test]
    fn a_background_listing_cannot_overwrite_a_newer_search_listing() {
        let mut manager = Manager::default();
        let now = Instant::now();
        manager.open_source(source("a"), now);
        manager.begin_search(true);
        manager.append_search("config");
        manager.confirm_search();
        search_answer(&mut manager, "/", vec![file("config.txt", 10)]);
        // The older ordinary root request failed before the server recovered, but arrived late.
        answer(
            &mut manager,
            &source("a").key(),
            "/",
            Err("old configuration failure".into()),
        );
        assert!(manager.tree().unwrap().stopped.is_none());
        assert_eq!(manager.current_row().unwrap().path, "/config.txt");
    }

    #[test]
    fn a_file_previews_after_a_short_pause_and_moving_on_reads_nothing_in_between() {
        let (mut manager, start) = explorer_with(vec![
            file("a.txt", 10),
            file("b.txt", 10),
            file("c.txt", 10),
        ]);
        assert!(manager.update_preview(start));
        assert!(!manager.update_preview(start));
        assert!(previews(&mut manager).is_empty(), "Not immediately");
        manager.selected = 1;
        manager.update_preview(start + Duration::from_millis(100));
        manager.selected = 2;
        manager.update_preview(start + Duration::from_millis(200));
        assert!(
            previews(&mut manager).is_empty(),
            "Passing over files reads nothing"
        );
        assert!(manager.update_preview(start + Duration::from_millis(400)));
        assert_eq!(previews(&mut manager), ["/c.txt"]);
        answer_preview(&mut manager, b"hello", false);
        assert!(!manager.update_preview(start + Duration::from_secs(1)));
        assert_eq!(
            manager.preview.as_ref().unwrap().status,
            FilePreview::Text {
                text: "hello".into(),
                truncated: false
            }
        );
    }

    #[test]
    fn matching_paths_on_cached_sources_never_share_previews_or_late_answers() {
        let (mut manager, start) = explorer_with(vec![file("same.txt", 10)]);
        let a = source("a").key();
        let b = source("b").key();
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        answer_preview(&mut manager, b"container a", false);

        manager.open_source(source("b"), start);
        answer(&mut manager, &b, "/", listing(vec![file("same.txt", 10)]));
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        answer_preview(&mut manager, b"container b", false);

        manager.close();
        manager.open_source(source("a"), start);
        manager.update_preview(start);
        assert!(matches!(&manager.preview.as_ref().unwrap().status,
            FilePreview::Text { text, .. } if text == "container a"));

        manager.invalidate_preview(&a, "/same.txt");
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        let late = manager.preview.as_ref().unwrap().generation;
        manager.open_source(source("b"), start);
        manager.update_preview(start);
        manager.apply(
            Event::Previewed {
                key: a,
                generation: late,
                path: "/same.txt".into(),
                result: Err("late answer from a".into()),
            },
            start,
        );
        assert!(matches!(&manager.preview.as_ref().unwrap().status,
            FilePreview::Text { text, .. } if text == "container b"));
        assert!(matches!(manager.tree().unwrap().previews.get("/same.txt"),
            Some(FilePreview::Text { text, .. }) if text == "container b"));
    }

    #[test]
    fn edit_invalidation_targets_the_edited_file_even_after_selection_or_source_changes() {
        let (mut manager, start) = explorer_with(vec![file("a.txt", 10), file("b.txt", 10)]);
        let key = source("a").key();
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        answer_preview(&mut manager, b"old a", false);
        manager.move_selection(1);
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        answer_preview(&mut manager, b"b", false);
        manager.invalidate_preview(&key, "/a.txt");
        assert_eq!(manager.preview.as_ref().unwrap().path, "/b.txt");
        assert!(!manager.tree().unwrap().previews.contains_key("/a.txt"));

        manager.move_selection(-1);
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        answer_preview(&mut manager, b"new a", false);
        manager.open_source(source("b"), start);
        manager.invalidate_preview(&key, "/a.txt");
        manager.open_source(source("a"), start);
        manager.update_preview(start);
        assert_eq!(
            manager.preview.as_ref().unwrap().status,
            FilePreview::Waiting
        );
    }

    #[test]
    fn big_files_warn_instead_of_loading_until_asked_and_binaries_are_never_shown() {
        let big = PREVIEW_LIMIT as u64 + 1;
        let (mut manager, start) = explorer_with(vec![file("huge.iso", big), file("tool", 10)]);
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        assert!(previews(&mut manager).is_empty());
        assert_eq!(
            manager.preview.as_ref().unwrap().status,
            FilePreview::TooBig(big)
        );

        manager.show_big_file();
        assert_eq!(previews(&mut manager), ["/huge.iso"]);
        answer_preview(&mut manager, b"start of it", true);
        assert!(matches!(
            manager.preview.as_ref().unwrap().status,
            FilePreview::Text {
                truncated: true,
                ..
            }
        ));

        manager.selected = 1;
        manager.update_preview(start + Duration::from_secs(2));
        manager.update_preview(start + Duration::from_secs(3));
        assert_eq!(previews(&mut manager), ["/tool"]);
        answer_preview(&mut manager, b"\x7fELF\0\0", false);
        assert!(manager.preview_is_binary());
    }

    #[test]
    fn refreshing_rereads_the_files_folder_and_shows_what_is_new() {
        let (mut manager, start) = explorer_with(vec![folder("logs"), file("a.txt", 1)]);
        let key = source("a").key();
        manager.selected = 1;
        manager.update_preview(start);
        manager.refresh_selected();
        // The walk has `/logs` in flight here, so the refresh waits its turn.
        answer(&mut manager, &key, "/logs", listing(vec![]));
        assert_eq!(lists(&mut manager), ["/"]);
        answer(
            &mut manager,
            &key,
            "/",
            listing(vec![folder("logs"), file("a.txt", 1), file("new.txt", 2)]),
        );
        let rows = manager.tree().unwrap().rows();
        assert!(rows.iter().any(|row| row.path == "/new.txt"));
    }

    #[test]
    fn refresh_retries_a_failed_root_listing_after_the_server_is_repaired() {
        let mut manager = Manager::default();
        let key = source("a").key();
        manager.open_source(source("a"), Instant::now());
        lists(&mut manager);
        answer(
            &mut manager,
            &key,
            "/",
            Err("Invalid agent configuration".into()),
        );
        manager.refresh_selected();
        assert_eq!(lists(&mut manager), ["/"]);
        answer(
            &mut manager,
            &key,
            "/",
            listing(vec![file("recovered.txt", 1)]),
        );
        assert!(manager.tree().unwrap().stopped.is_none());
        assert_eq!(manager.current_row().unwrap().path, "/recovered.txt");
    }

    #[test]
    fn reselecting_a_failed_preview_retries_instead_of_reusing_a_cached_error() {
        let (mut manager, start) = explorer_with(vec![file("a.txt", 10), file("b.txt", 10)]);
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        let preview = manager.preview.as_ref().unwrap();
        manager.apply(
            Event::Previewed {
                key: source("a").key(),
                generation: preview.generation,
                path: preview.path.clone(),
                result: Err("Invalid agent configuration".into()),
            },
            start,
        );
        manager.selected = 1;
        manager.update_preview(start);
        manager.selected = 0;
        manager.update_preview(start);
        assert_eq!(
            manager.preview.as_ref().unwrap().status,
            FilePreview::Waiting
        );
        manager.requests.clear();
        manager.update_preview(start + Duration::from_secs(1));
        assert_eq!(previews(&mut manager), ["/a.txt"]);
    }

    #[test]
    fn a_late_preview_never_lands_on_another_file_and_a_cached_one_reads_nothing() {
        let (mut manager, start) = explorer_with(vec![file("a.txt", 10), file("b.txt", 10)]);
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        previews(&mut manager);
        let late = manager.preview.as_ref().unwrap().generation;
        manager.selected = 1;
        manager.update_preview(start + Duration::from_secs(2));
        manager.apply(
            Event::Previewed {
                key: source("a").key(),
                generation: late,
                path: "/a.txt".into(),
                result: Err("late".into()),
            },
            start,
        );
        assert_eq!(
            manager.preview.as_ref().unwrap().status,
            FilePreview::Waiting
        );

        manager.update_preview(start + Duration::from_secs(3));
        previews(&mut manager);
        answer_preview(&mut manager, b"b", false);
        manager.selected = 0;
        manager.update_preview(start + Duration::from_secs(4));
        manager.selected = 1;
        manager.update_preview(start + Duration::from_secs(5));
        assert!(matches!(
            manager.preview.as_ref().unwrap().status,
            FilePreview::Text { .. }
        ));
        assert!(previews(&mut manager).is_empty());

        manager.invalidate_preview(&source("a").key(), "/b.txt");
        manager.update_preview(start + Duration::from_secs(6));
        manager.update_preview(start + Duration::from_secs(7));
        assert_eq!(
            previews(&mut manager),
            ["/b.txt"],
            "After an edit it reads again"
        );
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
                    mode: 0o644,
                    uid: 0,
                    gid: 0,
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
