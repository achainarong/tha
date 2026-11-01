use crate::{
    config::Connection,
    logs::LogBuffer,
    logs::view::LogView,
    model::{Condition, Container, Resource, Snapshot, Tone},
    workspace::Project,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug)]
pub struct Engine {
    pub connection: Connection,
    pub runtime: Option<crate::runtime::Target>,
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
    pub alias: Option<String>,
    pub metrics: crate::metrics::Store,
    pub metrics_revision: u64,
    pub events: crate::events::Timeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Network,
    Volume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// A whole machine. It has no single engine, so it reads and acts on nothing by itself.
    Machine(String),
    Engine,
    Namespace(String),
    Project {
        namespace: Option<String>,
        project: String,
    },
    Services {
        namespace: Option<String>,
        project: String,
    },
    Service {
        namespace: Option<String>,
        project: String,
        service: String,
    },
    Standalone(Option<String>),
    Container(String),
    Resources {
        project: Option<String>,
        kind: ResourceKind,
    },
    Resource {
        name: String,
        kind: ResourceKind,
    },
}

#[derive(Debug, Clone)]
pub struct Row {
    pub engine: usize,
    pub scope: Scope,
    pub title: String,
    pub depth: usize,
    pub branch: bool,
    pub key: String,
    pub running: Option<bool>,
}

/// What a mouse event over part of the screen acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseTarget {
    Overview(OverviewMode),
    Explorer,
    TreeRow(usize),
    TreeArrow(usize),
    Details,
    DetailTab(Tab),
    MetricsRange(usize),
    LogScrollbar,
    NetworkRow(usize),
    LayerRow(usize),
    FindingsPane,
    FindingRow(usize),
    Performance,
    CollapsedPerformance,
    PerformanceEngine(usize),
    InventoryPane,
    InventoryRuntime(usize),
    InventoryRow(usize),
    PickerRow(usize),
    SettingsRow(usize),
    ServerRow(usize),
    OptionRow(usize),
}

/// One cursor stop in the Networks tab, recorded by the last draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkTarget {
    pub line: usize,
    /// Explorer key of an attached container; absent for a network heading.
    pub container: Option<String>,
}

#[derive(Debug, Clone)]
struct Node {
    row: Row,
    children: Vec<Node>,
}

fn node(engine: usize, scope: Scope, title: String, children: Vec<Node>) -> Node {
    let key = format!("{engine}/{scope:?}");
    Node {
        row: Row {
            engine,
            scope,
            title,
            depth: 0,
            branch: !children.is_empty(),
            key,
            running: None,
        },
        children,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tab {
    Overview,
    Logs,
    Networks,
    Exec,
    Layers,
    History,
    Security,
    Events,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverviewMode {
    #[default]
    Containers,
    Images,
    Volumes,
    Security,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyField {
    Id,
    Name,
    Image,
}

/// One explicit connectivity test, shown with where it ran and what it contacted.
pub struct NetworkTestView {
    pub source: crate::files::Source,
    pub host: String,
    pub port: u16,
    pub result: Option<Result<crate::files::NetTest, String>>,
}

/// Review before a diagnostic bundle is written. Nothing is uploaded.
pub struct DiagnosticExport {
    pub path: String,
    pub redact: bool,
    pub error: Option<String>,
}

/// A Compose file to open in the editor, once `main` can hand over the terminal.
#[derive(Debug, Clone)]
pub struct EditRequest {
    pub location: crate::compose_edit::Location,
    pub label: String,
}

/// The incident list for what was selected when it opened. Its contents stay current.
pub struct TimelineView {
    pub engine: usize,
    pub title: String,
    pub containers: Vec<Container>,
    pub selected: usize,
}

/// The fleet list. Its contents are recomputed on each frame, so they stay current.
#[derive(Default)]
pub struct FleetView {
    pub selected: usize,
    pub vulnerabilities: crate::fleet::Vulnerabilities,
}

/// What the capability dialog is about, kept by name so it follows no later selection.
pub struct CapabilitiesView {
    pub title: String,
    pub engine: Option<usize>,
    pub server: Option<String>,
    pub scroll: usize,
}

#[derive(Debug, Default, Clone)]
pub struct InventoryView {
    pub zoom: bool,
    pub query: String,
    pub filter: usize,
    pub sort: usize,
    pub selected: Option<String>,
    pub index: usize,
    pub offset: usize,
    pub details: bool,
    pub details_scroll: usize,
    pub pending_g: bool,
    pub marked: HashSet<String>,
    pub scan_marked: HashSet<String>,
}

pub struct InventoryRemovalTarget {
    pub request: crate::backend::RemovalRequest,
    pub label: String,
}

pub struct InventoryRemoval {
    pub targets: Vec<InventoryRemovalTarget>,
    pub context: (usize, OverviewMode),
    pub scroll: usize,
    pub visible: bool,
    pub pending: bool,
    pub dismissed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
    InventorySearch,
    Command,
    /// Plain-text terms; F3 opens this one.
    LogTextFilter,
    /// The advanced JSON document.
    LogFilter,
    ExportLogs,
}

impl App {
    /// Opens a log filter field, with the other field's active value ready behind Tab.
    pub fn open_log_filter(&mut self, mode: InputMode) {
        let (input, other) = if mode == InputMode::LogFilter {
            (&self.log_filter_text, &self.log_filter_terms)
        } else {
            (&self.log_filter_terms, &self.log_filter_text)
        };
        self.input = input.clone();
        self.log_filter_other_draft = other.clone();
        self.input_mode = mode;
    }

    /// Switches fields without applying or losing what was typed. Only the shown field applies.
    pub fn toggle_log_filter_field(&mut self) {
        std::mem::swap(&mut self.input, &mut self.log_filter_other_draft);
        self.input_mode = if self.input_mode == InputMode::LogFilter {
            InputMode::LogTextFilter
        } else {
            InputMode::LogFilter
        };
    }

    /// Clears both parts of the filter. Retained entries and highlight search are untouched.
    pub fn clear_log_filters(&mut self) {
        self.log_view.filter = Default::default();
        self.log_filter_text = "{}".into();
        self.log_filter_terms.clear();
    }

    /// What currently narrows the log view, as shown in the status line and export review.
    pub fn log_filter_summary(&self) -> String {
        let filter = &self.log_view.filter;
        let terms = crate::model::safe_text(&self.log_filter_terms);
        match (filter.text_active(), filter.advanced_active()) {
            (false, false) => crate::i18n::t("off").into(),
            (true, false) => terms.split_whitespace().collect::<Vec<_>>().join(" "),
            (false, true) => crate::i18n::t("advanced").into(),
            (true, true) => crate::tf!(
                "{terms} + advanced",
                terms = terms.split_whitespace().collect::<Vec<_>>().join(" ")
            ),
        }
    }
}

#[derive(Clone)]
struct NavigationState {
    machine: Option<String>,
    project: Option<Project>,
    selected: Option<String>,
    tree_offset: usize,
    tree_query: String,
    expanded: HashSet<String>,
    detail_focus: bool,
    tab: Tab,
    detail_scroll: usize,
}

pub enum Choice {
    ActionResult(u64),
    Session(u64),
    Action {
        action: crate::actions::Action,
        row: String,
    },
    ContextSort(crate::context_sort::ContextSort),
    Project(Project),
    Exec {
        engine: usize,
        container: Box<Container>,
    },
    Compose(crate::compose::Action),
    Diagnose(crate::diagnostics::Kind),
    DiagnosticPeer(Box<Container>),
    DiagnosticPeerNone,
    SavedView(String),
    ComposeFile(Box<EditRequest>),
}

pub struct Picker {
    pub title: String,
    pub choices: Vec<Choice>,
    pub selected: usize,
}

/// UI-only timing/selection metadata; never changes observed workload state.
struct ActionFeedback {
    started_at: std::time::Instant,
    finished_at: Option<std::time::Instant>,
    row_key: Option<String>,
    result_at: Vec<Option<std::time::Instant>>,
}

pub struct RowActionFeedback {
    pub symbol: &'static str,
    pub label: String,
    pub tone: Tone,
    pub active: bool,
}

pub struct RowPresentation {
    pub condition: Option<Condition>,
    pub cpu: String,
    pub memory: String,
    metrics_ready: bool,
}

#[derive(PartialEq, Eq)]
struct PresentationContext {
    project: Option<Project>,
    demo: bool,
    stale_after: std::time::Duration,
    availability: Vec<(bool, bool, bool)>,
}

#[derive(Default)]
struct PresentationCache {
    rows: BTreeMap<String, RowPresentation>,
    context: Option<PresentationContext>,
    expires_at: Option<std::time::Instant>,
    #[cfg(test)]
    builds: usize,
}

impl ActionFeedback {
    fn active(&self, action: crate::actions::Action, now: std::time::Instant) -> RowActionFeedback {
        use crate::actions::Action;
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let elapsed = now.saturating_duration_since(self.started_at);
        let verb = match action {
            Action::Start => crate::i18n::t("Starting"),
            Action::Stop => crate::i18n::t("Stopping"),
            Action::Restart => crate::i18n::t("Restarting"),
            Action::Delete => crate::i18n::t("Deleting"),
            Action::Pause => crate::i18n::t("Pausing"),
            Action::Unpause => crate::i18n::t("Unpausing"),
            Action::Kill => crate::i18n::t("Killing"),
        };
        RowActionFeedback {
            symbol: FRAMES[((elapsed.as_millis() / 100) % FRAMES.len() as u128) as usize],
            label: format!("{verb}… {}s", elapsed.as_secs()),
            tone: Tone::Warning,
            active: true,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PerformanceScope {
    #[default]
    Runtime,
    Container,
}

pub struct App {
    pub service_modules: bool,
    pub overview_mode: OverviewMode,
    pub inventory_engine: usize,
    pub inventory_views: HashMap<(usize, OverviewMode), InventoryView>,
    pub inventory_removal: Option<InventoryRemoval>,
    pub inventory_removal_request: Option<Vec<crate::backend::RemovalRequest>>,
    pub inventory_removal_report: Option<crate::backend::RemovalReport>,
    pub inventory_removal_results_visible: bool,
    pub inventory_removal_results_scroll: usize,
    pub discover_connections: bool,
    pub history_view: crate::history::View,
    pub history_database: Option<PathBuf>,
    pub metrics_database: Option<PathBuf>,
    pub auto_history_database: Option<PathBuf>,
    presentations: PresentationCache,
    pub engines: Vec<Engine>,
    pub machines: Vec<crate::machines::Machine>,
    /// The focused machine: a server name, or `machines::LOCAL_MACHINE`.
    pub machine: Option<String>,
    pub remote_connect: crate::remote::ConnectMode,
    pub remotes_path: Option<PathBuf>,
    pub remotes_problem: Option<String>,
    pub remotes_modal: Option<crate::machines::RemotesModal>,
    pub remote_commands: Vec<crate::machines::RemoteCommand>,
    /// The server whose passphrase prompt is open, and those waiting behind it.
    pub unlocking: Option<String>,
    pub pending_unlocks: Vec<String>,
    pub remote_availability: Vec<(usize, bool)>,
    pub clipboard_request: Option<String>,
    pending_machine: Option<String>,
    local_machine_expanded: bool,
    pub context_order: Vec<usize>,
    pub context_pins: Vec<String>,
    pub context_sort: crate::context_sort::ContextSort,
    pub keymap: crate::keymap::Keymap,
    pub keyboard: crate::keyboard::KeyboardPreset,
    pub theme: crate::theme::Theme,
    pub language: crate::i18n::Language,
    pub base_settings: crate::settings::BaseSettings,
    pub saved_settings: crate::settings::SavedSettings,
    pub settings: Option<crate::settings::SettingsState>,
    pub settings_save_error: Option<String>,
    pub custom_bindings: std::collections::BTreeMap<String, String>,
    pub preferences_path: Option<PathBuf>,
    pub capabilities_view: Option<CapabilitiesView>,
    pub downloads: crate::downloads::Manager,
    pub fleet_view: Option<FleetView>,
    pub fleet_vulnerability_request: Option<Vec<crate::security::Target>>,
    pub timeline_view: Option<TimelineView>,
    pub alerts: crate::alerts::Alerts,
    pub alert_rules: crate::alerts::Rules,
    /// When the running review of the last Compose operation stops updating.
    pub compose_review_until: Option<std::time::Instant>,
    pub compose_before: Option<(usize, String, Vec<crate::compose::ServiceImage>)>,
    pub editor: crate::editor::Editor,
    pub vscode_path: Option<PathBuf>,
    pub edit_request: Option<EditRequest>,
    pub config_path: Option<PathBuf>,
    pub config_modified: Option<std::time::SystemTime>,
    pub check_updates: bool,
    pub network_test: Option<NetworkTestView>,
    pub network_test_request: bool,
    pub diagnostic_export: Option<DiagnosticExport>,
    pub diagnostic_export_request: Option<(PathBuf, String)>,
    pub saved_views: Vec<crate::saved_views::SavedView>,
    /// `None` keeps saved views for this session only, as in demo mode.
    pub saved_views_path: Option<PathBuf>,
    pending_preferences: Option<crate::preferences::Preferences>,
    /// Machine identity is provider + machine name, never its temporary socket path.
    podman_machines: BTreeMap<String, usize>,
    pub expanded: HashSet<String>,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub tree_offset: usize,
    pub detail_focus: bool,
    pub detail_zoom: Option<Tab>,
    pub explorer_zoom: bool,
    pub performance_open: bool,
    pub performance_scope: PerformanceScope,
    pub performance_expanded: bool,
    pub performance_focus: bool,
    /// Selected position in the connected, non-alias performance list.
    pub performance_scroll: usize,
    pub performance_offset: usize,
    pub detail_scroll: usize,
    pub network_cursor: usize,
    pub network_cursor_owner: Option<String>,
    pub network_targets: Vec<NetworkTarget>,
    pub mouse_enabled: bool,
    /// Rebuilt by every draw, in paint order, so the last match is on top.
    pub mouse_regions: Vec<(ratatui::layout::Rect, MouseTarget)>,
    /// Mouse row and display-row position at the start of a scrollbar drag.
    pub log_scrollbar_drag: Option<(u16, usize)>,
    pub log_view: LogView,
    pub log_filter_text: String,
    /// The text terms as typed, for reopening the editor.
    pub log_filter_terms: String,
    /// The unapplied other field while Tab switches between Text and Advanced.
    pub log_filter_other_draft: String,
    pub log_export_scope: Option<String>,
    pub log_export_review_visible: bool,
    pub log_export_request: Option<crate::logs::export::Request>,
    pub log_export_busy: bool,
    pub tab: Tab,
    pub input_mode: InputMode,
    pub input: String,
    pub tree_query: String,
    pub help: bool,
    pub help_scroll: usize,
    pub monochrome: bool,
    pub logs: LogBuffer,
    pub log_scope: Option<String>,
    /// Engine and container keys the retained logs were subscribed for.
    pub log_scope_targets: Option<(usize, Vec<String>)>,
    pub diagnostics: Option<crate::diagnostics::View>,
    pub diagnostics_requests: Vec<crate::diagnostics::Request>,
    pub diagnostics_log_request: Option<crate::diagnostics::LogRequest>,
    /// Inspect details for the one container the Overview tab is showing.
    pub overview_details: Option<crate::diagnostics::Side>,
    overview_details_at: Option<std::time::Instant>,
    diagnostics_generation: u64,
    pub log_generation: u64,
    /// Unix second the logs start from; None shows recent lines.
    pub log_since: Option<i64>,
    pub compose_confirmation: Option<crate::compose::Confirmation>,
    pub compose_request: Option<crate::compose::Request>,
    pub compose_job: Option<crate::compose::Job>,
    pub compose_output_visible: bool,
    /// Registry comparison results by runtime and image ID, from this session only.
    pub image_updates: HashMap<(usize, String), crate::images::UpdateStatus>,
    pub image_check_request: Option<Vec<crate::images::Target>>,
    pub image_pull_request: Option<crate::images::PullRequest>,
    pub image_pull_active: bool,
    pub notice: String,
    pub pending_g: bool,
    pub demo: bool,
    pub metrics_refresh_seconds: u64,
    pub local_metrics_refresh_seconds: u64,
    pub project: Option<Project>,
    history: Vec<NavigationState>,
    pub picker: Option<Picker>,
    pub exec_request: Option<crate::exec::Request>,
    pub exec_command: Vec<String>,
    pub sessions: crate::sessions::Sessions,
    pub exec_input: bool,
    pub exec_dimensions: (u16, u16),
    pub platforms: crate::platform::Cache,
    pub layers: crate::layers::View,
    pub security: crate::security::View,
    pub action_confirmation: Option<crate::actions::Confirmation>,
    pub action_request: Option<crate::actions::Request>,
    pub action_jobs: BTreeMap<u64, crate::actions::Progress>,
    action_feedback: BTreeMap<u64, ActionFeedback>,
    action_confirmation_row: Option<String>,
    pub action_selected: Option<u64>,
    next_action_id: u64,
    pub action_results_visible: bool,
    pub action_review_visible: bool,
    startup_cwd: Option<PathBuf>,
    startup_pending: HashSet<usize>,
    startup_discovery_pending: bool,
}

impl App {
    pub fn new(
        connections: Vec<Connection>,
        log_capacity: usize,
        monochrome: bool,
        demo: bool,
    ) -> Self {
        let context_order = (0..connections.len()).collect();
        let mut app = Self {
            machines: Vec::new(),
            machine: None,
            remote_connect: Default::default(),
            remotes_path: None,
            remotes_problem: None,
            remotes_modal: None,
            remote_commands: Vec::new(),
            unlocking: None,
            pending_unlocks: Vec::new(),
            remote_availability: Vec::new(),
            clipboard_request: None,
            pending_machine: None,
            local_machine_expanded: false,
            service_modules: false,
            discover_connections: true,
            history_view: Default::default(),
            history_database: None,
            metrics_database: None,
            auto_history_database: None,
            presentations: Default::default(),
            context_order,
            context_pins: Vec::new(),
            context_sort: Default::default(),
            keymap: Default::default(),
            keyboard: Default::default(),
            theme: Default::default(),
            language: Default::default(),
            base_settings: Default::default(),
            saved_settings: Default::default(),
            settings: None,
            settings_save_error: None,
            custom_bindings: Default::default(),
            preferences_path: None,
            capabilities_view: None,
            downloads: crate::downloads::Manager {
                demo,
                ..Default::default()
            },
            fleet_view: None,
            fleet_vulnerability_request: None,
            timeline_view: None,
            alerts: crate::alerts::Alerts {
                started_ms: crate::history::now_ms(),
                ..Default::default()
            },
            alert_rules: Default::default(),
            compose_review_until: None,
            compose_before: None,
            editor: Default::default(),
            vscode_path: None,
            edit_request: None,
            config_path: None,
            config_modified: None,
            check_updates: false,
            network_test: None,
            network_test_request: false,
            diagnostic_export: None,
            diagnostic_export_request: None,
            saved_views: Vec::new(),
            saved_views_path: None,
            pending_preferences: None,
            podman_machines: BTreeMap::new(),
            engines: connections
                .into_iter()
                .map(|connection| Engine {
                    runtime: std::env::var_os("HOME").and_then(|home| {
                        crate::runtime::Target::from_connection(
                            &connection,
                            std::path::Path::new(&home),
                        )
                    }),
                    connection,
                    snapshot: None,
                    error: None,
                    alias: None,
                    metrics: Default::default(),
                    metrics_revision: 0,
                    events: Default::default(),
                })
                .collect(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: 0,
            tree_offset: 0,
            detail_focus: false,
            detail_zoom: None,
            explorer_zoom: false,
            performance_open: false,
            performance_scope: PerformanceScope::Runtime,
            performance_expanded: false,
            performance_focus: false,
            performance_scroll: 0,
            performance_offset: 0,
            overview_mode: OverviewMode::Containers,
            inventory_engine: 0,
            inventory_views: HashMap::new(),
            inventory_removal: None,
            inventory_removal_request: None,
            inventory_removal_report: None,
            inventory_removal_results_visible: false,
            inventory_removal_results_scroll: 0,
            detail_scroll: 0,
            network_cursor: 0,
            network_cursor_owner: None,
            network_targets: Vec::new(),
            mouse_enabled: true,
            mouse_regions: Vec::new(),
            log_scrollbar_drag: None,
            log_view: LogView::new(true),
            log_filter_text: "{}".into(),
            log_filter_terms: String::new(),
            log_filter_other_draft: String::new(),
            log_export_scope: None,
            log_export_review_visible: false,
            log_export_request: None,
            log_export_busy: false,
            tab: Tab::Overview,
            input_mode: InputMode::Normal,
            input: String::new(),
            tree_query: String::new(),
            help: false,
            help_scroll: 0,
            monochrome,
            logs: LogBuffer::new(log_capacity),
            log_scope: None,
            log_scope_targets: None,
            diagnostics: None,
            diagnostics_requests: Vec::new(),
            diagnostics_log_request: None,
            overview_details: None,
            overview_details_at: None,
            diagnostics_generation: 0,
            log_generation: 0,
            log_since: None,
            compose_confirmation: None,
            compose_request: None,
            compose_job: None,
            compose_output_visible: false,
            image_updates: HashMap::new(),
            image_check_request: None,
            image_pull_request: None,
            image_pull_active: false,
            notice: String::new(),
            pending_g: false,
            demo,
            metrics_refresh_seconds: 1,
            local_metrics_refresh_seconds: 1,
            project: None,
            history: Vec::new(),
            picker: None,
            exec_request: None,
            exec_command: vec!["sh".into()],
            sessions: Default::default(),
            exec_input: false,
            exec_dimensions: (24, 80),
            platforms: Default::default(),
            layers: Default::default(),
            security: Default::default(),
            action_confirmation: None,
            action_request: None,
            action_jobs: BTreeMap::new(),
            action_feedback: BTreeMap::new(),
            action_confirmation_row: None,
            action_selected: None,
            next_action_id: 0,
            action_results_visible: false,
            action_review_visible: false,
            startup_cwd: None,
            startup_pending: HashSet::new(),
            startup_discovery_pending: false,
        };
        for i in 0..app.engines.len() {
            app.expanded.insert(format!("{i}/{:?}", Scope::Engine));
        }
        app.refresh_tree();
        app
    }

    pub fn update_engine(&mut self, index: usize, result: Result<Snapshot, String>) {
        let refreshed = result.is_ok();
        self.invalidate_presentations();
        let performance_engine = self.performance_engine();
        match result {
            Ok(snapshot) => {
                let previous_identity = self.engines[index]
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.engine_id.as_ref());
                if previous_identity != snapshot.engine_id.as_ref() {
                    self.engines[index].metrics = Default::default();
                }
                self.engines[index].metrics.reconcile(&snapshot.containers);
                self.engines[index].snapshot = Some(snapshot);
                self.engines[index].error = None;
            }
            Err(error) => self.engines[index].error = Some(error),
        }
        // Same-engine contexts are shown as aliases, never as duplicate workload trees.
        for i in 0..self.engines.len() {
            let identity = self.engines[i]
                .snapshot
                .as_ref()
                .and_then(|s| s.engine_id.as_deref());
            self.engines[i].alias = self.engines[i].snapshot.as_ref().and_then(|_| {
                (0..i)
                    .find(|&other| {
                        self.engines[other].error.is_none()
                            && self.engines[other].snapshot.is_some()
                            && (identity.is_some_and(|id| {
                                self.engines[other]
                                    .snapshot
                                    .as_ref()
                                    .and_then(|s| s.engine_id.as_deref())
                                    == Some(id)
                            }) || ((self.engines[i]
                                .connection
                                .endpoint
                                .starts_with("unix://")
                                || self.engines[i].connection.endpoint.starts_with('/'))
                                && crate::discovery::endpoint_key(&self.engines[i].connection)
                                    == crate::discovery::endpoint_key(
                                        &self.engines[other].connection,
                                    )))
                    })
                    .map(|other| self.engines[other].connection.name.clone())
            });
        }
        if refreshed
            && self.project.as_ref().is_some_and(|project| {
                project.engine == index
                    && self.engines[index]
                        .snapshot
                        .as_ref()
                        .is_some_and(|snapshot| {
                            !snapshot
                                .containers
                                .iter()
                                .any(|container| project.contains(container))
                        })
            })
        {
            while self.back() {}
            self.project = None;
            self.detail_scroll = 0;
            self.notice = crate::i18n::t("Compose project has no containers; returned to all runtimes. Retained resources are still available.").into();
        }
        self.refresh_tree();
        self.restore_performance_focus(performance_engine);
        self.startup_pending.remove(&index);
        self.complete_startup();
    }

    fn complete_startup(&mut self) {
        if !self.startup_discovery_pending
            && self.startup_pending.is_empty()
            && let Some(cwd) = self.startup_cwd.take()
        {
            let projects = crate::workspace::directory_matches(&self.engines, &cwd);
            match projects.len() {
                0 => {
                    self.notice.clear();
                    self.restore_navigation();
                }
                1 => {
                    self.open_project(projects[0].clone());
                    self.notice = crate::i18n::t("Opened the Compose project matching this directory. Backspace returns to all runtimes.").into();
                }
                _ => {
                    self.notice.clear();
                    self.picker = Some(Picker {
                        title: crate::i18n::t(
                            "Multiple projects match this directory · Choose a connection",
                        )
                        .into(),
                        choices: projects.into_iter().map(Choice::Project).collect(),
                        selected: 0,
                    });
                }
            }
        }
    }

    /// Reconcile discovery without moving any existing backend index or navigation key.
    /// Returns worker availability for native Podman machines only.
    pub fn reconcile_podman(
        &mut self,
        discovery: &crate::discovery::podman::Discovery,
    ) -> Vec<(usize, bool)> {
        self.invalidate_presentations();
        let performance_engine = self.performance_engine();
        for machine in &discovery.machines {
            if !self.podman_machines.contains_key(&machine.name) {
                let index = self.engines.len();
                self.engines.push(Engine {
                    runtime: Some(crate::runtime::Target::Podman {
                        machine: machine.name.clone(),
                    }),
                    connection: Connection {
                        name: format!("Podman / {}", machine.name),
                        kind: crate::config::BackendKind::Docker,
                        endpoint: String::new(),
                    },
                    snapshot: None,
                    error: None,
                    alias: None,
                    metrics: Default::default(),
                    metrics_revision: 0,
                    events: Default::default(),
                });
                let pin_rank = |engine: usize| {
                    self.context_pins
                        .iter()
                        .position(|name| name == &self.engines[engine].connection.name)
                        .unwrap_or(usize::MAX)
                };
                let position = self
                    .context_order
                    .iter()
                    .position(|&engine| pin_rank(index) < pin_rank(engine))
                    .unwrap_or(self.context_order.len());
                self.context_order.insert(position, index);
                self.expanded.insert(format!("{index}/{:?}", Scope::Engine));
                self.podman_machines.insert(machine.name.clone(), index);
                if self.startup_cwd.is_some() {
                    self.startup_pending.insert(index);
                }
            }
        }
        let mut availability = Vec::new();
        // Clone the small identity map so update_engine can also recompute aliases.
        for (name, index) in self.podman_machines.clone() {
            let endpoint = discovery
                .machines
                .iter()
                .find(|machine| machine.name == name)
                .map(|machine| &machine.endpoint);
            let error = if let Some(warning) = &discovery.warning {
                Some(warning.clone())
            } else {
                match endpoint {
                    Some(Ok(endpoint)) => {
                        if self.engines[index].connection.endpoint != *endpoint {
                            let engine = &mut self.engines[index];
                            engine.connection.endpoint = endpoint.clone();
                            engine.metrics = Default::default();
                            engine.metrics_revision = engine.metrics_revision.wrapping_add(1);
                            engine.error =
                                Some(crate::i18n::t("Podman socket changed; refreshing inventory").into());
                        }
                        // A Docker context may be the primary row for this same socket.
                        // Give it the verified machine target too, without trusting its name.
                        let key = crate::discovery::endpoint_key(&self.engines[index].connection);
                        for engine in &mut self.engines {
                            if engine.runtime.is_none()
                                && crate::discovery::endpoint_key(&engine.connection) == key
                            {
                                engine.runtime = Some(crate::runtime::Target::Podman {
                                    machine: name.clone(),
                                });
                            }
                        }
                        availability.push((index, true));
                        None
                    }
                    Some(Err(error)) => Some(error.clone()),
                    None => Some(
                        crate::i18n::t("Podman machine is no longer listed; refresh after starting it separately")
                            .into(),
                    ),
                }
            };
            if let Some(error) = error {
                self.update_engine(index, Err(error));
                availability.push((index, false));
            }
        }
        self.refresh_tree();
        self.restore_performance_focus(performance_engine);
        self.startup_discovery_pending = false;
        self.complete_startup();
        availability
    }

    pub fn wait_for_machine_discovery(&mut self) {
        self.startup_discovery_pending = self.startup_cwd.is_some();
    }

    pub fn current(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn request_action(&mut self, action: crate::actions::Action) {
        if self.action_confirmation.is_some() {
            return;
        }
        let request = self.plan_action(action);
        match request {
            Ok(request) => {
                self.action_confirmation_row = self.action_row().map(|row| row.key);
                self.action_review_visible = false;
                self.action_confirmation = Some(crate::actions::Confirmation {
                    request,
                    typed: String::new(),
                    scroll: 0,
                    error: None,
                });
            }
            Err(error) => self.notice = crate::tf!("{error}", error = format!("{error:#}")),
        }
    }

    /// Performance actions use the displayed target, not the Explorer row that
    /// happened to be selected before focus moved into the dock.
    fn action_row(&self) -> Option<Row> {
        if self.performance_focus {
            return match self.performance_scope {
                PerformanceScope::Runtime => {
                    let index = self.performance_engine()?;
                    Some(
                        node(
                            index,
                            Scope::Engine,
                            self.engines[index].connection.name.clone(),
                            vec![],
                        )
                        .row,
                    )
                }
                PerformanceScope::Container => {
                    self.performance_container()?;
                    self.current().cloned()
                }
            };
        }
        self.current().cloned()
    }

    fn plan_action(
        &self,
        action: crate::actions::Action,
    ) -> anyhow::Result<crate::actions::Request> {
        if self.demo {
            anyhow::bail!(
                "{}",
                crate::i18n::t("Demo mode: lifecycle mutations are disabled")
            );
        }
        if self.compose_busy() {
            anyhow::bail!(
                "{}",
                crate::i18n::t("Wait for the running Compose operation to finish first")
            );
        }
        let row = self.action_row().ok_or_else(|| {
            anyhow::anyhow!(if self.performance_focus {
                crate::i18n::t("Select a target in Performance before requesting an action")
            } else {
                crate::i18n::t("Select a container or Compose workload")
            })
        })?;
        // Checked first: a machine row only borrows an engine index to satisfy `Row`.
        if matches!(row.scope, Scope::Machine(_)) {
            anyhow::bail!(
                "{}",
                crate::i18n::t("Machines have no actions; select a workload inside one")
            );
        }
        let engine = &self.engines[row.engine];
        if crate::remote::is_remote(&engine.connection)
            && action == crate::actions::Action::Delete
            && !self
                .machine(self.machine_name(row.engine))
                .is_some_and(|machine| machine.allow_delete)
        {
            anyhow::bail!(
                "{}",
                crate::i18n::t("Deleting is turned off on this server")
            );
        }
        if row.scope == Scope::Engine {
            if engine.alias.is_some() {
                anyhow::bail!(
                    "{}",
                    crate::i18n::t("Select the primary runtime, not an alias")
                );
            }
            let target = engine.runtime.clone().ok_or_else(|| anyhow::anyhow!("{}", crate::i18n::t("Runtime control unsupported for this endpoint; select a workload for container actions")))?;
            if action != crate::actions::Action::Start
                && (engine.error.is_some() || engine.snapshot.is_none())
            {
                anyhow::bail!(
                    "{}",
                    crate::i18n::t("Runtime is offline; use Start to bring it online")
                );
            }
            let request =
                crate::actions::Request::runtime(row.engine, &engine.connection, target, action)?;
            self.check_action_conflicts(&request)?;
            return Ok(request);
        }
        if engine.error.is_some() || engine.alias.is_some() {
            anyhow::bail!(crate::i18n::t(
                "Connection unavailable or aliased; select its connected primary context"
            ));
        }
        let mut request = crate::actions::Request::build(
            row.engine,
            &engine.connection,
            engine
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.engine_id.as_deref()),
            &row.scope,
            self.containers_for(&row),
            action,
        )?;
        request.owner = engine.runtime.clone();
        self.check_action_conflicts(&request)?;
        Ok(request)
    }

    fn check_action_conflicts(&self, request: &crate::actions::Request) -> anyhow::Result<()> {
        if self.action_jobs.values().any(|job| {
            let current_owners_match = self
                .engines
                .get(job.request.engine)
                .and_then(|engine| engine.runtime.as_ref())
                .zip(
                    self.engines
                        .get(request.engine)
                        .and_then(|engine| engine.runtime.as_ref()),
                )
                .is_some_and(|(a, b)| a == b);
            !job.finished && (job.request.conflicts(request) || current_owners_match)
        }) {
            anyhow::bail!(crate::i18n::t(
                "An action is already active on this runtime; other runtimes remain available. :action-results shows progress"
            ));
        }
        Ok(())
    }

    pub fn action_availability(
        &self,
        action: crate::actions::Action,
        row: &str,
    ) -> Result<usize, String> {
        if self.action_row().is_none_or(|current| current.key != row) {
            return Err(crate::i18n::t("Selection changed; reopen Actions").into());
        }
        self.plan_action(action)
            .map(|request| request.target_count())
            .map_err(|error| crate::tf!("{error}", error = format!("{error:#}")))
    }

    pub fn open_actions(&mut self) {
        let Some(row) = self.action_row() else {
            return;
        };
        let compose = matches!(
            row.scope,
            Scope::Project {
                namespace: None,
                ..
            } | Scope::Services {
                namespace: None,
                ..
            } | Scope::Service {
                namespace: None,
                ..
            }
        );
        self.picker = Some(Picker {
            title: crate::tf!("Actions · {a0}", a0 = row.title),
            choices: [
                crate::actions::Action::Start,
                crate::actions::Action::Stop,
                crate::actions::Action::Restart,
                crate::actions::Action::Delete,
            ]
            .into_iter()
            .map(|action| Choice::Action {
                action,
                row: row.key.clone(),
            })
            .chain(
                crate::compose::Action::ALL
                    .into_iter()
                    .filter(|_| compose)
                    .map(Choice::Compose),
            )
            .chain(
                [
                    crate::diagnostics::Kind::Health,
                    crate::diagnostics::Kind::Network,
                ]
                .into_iter()
                .filter(|_| !self.performance_focus && self.diagnostic_container().is_some())
                .map(Choice::Diagnose),
            )
            .collect(),
            selected: 0,
        });
    }

    /// A row that means one workload. A runtime that merely has one container does not count.
    fn diagnostic_container(&self) -> Option<Container> {
        let row = self.current()?;
        if matches!(row.scope, Scope::Engine | Scope::Namespace(_)) {
            return None;
        }
        let mut containers = self.selected_containers();
        (containers.len() == 1).then(|| containers.remove(0))
    }

    fn diagnostic_side(&self, engine: usize, container: &Container) -> crate::diagnostics::Side {
        let state = &self.engines[engine];
        crate::diagnostics::Side {
            target: crate::diagnostics::Target::new(
                engine,
                &state.connection,
                state
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.engine_id.clone()),
                container,
            ),
            container: container.clone(),
            status: crate::diagnostics::Status::Loading,
            evidence: None,
            generation: 0,
        }
    }

    /// Opens read-only evidence for the one selected container. Sends only an inspect.
    pub fn open_diagnostics(&mut self, kind: crate::diagnostics::Kind) {
        let Some(engine) = self.current().map(|row| row.engine) else {
            self.notice = crate::i18n::t("Select a container").into();
            return;
        };
        if let Some(alias) = &self.engines[engine].alias {
            self.notice = crate::tf!(
                "Browse {a0} for this engine's resources.",
                a0 = crate::model::safe_text(alias)
            );
            return;
        }
        let Some(container) = self.diagnostic_container() else {
            self.notice =
                crate::i18n::t("Select a single container; diagnostics do not cover groups").into();
            return;
        };
        self.diagnostics = Some(crate::diagnostics::View {
            kind,
            main: self.diagnostic_side(engine, &container),
            peer: None,
            scroll: 0,
            message: None,
            loaded_logs: None,
        });
        self.load_diagnostics(crate::diagnostics::Role::Main);
    }

    /// Starts one load, invalidating whatever the same side had in flight.
    fn load_diagnostics(&mut self, role: crate::diagnostics::Role) {
        use crate::diagnostics::Status;
        self.diagnostics_generation = self.diagnostics_generation.wrapping_add(1);
        let generation = self.diagnostics_generation;
        let demo = self.demo;
        let side = if role == crate::diagnostics::Role::Overview {
            self.overview_details.as_mut()
        } else {
            self.diagnostics
                .as_mut()
                .and_then(|view| view.side_mut(role))
        };
        let Some(side) = side else {
            return;
        };
        side.generation = generation;
        let engine = &self.engines[side.target.engine];
        if engine.connection.kind != crate::config::BackendKind::Docker {
            side.status = Status::Unsupported;
        } else if engine.error.is_some() {
            side.status = Status::Failed(
                crate::i18n::t("Connection unavailable; detailed inspection was not attempted")
                    .into(),
            );
        } else if demo {
            side.evidence = Some((
                crate::demo::inspection(&side.container),
                crate::history::now_ms(),
            ));
            side.status = Status::Ready;
        } else {
            side.status = Status::Loading;
            let request = crate::diagnostics::Request {
                role,
                generation,
                target: side.target.clone(),
                connection: engine.connection.clone(),
            };
            self.diagnostics_requests.push(request);
        }
    }

    pub fn diagnostics_freshness(
        &self,
        target: &crate::diagnostics::Target,
    ) -> crate::diagnostics::Freshness {
        use crate::diagnostics::Freshness;
        let engine = &self.engines[target.engine];
        let Some(snapshot) = engine.snapshot.as_ref().filter(|_| engine.error.is_none()) else {
            return Freshness::EngineOffline;
        };
        if snapshot.engine_id != target.runtime || engine.connection.endpoint != target.endpoint {
            return Freshness::RuntimeChanged;
        }
        if snapshot
            .containers
            .iter()
            .any(|container| container.key() == target.container_key())
        {
            Freshness::Current
        } else {
            Freshness::Removed
        }
    }

    /// Reloads both sides. A replaced target is reported, never followed by its name.
    pub fn refresh_diagnostics(&mut self) {
        use crate::diagnostics::{Freshness, Role};
        let Some(view) = &self.diagnostics else {
            return;
        };
        let main = view.main.target.clone();
        let peer = view.peer.as_ref().map(|peer| peer.target.clone());
        if matches!(
            self.diagnostics_freshness(&main),
            Freshness::Removed | Freshness::RuntimeChanged
        ) {
            if let Some(view) = &mut self.diagnostics {
                view.message = Some(
                    crate::i18n::t(
                        "This container no longer exists here; close and select the current one",
                    )
                    .into(),
                );
            }
            return;
        }
        if let Some(view) = &mut self.diagnostics {
            view.message = None;
        }
        self.load_diagnostics(Role::Main);
        if let Some(peer) = peer
            && matches!(
                self.diagnostics_freshness(&peer),
                Freshness::Current | Freshness::EngineOffline
            )
        {
            self.load_diagnostics(Role::Peer);
        }
    }

    /// Reads a bounded range of this exact container's logs into the open view, on request only.
    /// They do not touch the Logs tab or any filter, and nothing is saved.
    pub fn load_diagnostic_logs(&mut self) {
        use crate::diagnostics::{Freshness, LoadedLogs, LogLoad, LogRequest};
        let Some(view) = &self.diagnostics else {
            return;
        };
        let target = view.main.target.clone();
        let container = view.main.container.clone();
        let failure = view
            .main
            .evidence
            .as_ref()
            .and_then(|(inspection, _)| crate::diagnostics::failure_time(inspection));
        let blocked = match self.diagnostics_freshness(&target) {
            Freshness::Current => None,
            Freshness::EngineOffline => Some(crate::i18n::t(
                "Connection unavailable; logs cannot be loaded right now",
            )),
            Freshness::Removed | Freshness::RuntimeChanged => Some(crate::i18n::t(
                "This container no longer exists here; close and select the current one",
            )),
        };
        if let Some(reason) = blocked {
            if let Some(view) = &mut self.diagnostics {
                view.message = Some(reason.into());
            }
            return;
        }
        self.diagnostics_generation = self.diagnostics_generation.wrapping_add(1);
        let generation = self.diagnostics_generation;
        let connection = self.engines[target.engine].connection.clone();
        let status = if self.demo {
            LogLoad::Ready(crate::demo::diagnostic_logs(&container, failure))
        } else {
            let (since, until) = crate::diagnostics::log_window(failure);
            self.diagnostics_log_request = Some(LogRequest {
                generation,
                target: target.clone(),
                connection,
                container,
                since,
                until,
            });
            LogLoad::Loading
        };
        if let Some(view) = &mut self.diagnostics {
            view.message = None;
            view.loaded_logs = Some(LoadedLogs {
                target,
                generation,
                status,
            });
        }
    }

    pub fn diagnostics_close(&mut self) {
        self.diagnostics = None;
        self.diagnostics_log_request = None;
        self.diagnostics_requests
            .retain(|request| request.role == crate::diagnostics::Role::Overview);
    }

    /// Keeps details current for the selected container only, and only while Overview shows it.
    /// No other container is ever inspected for this, however many there are.
    pub fn update_overview_details(&mut self) {
        use crate::diagnostics::{OVERVIEW_REFRESH, Role};
        let wanted = (self.tab == Tab::Overview && self.overview_mode == OverviewMode::Containers)
            .then(|| self.diagnostic_container())
            .flatten()
            .zip(self.current().map(|row| row.engine))
            .filter(|(_, engine)| self.engines[*engine].alias.is_none());
        let Some((container, engine)) = wanted else {
            self.overview_details = None;
            self.overview_details_at = None;
            return;
        };
        let side = self.diagnostic_side(engine, &container);
        let same = self
            .overview_details
            .as_ref()
            .is_some_and(|known| known.target == side.target);
        if same
            && self
                .overview_details_at
                .is_some_and(|at| at.elapsed() < OVERVIEW_REFRESH)
        {
            return;
        }
        if !same {
            self.overview_details = Some(side);
        }
        self.overview_details_at = Some(std::time::Instant::now());
        self.load_diagnostics(Role::Overview);
    }

    /// Links only ever open views for the same immutable container.
    pub fn diagnostics_link_ready(&self) -> Result<(), String> {
        let Some(view) = &self.diagnostics else {
            return Err(crate::i18n::t("Select a container").into());
        };
        let selected = self.selected_containers();
        if self.diagnostics_freshness(&view.main.target) == crate::diagnostics::Freshness::Removed
            || self.diagnostics_freshness(&view.main.target)
                == crate::diagnostics::Freshness::RuntimeChanged
            || !matches!(selected.as_slice(), [container] if container.key() == view.main.target.container_key())
        {
            return Err(crate::i18n::t(
                "This container is no longer the selection; close and review the current one",
            )
            .into());
        }
        Ok(())
    }

    /// Containers this view may compare with: same runtime, and inside the active workspace.
    pub fn diagnostic_peers(&self) -> Vec<Container> {
        let Some(view) = &self.diagnostics else {
            return vec![];
        };
        let target = &view.main.target;
        self.engines[target.engine]
            .snapshot
            .iter()
            .flat_map(|snapshot| &snapshot.containers)
            .filter(|container| container.key() != target.container_key())
            .filter(|container| {
                self.project
                    .as_ref()
                    .is_none_or(|project| project.contains(container))
            })
            .cloned()
            .collect()
    }

    pub fn open_diagnostic_peers(&mut self) {
        let peers = self.diagnostic_peers();
        let Some(view) = &mut self.diagnostics else {
            return;
        };
        if self.engines[view.main.target.engine].connection.kind
            != crate::config::BackendKind::Docker
        {
            view.message = Some(
                crate::i18n::t(
                    "Peer comparison needs detailed inspection, which this backend does not have",
                )
                .into(),
            );
            return;
        }
        if peers.is_empty() {
            view.message =
                Some(crate::i18n::t("No other container is visible on this runtime").into());
            return;
        }
        let selected = view
            .peer
            .as_ref()
            .and_then(|peer| {
                peers
                    .iter()
                    .position(|container| container.key() == peer.target.container_key())
            })
            .map_or(0, |index| index + 1);
        self.picker = Some(Picker {
            title: crate::i18n::t("Compare with a container on this runtime").into(),
            choices: std::iter::once(Choice::DiagnosticPeerNone)
                .chain(
                    peers
                        .into_iter()
                        .map(|peer| Choice::DiagnosticPeer(Box::new(peer))),
                )
                .collect(),
            selected,
        });
    }

    /// Only true when the log buffer was subscribed for exactly this one container.
    pub fn logs_cover(&self, target: &crate::diagnostics::Target) -> bool {
        self.log_scope_targets
            .as_ref()
            .is_some_and(|(engine, keys)| {
                *engine == target.engine && keys.as_slice() == [target.container_key()]
            })
    }

    pub fn compose_busy(&self) -> bool {
        self.compose_job
            .as_ref()
            .is_some_and(crate::compose::Job::running)
    }

    fn plan_compose(
        &self,
        action: crate::compose::Action,
    ) -> anyhow::Result<crate::compose::Request> {
        if self.demo {
            anyhow::bail!(
                "{}",
                crate::i18n::t("Demo mode: Compose operations are disabled")
            );
        }
        if let Some(row) = self.current()
            && let Some((server, _)) =
                crate::remote::parse_endpoint(&self.engines[row.engine].connection.endpoint)
        {
            match self
                .machine(server)
                .and_then(|machine| machine.capabilities.as_ref())
            {
                Some(reported) if reported.allow_compose => {}
                Some(_) => anyhow::bail!(
                    "{}",
                    crate::i18n::t(
                        "Compose actions are turned off on this server; rerun setup with --allow-compose"
                    )
                ),
                None => anyhow::bail!(
                    "{}",
                    crate::i18n::t(
                        "This server's agent does not support Compose actions; update tha-agent"
                    )
                ),
            }
        }
        if self.compose_busy() {
            anyhow::bail!(
                "{}",
                crate::i18n::t(
                    "Another Compose operation is still running; :compose-output shows it"
                )
            );
        }
        if self.has_active_actions() {
            anyhow::bail!(
                "{}",
                crate::i18n::t("Wait for running lifecycle actions to finish first")
            );
        }
        let row = self.action_row().ok_or_else(|| {
            anyhow::anyhow!("{}", crate::i18n::t("Select a Compose project or service"))
        })?;
        let engine = &self.engines[row.engine];
        if engine.error.is_some() || engine.alias.is_some() {
            anyhow::bail!(
                "{}",
                crate::i18n::t("Select a connected runtime, not an offline engine or alias")
            );
        }
        let project = match &row.scope {
            Scope::Project { project, .. }
            | Scope::Services { project, .. }
            | Scope::Service { project, .. } => project.clone(),
            _ => anyhow::bail!("{}", crate::i18n::t("Select a Compose project or service")),
        };
        // A selected service still needs its project's labels, so resolve against them all.
        let members: Vec<_> = engine
            .snapshot
            .iter()
            .flat_map(|snapshot| &snapshot.containers)
            .filter(|container| container.project.as_deref() == Some(project.as_str()))
            .cloned()
            .collect();
        crate::compose::Request::build(&engine.connection, &row.scope, &members, action)
    }

    pub fn request_compose(&mut self, action: crate::compose::Action) {
        match self.plan_compose(action) {
            Ok(request) => {
                self.compose_confirmation = Some(crate::compose::Confirmation {
                    request,
                    typed: String::new(),
                    error: None,
                });
            }
            Err(error) => self.notice = crate::tf!("{error}", error = format!("{error:#}")),
        }
    }

    pub fn confirm_compose(&mut self) {
        let Some(mut confirmation) = self.compose_confirmation.take() else {
            return;
        };
        let request = confirmation.request.clone();
        if request.action.requires_typing() && confirmation.typed != request.target.project {
            confirmation.error =
                Some(crate::i18n::t("Type the exact project name to confirm").into());
            self.compose_confirmation = Some(confirmation);
            return;
        }
        // The dialog may have stayed open while something else started.
        if self.compose_busy() || self.has_active_actions() {
            self.notice = crate::i18n::t("Another operation started; nothing was sent").into();
            return;
        }
        self.compose_job = Some(crate::compose::Job::new(format!(
            "{} · {}",
            request.action.label(),
            request.target.name()
        )));
        self.compose_output_visible = true;
        self.compose_review_until = None;
        self.compose_before = request.action.changes_images().then(|| {
            (
                self.current().map_or(0, |row| row.engine),
                request.target.project.clone(),
                request.before.clone(),
            )
        });
        self.compose_request = Some(request);
    }

    /// Events for the selected row. Project and service rows also match by label, so a
    /// container that was recreated or removed stays in its project's timeline.
    pub fn visible_events(&self) -> Vec<&crate::events::Event> {
        let Some(row) = self.current() else {
            return Vec::new();
        };
        let keys: HashSet<String> = self
            .containers_for(row)
            .iter()
            .map(Container::key)
            .collect();
        self.engines[row.engine]
            .events
            .newest_first()
            .filter(|event| match &row.scope {
                Scope::Engine => true,
                Scope::Project { project, .. } | Scope::Services { project, .. } => {
                    event.project.as_ref() == Some(project)
                }
                Scope::Service {
                    project, service, ..
                } => {
                    event.project.as_ref() == Some(project)
                        && event.service.as_ref() == Some(service)
                }
                Scope::Standalone(_) => event.project.is_none(),
                _ => keys.contains(&event.key),
            })
            .collect()
    }

    pub fn layers_target(&self) -> Option<(crate::layers::Key, Connection, Container)> {
        if self.tab != Tab::Layers {
            return None;
        }
        self.available_layers_target()
    }

    /// Detail tabs shown for the current selection, in tab-bar order.
    pub fn available_tabs(&self) -> Vec<Tab> {
        let mut tabs = vec![Tab::Overview, Tab::Logs, Tab::Networks];
        if self.exec_tab_available() {
            tabs.push(Tab::Exec);
        }
        tabs.push(Tab::Events);
        if self.performance_container().is_some() {
            tabs.push(Tab::History);
        }
        if self.layers_available() {
            tabs.push(Tab::Layers);
        }
        if self.security_tab_available() {
            tabs.push(Tab::Security);
        }
        tabs
    }

    /// An open session stays reachable while browsing, whatever is selected.
    pub fn exec_tab_available(&self) -> bool {
        self.sessions.active().is_some() || self.exec_selection_eligible()
    }

    fn exec_selection_eligible(&self) -> bool {
        let Some(row) = self.current() else {
            return false;
        };
        if let Scope::Machine(name) = &row.scope {
            return self
                .machines
                .iter()
                .any(|machine| machine.remote.name == *name && machine.connected());
        }
        let engine = &self.engines[row.engine];
        matches!(row.scope, Scope::Container(_) | Scope::Service { .. })
            && engine.error.is_none()
            && engine.alias.is_none()
            && self
                .selected_containers()
                .into_iter()
                .any(|c| c.running() && !c.managed)
    }

    pub fn layers_available(&self) -> bool {
        self.available_layers_target().is_some()
    }

    /// The saved report for the selected container's image, if there is exactly one image.
    fn container_security_target(&self) -> Option<crate::security::Target> {
        let (_, connection, container) = self.available_layers_target()?;
        let runtime = self
            .current()
            .and_then(|row| self.engines[row.engine].snapshot.as_ref())?
            .engine_id
            .clone()?;
        Some(crate::security::Target {
            connection,
            runtime,
            image: container.image_id?,
            label: container.image,
        })
    }

    pub fn security_tab_available(&self) -> bool {
        self.security.enabled && self.layers_available()
    }

    pub fn open_security(&mut self) {
        if !self.security.enabled {
            self.notice =
                crate::i18n::t("Install the security module first: tha module install security")
                    .into();
            return;
        }
        // Containers shows the report in place. Scanning belongs to the Security view.
        if self.overview_mode == OverviewMode::Containers {
            if !self.security_tab_available() {
                self.notice =
                    crate::i18n::t("Select a container or single-container service").into();
                return;
            }
            self.tab = Tab::Security;
            self.detail_focus = true;
            self.performance_focus = false;
            self.detail_scroll = 0;
            self.reconcile_detail_tab();
            return;
        }
        if self.security.busy {
            self.overview_mode = OverviewMode::Security;
            self.detail_focus = true;
            self.notice = crate::i18n::t("Showing the service's active queue; x cancels it").into();
            return;
        }
        let checked = if matches!(
            self.overview_mode,
            OverviewMode::Images | OverviewMode::Security
        ) {
            crate::ui::inventory::security_targets(self, true)
        } else {
            Vec::new()
        };
        let target = crate::ui::inventory::security_targets(self, false)
            .into_iter()
            .next();
        let Some(target) = target else {
            self.overview_mode = OverviewMode::Security;
            self.detail_focus = false;
            self.notice = crate::i18n::t("Select an image to inspect its security report").into();
            return;
        };
        if let Some(engine) = self
            .engines
            .iter()
            .position(|engine| engine.connection == target.connection)
        {
            self.inventory_engine = engine;
            let view = self
                .inventory_views
                .entry((engine, OverviewMode::Images))
                .or_default();
            view.selected = Some(target.image.clone());
        }
        self.security.select(Some(target));
        self.security.queue = checked;
        self.overview_mode = OverviewMode::Security;
        self.tab = Tab::Security;
        self.detail_focus = true;
        self.performance_focus = false;
        self.detail_scroll = 0;
    }

    pub fn request_security_scan(&mut self, checked: bool) {
        if self.overview_mode == OverviewMode::Containers {
            self.notice =
                crate::i18n::t("Scans run from the Security view (Ctrl+→ or Alt+4)").into();
            return;
        }
        let detail_focus = self.detail_focus;
        // Either shortcut honors checked images, even when filters hide them.
        let checked = checked
            || self
                .inventory_views
                .get(&(self.inventory_engine, OverviewMode::Images))
                .is_some_and(|view| !view.scan_marked.is_empty());
        if self.demo {
            self.notice =
                crate::i18n::t("Demo mode: security scans and downloads are disabled").into();
        } else if !self.security.enabled {
            self.notice = crate::i18n::t(
                "Install first: tha module install security (or installer MODULES=security)",
            )
            .into();
        } else if self.security.busy {
            self.notice = crate::i18n::t("A security queue is already running").into();
        } else {
            if self.overview_mode == OverviewMode::Images
                || (self.overview_mode == OverviewMode::Security && (!self.detail_focus || checked))
            {
                let targets = crate::ui::inventory::security_targets(self, checked);
                if targets.is_empty() {
                    self.notice =
                        crate::i18n::t("No checked images remain to scan; select images with m")
                            .into();
                    return;
                }
                let selected = crate::ui::inventory::security_targets(self, false)
                    .into_iter()
                    .next()
                    .or_else(|| targets.first().cloned());
                self.security.select(selected);
                self.security.queue = targets;
                self.overview_mode = OverviewMode::Security;
                self.tab = Tab::Security;
            } else if self.tab != Tab::Security {
                self.open_security();
            }
            if self.security.target.is_some() && self.tab == Tab::Security {
                self.detail_focus = detail_focus;
                if !checked {
                    self.security.queue.clear();
                }
                self.security.request = true;
                self.security.cancel = false;
            }
        }
    }

    pub fn reconcile_detail_tab(&mut self) {
        if self.tab == Tab::Security && self.overview_mode == OverviewMode::Containers {
            if self.security_tab_available() {
                let target = self.container_security_target();
                self.security.select(target);
            } else {
                self.tab = Tab::Overview;
                self.security.expanded = false;
                self.security.zoom = false;
            }
        }
        if self.tab == Tab::Exec && !self.exec_tab_available() {
            self.tab = Tab::Overview;
            self.exec_input = false;
            self.detail_scroll = 0;
        }
        if self.tab == Tab::History && self.performance_container().is_none() {
            self.tab = Tab::Overview;
            self.history_view.expanded = false;
        }
        if self.tab == Tab::Layers && !self.layers_available() {
            self.tab = Tab::Overview;
            self.detail_scroll = 0;
            self.layers.select(None);
        }
    }

    fn available_layers_target(&self) -> Option<(crate::layers::Key, Connection, Container)> {
        let row = self.current()?;
        if !matches!(row.scope, Scope::Container(_) | Scope::Service { .. }) {
            return None;
        }
        let engine = &self.engines[row.engine];
        if engine.error.is_some() || engine.alias.is_some() {
            return None;
        }
        let containers = self.selected_containers();
        if containers.len() != 1 {
            return None;
        }
        let container = containers.into_iter().next()?;
        let key = crate::layers::Key {
            image: crate::platform::Key::new(
                row.engine,
                engine.snapshot.as_ref()?.engine_id.clone(),
                &container,
            ),
            endpoint: engine.connection.key(),
        };
        Some((key, engine.connection.clone(), container))
    }

    pub fn confirm_action(&mut self) {
        if !self.action_review_visible {
            if let Some(confirmation) = &mut self.action_confirmation {
                confirmation.error = Some(
                    crate::i18n::t(
                        "Resize the terminal to review this confirmation before confirming",
                    )
                    .into(),
                );
            }
            return;
        }
        let Some(mut confirmation) = self.action_confirmation.take() else {
            return;
        };
        if confirmation.request.action == crate::actions::Action::Delete
            && confirmation.typed != confirmation.request.name
        {
            confirmation.error =
                Some(crate::i18n::t("Name does not match; nothing was sent").into());
            self.action_confirmation = Some(confirmation);
            return;
        }
        let mut request = confirmation.request;
        let validation = (|| {
            self.check_action_conflicts(&request)?;
            let engine = self
                .engines
                .get(request.engine)
                .ok_or_else(|| anyhow::anyhow!("{}", crate::i18n::t("Runtime no longer exists")))?;
            if self.demo || engine.alias.is_some() || engine.connection != request.connection {
                anyhow::bail!(
                    "{}",
                    crate::i18n::t(
                        "Connection changed or is unavailable; select and confirm again"
                    )
                );
            }
            if let Some(target) = &request.runtime {
                if engine.runtime.as_ref() != Some(target) {
                    anyhow::bail!(
                        "{}",
                        crate::i18n::t("Runtime provider changed; select and confirm again")
                    );
                }
                if request.action != crate::actions::Action::Start
                    && (engine.error.is_some() || engine.snapshot.is_none())
                {
                    anyhow::bail!(
                        "{}",
                        crate::i18n::t("Runtime is offline; use Start instead")
                    );
                }
                return Ok(());
            }
            if engine.error.is_some() {
                anyhow::bail!(
                    "{}",
                    crate::i18n::t(
                        "Connection changed or is unavailable; select and confirm again"
                    )
                );
            }
            let snapshot = engine.snapshot.as_ref().ok_or_else(|| {
                anyhow::anyhow!("{}", crate::i18n::t("Refresh inventory before retrying"))
            })?;
            if snapshot.engine_id.as_deref() != Some(request.identity.as_str()) {
                anyhow::bail!(
                    "{}",
                    crate::i18n::t("Runtime identity changed; select and confirm again")
                );
            }
            for container in &request.containers {
                let current = snapshot
                    .containers
                    .iter()
                    .find(|current| current.id == container.id)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "{}",
                            crate::i18n::t("Container disappeared; select and confirm again")
                        )
                    })?;
                request.validate_container(container, current)?;
            }
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = validation {
            self.notice = crate::tf!("{error}", error = format!("{error:#}"));
            return;
        }
        let Some(operation) = self.next_action_id.checked_add(1) else {
            self.notice =
                crate::i18n::t("Operation IDs exhausted; restart Tha before another action").into();
            return;
        };
        self.next_action_id = operation;
        request.operation = operation;
        // Keep the last confirmed result for each runtime; never evict active work.
        self.action_jobs
            .retain(|_, job| !job.finished || !job.request.conflicts(&request));
        self.action_feedback
            .retain(|id, _| self.action_jobs.contains_key(id));
        self.action_feedback.insert(
            operation,
            ActionFeedback {
                started_at: std::time::Instant::now(),
                finished_at: None,
                row_key: self.action_confirmation_row.take(),
                result_at: vec![None; request.target_count()],
            },
        );
        self.action_selected = Some(operation);
        self.action_jobs.insert(
            operation,
            crate::actions::Progress {
                results: vec![None; request.target_count()],
                request: request.clone(),
                finished: false,
                scroll: 0,
            },
        );
        self.action_results_visible = false;
        self.notice.clear();
        self.action_request = Some(request);
    }

    pub fn action_status(&self) -> Option<String> {
        let now = std::time::Instant::now();
        let active: Vec<_> = self
            .action_jobs
            .values()
            .filter(|job| !job.finished)
            .collect();
        let (failed, unknown) = self
            .action_jobs
            .values()
            .flat_map(|job| job.results.iter().flatten())
            .fold((0, 0), |(failed, unknown), status| {
                (
                    failed + usize::from(matches!(status, crate::actions::Status::Failed(_))),
                    unknown + usize::from(matches!(status, crate::actions::Status::Unknown(_))),
                )
            });
        let issues = if failed + unknown > 0 {
            crate::tf!(
                " · {failed} failed, {unknown} unknown",
                failed = failed,
                unknown = unknown
            )
        } else {
            String::new()
        };
        if self.action_jobs.len() > 1 && !active.is_empty() {
            let summary = active
                .iter()
                .take(2)
                .map(|job| {
                    format!(
                        "{}: {} {}",
                        job.request.connection.name,
                        self.active_action_label(job, now),
                        job.summary()
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ");
            return Some(crate::tf!(
                "{a0} running{issues} · {summary} · :action-results",
                a0 = active.len(),
                issues = issues,
                summary = summary
            ));
        }
        if self.action_jobs.len() > 1 {
            return Some(crate::tf!(
                "{a0} runtimes finished{issues} · :action-results",
                a0 = self.action_jobs.len(),
                issues = issues
            ));
        }
        let progress = self.action_progress()?;
        if !progress.finished {
            return Some(crate::tf!(
                "{a0} · {a1} · :action-results",
                a0 = self.active_action_label(progress, now),
                a1 = progress.summary()
            ));
        }
        Some(crate::tf!(
            "{a0} · {a1} · :action-results",
            a0 = progress.request.action.label(),
            a1 = progress.summary()
        ))
    }

    pub fn has_active_actions(&self) -> bool {
        self.action_jobs.values().any(|job| !job.finished)
    }

    fn active_action_label(
        &self,
        job: &crate::actions::Progress,
        now: std::time::Instant,
    ) -> String {
        self.action_feedback
            .get(&job.request.operation)
            .map(|feedback| {
                let active = feedback.active(job.request.action, now);
                format!("{} {}", active.symbol, active.label)
            })
            .unwrap_or_else(|| job.request.action.label().into())
    }

    pub fn row_action_feedback(
        &self,
        row: &Row,
        now: std::time::Instant,
    ) -> Option<RowActionFeedback> {
        use crate::actions::Status;
        for (&id, job) in self.action_jobs.iter().rev() {
            let Some(feedback) = self.action_feedback.get(&id) else {
                continue;
            };
            let same_runtime = job.request.engine == row.engine;
            let container_index = if same_runtime && job.request.runtime.is_none() {
                if let Scope::Container(key) = &row.scope {
                    job.request
                        .containers
                        .iter()
                        .position(|container| container.key() == *key)
                } else {
                    None
                }
            } else {
                None
            };
            let selected_row =
                same_runtime && feedback.row_key.as_deref() == Some(row.key.as_str());
            let runtime_row =
                same_runtime && job.request.runtime.is_some() && row.scope == Scope::Engine;
            if !selected_row && !runtime_row && container_index.is_none() {
                continue;
            }
            let (result, completed_at) = if let Some(index) = container_index {
                (job.results[index].as_ref(), feedback.result_at[index])
            } else if job.finished {
                let result = job
                    .results
                    .iter()
                    .flatten()
                    .find(|status| matches!(status, Status::Unknown(_)))
                    .or_else(|| {
                        job.results
                            .iter()
                            .flatten()
                            .find(|status| matches!(status, Status::Failed(_)))
                    })
                    .or_else(|| {
                        job.results
                            .iter()
                            .flatten()
                            .find(|status| matches!(status, Status::Skipped(_)))
                    })
                    .or_else(|| job.results.iter().flatten().next());
                (result, feedback.finished_at)
            } else {
                (None, None)
            };
            let Some(result) = result else {
                return (!job.finished).then(|| feedback.active(job.request.action, now));
            };
            let (symbol, label, tone) = match result {
                Status::Succeeded => {
                    if completed_at.is_none_or(|at| {
                        now.saturating_duration_since(at) >= std::time::Duration::from_secs(5)
                    }) {
                        return None;
                    }
                    let label = if job.request.runtime.is_some() {
                        crate::i18n::t("command completed")
                    } else {
                        "completed"
                    };
                    (
                        "✓",
                        format!("{} {label}", job.request.action.label()),
                        Tone::Good,
                    )
                }
                Status::Failed(_) => (
                    "×",
                    crate::tf!("{action} failed", action = job.request.action.label()),
                    Tone::Error,
                ),
                Status::Unknown(_) => {
                    ("?", crate::i18n::t("Outcome unknown").into(), Tone::Warning)
                }
                Status::Skipped(_) => ("–", crate::i18n::t("Not attempted").into(), Tone::Warning),
            };
            return Some(RowActionFeedback {
                symbol,
                label,
                tone,
                active: false,
            });
        }
        None
    }

    pub fn action_progress(&self) -> Option<&crate::actions::Progress> {
        self.action_jobs.get(&self.action_selected?)
    }

    pub fn action_progress_mut(&mut self) -> Option<&mut crate::actions::Progress> {
        self.action_jobs.get_mut(&self.action_selected?)
    }

    pub fn apply_action_update(&mut self, update: crate::actions::Update) -> bool {
        let Some(progress) = self.action_jobs.get_mut(&update.operation) else {
            return false;
        };
        if progress.finished {
            return false;
        }
        match update.event {
            crate::actions::Event::Result { index, status } => {
                let Some(result) = progress.results.get_mut(index) else {
                    return false;
                };
                *result = Some(status);
                if let Some(feedback) = self.action_feedback.get_mut(&update.operation) {
                    feedback.result_at[index] = Some(std::time::Instant::now());
                }
            }
            crate::actions::Event::Finished => {
                progress.finished = true;
                if let Some(feedback) = self.action_feedback.get_mut(&update.operation) {
                    feedback.finished_at = Some(std::time::Instant::now());
                }
            }
        }
        true
    }

    pub fn open_action_results(&mut self) {
        match self.action_jobs.len() {
            0 => self.notice = crate::i18n::t("No lifecycle action results yet").into(),
            1 => {
                self.action_selected = self.action_jobs.keys().next().copied();
                self.action_results_visible = true;
            }
            _ => {
                self.picker = Some(Picker {
                    title: crate::i18n::t("Action results · Choose a runtime").into(),
                    choices: self
                        .action_jobs
                        .keys()
                        .rev()
                        .copied()
                        .map(Choice::ActionResult)
                        .collect(),
                    selected: 0,
                })
            }
        }
    }

    pub fn apply_metrics(&mut self, event: crate::metrics::Event) {
        let engine = &mut self.engines[event.engine];
        if engine.metrics_revision != event.revision
            || engine.error.is_some()
            || engine.alias.is_some()
        {
            return;
        }
        if let Some(snapshot) = &engine.snapshot {
            // Only local engines use the daemon worker. Server readings are collected
            // by this UI and must remain available to the chart's session overlay.
            if self.metrics_database.is_some() && !crate::remote::is_remote(&engine.connection) {
                engine
                    .metrics
                    .apply_remote(&snapshot.containers, event.batch);
            } else {
                engine.metrics.apply(&snapshot.containers, event.batch);
            }
        }
        self.invalidate_presentations();
    }

    pub fn metrics_for(&self, row: &Row) -> crate::metrics::Summary {
        self.metrics_for_at(row, std::time::Instant::now())
    }

    fn metrics_for_at(&self, row: &Row, now: std::time::Instant) -> crate::metrics::Summary {
        if let Scope::Machine(name) = &row.scope {
            return self.machine_metrics(name);
        }
        let engine = &self.engines[row.engine];
        let Some(snapshot) = &engine.snapshot else {
            return Default::default();
        };
        let available = engine.error.is_none() && engine.alias.is_none();
        let mut summary = engine.metrics.summary(
            snapshot.containers.iter().filter(|c| {
                matches_container(&row.scope, c)
                    && self
                        .project
                        .as_ref()
                        .is_none_or(|project| project.contains(c))
            }),
            now,
            self.metrics_stale_after(),
            available,
        );
        // An observed empty workload has a known zero sum. This is distinct
        // from missing measurements for running containers or a stopped leaf.
        if available && summary.running == 0 && !matches!(row.scope, Scope::Container(_)) {
            summary.cpu_percent = Some(0.0);
            summary.memory_bytes = Some(0);
        }
        summary
    }

    /// Keep the last successful sample visible through normal polling gaps. All
    /// views and their redraw deadlines share this policy, independent of cadence.
    pub fn metrics_stale_after(&self) -> std::time::Duration {
        if self.demo {
            return std::time::Duration::MAX;
        }
        std::time::Duration::from_secs(30)
    }

    /// Totals across the machine's engines. Unmeasured engines add nothing, rather than zero.
    fn machine_metrics(&self, name: &str) -> crate::metrics::Summary {
        let mut total = crate::metrics::Summary::default();
        for index in self.machine_engines(name) {
            if self.engines[index].alias.is_some() {
                continue;
            }
            let summary = self.engine_metrics(index);
            total.cpu_percent = match (total.cpu_percent, summary.cpu_percent) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            };
            total.memory_bytes = match (total.memory_bytes, summary.memory_bytes) {
                (Some(a), Some(b)) => Some(a.saturating_add(b)),
                (a, b) => a.or(b),
            };
            total.running += summary.running;
            total.stopped += summary.stopped;
            total.cpu_samples += summary.cpu_samples;
            total.memory_samples += summary.memory_samples;
            total.warming += summary.warming;
            total.stale += summary.stale;
        }
        total
    }

    fn invalidate_presentations(&mut self) {
        self.presentations.rows.clear();
        self.presentations.context = None;
        self.presentations.expires_at = None;
    }

    fn next_metrics_expiry(&self, now: std::time::Instant) -> Option<std::time::Instant> {
        let stale_after = self.metrics_stale_after();
        self.engines
            .iter()
            .flat_map(|engine| engine.metrics.samples.values())
            .filter_map(|sample| {
                // Summary considers the boundary itself fresh; redraw just after it.
                sample
                    .sampled_at
                    .checked_add(stale_after)?
                    .checked_add(std::time::Duration::from_nanos(1))
            })
            .filter(|&at| at > now)
            .min()
    }

    pub fn prepare_row_presentations(
        &mut self,
        start: usize,
        count: usize,
        now: std::time::Instant,
        include_metrics: bool,
    ) {
        let context = PresentationContext {
            project: self.project.clone(),
            demo: self.demo,
            stale_after: self.metrics_stale_after(),
            availability: self
                .engines
                .iter()
                .map(|engine| {
                    (
                        engine.snapshot.is_some(),
                        engine.error.is_some(),
                        engine.alias.is_some(),
                    )
                })
                .collect(),
        };
        if self.presentations.context.as_ref() != Some(&context)
            || self.presentations.expires_at.is_some_and(|at| now >= at)
        {
            self.presentations.rows.clear();
            self.presentations.context = Some(context);
            self.presentations.expires_at = self.next_metrics_expiry(now);
        }
        for index in start..start.saturating_add(count).min(self.rows.len()) {
            let row = &self.rows[index];
            if self
                .presentations
                .rows
                .get(&row.key)
                .is_some_and(|value| !include_metrics || value.metrics_ready)
            {
                continue;
            }
            let condition = self.condition_for(row);
            let (cpu, memory) = if !include_metrics
                || matches!(row.scope, Scope::Resource { .. } | Scope::Resources { .. })
            {
                (String::new(), String::new())
            } else if self.engines[row.engine].alias.is_some() {
                ("↪".into(), "alias".into())
            } else {
                let usage = self.metrics_for_at(row, now);
                (usage.cpu_label(), usage.memory_label())
            };
            self.presentations.rows.insert(
                row.key.clone(),
                RowPresentation {
                    condition,
                    cpu,
                    memory,
                    metrics_ready: include_metrics,
                },
            );
            #[cfg(test)]
            {
                self.presentations.builds += 1;
            }
        }
    }

    pub fn row_presentation(&self, key: &str) -> &RowPresentation {
        &self.presentations.rows[key]
    }

    /// Only time-dependent visuals schedule an otherwise unchanged frame.
    pub fn next_visual_deadline(&self, now: std::time::Instant) -> Option<std::time::Instant> {
        let mut deadline = self.next_metrics_expiry(now);
        let mut consider = |at: Option<std::time::Instant>| {
            if let Some(at) = at {
                deadline = Some(deadline.map_or(at, |old| old.min(at)));
            }
        };
        if self.has_active_actions() {
            consider(now.checked_add(std::time::Duration::from_millis(100)));
        }
        if self.security.busy && self.overview_mode == OverviewMode::Security {
            consider(now.checked_add(std::time::Duration::from_secs(1)));
        }
        for feedback in self.action_feedback.values() {
            for at in feedback
                .finished_at
                .iter()
                .chain(feedback.result_at.iter().flatten())
            {
                consider(
                    at.checked_add(std::time::Duration::from_secs(5))
                        .filter(|&at| at > now),
                );
            }
        }
        // Keep visible sample-age diagnostics accurate without redrawing all idle views.
        if !self.demo
            && (self.tab == Tab::Overview || self.performance_open)
            && self.engines.iter().any(|engine| {
                !engine.metrics.samples.is_empty()
                    || self.performance_open && engine.metrics.has_history()
            })
        {
            consider(now.checked_add(std::time::Duration::from_secs(1)));
        }
        deadline
    }

    pub fn engine_metrics(&self, index: usize) -> crate::metrics::Summary {
        let engine = &self.engines[index];
        let Some(snapshot) = &engine.snapshot else {
            return Default::default();
        };
        let available = engine.error.is_none() && engine.alias.is_none();
        let mut summary = engine.metrics.summary(
            &snapshot.containers,
            std::time::Instant::now(),
            self.metrics_stale_after(),
            available,
        );
        if available && summary.running == 0 {
            summary.cpu_percent = Some(0.0);
            summary.memory_bytes = Some(0);
        }
        summary
    }

    pub fn condition_for(&self, row: &Row) -> Option<Condition> {
        if matches!(row.scope, Scope::Resource { .. } | Scope::Resources { .. }) {
            return None;
        }
        if let Scope::Machine(name) = &row.scope {
            return Some(self.machine_condition(name));
        }
        let engine = &self.engines[row.engine];
        if engine.alias.is_some() {
            return Some(Condition::new(Tone::Neutral, "alias"));
        }
        if engine.error.is_some() {
            return Some(Condition::new(Tone::Warning, "unavailable"));
        }
        let Some(snapshot) = &engine.snapshot else {
            return Some(Condition::new(Tone::Warning, "connecting"));
        };
        let members: Vec<_> = snapshot
            .containers
            .iter()
            .filter(|c| matches_container(&row.scope, c))
            .collect();
        if matches!(row.scope, Scope::Container(_)) {
            return members.first().map(|c| c.condition());
        }
        if row.scope == Scope::Engine && members.is_empty() {
            return Some(Condition::new(Tone::Good, "connected"));
        }
        if row.scope == Scope::Engine {
            return Some(crate::model::runtime_condition(members));
        }
        Some(crate::model::aggregate_condition(members))
    }

    pub fn platform_target(&self) -> Option<(crate::platform::Key, Connection, Container)> {
        if self.tab != Tab::Overview || self.demo {
            return None;
        }
        let row = self.current()?;
        let engine = &self.engines[row.engine];
        if engine.error.is_some() || engine.alias.is_some() {
            return None;
        }
        let containers = self.selected_containers();
        if containers.len() != 1 {
            return None;
        }
        let container = containers.into_iter().next()?;
        if container.image_platform.is_some() {
            return None;
        }
        let key = crate::platform::Key::new(
            row.engine,
            engine.snapshot.as_ref()?.engine_id.clone(),
            &container,
        );
        Some((key, engine.connection.clone(), container))
    }

    /// One ordering/filter shared by rendering, movement, Enter, and focus restoration.
    pub fn performance_engines(&self) -> Vec<usize> {
        self.context_order
            .iter()
            .copied()
            .filter(|&index| {
                let engine = &self.engines[index];
                self.engine_in_view(index)
                    && engine.snapshot.is_some()
                    && engine.error.is_none()
                    && engine.alias.is_none()
            })
            .collect()
    }

    pub fn performance_engine(&self) -> Option<usize> {
        let engines = self.performance_engines();
        engines
            .get(self.performance_scroll.min(engines.len().saturating_sub(1)))
            .copied()
    }

    /// Only actual container rows are eligible, never an implicit group member.
    pub fn performance_container(&self) -> Option<(usize, Container)> {
        let row = self.current()?;
        let Scope::Container(key) = &row.scope else {
            return None;
        };
        let engine = &self.engines[row.engine];
        if engine.alias.is_some() {
            return None;
        }
        let container = engine
            .snapshot
            .as_ref()?
            .containers
            .iter()
            .find(|container| container.key() == *key)?
            .clone();
        Some((row.engine, container))
    }

    /// Move within the active Performance scope, respecting workspace/search
    /// filters but revealing collapsed ancestors of the chosen container.
    pub fn move_performance_selection(&mut self, amount: isize) {
        if self.performance_scope == PerformanceScope::Runtime {
            let last = self.performance_engines().len().saturating_sub(1);
            self.performance_scroll = self
                .performance_scroll
                .min(last)
                .saturating_add_signed(amount)
                .min(last);
            return;
        }
        let mut rows = Vec::new();
        let query = self.tree_query.to_lowercase();
        for tree in self.explorer_nodes() {
            let mut expanded = HashSet::new();
            let mut pending = vec![&tree];
            while let Some(node) = pending.pop() {
                expanded.insert(node.row.key.clone());
                pending.extend(&node.children);
            }
            flatten(&tree, 0, &expanded, &query, &mut rows);
        }
        let selected = self
            .current()
            .and_then(|current| rows.iter().position(|row| row.key == current.key))
            .unwrap_or(0);
        let indices: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                matches!(row.scope, Scope::Container(_)) && self.engines[row.engine].alias.is_none()
            })
            .map(|(index, _)| index)
            .collect();
        if indices.is_empty() {
            self.notice = crate::i18n::t(
                "No containers in the current Explorer scope; adjust workspace or search",
            )
            .into();
            return;
        }
        let position = match indices.binary_search(&selected) {
            Ok(position) => position.saturating_add_signed(amount),
            Err(position) if amount > 0 => position.saturating_add(amount as usize - 1),
            Err(position) if amount < 0 => position.saturating_sub(amount.unsigned_abs()),
            Err(_) => return,
        }
        .min(indices.len() - 1);
        let target = &rows[indices[position]];
        let mut depth = target.depth;
        for ancestor in rows[..indices[position]].iter().rev() {
            if ancestor.depth < depth {
                self.expanded.insert(ancestor.key.clone());
                depth = ancestor.depth;
            }
        }
        self.refresh_tree();
        if let Some(index) = self.rows.iter().position(|row| row.key == target.key) {
            self.selected = index;
        }
        self.detail_scroll = 0;
    }

    pub fn switch_performance_scope(&mut self) {
        self.performance_scope = match self.performance_scope {
            PerformanceScope::Runtime => PerformanceScope::Container,
            PerformanceScope::Container => PerformanceScope::Runtime,
        };
        self.performance_open = true;
        self.performance_focus = true;
        self.detail_focus = false;
    }

    fn restore_performance_focus(&mut self, engine: Option<usize>) {
        let engines = self.performance_engines();
        self.performance_scroll = engine
            .and_then(|index| engines.iter().position(|&candidate| candidate == index))
            .unwrap_or(self.performance_scroll.min(engines.len().saturating_sub(1)));
    }

    pub fn toggle_performance(&mut self) {
        self.performance_open = !self.performance_open;
        if !self.performance_open {
            self.performance_focus = false;
            self.performance_expanded = false;
        }
    }

    pub fn cycle_focus(&mut self, backwards: bool) {
        self.exec_input = false;
        let current = if self.detail_focus {
            2
        } else if self.performance_focus {
            1
        } else {
            0
        };
        let next = if self.performance_open {
            if backwards {
                (current + 2) % 3
            } else {
                (current + 1) % 3
            }
        } else if current == 2 {
            0
        } else {
            2
        };
        self.detail_focus = next == 2;
        self.performance_focus = next == 1;
    }

    pub fn start_exec(&mut self, request: crate::exec::Request) {
        let result = self.sessions.open(request, self.exec_dimensions);
        match result {
            Ok(_) => {
                self.tab = Tab::Exec;
                self.detail_focus = true;
                self.performance_focus = false;
                self.exec_input = true;
                self.notice.clear();
            }
            Err(error) => {
                self.notice = crate::tf!("Exec unavailable: {error}", error = format!("{error:#}"))
            }
        }
    }

    pub fn close_exec(&mut self) {
        self.sessions.close_active();
        self.exec_input = false;
        self.notice = crate::i18n::t("Exec closed; the container was not stopped.").into();
    }

    pub fn pick_sessions(&mut self) {
        if self.sessions.is_empty() {
            self.notice = crate::i18n::t(
                "No retained Exec sessions. Select a running container and press e.",
            )
            .into();
            return;
        }
        self.picker = Some(Picker {
            title: crate::tf!(
                "Exec sessions · {a0}/{a1} retained",
                a0 = self.sessions.len(),
                a1 = self.sessions.limit()
            ),
            choices: self
                .sessions
                .iter()
                .map(|(id, _)| Choice::Session(id))
                .collect(),
            selected: self
                .sessions
                .iter()
                .position(|(id, _)| Some(id) == self.sessions.active_id())
                .unwrap_or(0),
        });
    }

    fn explorer_tree(&self, index: usize) -> Option<Node> {
        if !self.engine_visible(index) {
            return None;
        }
        if self
            .project
            .as_ref()
            .is_some_and(|project| project.engine != index)
        {
            return None;
        }
        let tree = engine_tree(index, &self.engines[index]);
        Some(if let Some(project) = &self.project {
            find_project(&tree, project).cloned().unwrap_or_else(|| {
                node(
                    index,
                    Scope::Project {
                        namespace: project.namespace.clone(),
                        project: project.name.clone(),
                    },
                    crate::tf!("{a0} [not in latest inventory]", a0 = project.name),
                    vec![],
                )
            })
        } else {
            tree
        })
    }

    /// Local engines must not vanish behind a collapsed row when the first server is added.
    fn expand_machines_once(&mut self) {
        if self.machines.is_empty() || self.local_machine_expanded {
            return;
        }
        let local = crate::machines::LOCAL_MACHINE;
        if let Some(first) = self.machine_engines(local).into_iter().min() {
            self.expanded
                .insert(format!("{first}/{:?}", Scope::Machine(local.to_owned())));
            self.local_machine_expanded = true;
        }
    }

    fn machine_condition(&self, name: &str) -> Condition {
        use crate::machines::MachineState;
        match self.machine(name).map(|machine| &machine.state) {
            Some(MachineState::NotConnected) => Condition::new(Tone::Neutral, "not connected"),
            Some(MachineState::Connecting) => Condition::new(Tone::Warning, "connecting"),
            Some(MachineState::Offline { .. }) => Condition::new(Tone::Warning, "unavailable"),
            Some(MachineState::Connected) | None => {
                let engines = self.machine_engines(name);
                if engines
                    .iter()
                    .any(|&index| self.engines[index].error.is_some())
                {
                    return Condition::new(Tone::Warning, "unavailable");
                }
                if engines
                    .iter()
                    .any(|&index| self.engines[index].snapshot.is_none())
                {
                    return Condition::new(Tone::Warning, "connecting");
                }
                let members: Vec<_> = engines
                    .iter()
                    .filter(|&&index| self.engines[index].alias.is_none())
                    .filter_map(|&index| self.engines[index].snapshot.as_ref())
                    .flat_map(|snapshot| &snapshot.containers)
                    .collect();
                if members.is_empty() {
                    Condition::new(Tone::Good, "connected")
                } else {
                    crate::model::runtime_condition(members)
                }
            }
        }
    }

    fn machine_node(&self, name: &str, children: Vec<Node>) -> Option<Node> {
        use crate::machines::{LOCAL_MACHINE, MachineState};
        if name == LOCAL_MACHINE {
            let first = self.machine_engines(name).into_iter().min()?;
            return Some(node(
                first,
                Scope::Machine(name.to_owned()),
                crate::i18n::t("Local").into(),
                children,
            ));
        }
        let machine = self.machine(name)?;
        let status = match &machine.state {
            MachineState::NotConnected => crate::i18n::t(" [not connected]"),
            MachineState::Connecting => crate::i18n::t(" [connecting]"),
            MachineState::Connected => "",
            MachineState::Offline { .. } => crate::i18n::t(" (offline)"),
        };
        Some(node(
            machine.anchor,
            Scope::Machine(name.to_owned()),
            format!("{name}{}{status}", crate::i18n::t(" (remote)")),
            children,
        ))
    }

    fn engine_nodes(&self, machine: &str) -> Vec<Node> {
        self.machine_engines(machine)
            .into_iter()
            .filter_map(|index| self.explorer_tree(index))
            .collect()
    }

    /// The Explorer's top level. Without servers, or inside a Compose project, it stays a
    /// plain list of engines, exactly as before machines existed.
    fn explorer_nodes(&self) -> Vec<Node> {
        use crate::machines::LOCAL_MACHINE;
        if self.machines.is_empty() || self.project.is_some() {
            return self
                .context_order
                .iter()
                .filter_map(|&index| self.explorer_tree(index))
                .collect();
        }
        if let Some(focus) = &self.machine {
            let engines = self.engine_nodes(focus);
            // Keeps a row, and so the state and the way to connect, while nothing is reachable.
            if engines.is_empty() {
                return self.machine_node(focus, Vec::new()).into_iter().collect();
            }
            return engines;
        }
        let mut forest: Vec<_> = self
            .machine_node(LOCAL_MACHINE, self.engine_nodes(LOCAL_MACHINE))
            .into_iter()
            .collect();
        for machine in &self.machines {
            let name = &machine.remote.name;
            forest.extend(self.machine_node(name, self.engine_nodes(name)));
        }
        forest
    }

    pub fn refresh_tree(&mut self) {
        let previous = self.current().map(|r| r.key.clone());
        self.expand_machines_once();
        if matches!(
            self.context_sort,
            crate::context_sort::ContextSort::ConnectedFirst
                | crate::context_sort::ContextSort::Name
        ) {
            self.reorder_contexts();
        }
        let mut rows = Vec::new();
        let query = self.tree_query.to_lowercase();
        for tree in self.explorer_nodes() {
            flatten(&tree, 0, &self.expanded, &query, &mut rows);
        }
        self.rows = rows;
        self.selected = previous
            .and_then(|key| self.rows.iter().position(|r| r.key == key))
            .unwrap_or(self.selected.min(self.rows.len().saturating_sub(1)));
    }

    pub fn move_selection(&mut self, amount: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(self.rows.len().saturating_sub(1));
    }

    /// The key shown in hints: the user's binding, then the layout preset's, then the default.
    pub fn key_hint(&self, action: crate::shortcuts::ActionId) -> String {
        self.keymap
            .bindings()
            .find(|(_, candidate)| *candidate == action)
            .map(|(key, _)| {
                key.as_letter().map_or_else(
                    || crokey::KeyCombinationFormat::default().to_string(key),
                    |letter| letter.to_string(),
                )
            })
            .or_else(|| {
                action
                    .info()
                    .bindings
                    .first()
                    .map(|binding| binding.key.to_owned())
            })
            .unwrap_or_default()
    }

    pub fn mouse_target(&self, column: u16, row: u16) -> Option<MouseTarget> {
        let position = ratatui::layout::Position::new(column, row);
        self.mouse_regions
            .iter()
            .rev()
            .find(|(area, _)| area.contains(position))
            .map(|(_, target)| *target)
    }

    /// Explorer zoom only shows while the Explorer has focus, like detail zoom on the other side.
    pub fn explorer_zoomed(&self) -> bool {
        self.explorer_zoom
            && self.overview_mode == OverviewMode::Containers
            && !self.detail_focus
            && !self.performance_focus
    }

    /// The Networks tab falls back to plain scrolling when it has no rows to select.
    pub fn network_cursor_active(&self) -> bool {
        self.tab == Tab::Networks && !self.network_targets.is_empty()
    }

    pub fn move_network_cursor(&mut self, amount: isize) {
        self.network_cursor = self
            .network_cursor
            .saturating_add_signed(amount)
            .min(self.network_targets.len().saturating_sub(1));
    }

    /// Selects the container under the Networks cursor in the Explorer.
    pub fn open_network_target(&mut self) {
        let Some(key) = self
            .network_targets
            .get(self.network_cursor)
            .and_then(|target| target.container.clone())
        else {
            return;
        };
        let Some(tree) = self
            .current()
            .and_then(|row| self.explorer_tree(row.engine))
        else {
            return;
        };
        let mut path = Vec::new();
        if !container_path(&tree, &key, &mut path) {
            return;
        }
        let Some((target, ancestors)) = path.split_last() else {
            return;
        };
        let target = target.clone();
        self.expanded.extend(ancestors.iter().cloned());
        self.tree_query.clear();
        self.refresh_tree();
        if let Some(index) = self.rows.iter().position(|row| row.key == target) {
            self.selected = index;
            self.detail_focus = false;
            self.detail_scroll = 0;
        }
    }

    pub fn next_tree_match(&mut self, backwards: bool) {
        if self.rows.is_empty() {
            return;
        }
        let query = self.tree_query.to_lowercase();
        for distance in 1..=self.rows.len() {
            let index = if backwards {
                (self.selected + self.rows.len() - distance) % self.rows.len()
            } else {
                (self.selected + distance) % self.rows.len()
            };
            if self.rows[index].title.to_lowercase().contains(&query) {
                self.selected = index;
                break;
            }
        }
    }

    pub fn expand(&mut self) {
        if let Some(row) = self.current().cloned() {
            if row.branch && self.expanded.insert(row.key.clone()) {
                self.refresh_tree();
            } else if row.branch && self.selected + 1 < self.rows.len() {
                self.selected += 1;
            }
        }
    }

    pub fn collapse(&mut self) {
        let Some(row) = self.current().cloned() else {
            return;
        };
        if row.branch && self.expanded.remove(&row.key) {
            self.refresh_tree();
        } else if let Some(parent) = (0..self.selected)
            .rev()
            .find(|&i| self.rows[i].depth < row.depth)
        {
            self.selected = parent;
        }
    }

    pub fn toggle_branch(&mut self) {
        let Some(row) = self.current().cloned().filter(|row| row.branch) else {
            return;
        };
        if !self.tree_query.is_empty() {
            // Search forces matching branches open. Leave the filter before
            // collapsing, preserving the ancestors needed to keep selection visible.
            let mut depth = row.depth;
            for ancestor in self.rows[..self.selected].iter().rev() {
                if ancestor.depth < depth {
                    self.expanded.insert(ancestor.key.clone());
                    depth = ancestor.depth;
                }
            }
            self.tree_query.clear();
            self.expanded.remove(&row.key);
            self.notice = crate::i18n::t("Tree search cleared; selected branch collapsed.").into();
        } else if !self.expanded.remove(&row.key) {
            self.expanded.insert(row.key);
        }
        self.refresh_tree();
    }

    pub fn selected_containers(&self) -> Vec<Container> {
        self.selected_container_refs().cloned().collect()
    }

    /// Read-only selection without copying container metadata. Async workers should
    /// take an owned snapshot only when their subscription or operation changes.
    pub fn selected_container_refs(&self) -> impl Iterator<Item = &Container> {
        self.current()
            .into_iter()
            .flat_map(|row| self.container_refs_for(row))
    }

    fn containers_for(&self, row: &Row) -> Vec<Container> {
        self.container_refs_for(row).cloned().collect()
    }

    fn container_refs_for<'a>(&'a self, row: &'a Row) -> impl Iterator<Item = &'a Container> {
        self.engines[row.engine]
            .snapshot
            .as_ref()
            .into_iter()
            .flat_map(|snapshot| &snapshot.containers)
            .filter(|c| matches_container(&row.scope, c))
            .filter(|c| {
                self.project
                    .as_ref()
                    .is_none_or(|project| project.contains(c))
            })
    }

    pub fn configure_startup(&mut self, cwd: PathBuf) {
        self.startup_cwd = Some(cwd);
        // Servers are excluded: they may never connect, and their paths are not local paths.
        self.startup_pending = (0..self.engines.len())
            .filter(|&index| !crate::remote::is_remote(&self.engines[index].connection))
            .collect();
        self.notice = crate::i18n::t(
            "Matching the current directory to Compose projects… (--overview skips this)",
        )
        .into();
    }

    /// Restores a focused machine without connecting it, even for a server set to manual.
    pub fn finish_preference_restore(&mut self) {
        if let Some(name) = self.pending_machine.take()
            && (self.machine(&name).is_some()
                || (name == crate::machines::LOCAL_MACHINE && !self.machines.is_empty()))
            && self.project.is_none()
        {
            // Without this entry, Backspace and :all could never leave the restored focus.
            self.push_navigation();
            self.machine = Some(name);
            self.refresh_tree();
            self.selected = 0;
        }
    }

    pub fn cancel_startup(&mut self) {
        self.pending_preferences = None;
        if self.startup_cwd.take().is_some() {
            self.notice.clear();
        }
    }

    pub fn apply_preferences(
        &mut self,
        preferences: crate::preferences::Preferences,
        restore_navigation: bool,
    ) {
        self.apply_saved_settings(preferences.settings);
        self.pending_machine = restore_navigation
            .then(|| preferences.machine.clone())
            .flatten();
        // Only validated provider/name metadata is retained; no endpoints or commands.
        if let Some(home) = std::env::var_os("HOME") {
            for saved in &preferences.runtimes {
                if saved.target.validate().is_err()
                    // A discovered runtime can use another socket than the remembered one, so
                    // the same name means the same runtime, not a second, offline row.
                    || self.engines.iter().any(|engine| {
                        engine.runtime.as_ref() == Some(&saved.target)
                            || engine.connection.name == saved.name
                    })
                {
                    continue;
                }
                let index = self.engines.len();
                self.engines.push(Engine {
                    connection: Connection {
                        name: saved.name.clone(),
                        kind: crate::config::BackendKind::Docker,
                        endpoint: saved.target.endpoint(std::path::Path::new(&home)),
                    },
                    runtime: Some(saved.target.clone()),
                    snapshot: None,
                    error: Some(crate::i18n::t("Offline · waiting for runtime discovery").into()),
                    alias: None,
                    metrics: Default::default(),
                    metrics_revision: 0,
                    events: Default::default(),
                });
                if let crate::runtime::Target::Podman { machine } = &saved.target {
                    self.podman_machines.insert(machine.clone(), index);
                }
                self.context_order.push(index);
                self.expanded.insert(format!("{index}/{:?}", Scope::Engine));
                if self.startup_cwd.is_some() {
                    self.startup_pending.insert(index);
                }
            }
        }
        self.performance_open = preferences.performance_open;
        self.log_view.wrap = preferences.log_wrap;
        self.context_sort = preferences.context_sort;
        // Never restore a shell tab, input focus, an editor, or a mutation dialog.
        self.tab = if preferences.tab == Tab::Exec {
            Tab::Overview
        } else {
            preferences.tab
        };
        self.reorder_contexts();
        self.refresh_tree();
        if restore_navigation {
            self.pending_preferences = Some(preferences);
        }
    }

    fn preference_connection(&self, engine: usize) -> String {
        let connection = &self.engines[engine].connection;
        // Display names and backend only: no endpoints, TLS data, or credentials on disk.
        format!("{:?}/{}", connection.kind, connection.name)
    }

    pub fn preferences(&self) -> crate::preferences::Preferences {
        let row = |engine, scope| crate::preferences::Row {
            connection: self.preference_connection(engine),
            scope,
        };
        let mut expanded: Vec<_> = self
            .expanded
            .iter()
            .filter_map(|key| {
                let (engine, scope) = key.split_once('/')?;
                let engine: usize = engine.parse().ok()?;
                (engine < self.engines.len()).then(|| row(engine, scope.to_owned()))
            })
            .collect();
        expanded.sort_by(|left, right| {
            (&left.connection, &left.scope).cmp(&(&right.connection, &right.scope))
        });
        expanded.truncate(1024);
        crate::preferences::Preferences {
            version: 1,
            settings: self.saved_settings,
            runtimes: self
                .engines
                .iter()
                .filter_map(|engine| {
                    engine.runtime.clone().map(|target| crate::runtime::Saved {
                        name: engine.connection.name.clone(),
                        target,
                    })
                })
                .take(128)
                .collect(),
            performance_open: self.performance_open,
            log_wrap: self.log_view.wrap,
            context_sort: self.context_sort,
            tab: if self.tab == Tab::Exec {
                Tab::Overview
            } else {
                self.tab
            },
            expanded,
            selected: self
                .current()
                .map(|current| row(current.engine, format!("{:?}", current.scope))),
            machine: self.machine.clone(),
            project: self.project.as_ref().map(|project| {
                row(
                    project.engine,
                    format!(
                        "{:?}",
                        Scope::Project {
                            namespace: project.namespace.clone(),
                            project: project.name.clone()
                        }
                    ),
                )
            }),
        }
    }

    fn restore_navigation(&mut self) {
        let Some(preferences) = self.pending_preferences.take() else {
            return;
        };
        let resolve = |saved: &crate::preferences::Row| {
            let engines: Vec<_> = (0..self.engines.len())
                .filter(|&engine| self.preference_connection(engine) == saved.connection)
                .collect();
            if engines.len() == 1 {
                Some(format!("{}/{scope}", engines[0], scope = saved.scope))
            } else {
                None
            }
        };
        self.expanded = preferences.expanded.iter().filter_map(resolve).collect();
        // Saved state may predate a server, so machine rows always start open.
        self.local_machine_expanded = false;
        for machine in &self.machines {
            self.expanded.insert(format!(
                "{}/{:?}",
                machine.anchor,
                Scope::Machine(machine.remote.name.clone())
            ));
        }
        self.refresh_tree();
        if let Some(saved) = &preferences.project
            && let Some(key) = resolve_preference_key(self, saved)
            && let Some(current) = self.rows.iter().find(|row| row.key == key)
            && let Scope::Project { namespace, project } = &current.scope
        {
            self.open_project(Project {
                engine: current.engine,
                namespace: namespace.clone(),
                name: project.clone(),
            });
        }
        if let Some(saved) = &preferences.selected
            && let Some(key) = resolve_preference_key(self, saved)
            && let Some(index) = self.rows.iter().position(|row| row.key == key)
        {
            self.selected = index;
        }
        self.tab = if preferences.tab == Tab::Exec {
            Tab::Overview
        } else {
            preferences.tab
        };
        self.detail_focus = false;
        self.exec_input = false;
    }

    pub fn open_selected(&mut self) {
        if let Some(Scope::Machine(name)) = self.current().map(|row| row.scope.clone()) {
            self.open_machine(&name);
            return;
        }
        if let Some(row) = self.current().cloned()
            && let Scope::Project { namespace, project } = row.scope
        {
            let project = Project {
                engine: row.engine,
                namespace,
                name: project,
            };
            if self.project.as_ref() != Some(&project) {
                self.open_project(project);
                return;
            }
        }
        self.detail_focus = true;
        self.detail_scroll = 0;
    }

    pub(crate) fn push_navigation(&mut self) {
        self.history.push(NavigationState {
            machine: self.machine.clone(),
            project: self.project.clone(),
            selected: self.current().map(|row| row.key.clone()),
            tree_offset: self.tree_offset,
            tree_query: self.tree_query.clone(),
            expanded: self.expanded.clone(),
            detail_focus: self.detail_focus,
            tab: self.tab,
            detail_scroll: self.detail_scroll,
        });
    }

    pub fn open_project(&mut self, project: Project) {
        self.push_navigation();
        let scope = Scope::Project {
            namespace: project.namespace.clone(),
            project: project.name.clone(),
        };
        self.expanded
            .insert(format!("{}/{scope:?}", project.engine));
        self.project = Some(project);
        self.performance_focus = false;
        self.exec_input = false;
        self.tree_query.clear();
        self.detail_focus = false;
        self.tree_offset = 0;
        self.detail_scroll = 0;
        self.tab = Tab::Overview;
        self.refresh_tree();
        self.selected = 0;
    }

    pub fn back(&mut self) -> bool {
        let Some(previous) = self.history.pop() else {
            return false;
        };
        // A server removed since this view was saved must not come back as a focus.
        self.machine = previous
            .machine
            .filter(|name| name == crate::machines::LOCAL_MACHINE || self.machine(name).is_some());
        self.project = previous.project;
        self.performance_focus = false;
        self.exec_input = false;
        self.expanded = previous.expanded;
        self.tree_query = previous.tree_query;
        self.detail_focus = previous.detail_focus;
        self.tab = previous.tab;
        self.detail_scroll = previous.detail_scroll;
        self.refresh_tree();
        self.selected = previous
            .selected
            .and_then(|key| self.rows.iter().position(|row| row.key == key))
            .unwrap_or(0);
        self.tree_offset = previous.tree_offset;
        true
    }

    pub fn request_exec(&mut self) {
        if self.demo {
            self.notice = crate::i18n::t("Exec is disabled in demo mode.").into();
            return;
        }
        let Some(row) = self.current().cloned() else {
            return;
        };
        if let Scope::Machine(name) = &row.scope
            && let Some(machine) = self
                .machines
                .iter()
                .find(|machine| machine.remote.name == *name)
        {
            if !machine.connected() {
                self.notice =
                    crate::i18n::t("Connect to this server before opening a shell.").into();
                return;
            }
            let remote = machine.remote.clone();
            let command = crate::remote_exec::host_command(&remote);
            self.open_local_tool(
                &format!("{} · {} · server shell", remote.name, remote.address()),
                "server-shell",
                command,
            );
            return;
        }
        if !matches!(row.scope, Scope::Container(_) | Scope::Service { .. }) {
            self.notice =
                crate::i18n::t("Select a container or service, then press e to exec.").into();
            return;
        }
        let engine = &self.engines[row.engine];
        if engine.error.is_some() || engine.alias.is_some() {
            self.notice =
                crate::i18n::t("Exec is unavailable on a disconnected or aliased engine.").into();
            return;
        }
        let containers: Vec<_> = self
            .selected_containers()
            .into_iter()
            .filter(|c| c.running() && !c.managed)
            .collect();
        if containers.is_empty() {
            self.notice =
                crate::i18n::t("Exec requires a running, non-Kubernetes-managed container.").into();
        } else if containers.len() == 1 {
            self.exec_request = Some(crate::exec::Request {
                connection: engine.connection.clone(),
                container: containers[0].clone(),
                command: self.exec_command.clone(),
            });
        } else {
            self.picker = Some(Picker {
                title: crate::i18n::t("Exec · Choose a running container").into(),
                choices: containers
                    .into_iter()
                    .map(|container| Choice::Exec {
                        engine: row.engine,
                        container: Box::new(container),
                    })
                    .collect(),
                selected: 0,
            });
        }
    }

    pub fn choose(&mut self) {
        if let Some(Choice::Action { action, row }) = self
            .picker
            .as_ref()
            .and_then(|picker| picker.choices.get(picker.selected))
            && let Err(reason) = self.action_availability(*action, row)
        {
            self.notice = reason;
            return;
        }
        let Some(mut picker) = self.picker.take() else {
            return;
        };
        if picker.selected >= picker.choices.len() {
            return;
        }
        match picker.choices.remove(picker.selected) {
            Choice::ActionResult(operation) => {
                if self.action_jobs.contains_key(&operation) {
                    self.action_selected = Some(operation);
                    self.action_results_visible = true;
                }
            }
            Choice::Session(id) => {
                if self.sessions.select(id) {
                    self.tab = Tab::Exec;
                    self.detail_focus = true;
                    self.performance_focus = false;
                    self.exec_input = false;
                    self.notice = crate::tf!(
                        "Selected Exec session #{id}; Enter resumes input if connected",
                        id = id
                    );
                }
            }
            Choice::Action { action, row } => {
                if self.action_row().is_some_and(|current| current.key == row) {
                    self.request_action(action);
                }
            }
            Choice::Compose(action) => self.request_compose(action),
            Choice::ContextSort(mode) => {
                self.context_sort = mode;
                self.reorder_contexts();
                self.refresh_tree();
                self.notice =
                    crate::tf!("Context order: {a0} · o to sort again", a0 = mode.label());
            }
            Choice::Project(project) => self.open_project(project),
            Choice::SavedView(name) => self.apply_saved_view(&name),
            Choice::ComposeFile(request) => self.edit_request = Some(*request),
            Choice::Diagnose(kind) => self.open_diagnostics(kind),
            Choice::DiagnosticPeerNone => {
                if let Some(view) = &mut self.diagnostics {
                    view.peer = None;
                    view.message = None;
                }
            }
            Choice::DiagnosticPeer(container) => {
                let Some(engine) = self
                    .diagnostics
                    .as_ref()
                    .map(|view| view.main.target.engine)
                else {
                    return;
                };
                let side = self.diagnostic_side(engine, &container);
                if let Some(view) = &mut self.diagnostics {
                    view.peer = Some(side);
                    view.message = None;
                    view.scroll = 0;
                }
                self.load_diagnostics(crate::diagnostics::Role::Peer);
            }
            Choice::Exec { engine, container } => {
                if self.engines[engine].error.is_some() || self.engines[engine].alias.is_some() {
                    self.notice = crate::i18n::t(
                        "Connection unavailable. Select a connected container and retry.",
                    )
                    .into();
                    return;
                }
                self.exec_request = Some(crate::exec::Request {
                    connection: self.engines[engine].connection.clone(),
                    container: *container,
                    command: self.exec_command.clone(),
                });
            }
        }
    }

    /// Everything the view shows right now, by name so it survives runtimes changing order.
    pub fn capture_view(&self, name: &str) -> crate::saved_views::SavedView {
        use crate::saved_views::{SavedInventory, SavedProject, SavedView};
        let inventory = (self.overview_mode != OverviewMode::Containers
            && self.inventory_engine < self.engines.len())
        .then(|| {
            let view = self
                .inventory_views
                .get(&(self.inventory_engine, self.overview_mode));
            SavedInventory {
                connection: self.preference_connection(self.inventory_engine),
                query: view.map(|view| view.query.clone()).unwrap_or_default(),
                filter: view.map_or(0, |view| view.filter),
                sort: view.map_or(0, |view| view.sort),
            }
        });
        SavedView {
            name: name.into(),
            overview: self.overview_mode.into(),
            // A shell is never restored.
            tab: if self.tab == Tab::Exec {
                Tab::Overview
            } else {
                self.tab
            },
            machine: self.machine.clone(),
            project: self.project.as_ref().map(|project| SavedProject {
                connection: self.preference_connection(project.engine),
                namespace: project.namespace.clone(),
                name: project.name.clone(),
            }),
            selected: self.current().map(|row| crate::preferences::Row {
                connection: self.preference_connection(row.engine),
                scope: format!("{:?}", row.scope),
            }),
            tree_query: self.tree_query.clone(),
            log_terms: self.log_filter_terms.clone(),
            log_advanced: self.log_filter_text.clone(),
            inventory,
        }
    }

    fn write_saved_views(&mut self) -> Result<(), String> {
        let Some(path) = &self.saved_views_path else {
            return Ok(());
        };
        crate::saved_views::save(path, &self.saved_views).map_err(|error| {
            crate::tf!(
                "Could not write saved views: {error}",
                error = format!("{error:#}")
            )
        })
    }

    /// Saving under an existing name replaces that view, as the notice says.
    pub fn save_view(&mut self, name: &str) {
        let name = name.trim();
        if let Err(error) = crate::saved_views::validate_name(name) {
            self.notice = error.to_string();
            return;
        }
        let view = self.capture_view(name);
        let existing = self.saved_views.iter().position(|view| view.name == name);
        let replaced = match existing {
            Some(index) => {
                self.saved_views[index] = view;
                true
            }
            None if self.saved_views.len() >= crate::saved_views::MAX_VIEWS => {
                self.notice =
                    crate::i18n::t("Saved view limit reached; delete one with :delete-view").into();
                return;
            }
            None => {
                self.saved_views.push(view);
                self.saved_views.sort_by(|a, b| a.name.cmp(&b.name));
                false
            }
        };
        self.notice = match self.write_saved_views() {
            Err(error) => error,
            Ok(()) if replaced => crate::tf!("Replaced saved view {name}", name = name),
            Ok(()) => crate::tf!(
                "Saved view {name}, including its filter text · :saved-views opens it",
                name = name
            ),
        };
    }

    pub fn delete_view(&mut self, name: &str) {
        let name = name.trim();
        let count = self.saved_views.len();
        self.saved_views.retain(|view| view.name != name);
        if self.saved_views.len() == count {
            self.notice = crate::tf!(
                "No saved view is named {name}",
                name = crate::model::safe_text(name)
            );
            return;
        }
        self.notice = match self.write_saved_views() {
            Err(error) => error,
            Ok(()) => crate::tf!("Deleted saved view {name}", name = name),
        };
    }

    pub fn pick_saved_view(&mut self) {
        if self.saved_views.is_empty() {
            self.notice =
                crate::i18n::t("No saved views yet; :save-view NAME saves the current one").into();
            return;
        }
        self.picker = Some(Picker {
            title: crate::i18n::t("Saved views").into(),
            choices: self
                .saved_views
                .iter()
                .map(|view| Choice::SavedView(view.name.clone()))
                .collect(),
            selected: 0,
        });
    }

    fn engine_named(&self, connection: &str) -> Option<usize> {
        let mut found =
            (0..self.engines.len()).filter(|&i| self.preference_connection(i) == connection);
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }

    /// Restores what still exists and names what does not. This changes only what is shown:
    /// no server connects, no shell opens, and `back` returns to where you were.
    pub fn apply_saved_view(&mut self, name: &str) {
        let Some(view) = self
            .saved_views
            .iter()
            .find(|view| view.name == name)
            .cloned()
        else {
            self.notice = crate::tf!(
                "No saved view is named {name}",
                name = crate::model::safe_text(name)
            );
            return;
        };
        self.push_navigation();
        let mut missing = Vec::new();
        self.machine = view.machine.clone().filter(|machine| {
            let known =
                machine == crate::machines::LOCAL_MACHINE || self.machine(machine).is_some();
            if !known {
                missing.push(crate::model::safe_text(machine));
            }
            known
        });
        self.project = view.project.as_ref().and_then(|saved| {
            let engine = self.engine_named(&saved.connection);
            if engine.is_none() {
                missing.push(crate::model::safe_text(&saved.name));
            }
            engine.map(|engine| Project {
                engine,
                namespace: saved.namespace.clone(),
                name: saved.name.clone(),
            })
        });
        if let Some(project) = &self.project {
            let scope = Scope::Project {
                namespace: project.namespace.clone(),
                project: project.name.clone(),
            };
            self.expanded
                .insert(format!("{}/{scope:?}", project.engine));
        }
        let overview = OverviewMode::from(view.overview);
        self.overview_mode = if overview == OverviewMode::Security && !self.security.enabled {
            missing.push(crate::i18n::t("Security").into());
            OverviewMode::Containers
        } else {
            overview
        };
        if let Some(saved) = &view.inventory {
            match self.engine_named(&saved.connection) {
                Some(engine) => {
                    self.inventory_engine = engine;
                    let inventory = self.inventory_views.entry((engine, overview)).or_default();
                    inventory.query = saved.query.clone();
                    inventory.filter = saved.filter;
                    inventory.sort = saved.sort;
                }
                None => missing.push(crate::model::safe_text(&saved.connection)),
            }
        }
        self.tab = if view.tab == Tab::Exec {
            Tab::Overview
        } else {
            view.tab
        };
        self.tree_query = view.tree_query.clone();
        self.log_view.filter.set_text(&view.log_terms);
        self.log_filter_terms = view.log_terms.clone();
        let advanced = if view.log_advanced.is_empty() {
            "{}"
        } else {
            view.log_advanced.as_str()
        };
        match crate::logs::filter::Filter::parse(advanced) {
            Ok(filter) => {
                self.log_view.filter.set_advanced(filter);
                self.log_filter_text = advanced.to_owned();
            }
            Err(_) => missing.push(crate::i18n::t("advanced log filter").into()),
        }
        self.performance_focus = false;
        self.exec_input = false;
        self.detail_focus = false;
        self.tree_offset = 0;
        self.detail_scroll = 0;
        self.refresh_tree();
        let selected = view
            .selected
            .as_ref()
            .and_then(|saved| resolve_preference_key(self, saved))
            .and_then(|key| self.rows.iter().position(|row| row.key == key));
        self.selected = selected.unwrap_or(0);
        self.notice = if missing.is_empty() {
            crate::tf!("Opened saved view {name} · Backspace returns", name = name)
        } else {
            crate::tf!(
                "Opened saved view {name}, but these are unavailable: {missing}",
                name = name,
                missing = missing.join(", ")
            )
        };
    }

    /// What the selection points at, for reading files. It never changes what is selected.
    fn download_source(&self) -> Result<crate::files::Source, String> {
        use crate::files::Source;
        let Some(row) = self.current() else {
            return Err(crate::i18n::t("Select a container or server first").into());
        };
        if let Scope::Machine(name) = &row.scope {
            let Some(machine) = self.machine(name) else {
                return Err(crate::i18n::t("Select a container or server first").into());
            };
            if !machine.connected() {
                return Err(crate::i18n::t("Connect to this server first").into());
            }
            match machine.capabilities.as_ref() {
                Some(reported) if reported.allow_server_download => {}
                Some(_) => {
                    return Err(crate::i18n::t(
                        "Server file access is turned off; rerun setup with --allow-server-download",
                    )
                    .into());
                }
                None => {
                    return Err(crate::i18n::t(
                        "This server's agent does not support file access; update tha-agent",
                    )
                    .into());
                }
            }
            let control = crate::remote::shell_control_path(&machine.remote)
                .map_err(|error| crate::model::safe_text(&format!("{error:#}")))?;
            return Ok(Source::Server {
                remote: machine.remote.clone(),
                control,
                label: name.clone(),
            });
        }
        let Some(container) = self.diagnostic_container() else {
            return Err(crate::i18n::t("Select a single container or a server").into());
        };
        let engine = &self.engines[row.engine];
        if engine.alias.is_some() || engine.error.is_some() {
            return Err(crate::i18n::t("Select a connected primary runtime").into());
        }
        if engine.connection.kind != crate::config::BackendKind::Docker {
            return Err(crate::i18n::t("Native containerd has no file access").into());
        }
        if container.managed {
            return Err(crate::i18n::t("Kubernetes-managed containers are excluded").into());
        }
        Ok(Source::Container {
            connection: engine.connection.clone(),
            id: container.id.clone(),
            label: format!("{} ({})", container.name, engine.connection.name),
        })
    }

    /// Opens the selected file in the user's editor, as a private copy for a remote file.
    pub fn edit_selected_file(&mut self) {
        use crate::compose_edit::Location;
        let Some(row) = self.downloads.current_row() else {
            return;
        };
        let refuse = |app: &mut Self, reason: &str| {
            app.downloads.message = Some(reason.to_owned());
        };
        if row.entry.kind != crate::files::Kind::File {
            refuse(self, crate::i18n::t("Select a file to edit"));
            return;
        }
        if self.demo {
            refuse(self, crate::i18n::t("Demo mode: files are not edited"));
            return;
        }
        if row.entry.size.is_some_and(|size| size > 1024 * 1024) {
            refuse(
                self,
                crate::i18n::t("This file is larger than 1 MiB, so it was not opened"),
            );
            return;
        }
        if self.downloads.preview_is_binary() {
            refuse(
                self,
                crate::i18n::t("This looks like a binary file, so it was not opened"),
            );
            return;
        }
        if !self.editor.available(self.vscode_path.as_deref()) {
            let reason = crate::tf!(
                "{editor} is not installed; choose another editor in Settings (,)",
                editor = self.editor.label()
            );
            refuse(self, &reason);
            return;
        }
        let Some(source) = self.downloads.tree().map(|tree| tree.source.clone()) else {
            return;
        };
        let location = match source {
            crate::files::Source::Server {
                remote, control, ..
            } => Location::Server {
                remote,
                control,
                path: row.path.clone(),
            },
            source @ crate::files::Source::Container { .. } => Location::Container {
                source,
                path: row.path.clone(),
            },
        };
        self.downloads.message = None;
        self.edit_request = Some(EditRequest {
            location,
            label: crate::model::safe_text(&row.path),
        });
    }

    /// Opens the panel. The tree starts building now, and only for this choice: selecting a
    /// container anywhere else never runs anything in it.
    pub fn open_file_explorer(&mut self) {
        match self.download_source() {
            Ok(source) => self
                .downloads
                .open_source(source, std::time::Instant::now()),
            Err(notice) => self.notice = notice,
        }
    }

    /// `:download PATH`, the direct route that needs no browsing and runs no command.
    pub fn download_path(&mut self, path: &str) {
        let path = match crate::files::clean_path(path) {
            Ok(path) => path,
            Err(error) => {
                self.notice = error.to_string();
                return;
            }
        };
        if path == "/" {
            self.notice =
                crate::i18n::t("Choose a folder below /, not the whole filesystem").into();
            return;
        }
        let source = self
            .downloads
            .tree()
            .map(|tree| tree.source.clone())
            .map_or_else(|| self.download_source(), Ok);
        match source {
            Ok(source) => self.downloads.open_dialog(source, path),
            Err(notice) => self.notice = notice,
        }
    }

    /// Hands real work to the event loop. Demo mode answers from fixtures and reaches nothing.
    pub fn take_download_requests(&mut self) -> Vec<crate::downloads::Request> {
        use crate::downloads::{Event, Request, demo_listing};
        let requests = std::mem::take(&mut self.downloads.requests);
        if !self.demo {
            return requests;
        }
        let now = std::time::Instant::now();
        for request in requests {
            match request {
                Request::List { key, path, .. } => {
                    let result = Ok(demo_listing(&path));
                    self.downloads
                        .apply(Event::Listed { key, path, result }, now);
                }
                Request::Inspect { dialog, path, .. } => {
                    let parent = path.rsplit_once('/').map_or("/", |(parent, _)| {
                        if parent.is_empty() { "/" } else { parent }
                    });
                    let found = demo_listing(parent)
                        .entries
                        .into_iter()
                        .find(|entry| entry.name == crate::files::file_name(&path));
                    let result = found
                        .map(|entry| crate::files::Header {
                            name: entry.name,
                            kind: entry.kind,
                            size: entry.size.unwrap_or(0),
                            modified: 0,
                            mode: 0o644,
                            uid: 0,
                            gid: 0,
                        })
                        .ok_or_else(|| crate::i18n::t("No such file or folder").to_owned());
                    self.downloads
                        .apply(Event::Inspected { dialog, result }, now);
                }
                Request::Count { dialog, path, .. } => {
                    let entries = demo_listing(&path).entries;
                    let totals = crate::files::Totals {
                        bytes: entries.iter().filter_map(|entry| entry.size).sum(),
                        files: entries.len() as u64,
                        done: true,
                    };
                    self.downloads.apply(
                        Event::Counted {
                            dialog,
                            result: Ok(totals),
                        },
                        now,
                    );
                }
                Request::Preview {
                    generation,
                    source,
                    path,
                } => {
                    let result = Ok(crate::files::FileStart {
                        header: crate::files::Header {
                            name: path.clone(),
                            kind: crate::files::Kind::File,
                            size: 0,
                            modified: 0,
                            mode: 0o644,
                            uid: 0,
                            gid: 0,
                        },
                        bytes: format!("Sample contents of {path}\n").into_bytes(),
                        truncated: false,
                    });
                    self.downloads.apply(
                        Event::Previewed {
                            key: source.key(),
                            generation,
                            path,
                            result,
                        },
                        now,
                    );
                }
                Request::Start { .. }
                | Request::Upload { .. }
                | Request::Cancel { .. }
                | Request::CloseDialog => {}
            }
        }
        // Answering a listing can queue the next folder.
        if !self.downloads.requests.is_empty() {
            return self.take_download_requests();
        }
        Vec::new()
    }

    pub fn evaluate_alerts(&mut self) {
        if !self.alert_rules.enabled {
            return;
        }
        let now = crate::history::now_ms();
        let evaluation = crate::alerts::evaluate(self, &self.alert_rules, now);
        self.alerts.update(evaluation, now);
    }

    pub fn open_alert_selection(&mut self) {
        let Some((_, alert)) = self.alerts.open.get(self.alerts.selected) else {
            return;
        };
        let Some((engine, key)) = alert.finding.target.clone() else {
            self.notice =
                crate::i18n::t("This alert has no workload to open; press . to manage servers")
                    .into();
            return;
        };
        self.alerts.view_open = false;
        self.reveal_container(engine, &key);
    }

    /// Keeps the last Compose operation's review current for a short while after it ends.
    pub fn update_compose_review(&mut self) {
        let Some(job) = &self.compose_job else {
            return;
        };
        if !matches!(job.outcome, Some(Ok(()))) {
            return;
        }
        let now = std::time::Instant::now();
        let until = *self.compose_review_until.get_or_insert_with(|| {
            now + std::time::Duration::from_secs(crate::compose::REVIEW_SECONDS)
        });
        if now > until {
            return;
        }
        let Some((engine, project, before)) = &self.compose_before else {
            return;
        };
        let Some(snapshot) = self
            .engines
            .get(*engine)
            .and_then(|engine| engine.snapshot.as_ref())
        else {
            return;
        };
        let review = crate::compose::review(before, project, &snapshot.containers);
        if let Some(job) = &mut self.compose_job {
            job.review = review;
        }
    }

    /// Opens the selected project's Compose file. Several files ask which one first.
    pub fn request_compose_edit(&mut self) {
        use crate::compose_edit::Location;
        let Some(row) = self.current().cloned() else {
            self.notice = crate::i18n::t("Select a Compose project or service").into();
            return;
        };
        if self.demo {
            self.notice = crate::i18n::t("Demo mode: files are not edited").into();
            return;
        }
        let containers = self.containers_for(&row);
        let target = match crate::compose::Target::resolve(&row.scope, &containers) {
            Ok(target) => target,
            Err(error) => {
                self.notice = crate::tf!("{error}", error = format!("{error:#}"));
                return;
            }
        };
        if !self.editor.available(self.vscode_path.as_deref()) {
            self.notice = crate::tf!(
                "{editor} is not installed; choose another editor in Settings (,)",
                editor = self.editor.label()
            );
            return;
        }
        let endpoint = &self.engines[row.engine].connection.endpoint;
        let server = crate::remote::parse_endpoint(endpoint).map(|(server, _)| server.to_owned());
        if let Some(server) = &server {
            match self
                .machine(server)
                .and_then(|machine| machine.capabilities.as_ref())
            {
                Some(reported) if reported.allow_compose_edit => {}
                Some(_) => {
                    self.notice = crate::i18n::t(
                        "Compose file editing is turned off on this server; rerun setup with --allow-compose-edit",
                    )
                    .into();
                    return;
                }
                None => {
                    self.notice = crate::i18n::t(
                        "This server's agent does not support Compose file editing; update tha-agent",
                    )
                    .into();
                    return;
                }
            }
        }
        let locations: Vec<Result<Location, String>> = target
            .files
            .iter()
            .map(
                |file| match server.as_ref().and_then(|name| self.machine(name)) {
                    Some(machine) => crate::remote::shell_control_path(&machine.remote)
                        .map(|control| Location::Server {
                            remote: machine.remote.clone(),
                            control,
                            path: file.to_string_lossy().into_owned(),
                        })
                        .map_err(|error| crate::model::safe_text(&format!("{error:#}"))),
                    None => Ok(Location::Local(file.clone())),
                },
            )
            .collect();
        let mut choices = Vec::new();
        for (file, location) in target.files.iter().zip(locations) {
            match location {
                Ok(location) => choices.push(Choice::ComposeFile(Box::new(EditRequest {
                    location,
                    label: crate::model::safe_text(&file.to_string_lossy()),
                }))),
                Err(error) => {
                    self.notice = error;
                    return;
                }
            }
        }
        match choices.len() {
            0 => {}
            1 => {
                if let Some(Choice::ComposeFile(request)) = choices.pop() {
                    self.edit_request = Some(*request);
                }
            }
            _ => {
                self.picker = Some(Picker {
                    title: crate::i18n::t("Which Compose file?").into(),
                    choices,
                    selected: 0,
                });
            }
        }
    }

    fn copy_text(&mut self, text: String) {
        // Terminals drop larger OSC 52 requests silently, which would look like a copy that worked.
        if text.len() > 64 * 1024 {
            self.notice = crate::i18n::t("This is too long to copy through the terminal").into();
            return;
        }
        self.clipboard_request = Some(text);
        self.notice = crate::i18n::t("Sent to the clipboard, if your terminal allows it.").into();
    }

    pub fn copy_selected(&mut self, field: CopyField) {
        let containers = self.selected_containers();
        let [container] = containers.as_slice() else {
            self.notice = crate::i18n::t("Select a single container").into();
            return;
        };
        let text = match field {
            CopyField::Id => container.id.clone(),
            CopyField::Name => container.name.clone(),
            CopyField::Image => container.image.clone(),
        };
        self.copy_text(crate::model::safe_text(&text));
    }

    /// The log view has no line cursor, so this takes what is at the bottom of what is showing.
    pub fn copy_log_line(&mut self) {
        let Some(row) = self.log_view.visible(&self.logs).pop() else {
            self.notice = crate::i18n::t("No log line is showing").into();
            return;
        };
        let line = self
            .logs
            .entries
            .iter()
            .find(|entry| entry.sequence == row.sequence)
            .map(|entry| crate::model::safe_text(&entry.text));
        match line {
            Some(line) => self.copy_text(line),
            None => self.notice = crate::i18n::t("No log line is showing").into(),
        }
    }

    /// Picks up changes to the configuration file. Only settings that are safe to change live
    /// apply; connections need a restart. A broken file changes nothing and says why.
    pub fn reload_config(&mut self) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
        let modified = std::fs::metadata(&path)
            .and_then(|data| data.modified())
            .ok();
        if modified == self.config_modified {
            return;
        }
        self.config_modified = modified;
        let loaded = crate::config::Config::load(Some(&path)).and_then(|config| {
            crate::keymap::Keymap::parse(&config.bindings)?;
            Ok(config)
        });
        match loaded {
            Ok(config) => {
                self.custom_bindings = config.bindings.clone();
                self.alert_rules = config.alerts.clone();
                self.vscode_path = config.vscode_path.clone();
                let saved = self.saved_settings;
                self.apply_base_settings(crate::settings::BaseSettings {
                    theme: config.theme,
                    keyboard: config.keyboard,
                    mouse: config.mouse,
                    log_wrap: self.log_view.wrap,
                    language: config.language,
                    remote_connect: config.remote_connect,
                    editor: config.editor,
                });
                self.apply_saved_settings(saved);
                self.notice =
                    crate::i18n::t("Configuration reloaded. Connections apply after a restart.")
                        .into();
            }
            Err(error) => {
                self.notice = crate::tf!(
                    "Configuration not reloaded, keeping the current one: {error}",
                    error = crate::model::safe_text(&format!("{error:#}"))
                );
            }
        }
    }

    /// `:upload LOCAL /container/folder`, reviewed in a dialog before anything is written.
    pub fn request_upload(&mut self, arguments: &str) {
        let Some((local, folder)) = arguments.trim().rsplit_once(char::is_whitespace) else {
            self.notice = crate::i18n::t("Use :upload LOCAL-PATH /folder/in/the/container").into();
            return;
        };
        let folder = match crate::files::clean_path(folder) {
            Ok(folder) => folder,
            Err(error) => {
                self.notice = error.to_string();
                return;
            }
        };
        let source = match self.download_source() {
            Ok(source @ crate::files::Source::Container { .. }) => source,
            Ok(_) => {
                self.notice = crate::i18n::t("Uploads go into containers; select one").into();
                return;
            }
            Err(notice) => {
                self.notice = notice;
                return;
            }
        };
        if let Err(notice) = self.downloads.open_upload(source, local, folder) {
            self.notice = notice;
        }
    }

    /// `:network-test host:port` runs in the selected container, so it is always explicit.
    pub fn request_network_test(&mut self, destination: &str) {
        let (host, port) = match crate::files::parse_destination(destination) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.notice = error.to_string();
                return;
            }
        };
        let source = match self.download_source() {
            Ok(source @ crate::files::Source::Container { .. }) => source,
            Ok(_) => {
                self.notice = crate::i18n::t("Network tests run in a container; select one").into();
                return;
            }
            Err(notice) => {
                self.notice = notice;
                return;
            }
        };
        let demo = self.demo;
        self.network_test = Some(NetworkTestView {
            source,
            host,
            port,
            result: demo.then(|| Err(crate::i18n::t("Demo mode: nothing is contacted").to_owned())),
        });
        self.network_test_request = !demo;
    }

    /// The bundle: what the open health view shows, from this container only. It never holds
    /// environment variables or commands, and redaction is a best effort that is labeled so.
    pub fn diagnostic_bundle(&self, redact: bool) -> Option<String> {
        let view = self.diagnostics.as_ref()?;
        let side = &view.main;
        let clean = |text: &str| {
            if redact {
                crate::diagnostics::redact(text)
            } else {
                text.to_owned()
            }
        };
        let engine = self.engines.get(side.target.engine)?;
        let events: Vec<_> = engine
            .events
            .newest_first()
            .filter(|event| event.key == side.target.container_key())
            .take(200)
            .map(|event| {
                serde_json::json!({
                    "at_ms": event.at_ms,
                    "event": event.label(),
                })
            })
            .collect();
        let loaded = view
            .loaded_logs
            .as_ref()
            .and_then(|loaded| match &loaded.status {
                crate::diagnostics::LogLoad::Ready(buffer) if loaded.target == side.target => {
                    Some(buffer)
                }
                _ => None,
            });
        let buffer = loaded.or_else(|| self.logs_cover(&side.target).then_some(&self.logs));
        let logs: Vec<_> = buffer
            .map(|buffer| {
                buffer
                    .entries
                    .iter()
                    .filter(|entry| entry.source != "Tha")
                    .rev()
                    .take(300)
                    .map(|entry| clean(&entry.text))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default();
        let inspection = side.evidence.as_ref().map(|(inspection, _)| {
            let endpoints: Vec<_> = inspection
                .endpoints
                .iter()
                .flatten()
                .map(|endpoint| endpoint.network.clone())
                .collect();
            serde_json::json!({
                "state": inspection.state,
                "restart_count": inspection.restart_count,
                "exit_code": inspection.exit_code,
                "oom_killed": inspection.oom_killed,
                "error": inspection.error.as_deref().map(clean),
                "started_at": inspection.started_at,
                "finished_at": inspection.finished_at,
                "network_mode": inspection.network_mode,
                "networks": endpoints,
            })
        });
        let bundle = serde_json::json!({
            "format": "tha-diagnostics-1",
            "created_ms": crate::history::now_ms(),
            "redaction": if redact { "best effort; may have missed secrets" } else { "off" },
            "container": {
                "id": side.target.id,
                "name": side.target.name,
                "image": side.container.image,
                "state": side.container.state,
                "status": side.container.status,
            },
            "inspection": inspection,
            "events": events,
            "logs": logs,
        });
        serde_json::to_string_pretty(&bundle).ok()
    }

    pub fn confirm_diagnostic_export(&mut self) {
        let Some(export) = &self.diagnostic_export else {
            return;
        };
        let (path, redact) = (export.path.clone(), export.redact);
        let destination = crate::downloads::expand_home(path.trim());
        let parent_exists = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .is_none_or(std::path::Path::is_dir);
        let problem = if path.trim().is_empty() || !parent_exists {
            Some(crate::i18n::t("The destination folder does not exist"))
        } else if destination.symlink_metadata().is_ok() {
            Some(crate::i18n::t("A file with this name already exists"))
        } else {
            None
        };
        if let Some(problem) = problem {
            if let Some(export) = &mut self.diagnostic_export {
                export.error = Some(problem.into());
            }
            return;
        }
        if let Some(bundle) = self.diagnostic_bundle(redact) {
            self.diagnostic_export_request = Some((destination, bundle));
            self.diagnostic_export = None;
        }
    }

    /// Whether this engine's events outlast Tha, which decides what the timeline can claim.
    pub fn events_saved(&self, engine: usize) -> bool {
        let Some(engine) = self.engines.get(engine) else {
            return false;
        };
        match crate::remote::parse_endpoint(&engine.connection.endpoint) {
            Some((server, _)) => self
                .machine(server)
                .and_then(|machine| machine.capabilities.as_ref())
                .is_some_and(|reported| reported.history),
            None => self.metrics_database.is_some(),
        }
    }

    pub fn open_timeline(&mut self) {
        let Some(row) = self.current().cloned() else {
            self.notice = crate::i18n::t("Select a workload first").into();
            return;
        };
        if matches!(row.scope, Scope::Machine(_)) {
            self.notice = crate::i18n::t("Select a runtime or workload inside this server").into();
            return;
        }
        self.timeline_view = Some(TimelineView {
            engine: row.engine,
            title: crate::model::safe_text(&row.title),
            containers: self.containers_for(&row),
            selected: 0,
        });
    }

    /// What the open timeline shows, flattened for selection: each entry with its incident.
    pub fn timeline_incidents(&self) -> Vec<crate::timeline::Incident> {
        let Some(view) = &self.timeline_view else {
            return Vec::new();
        };
        let Some(engine) = self.engines.get(view.engine) else {
            return Vec::new();
        };
        let keys: HashSet<String> = view.containers.iter().map(Container::key).collect();
        let events: Vec<_> = engine
            .events
            .newest_first()
            .filter(|event| keys.contains(&event.key))
            .collect();
        let points: Vec<_> = view
            .containers
            .iter()
            .map(|container| {
                let key = container.key();
                let points = engine.metrics.session_points(&key);
                (key, container.name.as_str(), points)
            })
            .collect();
        let readings: Vec<_> = points
            .iter()
            .map(|(key, name, points)| crate::timeline::Readings { key, name, points })
            .collect();
        crate::timeline::build(&events, &readings)
    }

    /// Shows logs from shortly before the selected moment, for the container it happened to.
    pub fn open_timeline_logs(&mut self) {
        let Some(view) = &self.timeline_view else {
            return;
        };
        let (engine, selected) = (view.engine, view.selected);
        let incidents = self.timeline_incidents();
        let Some(entry) = incidents
            .iter()
            .flat_map(|incident| &incident.entries)
            .nth(selected)
            .cloned()
        else {
            return;
        };
        self.timeline_view = None;
        self.reveal_container(engine, &entry.key);
        let since = entry.at_ms.div_euclid(1000) - 60;
        self.log_since = Some(since);
        self.tab = Tab::Logs;
        self.detail_focus = true;
        self.performance_focus = false;
        self.notice = crate::tf!(
            "Logs from 60 s before {name}'s entry · :logs-since off shows all · Backspace returns",
            name = entry.name
        );
    }

    /// Opens the fleet list. Saved scan results load separately, so it never waits for disk.
    pub fn open_fleet(&mut self) {
        self.fleet_view = Some(FleetView::default());
        if !self.security.enabled {
            return;
        }
        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        for engine in &self.engines {
            let Some(snapshot) = &engine.snapshot else {
                continue;
            };
            let Some(runtime) = snapshot.engine_id.clone() else {
                continue;
            };
            for container in snapshot.containers.iter().filter(|c| c.running()) {
                let Some(image) = &container.image_id else {
                    continue;
                };
                if seen.insert((engine.connection.endpoint.clone(), image.clone())) {
                    targets.push(crate::security::Target {
                        connection: engine.connection.clone(),
                        runtime: runtime.clone(),
                        image: image.clone(),
                        label: container.image.clone(),
                    });
                }
            }
        }
        self.fleet_vulnerability_request = Some(targets);
    }

    pub fn fleet_summary(&self) -> crate::fleet::Summary {
        let empty = crate::fleet::Vulnerabilities::new();
        let found = self
            .fleet_view
            .as_ref()
            .map_or(&empty, |view| &view.vulnerabilities);
        crate::fleet::summarize(self, found)
    }

    /// Jumps to the selected problem's workload and leaves the list. `Backspace` comes back to
    /// where you were. A problem with no workload stays open and says why nothing happened.
    pub fn open_fleet_selection(&mut self) {
        let selected = self.fleet_view.as_ref().map_or(0, |view| view.selected);
        let Some(problem) = self.fleet_summary().problems.into_iter().nth(selected) else {
            return;
        };
        let Some((engine, key)) = problem.target else {
            self.notice =
                crate::i18n::t("This row is a server or runtime; press . to manage servers").into();
            return;
        };
        self.fleet_view = None;
        self.reveal_container(engine, &key);
    }

    fn reveal_container(&mut self, engine: usize, key: &str) {
        self.push_navigation();
        self.machine = None;
        self.project = None;
        self.tree_query.clear();
        self.overview_mode = OverviewMode::Containers;
        let Some(tree) = self.explorer_tree(engine) else {
            return;
        };
        let mut path = Vec::new();
        if !container_path(&tree, key, &mut path) {
            return;
        }
        let Some((target, ancestors)) = path.split_last() else {
            return;
        };
        let target = target.clone();
        self.expanded.extend(ancestors.iter().cloned());
        self.refresh_tree();
        if let Some(index) = self.rows.iter().position(|row| row.key == target) {
            self.selected = index;
            self.detail_focus = false;
            self.detail_scroll = 0;
            self.performance_focus = false;
            self.tab = Tab::Overview;
        }
    }

    pub fn open_capabilities(&mut self) {
        let Some(row) = self.current() else {
            self.notice = crate::i18n::t("Select a connection to check").into();
            return;
        };
        let server = match &row.scope {
            Scope::Machine(name) if name != crate::machines::LOCAL_MACHINE => Some(name.clone()),
            _ => None,
        };
        let engine = server.is_none().then_some(row.engine);
        let title = server.clone().unwrap_or_else(|| {
            self.engines
                .get(row.engine)
                .map_or_else(String::new, |engine| engine.connection.name.clone())
        });
        self.capabilities_view = Some(CapabilitiesView {
            title: crate::model::safe_text(&title),
            engine,
            server,
            scroll: 0,
        });
    }

    pub fn pick_context_sort(&mut self) {
        use crate::context_sort::ContextSort;
        self.picker = Some(Picker {
            title: crate::i18n::t("Sort contexts · pins stay first").into(),
            choices: ContextSort::ALL
                .into_iter()
                .map(Choice::ContextSort)
                .collect(),
            selected: ContextSort::ALL
                .iter()
                .position(|mode| *mode == self.context_sort)
                .unwrap_or(0),
        });
    }

    fn reorder_contexts(&mut self) {
        let focused_engine = self.performance_engine();
        let contexts: Vec<_> = self
            .engines
            .iter()
            .enumerate()
            .map(|(index, engine)| {
                let summary = self.engine_metrics(index);
                crate::context_sort::Context {
                    engine: index,
                    name: &engine.connection.name,
                    endpoint_key: engine.connection.key(),
                    connected: engine.snapshot.is_some() && engine.error.is_none(),
                    cpu: summary
                        .cpu_percent
                        .filter(|_| summary.cpu_samples == summary.running),
                    memory: summary
                        .memory_bytes
                        .filter(|_| summary.memory_samples == summary.running),
                }
            })
            .collect();
        self.context_order =
            crate::context_sort::order(&contexts, &self.context_pins, self.context_sort);
        self.restore_performance_focus(focused_engine);
    }
}

fn resolve_preference_key(app: &App, saved: &crate::preferences::Row) -> Option<String> {
    let engines: Vec<_> = (0..app.engines.len())
        .filter(|&engine| app.preference_connection(engine) == saved.connection)
        .collect();
    (engines.len() == 1).then(|| format!("{}/{scope}", engines[0], scope = saved.scope))
}

fn find_project<'a>(node: &'a Node, project: &Project) -> Option<&'a Node> {
    if node.row.scope
        == (Scope::Project {
            namespace: project.namespace.clone(),
            project: project.name.clone(),
        })
    {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_project(child, project))
}

pub fn matches_container(scope: &Scope, c: &Container) -> bool {
    match scope {
        Scope::Machine(_) => false,
        Scope::Engine => true,
        Scope::Namespace(ns) => c.namespace.as_ref() == Some(ns),
        Scope::Project { namespace, project } | Scope::Services { namespace, project } => {
            &c.namespace == namespace && c.project.as_ref() == Some(project)
        }
        Scope::Service {
            namespace,
            project,
            service,
        } => {
            &c.namespace == namespace
                && c.project.as_ref() == Some(project)
                && c.service.as_deref().unwrap_or("Unassigned service") == service
        }
        Scope::Standalone(ns) => &c.namespace == ns && c.project.is_none(),
        Scope::Container(key) => c.key() == *key,
        Scope::Resources { project, kind } => {
            (project.is_none() || &c.project == project)
                && match kind {
                    ResourceKind::Network => !c.networks.is_empty(),
                    ResourceKind::Volume => !c.volumes.is_empty(),
                }
        }
        Scope::Resource { name, kind } => match kind {
            ResourceKind::Network => c.networks.contains(name),
            ResourceKind::Volume => c.volumes.contains(name),
        },
    }
}

fn engine_tree(index: usize, engine: &Engine) -> Node {
    let mut children = Vec::new();
    if engine.alias.is_none()
        && let Some(snapshot) = &engine.snapshot
    {
        if snapshot.namespaces.is_empty() {
            children = workload_tree(index, snapshot, None);
        } else {
            for namespace in &snapshot.namespaces {
                children.push(node(
                    index,
                    Scope::Namespace(namespace.clone()),
                    crate::tf!("Namespace: {namespace}", namespace = namespace),
                    workload_tree(index, snapshot, Some(namespace)),
                ));
            }
        }
    }
    let status = if engine.alias.is_some() {
        crate::i18n::t(" [alias]")
    } else if engine.error.is_some() {
        crate::i18n::t(" (offline)")
    } else if engine.snapshot.is_some() {
        ""
    } else {
        crate::i18n::t(" [connecting]")
    };
    node(
        index,
        Scope::Engine,
        format!(
            "{}{status}",
            crate::remote::parse_endpoint(&engine.connection.endpoint)
                .map_or(engine.connection.name.as_str(), |(_, name)| name)
        ),
        children,
    )
}

fn workload_tree(index: usize, snapshot: &Snapshot, namespace: Option<&String>) -> Vec<Node> {
    let containers: Vec<_> = snapshot
        .containers
        .iter()
        .filter(|c| c.namespace.as_ref() == namespace)
        .collect();
    let mut projects: BTreeMap<&String, BTreeMap<&str, Vec<&Container>>> = BTreeMap::new();
    for &container in &containers {
        if let Some(project) = &container.project {
            projects
                .entry(project)
                .or_default()
                .entry(container.service.as_deref().unwrap_or("Unassigned service"))
                .or_default()
                .push(container);
        }
    }
    let resource_index = ResourceIndex::new(snapshot);
    let mut nodes = Vec::new();
    for (project, services) in projects {
        let scope = Scope::Project {
            namespace: namespace.cloned(),
            project: project.clone(),
        };
        let member_count: usize = services.values().map(Vec::len).sum();
        let running = services.values().flatten().filter(|c| c.running()).count();
        let service_nodes: Vec<_> = services
            .into_iter()
            .map(|(service, instances)| {
                let running = instances.iter().filter(|c| c.running()).count();
                let title = group_title(service, running, instances.len());
                node(
                    index,
                    Scope::Service {
                        namespace: namespace.cloned(),
                        project: project.clone(),
                        service: service.into(),
                    },
                    title,
                    instances
                        .into_iter()
                        .map(|c| container_node(index, c))
                        .collect(),
                )
            })
            .collect();
        let mut project_nodes = vec![node(
            index,
            Scope::Services {
                namespace: namespace.cloned(),
                project: project.clone(),
            },
            crate::tf!("Services ({a0})", a0 = service_nodes.len()),
            service_nodes,
        )];
        if namespace.is_none() {
            project_nodes.extend(resource_nodes(
                index,
                snapshot,
                Some(project),
                &resource_index,
            ));
        }
        nodes.push(node(
            index,
            scope,
            group_title(project, running, member_count),
            project_nodes,
        ));
    }
    let standalone: Vec<_> = containers
        .iter()
        .copied()
        .filter(|c| c.project.is_none())
        .map(|c| container_node(index, c))
        .collect();
    if !standalone.is_empty() {
        nodes.push(node(
            index,
            Scope::Standalone(namespace.cloned()),
            crate::tf!("Standalone ({a0})", a0 = standalone.len()),
            standalone,
        ));
    }
    if namespace.is_none() {
        nodes.extend(resource_nodes(index, snapshot, None, &resource_index));
    }
    nodes
}

/// Which project shows which resource, built once per tree. Without it, every project scans
/// every resource and container, which takes minutes with thousands of containers.
struct ResourceIndex<'a> {
    networks: std::collections::HashMap<&'a str, Vec<&'a Resource>>,
    volumes: std::collections::HashMap<&'a str, Vec<&'a Resource>>,
    unnamespaced_projects: HashSet<Option<&'a str>>,
}

impl<'a> ResourceIndex<'a> {
    fn new(snapshot: &'a Snapshot) -> Self {
        // Nothing to place, so building the lookups would only slow every refresh down.
        if snapshot.networks.is_empty() && snapshot.volumes.is_empty() {
            return Self {
                networks: HashMap::new(),
                volumes: HashMap::new(),
                unnamespaced_projects: HashSet::new(),
            };
        }
        let mut projects_of = std::collections::HashMap::<&str, Vec<&str>>::new();
        for container in &snapshot.containers {
            if let Some(project) = &container.project {
                projects_of
                    .entry(&container.name)
                    .or_default()
                    .push(project);
            }
        }
        let by_project = |resources: &'a [Resource]| {
            let mut index = std::collections::HashMap::<&str, Vec<&Resource>>::new();
            for resource in resources {
                let mut projects: HashSet<&str> = resource.project.as_deref().into_iter().collect();
                for name in &resource.containers {
                    if let Some(found) = projects_of.get(name.as_str()) {
                        projects.extend(found.iter().copied());
                    }
                }
                for project in projects {
                    index.entry(project).or_default().push(resource);
                }
            }
            index
        };
        Self {
            networks: by_project(&snapshot.networks),
            volumes: by_project(&snapshot.volumes),
            unnamespaced_projects: snapshot
                .containers
                .iter()
                .filter(|container| container.namespace.is_none())
                .map(|container| container.project.as_deref())
                .collect(),
        }
    }
}

fn resource_nodes(
    index: usize,
    snapshot: &Snapshot,
    project: Option<&String>,
    resource_index: &ResourceIndex,
) -> Vec<Node> {
    [
        (
            ResourceKind::Network,
            crate::i18n::t("Networks"),
            &snapshot.networks,
            &resource_index.networks,
        ),
        (
            ResourceKind::Volume,
            crate::i18n::t("Volumes"),
            &snapshot.volumes,
            &resource_index.volumes,
        ),
    ]
    .into_iter()
    .filter_map(|(kind, label, resources, by_project)| {
        let selected: Vec<&Resource> = match project {
            Some(project) => by_project
                .get(project.as_str())
                .cloned()
                .unwrap_or_default(),
            None => resources
                .iter()
                .filter(|r| {
                    r.project.is_none()
                        || r.external
                        || !resource_index
                            .unnamespaced_projects
                            .contains(&r.project.as_deref())
                })
                .collect(),
        };
        let children: Vec<_> = selected
            .into_iter()
            .map(|r| {
                node(
                    index,
                    Scope::Resource {
                        name: r.name.clone(),
                        kind,
                    },
                    format!(
                        "{}{}",
                        r.name,
                        if r.external
                            || project.is_some_and(|project| r.project.as_ref() != Some(project))
                        {
                            crate::i18n::t(" [shared/external]")
                        } else {
                            ""
                        }
                    ),
                    vec![],
                )
            })
            .collect();
        if children.is_empty() {
            None
        } else {
            Some(node(
                index,
                Scope::Resources {
                    project: project.cloned(),
                    kind,
                },
                format!(
                    "{}{} ({})",
                    if project.is_none() {
                        crate::i18n::t("Shared ")
                    } else {
                        ""
                    },
                    label,
                    children.len()
                ),
                children,
            ))
        }
    })
    .collect()
}

/// A count of zero running reads as if something were running, so a stopped group says so.
fn group_title(name: &str, running: usize, total: usize) -> String {
    if running == 0 && total > 0 {
        crate::tf!("{name} [stopped]", name = name)
    } else {
        crate::tf!(
            "{name} [{running}/{total} running]",
            name = name,
            running = running,
            total = total
        )
    }
}

fn container_node(index: usize, c: &Container) -> Node {
    let mut item = node(index, Scope::Container(c.key()), c.name.clone(), vec![]);
    item.row.running = Some(c.running());
    item
}

fn container_path(node: &Node, key: &str, path: &mut Vec<String>) -> bool {
    path.push(node.row.key.clone());
    let found = matches!(&node.row.scope, Scope::Container(candidate) if candidate == key)
        || node
            .children
            .iter()
            .any(|child| container_path(child, key, path));
    if !found {
        path.pop();
    }
    found
}

fn flatten(
    node: &Node,
    depth: usize,
    expanded: &HashSet<String>,
    query: &str,
    rows: &mut Vec<Row>,
) {
    let row = |depth| {
        let mut row = node.row.clone();
        row.depth = depth;
        row
    };
    if query.is_empty() {
        rows.push(row(depth));
        if expanded.contains(&node.row.key) {
            for child in &node.children {
                flatten(child, depth + 1, expanded, "", rows);
            }
        }
        return;
    }
    if node.row.title.to_lowercase().contains(query) {
        // A matching parent exposes its complete subtree during search.
        rows.push(row(depth));
        for child in &node.children {
            flatten(child, depth + 1, expanded, "", rows);
        }
        return;
    }
    // Each node is checked once: a parent appears only if a descendant added a row.
    let start = rows.len();
    for child in &node.children {
        flatten(child, depth + 1, expanded, query, rows);
    }
    if rows.len() > start {
        rows.insert(start, row(depth));
    }
}

#[cfg(test)]
mod resource_index_tests {
    use super::*;

    fn titles(nodes: &[Node]) -> Vec<String> {
        nodes
            .iter()
            .flat_map(|group| group.children.iter().map(|child| child.row.title.clone()))
            .collect()
    }

    #[test]
    fn projects_show_owned_and_attached_resources_and_the_rest_stay_shared() {
        let resource =
            |name: &str, project: Option<&str>, external: bool, containers: &[&str]| Resource {
                name: name.into(),
                project: project.map(Into::into),
                external,
                containers: containers.iter().map(|c| (*c).into()).collect(),
                ..Default::default()
            };
        let container = |name: &str, project: Option<&str>| Container {
            name: name.into(),
            project: project.map(Into::into),
            ..Default::default()
        };
        let snapshot = Snapshot {
            containers: vec![
                container("web", Some("shop")),
                container("api", Some("blog")),
                container("solo", None),
            ],
            networks: vec![
                resource("owned-unused", Some("shop"), false, &[]),
                resource("attached", None, true, &["web"]),
                resource("orphan", Some("gone"), false, &[]),
                resource("loose", None, false, &["solo"]),
            ],
            ..Default::default()
        };
        let index = ResourceIndex::new(&snapshot);
        let shop = "shop".to_owned();
        let blog = "blog".to_owned();
        assert_eq!(
            titles(&resource_nodes(0, &snapshot, Some(&shop), &index)),
            ["owned-unused", "attached [shared/external]"]
        );
        assert!(titles(&resource_nodes(0, &snapshot, Some(&blog), &index)).is_empty());
        // A resource whose project has no containers here would otherwise be unreachable.
        assert_eq!(
            titles(&resource_nodes(0, &snapshot, None, &index)),
            ["attached [shared/external]", "orphan", "loose"]
        );
    }
}

#[cfg(test)]
mod group_title_tests {
    use super::group_title;

    #[test]
    fn a_group_with_nothing_running_says_stopped_instead_of_a_zero_count() {
        assert_eq!(group_title("db", 0, 1), "db [stopped]");
        assert_eq!(group_title("db", 0, 3), "db [stopped]");
        assert_eq!(group_title("shop", 4, 5), "shop [4/5 running]");
        assert_eq!(group_title("shop", 5, 5), "shop [5/5 running]");
    }
}

#[cfg(test)]
mod tests;
