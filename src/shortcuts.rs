//! Shared vocabulary for navigation, help and command discovery.
//!
//! This module describes actions; it never executes them. Input owners (PTYs,
//! editors and confirmations) must be handled before navigation resolution.
use crate::app::{App, OverviewMode, Tab};
use crokey::KeyCombination;
use crossterm::event::{KeyEvent, KeyEventKind};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NavigationContext {
    Explorer,
    Performance,
    Overview,
    Logs,
    Networks,
    Exec,
    Metrics,
    Layers,
    Events,
    ContainerSecurity,
    Images,
    ImageDetails,
    Volumes,
    VolumeDetails,
    SecurityImages,
    SecurityFindings,
    SecurityAdvisory,
}

impl NavigationContext {
    pub const ALL: [Self; 17] = [
        Self::Explorer,
        Self::Performance,
        Self::Overview,
        Self::Logs,
        Self::Networks,
        Self::Exec,
        Self::Metrics,
        Self::Layers,
        Self::Events,
        Self::ContainerSecurity,
        Self::Images,
        Self::ImageDetails,
        Self::Volumes,
        Self::VolumeDetails,
        Self::SecurityImages,
        Self::SecurityFindings,
        Self::SecurityAdvisory,
    ];

    pub fn of(app: &App) -> Self {
        match app.overview_mode {
            OverviewMode::Images | OverviewMode::Volumes => {
                let details = app
                    .inventory_views
                    .get(&(app.inventory_engine, app.overview_mode))
                    .is_some_and(|view| view.details);
                match (app.overview_mode, details) {
                    (OverviewMode::Images, false) => Self::Images,
                    (OverviewMode::Images, true) => Self::ImageDetails,
                    (_, false) => Self::Volumes,
                    (_, true) => Self::VolumeDetails,
                }
            }
            OverviewMode::Security if app.security.expanded => Self::SecurityAdvisory,
            OverviewMode::Security if app.detail_focus => Self::SecurityFindings,
            OverviewMode::Security => Self::SecurityImages,
            OverviewMode::Containers if app.performance_focus => Self::Performance,
            OverviewMode::Containers if !app.detail_focus => Self::Explorer,
            OverviewMode::Containers => match app.tab {
                Tab::Overview => Self::Overview,
                Tab::Logs => Self::Logs,
                Tab::Networks => Self::Networks,
                Tab::Exec => Self::Exec,
                Tab::History => Self::Metrics,
                Tab::Layers => Self::Layers,
                Tab::Events => Self::Events,
                Tab::Security => Self::ContainerSecurity,
            },
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Explorer => "Explorer",
            Self::Performance => "Performance",
            Self::Overview => "Overview",
            Self::Logs => "Logs",
            Self::Networks => "Networks",
            Self::Exec => "Exec navigation",
            Self::Metrics => "Metrics",
            Self::Layers => "Layers",
            Self::Events => "Events",
            Self::ContainerSecurity => "Container Security",
            Self::Images => "Images",
            Self::ImageDetails => "Image details",
            Self::Volumes => "Volumes",
            Self::VolumeDetails => "Volume details",
            Self::SecurityImages => "Security images",
            Self::SecurityFindings => "Security findings",
            Self::SecurityAdvisory => "Security advisory",
        }
    }

    fn containers(self) -> bool {
        matches!(
            self,
            Self::Explorer
                | Self::Performance
                | Self::Overview
                | Self::Logs
                | Self::Networks
                | Self::Exec
                | Self::Metrics
                | Self::Layers
                | Self::Events
                | Self::ContainerSecurity
        )
    }

    fn inventory(self) -> bool {
        matches!(
            self,
            Self::Images | Self::ImageDetails | Self::Volumes | Self::VolumeDetails
        )
    }

    pub fn security(self) -> bool {
        matches!(
            self,
            Self::ContainerSecurity
                | Self::SecurityImages
                | Self::SecurityFindings
                | Self::SecurityAdvisory
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Containers,
    Explorer,
    Workloads,
    TreeOrLogs,
    TreeOrTabs,
    DetailTabs,
    Logs,
    Metrics,
    Inventory,
    Images,
    Security,
    SecurityImages,
    Zoom,
    Performance,
    FocusedPerformance,
}

impl Scope {
    pub fn includes(self, context: NavigationContext) -> bool {
        use NavigationContext as C;
        match self {
            Self::Global => true,
            Self::Containers => context.containers(),
            Self::Explorer => context == C::Explorer,
            Self::Workloads => matches!(context, C::Explorer | C::Performance),
            Self::TreeOrLogs => matches!(context, C::Explorer | C::Logs),
            // Metrics keeps these keys for panning; see DetailTabs for its way out.
            Self::TreeOrTabs => {
                context == C::Explorer
                    || (context != C::Metrics && Self::DetailTabs.includes(context))
            }
            Self::DetailTabs => matches!(
                context,
                C::Overview
                    | C::Logs
                    | C::Networks
                    | C::Exec
                    | C::Metrics
                    | C::Layers
                    | C::Events
                    | C::ContainerSecurity
            ),
            Self::Logs => context == C::Logs,
            Self::Metrics => context == C::Metrics,
            Self::Inventory => context.inventory(),
            Self::Images => matches!(context, C::Images | C::ImageDetails | C::SecurityImages),
            Self::Security => matches!(
                context,
                C::SecurityImages | C::SecurityFindings | C::SecurityAdvisory
            ),
            Self::SecurityImages => context == C::SecurityImages,
            Self::Zoom => {
                matches!(
                    context,
                    C::Explorer
                        | C::Performance
                        | C::Overview
                        | C::Logs
                        | C::Networks
                        | C::Exec
                        | C::Layers
                        | C::Events
                        | C::Metrics
                        | C::Images
                        | C::ImageDetails
                        | C::Volumes
                        | C::VolumeDetails
                ) || context.security()
            }
            Self::Performance => matches!(
                context,
                C::Explorer
                    | C::Performance
                    | C::Overview
                    | C::Logs
                    | C::Networks
                    | C::Exec
                    | C::Metrics
                    | C::Layers
                    | C::Events
                    | C::ContainerSecurity
            ),
            Self::FocusedPerformance => context == C::Performance,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// crokey syntax, also used as the displayed key. The first key is primary.
    pub key: &'static str,
    pub compatibility: bool,
}

#[derive(Debug)]
pub struct ActionInfo {
    pub id: ActionId,
    pub label: &'static str,
    pub commands: &'static [&'static str],
    pub bindings: &'static [Binding],
    pub scope: Scope,
    pub hint_priority: u8,
    pub security_module: bool,
}

// Defining ids and metadata together prevents orphaned ids or duplicate records.
macro_rules! catalog {
    ($( $id:ident, $label:literal, [$($command:literal),*], $scope:ident,
        $priority:literal, $security:literal, [$($key:literal),*], [$($alias:literal),*]; )*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ActionId { $($id),* }
        pub const ACTIONS: &[ActionInfo] = &[$(ActionInfo {
            id: ActionId::$id, label: $label, commands: &[$($command),*],
            bindings: &[$(Binding { key: $key, compatibility: false },)*
                $(Binding { key: $alias, compatibility: true },)*],
            scope: Scope::$scope, hint_priority: $priority, security_module: $security,
        }),*];
    };
}

catalog! {
    Noop, "Disabled key", [], Global, 40, false, [], [];
    Help, "Help", ["help"], Global, 0, false, ["?"], ["F1"];
    Views, "Views", ["views"], Global, 1, false, ["v"], [];
    Commands, "Commands", [], Global, 2, false, [":"], [];
    Back, "Back / quit from home", [], Global, 3, false, ["q"], [];
    Escape, "Cancel / restore / back", [], Global, 30, false, ["esc"], [];
    Down, "Move / scroll down", [], Global, 30, false, ["j"], ["down"];
    Up, "Move / scroll up", [], Global, 30, false, ["k"], ["up"];
    PageDown, "Page down", [], Global, 40, false, ["pagedown"], [];
    PageUp, "Page up", [], Global, 40, false, ["pageup"], [];
    First, "First (gg in tree and inventories)", [], Global, 40, false, ["g"], ["home"];
    Last, "Last / resume log following", [], Global, 40, false, ["shift-g"], ["end"];
    Open, "Open / inspect", [], Global, 4, false, ["enter"], [];
    FocusNext, "Next pane", [], Containers, 5, false, ["tab"], [];
    FocusPrevious, "Previous pane", [], Containers, 40, false, ["shift-tab"], [];
    SecurityFocusNext, "Images / findings pane", [], Security, 5, true, ["tab"], ["shift-tab"];
    PreviousView, "Previous overview", [], Global, 40, false, ["ctrl-left"], ["shift-left", "F7"];
    NextView, "Next overview", [], Global, 40, false, ["ctrl-right"], ["shift-right", "F8"];
    ContainersView, "Containers view", [], Global, 40, false, [], ["alt-1"];
    ImagesView, "Images view", ["images"], Global, 40, false, [], ["alt-2"];
    VolumesView, "Volumes view", ["volumes"], Global, 40, false, [], ["alt-3"];
    SecurityView, "Security view", [], Global, 40, true, [], ["alt-4"];
    Overview, "Workload overview", ["overview"], Global, 30, false, ["1"], [];
    Logs, "Logs", ["logs"], Global, 30, false, ["2"], [];
    Networks, "Network membership", ["networks"], Global, 30, false, ["3"], [];
    Exec, "Exec tab (Enter resumes input)", [], Global, 30, false, ["4"], [];
    Metrics, "Container metrics", ["metrics", "history"], Global, 30, false, ["6"], [];
    Layers, "Image layers (read-only)", ["layers"], Global, 30, false, ["7"], [];
    Security, "Selected image security", ["security"], Global, 30, true, ["8"], [];
    Events, "Container events timeline", ["events"], Global, 30, false, ["5"], [];
    Search, "Search", [], Global, 6, false, ["/"], [];
    Left, "Collapse / parent; previous tab; scroll unwrapped logs", [], TreeOrTabs, 40, false, ["h"], ["left"];
    Right, "Expand / child; next tab; scroll unwrapped logs", [], TreeOrTabs, 40, false, ["l"], ["right"];
    PreviousTab, "Previous detail tab", [], DetailTabs, 40, false, ["shift-h"], [];
    NextTab, "Next detail tab", [], DetailTabs, 40, false, ["shift-l"], [];
    PreviousRuntime, "Previous runtime", [], Inventory, 40, false, ["left"], [];
    NextRuntime, "Next runtime", [], Inventory, 40, false, ["right"], [];
    SecurityPreviousRuntime, "Previous runtime", [], SecurityImages, 40, true, ["left"], [];
    SecurityNextRuntime, "Next runtime", [], SecurityImages, 40, true, ["right"], [];
    ToggleBranch, "Toggle explorer branch", [], Explorer, 10, false, ["space"], [];
    PerformanceSpace, "Close Performance", [], FocusedPerformance, 40, false, ["space"], [];
    HalfPageDown, "Half-page down", [], Containers, 40, false, ["ctrl-d"], [];
    HalfPageUp, "Half-page up", [], Containers, 40, false, ["ctrl-u"], [];
    NextIssue, "Next possible issue in the logs", ["next-issue"], Logs, 25, false, ["i"], [];
    PreviousIssue, "Previous possible issue in the logs", ["previous-issue"], Logs, 40, false, ["shift-i"], [];
    NextMatch, "Next match", [], TreeOrLogs, 25, false, ["n"], [];
    PreviousMatch, "Previous match", [], TreeOrLogs, 40, false, ["shift-n"], [];
    WorkspaceBack, "Previous workspace", ["back"], Containers, 40, false, ["backspace"], [];
    AllRuntimes, "All runtimes", ["all"], Containers, 40, false, [], [];
    OpenExec, "Open container or server shell", ["exec"], Containers, 8, false, ["e"], [];
    Actions, "Contextual actions and disabled reasons", ["actions"], Workloads, 7, false, ["a"], [];
    Start, "Start (confirmation required)", [], Workloads, 20, false, ["shift-s"], [];
    Stop, "Stop (confirmation required)", [], Workloads, 20, false, ["s"], [];
    Restart, "Restart (confirmation required)", [], Workloads, 20, false, ["r"], [];
    DeleteContainer, "Delete stopped containers (type name to confirm)", [], Workloads, 20, false, ["d"], [];
    Diagnose, "Why is this unhealthy? (read-only evidence)", ["diagnose"], Containers, 40, false, [], [];
    NetworkDiagnose, "Troubleshoot network (read-only, no probes)", ["network-diagnose"], Containers, 40, false, [], [];
    Performance, "Performance", ["performance"], Performance, 10, false, ["p"], [];
    PerformanceScope, "Runtime / container Performance scope", [], Performance, 25, false, ["shift-p"], [];
    SortContexts, "Sort contexts", ["sort-contexts"], Containers, 25, false, ["o"], [];
    SavedViews, "Open a saved view", ["saved-views"], Containers, 40, false, [], [];
    Upload, "Copy a local file or folder into a container", ["upload"], Containers, 40, false, [], [];
    NetworkTest, "Test a connection from inside a container", ["network-test"], Containers, 40, false, [], [];
    Pause, "Pause (confirmation required)", ["pause"], Workloads, 40, false, [], [];
    Unpause, "Unpause (confirmation required)", ["unpause"], Workloads, 40, false, [], [];
    Kill, "Kill immediately (confirmation required)", ["kill"], Workloads, 40, false, [], [];
    CopyId, "Copy the container ID", ["copy-id"], Containers, 40, false, [], [];
    CopyName, "Copy the container name", ["copy-name"], Containers, 40, false, [], [];
    CopyImage, "Copy the image name", ["copy-image"], Containers, 40, false, [], [];
    CopyLogLine, "Copy the newest visible log line", ["copy-log-line"], Containers, 40, false, [], [];
    ComposeEdit, "Edit the Compose file in your editor", ["compose-edit"], Containers, 40, false, [], [];
    Alerts, "Show alerts", ["alerts"], Containers, 15, false, [], [];
    Timeline, "Incidents for the selected workload", ["timeline"], Containers, 15, false, [], [];
    Fleet, "What needs attention across all connections", ["fleet"], Containers, 15, false, [], [];
    DownloadManager, "Browse and download files", ["download-manager"], Containers, 40, false, [], [];
    Download, "Download a file or folder by path", ["download"], Containers, 40, false, [], [];
    Capabilities, "Show what this connection can do", ["capabilities"], Containers, 40, false, [], [];
    SaveView, "Save this view under a name", ["save-view"], Containers, 40, false, [], [];
    OpenView, "Open a saved view by name", ["open-view"], Containers, 40, false, [], [];
    DeleteView, "Delete a saved view", ["delete-view"], Containers, 40, false, [], [];
    Zoom, "Expand / restore focused view", [], Zoom, 9, false, ["z"], [];
    Wrap, "Wrap logs", ["wrap"], Logs, 7, false, ["w"], [];
    Follow, "Follow logs", [], Logs, 8, false, ["f"], [];
    FilterLogs, "Filter retained logs by text", ["filter-logs"], Logs, 9, false, ["F3"], [];
    AdvancedLogFilter, "Advanced JSON log filter", ["filter-logs-advanced"], Logs, 40, false, [], [];
    ClearLogFilter, "Clear text and advanced log filters", ["clear-log-filter"], Logs, 40, false, [], [];
    ExportLogs, "Export retained logs (may contain secrets)", ["export-logs"], Logs, 40, false, [], [];
    ReconnectLogs, "Reload logs now (streams also reconnect automatically)", ["reconnect-logs"], Logs, 40, false, [], [];
    LogsSince, "Load logs from a start time", ["logs-since"], Logs, 12, false, [], [];
    ComposeUpdate, "Compose pull, then up -d (confirmation required)", ["compose-update"], Workloads, 40, false, [], [];
    ComposePull, "Compose pull (confirmation required)", ["compose-pull"], Workloads, 40, false, [], [];
    ComposeUp, "Compose up -d (confirmation required)", ["compose-up"], Workloads, 40, false, [], [];
    ComposeRecreate, "Compose force recreate (confirmation required)", ["compose-recreate"], Workloads, 40, false, [], [];
    ComposeDown, "Compose down, keeping volumes (type name to confirm)", ["compose-down"], Workloads, 40, false, [], [];
    ComposeOutput, "Last Compose output", ["compose-output"], Global, 40, false, [], [];
    PrettyJson, "Pretty-print JSON logs", ["json"], Logs, 8, false, ["shift-j"], [];
    ExecSessions, "Retained shell sessions", ["exec-sessions"], Containers, 12, false, ["F6"], [];
    CloseExec, "Close shell session, not container", ["close-exec"], Containers, 40, false, [], [];
    PreviousRange, "Shorter metrics time range", [], Metrics, 12, false, ["["], [];
    NextRange, "Longer metrics time range", [], Metrics, 12, false, ["]"], [];
    MetricsLeft, "Pan / inspect earlier metrics", [], Metrics, 25, false, ["h"], ["left"];
    MetricsRight, "Pan / inspect later metrics", [], Metrics, 25, false, ["l"], ["right"];
    MetricsCursor, "Toggle metrics cursor", [], Metrics, 15, false, ["c"], [];
    MetricsLive, "Live metrics", [], Metrics, 15, false, ["f"], [];
    Mark, "Mark unused resource for cleanup", [], Inventory, 7, false, ["space"], [];
    MarkUnused, "Mark visible unused resources", [], Inventory, 15, false, ["u"], [];
    ClearMarks, "Clear resource and scan marks", [], Inventory, 15, false, ["c"], [];
    RemoveResources, "Delete unused resources (confirmation required)", [], Inventory, 10, false, ["d"], ["delete"];
    PruneResources, "Prune all unreferenced resources (confirmation required)", ["prune"], Inventory, 11, false, ["p"], [];
    InventoryFilter, "Filter inventory", [], Inventory, 8, false, ["f"], [];
    InventorySort, "Sort inventory", [], Inventory, 9, false, ["s"], [];
    RefreshInventory, "Refresh all engines", [], Inventory, 25, false, ["r"], [];
    CheckUpdates, "Compare visible images with their registries", ["check-updates"], Images, 12, false, ["shift-u"], [];
    PullImage, "Pull the selected image's tags", ["pull-image"], Images, 13, false, [], [];
    CheckImage, "Check image for scanning", [], Images, 10, true, ["m"], [];
    CheckImages, "Check all visible images for scanning", [], Images, 15, true, ["shift-m"], [];
    CheckSecurityImage, "Check image for scanning", [], SecurityImages, 10, true, ["space"], [];
    ClearScanChecks, "Clear scan checks", [], SecurityImages, 15, true, ["c"], [];
    ScanImage, "Scan checked or selected images", ["scan-image"], Security, 7, true, ["s"], [];
    ScanImages, "Scan checked images", ["scan-images"], Security, 8, true, ["shift-s"], [];
    CancelScan, "Cancel security queue", ["cancel-scan"], Security, 9, true, ["x"], [];
    UpdateSecurity, "Update scanner database and rescan", ["update-security-db"], Security, 40, true, [], [];
    ReloadLayers, "Retry image layers lookup", ["reload-layers"], Containers, 40, false, [], [];
    Refresh, "Refresh all engines", ["refresh"], Global, 40, false, [], [];
    ActionResults, "Background lifecycle progress and results", ["action-results"], Global, 40, false, [], [];
    RemovalResults, "Last cleanup results", ["removal-results"], Global, 40, false, [], [];
    Monochrome, "Toggle terminal colors", ["monochrome"], Global, 40, false, [], [];
    Settings, "Settings: theme, language, keyboard layout, mouse", ["settings"], Global, 25, false, [","], [];
    Servers, "Servers: add, edit or remove remote servers", ["servers"], Global, 26, false, ["."], [];
    ToggleConnection, "Connect or disconnect the selected server", ["toggle-connection"], Containers, 41, false, ["shift-c"], [];
    Quit, "Quit Tha", ["quit", "q"], Global, 40, false, ["ctrl-c"], [];
}

impl ActionId {
    pub fn info(self) -> &'static ActionInfo {
        ACTIONS
            .iter()
            .find(|info| info.id == self)
            .expect("catalog action")
    }

    pub fn lifecycle(self) -> Option<crate::actions::Action> {
        use crate::actions::Action;
        match self {
            Self::Start => Some(Action::Start),
            Self::Stop => Some(Action::Stop),
            Self::Restart => Some(Action::Restart),
            Self::DeleteContainer => Some(Action::Delete),
            Self::Pause => Some(Action::Pause),
            Self::Unpause => Some(Action::Unpause),
            Self::Kill => Some(Action::Kill),
            _ => None,
        }
    }

    /// Stable configuration names for every action in the shared catalog.
    /// Bindings only apply in navigation; input owners and confirmations go first.
    pub fn config_name(self) -> &'static str {
        match self {
            Self::Noop => "none",
            Self::Help => "help",
            Self::Views => "views",
            Self::Commands => "commands",
            Self::Back => "back",
            Self::Escape => "escape",
            Self::Down => "down",
            Self::Up => "up",
            Self::PageDown => "page-down",
            Self::PageUp => "page-up",
            Self::First => "first",
            Self::Last => "last",
            Self::Open => "open",
            Self::FocusNext => "focus-next",
            Self::FocusPrevious => "focus-previous",
            Self::SecurityFocusNext => "security-focus-next",
            Self::PreviousView => "previous-view",
            Self::NextView => "next-view",
            Self::ContainersView => "containers-view",
            Self::ImagesView => "images-view",
            Self::VolumesView => "volumes-view",
            Self::SecurityView => "security-view",
            Self::Overview => "overview",
            Self::Logs => "logs",
            Self::Networks => "networks",
            Self::Exec => "exec",
            Self::Metrics => "metrics",
            Self::Layers => "layers",
            Self::Security => "security",
            Self::Search => "search",
            Self::Left => "left",
            Self::Right => "right",
            Self::PreviousTab => "previous-tab",
            Self::NextTab => "next-tab",
            Self::PreviousRuntime => "previous-runtime",
            Self::NextRuntime => "next-runtime",
            Self::SecurityPreviousRuntime => "security-previous-runtime",
            Self::SecurityNextRuntime => "security-next-runtime",
            Self::ToggleBranch => "toggle-branch",
            Self::PerformanceSpace => "performance-close",
            Self::HalfPageDown => "half-page-down",
            Self::HalfPageUp => "half-page-up",
            Self::NextIssue => "next-issue",
            Self::PreviousIssue => "previous-issue",
            Self::NextMatch => "next-match",
            Self::PreviousMatch => "previous-match",
            Self::WorkspaceBack => "workspace-back",
            Self::AllRuntimes => "all-runtimes",
            Self::OpenExec => "open-exec",
            Self::Actions => "actions",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::DeleteContainer => "delete-container",
            Self::Performance => "performance",
            Self::PerformanceScope => "performance-scope",
            Self::SortContexts => "sort-contexts",
            Self::SavedViews => "saved-views",
            Self::Upload => "upload",
            Self::NetworkTest => "network-test",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
            Self::Kill => "kill",
            Self::CopyId => "copy-id",
            Self::CopyName => "copy-name",
            Self::CopyImage => "copy-image",
            Self::CopyLogLine => "copy-log-line",
            Self::ComposeEdit => "compose-edit",
            Self::Alerts => "alerts",
            Self::Timeline => "timeline",
            Self::Fleet => "fleet",
            Self::DownloadManager => "download-manager",
            Self::Download => "download",
            Self::Capabilities => "capabilities",
            Self::SaveView => "save-view",
            Self::OpenView => "open-view",
            Self::DeleteView => "delete-view",
            Self::Zoom => "zoom",
            Self::Wrap => "wrap",
            Self::Follow => "follow",
            Self::Diagnose => "diagnose",
            Self::NetworkDiagnose => "network-diagnose",
            Self::FilterLogs => "filter-logs",
            Self::AdvancedLogFilter => "filter-logs-advanced",
            Self::ClearLogFilter => "clear-log-filter",
            Self::ExportLogs => "export-logs",
            Self::ReconnectLogs => "reconnect-logs",
            Self::LogsSince => "logs-since",
            Self::PrettyJson => "json",
            Self::Events => "events",
            Self::ComposeUpdate => "compose-update",
            Self::ComposePull => "compose-pull",
            Self::ComposeUp => "compose-up",
            Self::ComposeRecreate => "compose-recreate",
            Self::ComposeDown => "compose-down",
            Self::ComposeOutput => "compose-output",
            Self::CheckUpdates => "check-updates",
            Self::PullImage => "pull-image",
            Self::ExecSessions => "exec-sessions",
            Self::CloseExec => "close-exec",
            Self::PreviousRange => "previous-range",
            Self::NextRange => "next-range",
            Self::MetricsLeft => "metrics-left",
            Self::MetricsRight => "metrics-right",
            Self::MetricsCursor => "metrics-cursor",
            Self::MetricsLive => "metrics-live",
            Self::Mark => "mark",
            Self::MarkUnused => "mark-unused",
            Self::ClearMarks => "clear-marks",
            Self::RemoveResources => "remove-resources",
            Self::PruneResources => "prune-resources",
            Self::InventoryFilter => "inventory-filter",
            Self::InventorySort => "inventory-sort",
            Self::RefreshInventory => "refresh-inventory",
            Self::CheckImage => "check-image",
            Self::CheckImages => "check-images",
            Self::CheckSecurityImage => "check-security-image",
            Self::ClearScanChecks => "clear-scan-checks",
            Self::ScanImage => "scan-image",
            Self::ScanImages => "scan-images",
            Self::CancelScan => "cancel-scan",
            Self::UpdateSecurity => "update-security-db",
            Self::ReloadLayers => "reload-layers",
            Self::Refresh => "refresh",
            Self::ActionResults => "action-results",
            Self::RemovalResults => "removal-results",
            Self::Monochrome => "monochrome",
            Self::Settings => "settings",
            Self::Servers => "servers",
            Self::ToggleConnection => "toggle-connection",
            Self::Quit => "quit",
        }
    }

    pub fn configurable(name: &str) -> Option<Self> {
        ACTIONS
            .iter()
            .find(|info| info.id.config_name() == name)
            .map(|info| info.id)
    }
}

pub fn command(name: &str) -> Option<ActionId> {
    ACTIONS
        .iter()
        .find(|info| {
            info.commands
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(name))
        })
        .map(|info| info.id)
}

static DEFAULT_KEYS: LazyLock<Vec<(KeyCombination, &'static ActionInfo, &'static Binding)>> =
    LazyLock::new(|| {
        ACTIONS
            .iter()
            .flat_map(|info| {
                info.bindings.iter().map(move |binding| {
                    (
                        binding
                            .key
                            .parse::<KeyCombination>()
                            .expect("catalog key syntax")
                            .normalized(),
                        info,
                        binding,
                    )
                })
            })
            .collect()
    });

pub fn default_action(
    context: NavigationContext,
    event: KeyEvent,
    security: bool,
) -> Option<ActionId> {
    if event.kind == KeyEventKind::Release {
        return None;
    }
    let key = KeyCombination::from(event).normalized();
    DEFAULT_KEYS
        .iter()
        .find(|(candidate, info, _)| {
            *candidate == key
                && info.scope.includes(context)
                && (!info.security_module || security)
                && (info.id.lifecycle().is_none() || event.kind == KeyEventKind::Press)
        })
        .map(|(_, info, _)| info.id)
}

pub fn resolve(
    context: NavigationContext,
    event: KeyEvent,
    keymap: &crate::keymap::Keymap,
    security: bool,
) -> Option<ActionId> {
    if event.kind == KeyEventKind::Release {
        return None;
    }
    keymap
        .action(event)
        .map(|action| match (action, context) {
            (ActionId::Left, NavigationContext::Metrics) => ActionId::MetricsLeft,
            (ActionId::Right, NavigationContext::Metrics) => ActionId::MetricsRight,
            (ActionId::Left, c) if c.inventory() => ActionId::PreviousRuntime,
            (ActionId::Right, c) if c.inventory() => ActionId::NextRuntime,
            (ActionId::Left, NavigationContext::SecurityImages) => {
                ActionId::SecurityPreviousRuntime
            }
            (ActionId::Right, NavigationContext::SecurityImages) => ActionId::SecurityNextRuntime,
            (
                ActionId::FocusNext | ActionId::FocusPrevious,
                NavigationContext::SecurityImages
                | NavigationContext::SecurityFindings
                | NavigationContext::SecurityAdvisory,
            ) => ActionId::SecurityFocusNext,
            _ => action,
        })
        .or_else(|| default_action(context, event, security))
        .filter(|action| action.lifecycle().is_none() || event.kind == KeyEventKind::Press)
}

/// Reasons are read-only projections. Execution must still revalidate state.
pub fn unavailable(app: &App, action: ActionId) -> Option<String> {
    use ActionId as A;
    if action.info().security_module && !app.security.enabled {
        return Some(crate::i18n::t("Security module is disabled").into());
    }
    if let Some(lifecycle) = action.lifecycle() {
        return app.current().map_or_else(
            || Some(crate::i18n::t("Select a workload").into()),
            |row| app.action_availability(lifecycle, &row.key).err(),
        );
    }
    match action {
        A::Metrics if app.performance_container().is_none() => {
            Some(crate::i18n::t("Select a container").into())
        }
        A::ScanImage | A::ScanImages | A::UpdateSecurity | A::CancelScan
            if app.overview_mode == crate::app::OverviewMode::Containers =>
        {
            Some(crate::i18n::t("Scans run from the Security view").into())
        }
        A::Exec if !app.exec_tab_available() => {
            Some(crate::i18n::t("Select a running container or service").into())
        }
        A::Layers if !app.layers_available() => {
            Some(crate::i18n::t("Select a container or single-container service").into())
        }
        A::Actions if app.current().is_none() => Some(crate::i18n::t("Select a workload").into()),
        A::ScanImage | A::ScanImages | A::UpdateSecurity if app.demo => {
            Some(crate::i18n::t("Scans are disabled in demo mode").into())
        }
        A::ScanImage | A::ScanImages | A::UpdateSecurity if app.security.busy => {
            Some(crate::i18n::t("A security queue is already running").into())
        }
        A::CancelScan if !app.security.busy => {
            Some(crate::i18n::t("No active security queue").into())
        }
        A::ExportLogs if app.tab != Tab::Logs || app.log_scope.is_none() => {
            Some(crate::i18n::t("Open and review Logs before exporting").into())
        }
        A::ExportLogs if app.log_export_busy => {
            Some(crate::i18n::t("Another export is running").into())
        }
        A::RemovalResults if app.inventory_removal_report.is_none() => {
            Some(crate::i18n::t("No cleanup results yet").into())
        }
        A::ActionResults if app.action_jobs.is_empty() => {
            Some(crate::i18n::t("No lifecycle results yet").into())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn overview_navigation_defaults_to_control_arrows_and_keeps_compatibility_aliases() {
        for context in NavigationContext::ALL {
            for (code, action) in [
                (crossterm::event::KeyCode::Left, ActionId::PreviousView),
                (crossterm::event::KeyCode::Right, ActionId::NextView),
            ] {
                for modifiers in [
                    crossterm::event::KeyModifiers::CONTROL,
                    crossterm::event::KeyModifiers::SHIFT,
                ] {
                    assert_eq!(
                        default_action(context, KeyEvent::new(code, modifiers), true),
                        Some(action)
                    );
                }
            }
        }
    }

    #[test]
    fn configured_lifecycle_bindings_ignore_repeat_and_release_events() {
        let keymap = crate::keymap::Keymap::parse(&std::collections::BTreeMap::from([(
            "alt-s".into(),
            "stop".into(),
        )]))
        .unwrap();
        let event = KeyEvent::new(
            crossterm::event::KeyCode::Char('s'),
            crossterm::event::KeyModifiers::ALT,
        );
        assert_eq!(
            resolve(NavigationContext::Explorer, event, &keymap, true),
            Some(ActionId::Stop)
        );
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            assert_eq!(
                resolve(
                    NavigationContext::Explorer,
                    KeyEvent { kind, ..event },
                    &keymap,
                    true
                ),
                None
            );
        }
    }

    #[test]
    fn catalog_has_unique_ids_commands_and_contextual_keys() {
        let mut ids = HashSet::new();
        let mut commands = HashSet::new();
        for info in ACTIONS {
            assert!(ids.insert(info.id), "{:?}", info.id);
            for command in info.commands {
                assert!(commands.insert(command), "{command}");
            }
        }
        for context in NavigationContext::ALL {
            let mut keys = HashSet::new();
            for (key, info, _) in DEFAULT_KEYS
                .iter()
                .filter(|(_, info, _)| info.scope.includes(context))
            {
                assert!(keys.insert(*key), "{context:?}: {} ({:?})", key, info.id);
            }
        }
    }

    #[test]
    fn cleanup_marks_work_with_the_details_panel_open() {
        for context in [
            NavigationContext::Images,
            NavigationContext::ImageDetails,
            NavigationContext::Volumes,
            NavigationContext::VolumeDetails,
        ] {
            for (key, action) in [
                ("space", ActionId::Mark),
                ("u", ActionId::MarkUnused),
                ("c", ActionId::ClearMarks),
            ] {
                let event: KeyEvent = key.parse::<KeyCombination>().unwrap().into();
                assert_eq!(default_action(context, event, true), Some(action));
            }
        }
    }

    #[test]
    fn catalog_retains_every_existing_command() {
        for (name, _) in crate::ui::COMMANDS {
            assert!(command(name).is_some(), "{name}");
        }
        assert_eq!(command("history"), command("metrics"));
        assert_eq!(command("q"), command("quit"));
        assert_eq!(command("views"), Some(ActionId::Views));
        assert_eq!(command("HELP"), Some(ActionId::Help));
    }

    #[test]
    fn defaults_disambiguate_reused_keys_and_reject_release_events() {
        let s: KeyEvent = "s".parse::<KeyCombination>().unwrap().into();
        assert_eq!(
            default_action(NavigationContext::Explorer, s, true),
            Some(ActionId::Stop)
        );
        assert_eq!(
            default_action(NavigationContext::Images, s, true),
            Some(ActionId::InventorySort)
        );
        assert_eq!(
            default_action(NavigationContext::SecurityImages, s, true),
            Some(ActionId::ScanImage)
        );
        assert_eq!(default_action(NavigationContext::Logs, s, true), None);
        assert_eq!(
            default_action(
                NavigationContext::Explorer,
                KeyEvent {
                    kind: KeyEventKind::Release,
                    ..s
                },
                true
            ),
            None
        );
        assert_eq!(
            default_action(
                NavigationContext::Explorer,
                KeyEvent {
                    kind: KeyEventKind::Repeat,
                    ..s
                },
                true
            ),
            None
        );
    }
}
