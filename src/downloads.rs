//! The download manager's state, free of I/O so its behavior can be tested directly: a file tree
//! filled in the background, a dialog, and the session's downloads. `main` runs what it requests.
use crate::files::{self, Entry, Header, Kind, Listing, Source, Totals};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    ops::ControlFlow,
    path::PathBuf,
    sync::Arc,
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
        text: Arc<PreviewText>,
        truncated: bool,
    },
    Binary,
    /// Over the limit, so it was not read. `p` shows the start of it anyway.
    TooBig(u64),
    Folder,
    Special,
    Failed(String),
}

/// File contents, cleaned once when they arrive so that a frame only touches what it shows.
/// Offsets rather than a string per line, so a file of empty lines stays small in the cache.
#[derive(Debug, PartialEq, Eq)]
pub struct PreviewText {
    text: String,
    starts: Vec<usize>,
}

impl PreviewText {
    pub fn new(raw: &str) -> Self {
        let mut preview = Self {
            text: String::with_capacity(raw.len()),
            starts: Vec::new(),
        };
        for line in raw.lines() {
            preview.starts.push(preview.text.len());
            preview.text.push_str(&crate::model::safe_text(line));
            preview.text.push('\n');
        }
        preview
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    pub fn line(&self, index: usize) -> Option<&str> {
        let start = *self.starts.get(index)?;
        let end = self.starts.get(index + 1).unwrap_or(&self.text.len()) - 1;
        Some(&self.text[start..end])
    }
}

impl FilePreview {
    pub fn text(raw: &str, truncated: bool) -> Self {
        Self::Text {
            text: Arc::new(PreviewText::new(raw)),
            truncated,
        }
    }
}

pub struct PreviewState {
    pub path: String,
    pub status: FilePreview,
    pub scroll: usize,
    since: Instant,
    generation: u64,
}

fn parent_of(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/",
        Some((parent, _)) => parent,
    }
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
        // A folder being read again stays on screen until the answer replaces it.
        if !matches!(self.folders.get(&path), Some(Folder::Loaded(_))) {
            self.folders.insert(path.clone(), Folder::Loading);
        }
        self.in_flight = Some(path.clone());
        Some(path)
    }

    /// Reads a folder again next, even one already read. What it shows now stays until then.
    fn reread(&mut self, path: &str) {
        self.queue.retain(|queued| queued != path);
        self.queue.push_front(path.to_owned());
        if !matches!(
            self.folders.get(path),
            Some(Folder::Loaded(_) | Folder::Loading)
        ) {
            self.folders.insert(path.to_owned(), Folder::Queued);
        }
    }

    /// Only a parent's listing can say what kind of thing a path is, so the parent goes first.
    fn reread_with_parent(&mut self, path: &str) {
        self.reread(path);
        if path != "/" {
            self.reread(parent_of(path));
        }
    }

    /// Drops everything known at and below a path that is no longer a folder. A read still on its
    /// way for it would otherwise bring a stale listing or error back.
    fn forget(&mut self, path: &str) {
        let prefix = format!("{path}/");
        let inside = |other: &str| other == path || other.starts_with(&prefix);
        let removed: usize = self
            .folders
            .iter()
            .filter(|(folder, _)| inside(folder))
            .map(|(_, folder)| match folder {
                Folder::Loaded(listing) => listing.entries.len(),
                _ => 0,
            })
            .sum();
        self.entries = self.entries.saturating_sub(removed);
        self.folders.retain(|folder, _| !inside(folder));
        self.expanded.retain(|folder| !inside(folder));
        self.queue.retain(|folder| !inside(folder));
        self.previews.retain(|file, _| !inside(file));
        if self.in_flight.as_deref().is_some_and(inside) {
            self.superseded_in_flight = true;
        }
    }

    /// Takes the answer for one folder and returns the paths in it that are no longer what they
    /// were, so that nothing shown for them outlives the change.
    fn listed(&mut self, path: &str, result: Result<Listing, String>) -> Vec<String> {
        if self.in_flight.as_deref() == Some(path) {
            self.in_flight = None;
            if std::mem::take(&mut self.superseded_in_flight) {
                return Vec::new();
            }
        }
        let mut changed = Vec::new();
        match result {
            Ok(listing) => {
                let old = match self.folders.get(path) {
                    Some(Folder::Loaded(old)) => old.entries.len(),
                    _ => 0,
                };
                self.entries = self.entries.saturating_sub(old) + listing.entries.len();
                if let Some(Folder::Loaded(previous)) = self.folders.get(path) {
                    let now: HashMap<&str, &Entry> = listing
                        .entries
                        .iter()
                        .map(|entry| (entry.name.as_str(), entry))
                        .collect();
                    let mut stale_folders = Vec::new();
                    for before in &previous.entries {
                        let current = now.get(before.name.as_str());
                        // Past the listing limit, a missing name proves nothing.
                        let gone = current.is_none() && !listing.truncated;
                        let other_kind = current.is_some_and(|entry| entry.kind != before.kind);
                        let other_size = current.is_some_and(|entry| entry.size != before.size);
                        if gone || other_kind || other_size {
                            changed.push(files::join(path, &before.name));
                        }
                        if before.kind == Kind::Folder && (gone || other_kind) {
                            stale_folders.push(files::join(path, &before.name));
                        }
                    }
                    for folder in stale_folders {
                        self.forget(&folder);
                    }
                    for file in &changed {
                        self.previews.remove(file);
                    }
                }
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
                let top = path == "/";
                if top {
                    self.stopped = Some(error.clone());
                    self.queue.clear();
                } else if files::path_gone(&error) {
                    // The folder was replaced or removed, which its parent's listing will show.
                    self.reread(parent_of(path));
                }
                // A failed re-read of the top keeps what was read, with the reason shown above it.
                if !(top && matches!(self.folders.get(path), Some(Folder::Loaded(_)))) {
                    self.folders.insert(path.to_owned(), Folder::Failed(error));
                }
            }
        }
        changed
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

    /// Walks what is on screen, in order, following only open folders. Paths are only built for
    /// folders, so counting or finding a row costs little even in a large tree.
    fn visit_rows<B>(&self, mut visit: impl FnMut(RowRef<'_>) -> ControlFlow<B>) -> ControlFlow<B> {
        fn walk<B>(
            tree: &Tree,
            path: &str,
            level: usize,
            visit: &mut impl FnMut(RowRef<'_>) -> ControlFlow<B>,
        ) -> ControlFlow<B> {
            let Some(Folder::Loaded(listing)) = tree.folders.get(path) else {
                return ControlFlow::Continue(());
            };
            for entry in &listing.entries {
                let folder = (entry.kind == Kind::Folder).then(|| files::join(path, &entry.name));
                let open = folder
                    .as_ref()
                    .is_some_and(|folder| tree.expanded.contains(folder));
                visit(RowRef {
                    parent: path,
                    folder: folder.as_deref(),
                    entry,
                    level,
                    open,
                })?;
                if let Some(folder) = folder.as_deref().filter(|_| open) {
                    walk(tree, folder, level + 1, visit)?;
                }
            }
            ControlFlow::Continue(())
        }
        walk(self, "/", 0, &mut visit)
    }

    fn row(&self, found: RowRef<'_>) -> Row {
        let path = found.folder.map_or_else(
            || files::join(found.parent, &found.entry.name),
            str::to_owned,
        );
        let state = match self.folders.get(&path) {
            Some(Folder::Loading) => RowState::Loading,
            Some(Folder::Failed(error)) => RowState::Failed(error.clone()),
            _ => RowState::Idle,
        };
        Row {
            path,
            entry: found.entry.clone(),
            level: found.level,
            open: found.open,
            state,
        }
    }

    /// What is on screen, in order, following only open folders.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let _ = self.visit_rows(|found| {
            rows.push(self.row(found));
            ControlFlow::<()>::Continue(())
        });
        rows
    }

    pub fn row_count(&self) -> usize {
        let mut count = 0;
        let _ = self.visit_rows(|_| {
            count += 1;
            ControlFlow::<()>::Continue(())
        });
        count
    }

    pub fn row_at(&self, index: usize) -> Option<Row> {
        let mut left = index;
        match self.visit_rows(|found| {
            if left == 0 {
                return ControlFlow::Break(self.row(found));
            }
            left -= 1;
            ControlFlow::Continue(())
        }) {
            ControlFlow::Break(row) => Some(row),
            ControlFlow::Continue(()) => None,
        }
    }
}

struct RowRef<'a> {
    parent: &'a str,
    folder: Option<&'a str>,
    entry: &'a Entry,
    level: usize,
    open: bool,
}

/// What a row shows about its own folder. Never the listing, which the row does not need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    Idle,
    Loading,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: String,
    pub entry: Entry,
    pub level: usize,
    pub open: bool,
    pub state: RowState,
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

/// Review before deleting. The tree may be out of date, so the file is checked again first, and
/// confirming stays impossible until that check has said it is still what the row showed.
pub struct DeleteReview {
    pub id: u64,
    pub source: Source,
    pub path: String,
    pub kind: Kind,
    pub check: Preview,
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
    CreateFile {
        source: Source,
        folder: String,
        name: String,
    },
    DeleteFile {
        source: Source,
        path: String,
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
    FileCreated {
        key: String,
        path: String,
        result: Result<(), String>,
    },
    FileDeleted {
        key: String,
        path: String,
        result: Result<(), String>,
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
    pub delete_review: Option<DeleteReview>,
    pub preview: Option<PreviewState>,
    /// Text typed after `:` in the explorer, as in vim.
    pub command: Option<String>,
    pub search_input: Option<String>,
    pub new_file_input: Option<String>,
    pub new_file_folder: String,
    /// A create or delete is on its way. One at a time, so results cannot be mixed up.
    pub changing: bool,
    /// Selected once the listing that contains it arrives.
    pub select_after: Option<String>,
    /// The result just shown should survive the selection moving because of it.
    pub keep_message: bool,
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
            delete_review: None,
            preview: None,
            command: None,
            search_input: None,
            new_file_input: None,
            new_file_folder: "/".into(),
            changing: false,
            select_after: None,
            keep_message: false,
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
        let tree = self
            .trees
            .entry(key.clone())
            .or_insert_with(|| Tree::new(source, now));
        tree.last_used = now;
        if self.open.as_deref() != Some(&key) {
            // A kept tree is as old as its reads, and what is on screen should not stay that way.
            // Outermost first, so that a folder replaced by a file is noticed before it is read.
            let open: Vec<String> = tree
                .expanded
                .iter()
                .rev()
                .filter(|folder| matches!(tree.folders.get(*folder), Some(Folder::Loaded(_))))
                .cloned()
                .collect();
            for folder in open {
                tree.reread(&folder);
            }
            self.cancel_search();
            self.selected = 0;
            self.preview = None;
            self.command = None;
            self.search_input = None;
            self.new_file_input = None;
            self.search = None;
            self.message = None;
        }
        self.open = Some(key);
        self.pane = Pane::Browse;
        self.pump();
    }

    /// The selected folder, or the folder holding the selected file.
    fn selected_folder(&self) -> String {
        self.current_row().map_or_else(
            || "/".to_owned(),
            |row| {
                if row.entry.kind == Kind::Folder {
                    row.path
                } else {
                    parent_of(&row.path).to_owned()
                }
            },
        )
    }

    pub fn begin_search(&mut self, system: bool) {
        self.touch();
        self.search_root = if system {
            "/".to_owned()
        } else {
            self.selected_folder()
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
            if !std::mem::take(&mut self.keep_message) {
                self.message = None;
            }
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
    /// each folder as it was when read, so this is how new files appear. A selected folder's
    /// parent is read too, because that is what shows it has become something else.
    pub fn refresh_selected(&mut self) {
        self.touch();
        let folder = self.selected_folder();
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
        tree.reread_with_parent(&folder);
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
        self.new_file_input = None;
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
        self.tree()?.row_at(self.selected)
    }

    pub fn move_selection(&mut self, amount: isize) {
        self.touch();
        match self.pane {
            Pane::Browse => {
                let count = self.tree().map_or(0, Tree::row_count);
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
            // Opening is when what was read earlier starts to matter, so look again.
            if matches!(
                tree.folders.get(&row.path),
                Some(Folder::Loaded(_) | Folder::Failed(_))
            ) {
                tree.reread_with_parent(&row.path);
            } else {
                tree.prioritize(&row.path);
            }
        }
        self.pump();
    }

    /// Why nothing may change right now, if anything.
    fn change_refused(&mut self, demo_reason: &'static str) -> bool {
        let reason = if self.demo {
            demo_reason
        } else if self.changing {
            crate::i18n::t("Wait for the last file change to finish")
        } else {
            return false;
        };
        self.message = Some(reason.to_owned());
        true
    }

    /// `a`: asks for a name. The new file goes in the selected folder, or beside the selected file.
    pub fn begin_new_file(&mut self) {
        self.touch();
        if self.change_refused(crate::i18n::t("Demo mode: files are not created")) {
            return;
        }
        self.new_file_folder = self.selected_folder();
        self.new_file_input = Some(String::new());
        self.message = None;
    }

    /// Shared by typed and pasted input. The name is checked when it is confirmed.
    pub fn append_new_file(&mut self, text: &str) {
        if let Some(input) = &mut self.new_file_input {
            for character in crate::model::safe_text(text).chars() {
                if input.len() + character.len_utf8() > 255 {
                    break;
                }
                input.push(character);
            }
            self.message = None;
        }
    }

    /// A bad name keeps the prompt open, so it can be corrected rather than typed again.
    pub fn confirm_new_file(&mut self) {
        let Some(input) = self.new_file_input.take() else {
            return;
        };
        let name = input.trim().to_owned();
        if name.is_empty() {
            return;
        }
        if let Err(error) = files::check_new_name(&name) {
            self.message = Some(error.to_string());
            self.new_file_input = Some(input);
            return;
        }
        let Some(source) = self.tree().map(|tree| tree.source.clone()) else {
            return;
        };
        self.changing = true;
        self.message = Some(crate::tf!(
            "Creating {path}…",
            path = files::join(&self.new_file_folder, &name)
        ));
        self.requests.push(Request::CreateFile {
            source,
            folder: self.new_file_folder.clone(),
            name,
        });
    }

    /// `D`: only opens the review. Nothing is removed until that has checked the file and the
    /// operator has confirmed.
    pub fn begin_delete(&mut self) {
        self.touch();
        let Some(row) = self.current_row() else {
            return;
        };
        if self.change_refused(crate::i18n::t("Demo mode: files are not deleted")) {
            return;
        }
        if !matches!(row.entry.kind, Kind::File | Kind::Link) {
            self.message = Some(crate::i18n::t("Only files and links can be deleted").to_owned());
            return;
        }
        let Some(source) = self.tree().map(|tree| tree.source.clone()) else {
            return;
        };
        let id = self.id();
        self.requests.push(Request::CloseDialog);
        self.requests.push(Request::Inspect {
            dialog: id,
            source: source.clone(),
            path: row.path.clone(),
        });
        self.message = None;
        self.delete_review = Some(DeleteReview {
            id,
            source,
            path: row.path,
            kind: row.entry.kind,
            check: Preview::Loading,
            error: None,
        });
    }

    pub fn close_delete_review(&mut self) {
        self.delete_review = None;
        self.requests.push(Request::CloseDialog);
    }

    pub fn confirm_delete(&mut self) {
        let Some(review) = &mut self.delete_review else {
            return;
        };
        match &review.check {
            Preview::Ready(_) => {}
            Preview::Loading => {
                review.error = Some(crate::i18n::t("Wait until the file has been checked").into());
                return;
            }
            // The reason is on screen already, and it is why this cannot go ahead.
            Preview::Failed(_) => return,
        }
        let (source, path) = (review.source.clone(), review.path.clone());
        self.close_delete_review();
        self.changing = true;
        self.message = Some(crate::tf!("Deleting {path}…", path = path));
        self.requests.push(Request::DeleteFile { source, path });
    }

    /// Something said this path is not there. Whatever was asked, the tree is out of date.
    fn notice_gone(&mut self, path: &str, error: &str) {
        if files::path_gone(error)
            && let Some(tree) = self.tree_mut()
        {
            tree.reread(parent_of(path));
            self.pump();
        }
    }

    /// An action outside this panel failed on a path. `key` says which tree it belongs to.
    pub fn action_failed(&mut self, key: &str, path: &str, error: &str) {
        if self.open.as_deref() == Some(key) {
            self.notice_gone(path, error);
        }
    }

    fn file_created(&mut self, key: &str, path: String, result: Result<(), String>) {
        self.changing = false;
        let folder = parent_of(&path).to_owned();
        let created = result.is_ok();
        if let Some(tree) = self.trees.get_mut(key) {
            // Also after a failure or a timeout, since the file may be there all the same.
            tree.reread(&folder);
            if created {
                tree.expanded.insert(folder);
            }
        }
        if self.open.as_deref() != Some(key) {
            return;
        }
        self.message = Some(match result {
            Ok(()) => {
                self.keep_message = true;
                let message = crate::tf!("Created {path}", path = path);
                self.select_after = Some(path);
                message
            }
            Err(error) => crate::tf!(
                "Could not create {path}: {error}",
                path = path,
                error = error
            ),
        });
        self.pump();
    }

    fn file_deleted(&mut self, key: &str, path: String, result: Result<(), String>) {
        self.changing = false;
        let folder = parent_of(&path).to_owned();
        let name = files::file_name(&path).to_owned();
        let gone = result
            .as_ref()
            .err()
            .is_some_and(|error| files::path_gone(error));
        if let Some(tree) = self.trees.get_mut(key) {
            tree.previews.remove(&path);
            // Off the screen at once, so nothing more can be asked of it; the read confirms this.
            if (result.is_ok() || gone)
                && let Some(Folder::Loaded(listing)) = tree.folders.get_mut(&folder)
            {
                let before = listing.entries.len();
                listing.entries.retain(|entry| entry.name != name);
                tree.entries = tree.entries.saturating_sub(before - listing.entries.len());
            }
            tree.reread(&folder);
        }
        if self.open.as_deref() != Some(key) {
            return;
        }
        self.invalidate_preview(key, &path);
        let count = self.tree().map_or(0, Tree::row_count);
        self.selected = self.selected.min(count.saturating_sub(1));
        self.message = Some(match &result {
            Ok(()) => crate::tf!("Deleted {path}", path = path),
            Err(_) if gone => crate::tf!("{path} was already gone", path = path),
            Err(error) => crate::tf!(
                "Could not delete {path}: {error}",
                path = path,
                error = error
            ),
        });
        if result.is_ok() || gone {
            self.keep_message = true;
            // A running search would hold up the read; what it found so far stays.
            self.stop_search();
            if let Some(search) = &mut self.search {
                search.matches.retain(|found| *found != path);
            }
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
                let gone = result.as_ref().err().cloned();
                let status = match result {
                    Err(error) => FilePreview::Failed(error),
                    Ok(start) if files::looks_binary(&start.bytes, start.truncated) => {
                        FilePreview::Binary
                    }
                    Ok(start) => {
                        FilePreview::text(&String::from_utf8_lossy(&start.bytes), start.truncated)
                    }
                };
                preview.status = status.clone();
                // A transient read/configuration failure must not remain cached after recovery.
                if !matches!(status, FilePreview::Failed(_))
                    && let Some(tree) = self.tree_mut()
                {
                    if tree.previews.len() >= MAX_CACHED_PREVIEWS {
                        tree.previews.clear();
                    }
                    tree.previews.insert(path.clone(), status);
                }
                if let Some(error) = gone {
                    self.notice_gone(&path, &error);
                }
            }
            Event::Listed { key, path, result } => {
                // Rows above the cursor can come and go, so the cursor stays on its path.
                let open = self.open.as_deref() == Some(&key);
                let selected = if open {
                    self.current_row().map(|row| row.path)
                } else {
                    None
                };
                let changed = self
                    .trees
                    .get_mut(&key)
                    .map_or_else(Vec::new, |tree| tree.listed(&path, result));
                for file in &changed {
                    self.invalidate_preview(&key, file);
                }
                if open && let Some(tree) = self.tree() {
                    let rows = tree.rows();
                    let find = |wanted: &str| rows.iter().position(|row| row.path == wanted);
                    let created = self.select_after.as_deref().and_then(find);
                    let index = created.or_else(|| selected.as_deref().and_then(find));
                    if created.is_some() {
                        self.select_after = None;
                    }
                    self.selected =
                        index.unwrap_or(self.selected.min(rows.len().saturating_sub(1)));
                }
                self.pump();
            }
            Event::FileCreated { key, path, result } => self.file_created(&key, path, result),
            Event::FileDeleted { key, path, result } => self.file_deleted(&key, path, result),
            Event::Inspected { dialog, result }
                if self
                    .delete_review
                    .as_ref()
                    .is_some_and(|review| review.id == dialog) =>
            {
                let Some(review) = self.delete_review.as_mut() else {
                    return;
                };
                let path = review.path.clone();
                match result {
                    Ok(header) if header.kind == review.kind => {
                        review.check = Preview::Ready(header);
                    }
                    Ok(_) => {
                        review.check = Preview::Failed(
                            crate::i18n::t("This changed since it was read, so it was not deleted")
                                .to_owned(),
                        );
                        if let Some(tree) = self.tree_mut() {
                            tree.reread_with_parent(parent_of(&path));
                        }
                        self.pump();
                    }
                    Err(error) => {
                        review.check = Preview::Failed(error.clone());
                        self.notice_gone(&path, &error);
                    }
                }
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
                    Err(error) => {
                        open.preview = Preview::Failed(error.clone());
                        let path = open.path.clone();
                        self.notice_gone(&path, &error);
                    }
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
            FilePreview::text("hello", false)
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
            FilePreview::Text { text, .. } if text.line(0) == Some("container a")));

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
            FilePreview::Text { text, .. } if text.line(0) == Some("container b")));
        assert!(matches!(manager.tree().unwrap().previews.get("/same.txt"),
            Some(FilePreview::Text { text, .. }) if text.line(0) == Some("container b")));
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

    /// `/data` is a folder with one file in it, read by the background walk, and selected.
    fn explorer_with_data_folder() -> (Manager, String) {
        let (mut manager, _) = explorer_with(vec![folder("data"), file("z.txt", 1)]);
        let key = source("a").key();
        answer(&mut manager, &key, "/data", listing(vec![file("old", 1)]));
        manager.requests.clear();
        manager.selected = 0;
        (manager, key)
    }

    #[test]
    fn refreshing_a_folder_that_became_a_file_notices_the_change() {
        let (mut manager, key) = explorer_with_data_folder();
        manager.refresh_selected();
        // Only the parent's listing can say what kind of thing the path is now.
        assert_eq!(lists(&mut manager), ["/"]);
        answer(
            &mut manager,
            &key,
            "/",
            listing(vec![file("data", 5), file("z.txt", 1)]),
        );
        // The queued read of the old folder is dropped, so it cannot bring a stale error back.
        assert!(lists(&mut manager).is_empty());
        let row = manager.current_row().unwrap();
        assert_eq!((row.path.as_str(), row.entry.kind), ("/data", Kind::File));
        assert_eq!(row.state, RowState::Idle);
        let tree = manager.tree().unwrap();
        assert!(!tree.folders.contains_key("/data"));
        assert!(!tree.building());
    }

    #[test]
    fn a_stale_folder_listing_cannot_return_after_the_path_became_a_file() {
        let (mut manager, key) = explorer_with_data_folder();
        manager.toggle_selected();
        assert_eq!(lists(&mut manager), ["/"]);
        answer(&mut manager, &key, "/", listing(vec![folder("data")]));
        // The folder is read next, and is replaced while that read is on its way.
        assert_eq!(lists(&mut manager), ["/data"]);
        manager.tree_mut().unwrap().reread("/");
        answer(&mut manager, &key, "/data", listing(vec![file("old", 1)]));
        assert_eq!(lists(&mut manager), ["/"]);
        answer(&mut manager, &key, "/", listing(vec![file("data", 5)]));
        manager.tree_mut().unwrap().reread("/");
        manager.pump();
        assert_eq!(lists(&mut manager), ["/"]);
        // Now back to a folder, with nothing left from its previous life.
        answer(&mut manager, &key, "/", listing(vec![folder("data")]));
        let tree = manager.tree().unwrap();
        assert!(!tree.expanded.contains("/data"));
        assert!(!matches!(
            tree.folders.get("/data"),
            Some(Folder::Loaded(_))
        ));
        assert_eq!(lists(&mut manager), ["/data"]);
    }

    #[test]
    fn a_read_on_its_way_for_a_forgotten_folder_is_discarded() {
        let (mut manager, _) = explorer_with(vec![folder("data")]);
        let key = source("a").key();
        // The walk has `/data` in flight when the parent says it is a file now.
        manager.tree_mut().unwrap().in_flight = Some("/data".into());
        let tree = manager.tree_mut().unwrap();
        tree.folders.insert(
            "/".into(),
            Folder::Loaded(listing(vec![folder("data")]).unwrap()),
        );
        tree.listed("/", listing(vec![file("data", 1)]));
        answer(&mut manager, &key, "/data", Err("Not a directory".into()));
        let tree = manager.tree().unwrap();
        assert!(!tree.folders.contains_key("/data"));
        assert_eq!(tree.row_at(0).unwrap().state, RowState::Idle);
    }

    #[test]
    fn a_read_that_finds_no_folder_makes_the_parent_look_again_once() {
        let (mut manager, _) = explorer_with(vec![folder("data")]);
        let key = source("a").key();
        answer(
            &mut manager,
            &key,
            "/data",
            Err("ls: /data/: Not a directory".into()),
        );
        assert_eq!(lists(&mut manager), ["/"]);
        answer(&mut manager, &key, "/", listing(vec![folder("data")]));
        // It still says folder, so nothing goes round in circles.
        assert!(lists(&mut manager).is_empty());
        answer(&mut manager, &key, "/", listing(vec![file("data", 3)]));
        assert_eq!(manager.current_row().unwrap().entry.kind, Kind::File);
    }

    #[test]
    fn opening_a_folder_or_the_explorer_again_looks_at_it_again() {
        let (mut manager, key) = explorer_with_data_folder();
        manager.toggle_selected();
        assert_eq!(lists(&mut manager), ["/"]);
        answer(&mut manager, &key, "/", listing(vec![folder("data")]));
        assert_eq!(lists(&mut manager), ["/data"]);
        answer(&mut manager, &key, "/data", listing(vec![file("new", 1)]));
        // Closing asks for nothing, and neither does a second tree being left alone.
        manager.toggle_selected();
        assert!(lists(&mut manager).is_empty());
        manager.toggle_selected();
        assert_eq!(lists(&mut manager), ["/"]);
        answer(&mut manager, &key, "/", listing(vec![folder("data")]));
        lists(&mut manager);
        answer(&mut manager, &key, "/data", listing(vec![file("new", 1)]));

        manager.close();
        manager.open_source(source("a"), Instant::now());
        assert_eq!(lists(&mut manager), ["/"]);
        answer(&mut manager, &key, "/", listing(vec![file("data", 1)]));
        assert!(lists(&mut manager).is_empty(), "the old folder is not read");
        assert_eq!(manager.tree().unwrap().row_count(), 1);
    }

    #[test]
    fn a_failed_reread_of_the_top_keeps_the_rows_and_says_why() {
        let (mut manager, _) = explorer_with(vec![file("a.txt", 1)]);
        let key = source("a").key();
        manager.refresh_selected();
        lists(&mut manager);
        answer(
            &mut manager,
            &key,
            "/",
            Err("The connection was lost".into()),
        );
        let tree = manager.tree().unwrap();
        assert_eq!(tree.stopped.as_deref(), Some("The connection was lost"));
        assert_eq!(tree.row_count(), 1);
    }

    #[test]
    fn the_cursor_stays_on_its_file_when_rows_above_it_change() {
        let (mut manager, _) = explorer_with(vec![file("b", 1), file("c", 1)]);
        let key = source("a").key();
        manager.selected = 1;
        manager.refresh_selected();
        lists(&mut manager);
        answer(
            &mut manager,
            &key,
            "/",
            listing(vec![file("a", 1), file("b", 1), file("c", 1)]),
        );
        assert_eq!(manager.current_row().unwrap().path, "/c");
        manager.refresh_selected();
        lists(&mut manager);
        answer(&mut manager, &key, "/", listing(vec![file("a", 1)]));
        assert_eq!(manager.current_row().unwrap().path, "/a");
    }

    #[test]
    fn a_preview_of_something_that_changed_is_not_kept() {
        let (mut manager, start) = explorer_with(vec![file("a.txt", 5)]);
        let key = source("a").key();
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        answer_preview(&mut manager, b"hello", false);
        assert!(manager.tree().unwrap().previews.contains_key("/a.txt"));
        manager.tree_mut().unwrap().reread("/");
        manager.pump();
        answer(&mut manager, &key, "/", listing(vec![folder("a.txt")]));
        assert!(manager.preview.is_none());
        assert!(manager.tree().unwrap().previews.is_empty());
    }

    #[test]
    fn a_preview_that_finds_nothing_corrects_the_tree() {
        let (mut manager, start) = explorer_with(vec![file("a.txt", 5)]);
        let key = source("a").key();
        manager.update_preview(start);
        manager.update_preview(start + Duration::from_secs(1));
        let preview = manager.preview.as_ref().unwrap();
        manager.apply(
            Event::Previewed {
                key: key.clone(),
                generation: preview.generation,
                path: "/a.txt".into(),
                result: Err("No such file or folder".into()),
            },
            start,
        );
        assert_eq!(lists(&mut manager), ["/"]);
        answer(&mut manager, &key, "/", listing(vec![]));
        assert!(manager.current_row().is_none());
    }

    fn inspected(manager: &mut Manager, kind: Kind) {
        let dialog = manager.delete_review.as_ref().unwrap().id;
        manager.apply(
            Event::Inspected {
                dialog,
                result: Ok(Header {
                    name: "a.txt".into(),
                    kind,
                    size: 5,
                    modified: 0,
                    mode: 0o644,
                    uid: 0,
                    gid: 0,
                }),
            },
            Instant::now(),
        );
    }

    fn deletes(manager: &mut Manager) -> Vec<String> {
        manager
            .requests
            .iter()
            .filter_map(|request| match request {
                Request::DeleteFile { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn nothing_is_deleted_before_the_check_and_the_confirmation() {
        let (mut manager, _) = explorer_with(vec![file("a.txt", 5), file("b.txt", 5)]);
        let key = source("a").key();
        manager.begin_delete();
        assert!(deletes(&mut manager).is_empty());
        manager.confirm_delete();
        assert!(
            deletes(&mut manager).is_empty(),
            "the check has not answered"
        );
        assert!(manager.delete_review.as_ref().unwrap().error.is_some());

        inspected(&mut manager, Kind::File);
        assert!(deletes(&mut manager).is_empty(), "not confirmed yet");
        manager.confirm_delete();
        assert_eq!(deletes(&mut manager), ["/a.txt"]);
        assert!(manager.delete_review.is_none());

        // One change at a time.
        manager.requests.clear();
        manager.begin_new_file();
        assert!(manager.new_file_input.is_none());
        manager.begin_delete();
        assert!(manager.delete_review.is_none());

        manager.apply(
            Event::FileDeleted {
                key: key.clone(),
                path: "/a.txt".into(),
                result: Ok(()),
            },
            Instant::now(),
        );
        // It leaves the screen at once, and the folder is read to confirm.
        assert_eq!(manager.current_row().unwrap().path, "/b.txt");
        assert!(
            manager
                .message
                .as_deref()
                .unwrap()
                .contains("Deleted /a.txt")
        );
        assert_eq!(lists(&mut manager), ["/"]);
        manager.update_preview(Instant::now());
        assert!(
            manager.message.is_some(),
            "the result stays when the cursor moves"
        );
        manager.begin_new_file();
        assert!(manager.new_file_input.is_some());
    }

    #[test]
    fn a_file_that_changed_since_it_was_read_cannot_be_deleted() {
        let (mut manager, _) = explorer_with(vec![file("a.txt", 5)]);
        let key = source("a").key();
        manager.begin_delete();
        manager.requests.clear();
        inspected(&mut manager, Kind::Folder);
        assert!(matches!(
            manager.delete_review.as_ref().unwrap().check,
            Preview::Failed(_)
        ));
        manager.confirm_delete();
        assert!(deletes(&mut manager).is_empty());
        assert_eq!(lists(&mut manager), ["/"]);
        manager.close_delete_review();

        // Gone by the time of the check: same, and the tree corrects itself.
        answer(&mut manager, &key, "/", listing(vec![file("b.txt", 5)]));
        manager.begin_delete();
        manager.requests.clear();
        let dialog = manager.delete_review.as_ref().unwrap().id;
        manager.apply(
            Event::Inspected {
                dialog,
                result: Err("No such file or folder".into()),
            },
            Instant::now(),
        );
        manager.confirm_delete();
        assert!(deletes(&mut manager).is_empty());
        assert_eq!(lists(&mut manager), ["/"]);
    }

    #[test]
    fn a_delete_that_finds_nothing_says_so_and_removes_the_row() {
        let (mut manager, _) = explorer_with(vec![file("a.txt", 5)]);
        let key = source("a").key();
        manager.apply(
            Event::FileDeleted {
                key,
                path: "/a.txt".into(),
                result: Err("rm: cannot remove '/a.txt': No such file or directory".into()),
            },
            Instant::now(),
        );
        assert!(manager.current_row().is_none());
        assert!(manager.message.as_deref().unwrap().contains("already gone"));
    }

    #[test]
    fn a_refused_delete_keeps_the_row_and_says_why() {
        let (mut manager, _) = explorer_with(vec![file("a.txt", 5)]);
        let key = source("a").key();
        manager.apply(
            Event::FileDeleted {
                key,
                path: "/a.txt".into(),
                result: Err("Not allowed by server: --allow-upload".into()),
            },
            Instant::now(),
        );
        assert_eq!(manager.current_row().unwrap().path, "/a.txt");
        assert!(
            manager
                .message
                .as_deref()
                .unwrap()
                .contains("--allow-upload")
        );
    }

    #[test]
    fn folders_cannot_be_deleted_and_demo_mode_changes_nothing() {
        let (mut manager, _) = explorer_with(vec![folder("data")]);
        manager.begin_delete();
        assert!(manager.delete_review.is_none());
        assert!(manager.message.is_some());

        let (mut manager, _) = explorer_with(vec![file("a.txt", 5)]);
        manager.demo = true;
        manager.begin_delete();
        assert!(manager.delete_review.is_none());
        manager.begin_new_file();
        assert!(manager.new_file_input.is_none());
        assert!(manager.requests.is_empty());
    }

    #[test]
    fn a_new_file_goes_beside_the_selection_and_is_selected_once_it_appears() {
        let (mut manager, _) = explorer_with_data_folder();
        let key = source("a").key();
        // On a folder, it goes inside, and the folder opens to show it.
        manager.begin_new_file();
        assert_eq!(manager.new_file_folder, "/data");
        manager.append_new_file("new.txt");
        manager.confirm_new_file();
        assert!(matches!(
            &manager.requests[..],
            [Request::CreateFile { folder, name, .. }] if folder == "/data" && name == "new.txt"
        ));
        manager.requests.clear();
        manager.apply(
            Event::FileCreated {
                key: key.clone(),
                path: "/data/new.txt".into(),
                result: Ok(()),
            },
            Instant::now(),
        );
        assert_eq!(lists(&mut manager), ["/data"]);
        answer(
            &mut manager,
            &key,
            "/data",
            listing(vec![file("new.txt", 0), file("old", 1)]),
        );
        assert_eq!(manager.current_row().unwrap().path, "/data/new.txt");
        manager.update_preview(Instant::now());
        assert!(manager.message.as_deref().unwrap().contains("Created"));

        // On a file, it goes beside it.
        manager.begin_new_file();
        assert_eq!(manager.new_file_folder, "/data");
    }

    #[test]
    fn a_bad_name_keeps_the_prompt_and_a_failed_create_is_reported() {
        let (mut manager, _) = explorer_with(vec![file("a.txt", 5)]);
        let key = source("a").key();
        for bad in ["a/b", "..", "."] {
            manager.begin_new_file();
            manager.append_new_file(bad);
            manager.confirm_new_file();
            assert_eq!(manager.new_file_input.as_deref(), Some(bad));
            assert!(manager.message.is_some());
            assert!(manager.requests.is_empty());
            manager.new_file_input = None;
        }
        manager.begin_new_file();
        manager.confirm_new_file();
        assert!(manager.requests.is_empty(), "an empty name is a cancel");

        manager.begin_new_file();
        manager.append_new_file("a.txt");
        manager.confirm_new_file();
        manager.requests.clear();
        manager.apply(
            Event::FileCreated {
                key,
                path: "/a.txt".into(),
                result: Err("A file with this name already exists".into()),
            },
            Instant::now(),
        );
        assert!(
            manager
                .message
                .as_deref()
                .unwrap()
                .contains("already exists")
        );
        assert!(manager.select_after.is_none());
        assert_eq!(lists(&mut manager), ["/"]);
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

        // Reopening within the window keeps what was already read, and looks again only at what
        // is on screen, so it is not a new walk. The rows stay while it does.
        manager.close();
        manager.open_source(source("a"), start + Duration::from_secs(60));
        assert_eq!(lists(&mut manager), ["/"]);
        assert_eq!(manager.tree().unwrap().rows().len(), 1);
        answer(&mut manager, &key, "/", listing(vec![file("x", 1)]));
        assert!(lists(&mut manager).is_empty());

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

    fn nested_tree() -> (Manager, Instant) {
        let (mut manager, start) = explorer_with(vec![
            folder("a"),
            file("b.txt", 1),
            folder("c"),
            folder("d"),
        ]);
        let tree = manager.tree_mut().unwrap();
        tree.queue.clear();
        tree.in_flight = None;
        tree.expanded.insert("/a".into());
        tree.expanded.insert("/a/inner".into());
        for (path, entries) in [
            ("/a", vec![folder("inner"), file("x", 2)]),
            ("/a/inner", vec![file("y", 3)]),
            ("/c", vec![file("hidden", 4)]),
        ] {
            tree.folders.insert(
                path.into(),
                Folder::Loaded(Listing {
                    entries,
                    truncated: false,
                }),
            );
        }
        tree.folders
            .insert("/d".into(), Folder::Failed("denied".into()));
        (manager, start)
    }

    #[test]
    fn counting_and_finding_a_row_agree_with_the_full_list() {
        let (manager, _) = nested_tree();
        let tree = manager.tree().unwrap();
        let rows = tree.rows();
        assert_eq!(
            rows.iter().map(|row| row.path.as_str()).collect::<Vec<_>>(),
            ["/a", "/a/inner", "/a/inner/y", "/a/x", "/b.txt", "/c", "/d"]
        );
        assert_eq!(rows[2].level, 2);
        assert_eq!(tree.row_count(), rows.len());
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(tree.row_at(index).as_ref(), Some(row));
        }
        assert_eq!(tree.row_at(rows.len()), None);
    }

    #[test]
    fn a_row_reports_its_folder_state_without_carrying_the_listing() {
        let (mut manager, _) = nested_tree();
        manager
            .tree_mut()
            .unwrap()
            .folders
            .insert("/c".into(), Folder::Loading);
        let rows = manager.tree().unwrap().rows();
        let state = |path: &str| &rows.iter().find(|row| row.path == path).unwrap().state;
        assert_eq!(state("/a"), &RowState::Idle);
        assert_eq!(state("/b.txt"), &RowState::Idle);
        assert_eq!(state("/c"), &RowState::Loading);
        assert_eq!(state("/d"), &RowState::Failed("denied".into()));
    }

    #[test]
    fn preview_text_is_cleaned_once_and_read_by_line() {
        let text = PreviewText::new("one\u{1b}[2J\r\n\n\tthree");
        assert_eq!(text.line_count(), 3);
        assert_eq!(text.line(0), Some("one"));
        assert_eq!(text.line(1), Some(""));
        assert_eq!(text.line(2), Some("    three"));
        assert_eq!(text.line(3), None);
        assert_eq!(PreviewText::new("").line_count(), 0);
    }

    /// Opt-in timings: `cargo test --release --lib benchmark_explorer -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn benchmark_explorer_rows() {
        fn time(name: &str, mut action: impl FnMut()) {
            let mut samples: Vec<f64> = (0..15)
                .map(|_| {
                    let started = Instant::now();
                    for _ in 0..5 {
                        action();
                    }
                    started.elapsed().as_secs_f64() * 1e6 / 5.0
                })
                .collect();
            samples.sort_by(f64::total_cmp);
            println!("{name}: median_us={:.1}", samples[7]);
        }
        for (folders, each) in [(50, 40), (200, 100), (200, 500), (2000, 50)] {
            let (mut manager, start) = explorer_with(
                (0..folders)
                    .map(|index| folder(&format!("folder{index:04}")))
                    .collect(),
            );
            let tree = manager.tree_mut().unwrap();
            tree.queue.clear();
            tree.in_flight = None;
            for index in 0..folders {
                let entries = (0..each)
                    .map(|file_index| file(&format!("some-file-name-{file_index:05}.conf"), 1234))
                    .collect();
                tree.folders.insert(
                    format!("/folder{index:04}"),
                    Folder::Loaded(Listing {
                        entries,
                        truncated: false,
                    }),
                );
            }
            time(&format!("explorer/{folders}x{each}/rows"), || {
                std::hint::black_box(manager.tree().unwrap().rows());
            });
            let later = start + Duration::from_secs(1);
            manager.update_preview(start);
            manager.update_preview(later);
            for selected in [0, folders - 1] {
                manager.selected = selected;
                manager.update_preview(later);
                manager.update_preview(later + PREVIEW_DELAY);
                time(
                    &format!("explorer/{folders}x{each}/idle_loop_pass/selected_{selected}"),
                    || {
                        std::hint::black_box(manager.update_preview(later + PREVIEW_DELAY));
                    },
                );
            }
        }
    }
}
