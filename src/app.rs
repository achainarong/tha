use crate::{
    config::Connection,
    logs::LogBuffer,
    logs::view::LogView,
    model::{Condition, Container, Snapshot, Tone},
    workspace::Project,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

#[derive(Debug)]
pub struct Engine {
    pub connection: Connection,
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
    pub alias: Option<String>,
    pub metrics: crate::metrics::Store,
    pub metrics_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Network,
    Volume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
    Command,
    LogFilter,
    ExportLogs,
}

#[derive(Clone)]
struct NavigationState {
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
}

pub struct Picker {
    pub title: String,
    pub choices: Vec<Choice>,
    pub selected: usize,
}

pub struct App {
    pub engines: Vec<Engine>,
    pub context_order: Vec<usize>,
    pub context_pins: Vec<String>,
    pub context_sort: crate::context_sort::ContextSort,
    pub keymap: crate::keymap::Keymap,
    pub custom_bindings: std::collections::BTreeMap<String, String>,
    pub preferences_path: Option<PathBuf>,
    pending_preferences: Option<crate::preferences::Preferences>,
    /// Machine identity is provider + machine name, never its temporary socket path.
    podman_machines: BTreeMap<String, usize>,
    pub expanded: HashSet<String>,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub tree_offset: usize,
    pub detail_focus: bool,
    pub performance_open: bool,
    pub performance_focus: bool,
    pub performance_scroll: usize,
    pub detail_scroll: usize,
    pub log_view: LogView,
    pub log_filter_text: String,
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
    pub log_generation: u64,
    pub notice: String,
    pub pending_g: bool,
    pub demo: bool,
    pub metrics_refresh_seconds: u64,
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
    pub action_confirmation: Option<crate::actions::Confirmation>,
    pub action_request: Option<crate::actions::Request>,
    pub action_progress: Option<crate::actions::Progress>,
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
            context_order,
            context_pins: Vec::new(),
            context_sort: Default::default(),
            keymap: Default::default(),
            custom_bindings: Default::default(),
            preferences_path: None,
            pending_preferences: None,
            podman_machines: BTreeMap::new(),
            engines: connections
                .into_iter()
                .map(|connection| Engine {
                    connection,
                    snapshot: None,
                    error: None,
                    alias: None,
                    metrics: Default::default(),
                    metrics_revision: 0,
                })
                .collect(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: 0,
            tree_offset: 0,
            detail_focus: false,
            performance_open: false,
            performance_focus: false,
            performance_scroll: 0,
            detail_scroll: 0,
            log_view: LogView::new(true),
            log_filter_text: "{}".into(),
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
            log_generation: 0,
            notice: String::new(),
            pending_g: false,
            demo,
            metrics_refresh_seconds: 1,
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
            action_confirmation: None,
            action_request: None,
            action_progress: None,
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
        self.refresh_tree();
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
                    self.notice = "Opened the Compose project matching this directory. Backspace returns to all runtimes.".into();
                }
                _ => {
                    self.notice.clear();
                    self.picker = Some(Picker {
                        title: "Multiple projects match this directory · Choose a connection"
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
        for machine in &discovery.machines {
            if !self.podman_machines.contains_key(&machine.name) {
                let index = self.engines.len();
                self.engines.push(Engine {
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
                if !self.context_order.is_empty() && position <= self.performance_scroll {
                    self.performance_scroll += 1;
                }
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
                                Some("Podman socket changed; refreshing inventory".into());
                        }
                        availability.push((index, true));
                        None
                    }
                    Some(Err(error)) => Some(error.clone()),
                    None => Some(
                        "Podman machine is no longer listed; refresh after starting it separately"
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
        if self.action_progress.is_some()
            || self.action_request.is_some()
            || self.action_confirmation.is_some()
        {
            return;
        }
        let request = self.plan_action(action);
        match request {
            Ok(request) => {
                self.action_review_visible = false;
                self.action_confirmation = Some(crate::actions::Confirmation {
                    request,
                    typed: String::new(),
                    scroll: 0,
                    error: None,
                });
            }
            Err(error) => self.notice = format!("{error:#}"),
        }
    }

    fn plan_action(
        &self,
        action: crate::actions::Action,
    ) -> anyhow::Result<crate::actions::Request> {
        if self.demo {
            anyhow::bail!("Demo mode: lifecycle mutations are disabled");
        }
        let row = self
            .current()
            .ok_or_else(|| anyhow::anyhow!("Select a container or Compose workload"))?;
        let engine = &self.engines[row.engine];
        if engine.error.is_some() || engine.alias.is_some() {
            anyhow::bail!(
                "Connection unavailable or aliased; select its connected primary context"
            );
        }
        crate::actions::Request::build(
            row.engine,
            &engine.connection,
            engine
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.engine_id.as_deref()),
            &row.scope,
            self.selected_containers(),
            action,
        )
    }

    pub fn action_availability(
        &self,
        action: crate::actions::Action,
        row: &str,
    ) -> Result<usize, String> {
        if self.current().is_none_or(|current| current.key != row) {
            return Err("Selection changed; reopen Actions".into());
        }
        self.plan_action(action)
            .map(|request| request.containers.len())
            .map_err(|error| format!("{error:#}"))
    }

    pub fn open_actions(&mut self) {
        let Some(row) = self.current() else {
            return;
        };
        self.picker = Some(Picker {
            title: format!("Actions · {}", row.title),
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
            .collect(),
            selected: 0,
        });
    }

    pub fn layers_target(&self) -> Option<(crate::layers::Key, Connection, Container)> {
        if self.tab != Tab::Layers {
            return None;
        }
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
                confirmation.error =
                    Some("Review targets in a terminal at least 60 × 18 before confirming".into());
            }
            return;
        }
        let Some(mut confirmation) = self.action_confirmation.take() else {
            return;
        };
        if confirmation.request.action == crate::actions::Action::Delete
            && confirmation.typed != confirmation.request.name
        {
            confirmation.error = Some("Name does not match; nothing was sent".into());
            self.action_confirmation = Some(confirmation);
            return;
        }
        let request = confirmation.request;
        let validation = (|| {
            let engine = self
                .engines
                .get(request.engine)
                .ok_or_else(|| anyhow::anyhow!("Runtime no longer exists"))?;
            if self.demo
                || engine.error.is_some()
                || engine.alias.is_some()
                || engine.connection != request.connection
            {
                anyhow::bail!("Connection changed or is unavailable; select and confirm again");
            }
            let snapshot = engine
                .snapshot
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Refresh inventory before retrying"))?;
            if snapshot.engine_id.as_deref() != Some(request.identity.as_str()) {
                anyhow::bail!("Runtime identity changed; select and confirm again");
            }
            for container in &request.containers {
                let current = snapshot
                    .containers
                    .iter()
                    .find(|current| current.id == container.id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Container disappeared; select and confirm again")
                    })?;
                request.validate_container(container, current)?;
            }
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = validation {
            self.notice = format!("{error:#}");
            return;
        }
        self.action_progress = Some(crate::actions::Progress {
            results: vec![None; request.containers.len()],
            request: request.clone(),
            finished: false,
            scroll: 0,
        });
        self.action_request = Some(request);
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
            engine.metrics.apply(&snapshot.containers, event.batch);
        }
    }

    pub fn metrics_for(&self, row: &Row) -> crate::metrics::Summary {
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
            std::time::Instant::now(),
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

    pub fn metrics_stale_after(&self) -> std::time::Duration {
        if self.demo {
            return std::time::Duration::MAX;
        }
        std::time::Duration::from_secs(self.metrics_refresh_seconds.saturating_mul(3).max(3))
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

    pub fn toggle_performance(&mut self) {
        self.performance_open = !self.performance_open;
        if !self.performance_open {
            self.performance_focus = false;
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
            Err(error) => self.notice = format!("Exec unavailable: {error:#}"),
        }
    }

    pub fn close_exec(&mut self) {
        self.sessions.close_active();
        self.exec_input = false;
        self.notice = "Exec closed; the container was not stopped.".into();
    }

    pub fn pick_sessions(&mut self) {
        if self.sessions.is_empty() {
            self.notice =
                "No retained Exec sessions. Select a running container and press e.".into();
            return;
        }
        self.picker = Some(Picker {
            title: format!(
                "Exec sessions · {}/{} retained",
                self.sessions.len(),
                self.sessions.limit()
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

    pub fn refresh_tree(&mut self) {
        let previous = self.current().map(|r| r.key.clone());
        if matches!(
            self.context_sort,
            crate::context_sort::ContextSort::ConnectedFirst
                | crate::context_sort::ContextSort::Name
        ) {
            self.reorder_contexts();
        }
        let mut rows = Vec::new();
        for &i in &self.context_order {
            let engine = &self.engines[i];
            let tree = engine_tree(i, engine);
            let focused;
            let tree = if let Some(project) = &self.project {
                if project.engine != i {
                    continue;
                }
                focused = find_project(&tree, project).cloned().unwrap_or_else(|| {
                    node(
                        i,
                        Scope::Project {
                            namespace: project.namespace.clone(),
                            project: project.name.clone(),
                        },
                        format!("{} [not in latest inventory]", project.name),
                        vec![],
                    )
                });
                &focused
            } else {
                &tree
            };
            flatten(
                tree,
                0,
                &self.expanded,
                &self.tree_query.to_lowercase(),
                &mut rows,
            );
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
            self.notice = "Tree search cleared; selected branch collapsed.".into();
        } else if !self.expanded.remove(&row.key) {
            self.expanded.insert(row.key);
        }
        self.refresh_tree();
    }

    pub fn selected_containers(&self) -> Vec<Container> {
        let Some(row) = self.current() else {
            return vec![];
        };
        let Some(snapshot) = &self.engines[row.engine].snapshot else {
            return vec![];
        };
        snapshot
            .containers
            .iter()
            .filter(|c| matches_container(&row.scope, c))
            .filter(|c| {
                self.project
                    .as_ref()
                    .is_none_or(|project| project.contains(c))
            })
            .cloned()
            .collect()
    }

    pub fn configure_startup(&mut self, cwd: PathBuf) {
        self.startup_cwd = Some(cwd);
        self.startup_pending = (0..self.engines.len()).collect();
        self.notice =
            "Matching the current directory to Compose projects… (--overview skips this)".into();
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

    pub fn open_project(&mut self, project: Project) {
        self.history.push(NavigationState {
            project: self.project.clone(),
            selected: self.current().map(|row| row.key.clone()),
            tree_offset: self.tree_offset,
            tree_query: self.tree_query.clone(),
            expanded: self.expanded.clone(),
            detail_focus: self.detail_focus,
            tab: self.tab,
            detail_scroll: self.detail_scroll,
        });
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
            self.notice = "Exec is disabled in demo mode.".into();
            return;
        }
        let Some(row) = self.current().cloned() else {
            return;
        };
        if !matches!(row.scope, Scope::Container(_) | Scope::Service { .. }) {
            self.notice = "Select a container or service, then press e to exec.".into();
            return;
        }
        let engine = &self.engines[row.engine];
        if engine.error.is_some() || engine.alias.is_some() {
            self.notice = "Exec is unavailable on a disconnected or aliased engine.".into();
            return;
        }
        let containers: Vec<_> = self
            .selected_containers()
            .into_iter()
            .filter(|c| c.running() && !c.managed)
            .collect();
        if containers.is_empty() {
            self.notice = "Exec requires a running, non-Kubernetes-managed container.".into();
        } else if containers.len() == 1 {
            self.exec_request = Some(crate::exec::Request {
                connection: engine.connection.clone(),
                container: containers[0].clone(),
                command: self.exec_command.clone(),
            });
        } else {
            self.picker = Some(Picker {
                title: "Exec · Choose a running container".into(),
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
            Choice::Session(id) => {
                if self.sessions.select(id) {
                    self.tab = Tab::Exec;
                    self.detail_focus = true;
                    self.performance_focus = false;
                    self.exec_input = false;
                    self.notice =
                        format!("Selected Exec session #{id}; Enter resumes input if connected");
                }
            }
            Choice::Action { action, row } => {
                if self.current().is_some_and(|current| current.key == row) {
                    self.request_action(action);
                }
            }
            Choice::ContextSort(mode) => {
                self.context_sort = mode;
                self.reorder_contexts();
                self.refresh_tree();
                self.notice = format!("Context order: {} · o to sort again", mode.label());
            }
            Choice::Project(project) => self.open_project(project),
            Choice::Exec { engine, container } => {
                if self.engines[engine].error.is_some() || self.engines[engine].alias.is_some() {
                    self.notice =
                        "Connection unavailable. Select a connected container and retry.".into();
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

    pub fn pick_context_sort(&mut self) {
        use crate::context_sort::ContextSort;
        self.picker = Some(Picker {
            title: "Sort contexts · pins stay first".into(),
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
        let focused_engine = self.context_order.get(self.performance_scroll).copied();
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
        if let Some(index) = focused_engine {
            self.performance_scroll = self
                .context_order
                .iter()
                .position(|&engine| engine == index)
                .unwrap_or(0);
        }
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
                    format!("Namespace: {namespace}"),
                    workload_tree(index, snapshot, Some(namespace)),
                ));
            }
        }
    }
    let status = if engine.alias.is_some() {
        " [alias]"
    } else if engine.error.is_some() {
        " [unavailable]"
    } else if engine.snapshot.is_some() {
        ""
    } else {
        " [connecting]"
    };
    node(
        index,
        Scope::Engine,
        format!("{}{status}", engine.connection.name),
        children,
    )
}

fn workload_tree(index: usize, snapshot: &Snapshot, namespace: Option<&String>) -> Vec<Node> {
    let containers: Vec<_> = snapshot
        .containers
        .iter()
        .filter(|c| c.namespace.as_ref() == namespace)
        .collect();
    let mut projects: BTreeSet<_> = containers
        .iter()
        .filter_map(|c| c.project.as_ref())
        .collect();
    if namespace.is_none() {
        projects.extend(
            snapshot
                .networks
                .iter()
                .chain(&snapshot.volumes)
                .filter_map(|r| r.project.as_ref()),
        );
    }
    let mut nodes = Vec::new();
    for project in projects {
        let scope = Scope::Project {
            namespace: namespace.cloned(),
            project: project.clone(),
        };
        let members: Vec<_> = containers
            .iter()
            .copied()
            .filter(|c| c.project.as_ref() == Some(project))
            .collect();
        let services: BTreeSet<_> = members
            .iter()
            .map(|c| c.service.as_deref().unwrap_or("Unassigned service"))
            .collect();
        let service_nodes: Vec<_> = services
            .into_iter()
            .map(|service| {
                let instances: Vec<_> = members
                    .iter()
                    .copied()
                    .filter(|c| c.service.as_deref().unwrap_or("Unassigned service") == service)
                    .collect();
                let running = instances.iter().filter(|c| c.running()).count();
                let title = format!("{service} [{running}/{} running]", instances.len());
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
            format!("Services ({})", service_nodes.len()),
            service_nodes,
        )];
        if namespace.is_none() {
            project_nodes.extend(resource_nodes(index, snapshot, Some(project)));
        }
        let running = members.iter().filter(|c| c.running()).count();
        nodes.push(node(
            index,
            scope,
            format!("{project} [{running}/{} running]", members.len()),
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
            format!("Standalone ({})", standalone.len()),
            standalone,
        ));
    }
    if namespace.is_none() {
        nodes.extend(resource_nodes(index, snapshot, None));
    }
    nodes
}

fn resource_nodes(index: usize, snapshot: &Snapshot, project: Option<&String>) -> Vec<Node> {
    [
        (ResourceKind::Network, "Networks", &snapshot.networks),
        (ResourceKind::Volume, "Volumes", &snapshot.volumes),
    ]
    .into_iter()
    .filter_map(|(kind, label, resources)| {
        let children: Vec<_> = resources
            .iter()
            .filter(|r| {
                if let Some(project) = project {
                    r.project.as_ref() == Some(project)
                        || snapshot.containers.iter().any(|c| {
                            c.project.as_ref() == Some(project) && r.containers.contains(&c.name)
                        })
                } else {
                    r.project.is_none() || r.external
                }
            })
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
                            " [shared/external]"
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
                    if project.is_none() { "Shared " } else { "" },
                    label,
                    children.len()
                ),
                children,
            ))
        }
    })
    .collect()
}

fn container_node(index: usize, c: &Container) -> Node {
    let mut item = node(index, Scope::Container(c.key()), c.name.clone(), vec![]);
    item.row.running = Some(c.running());
    item
}

fn contains_match(node: &Node, query: &str) -> bool {
    node.row.title.to_lowercase().contains(query)
        || node
            .children
            .iter()
            .any(|child| contains_match(child, query))
}

fn flatten(
    node: &Node,
    depth: usize,
    expanded: &HashSet<String>,
    query: &str,
    rows: &mut Vec<Row>,
) {
    if !query.is_empty() && !contains_match(node, query) {
        return;
    }
    let mut row = node.row.clone();
    row.depth = depth;
    rows.push(row);
    if expanded.contains(&node.row.key) || !query.is_empty() {
        // A matching parent exposes its complete subtree during search.
        let child_query = if node.row.title.to_lowercase().contains(query) {
            ""
        } else {
            query
        };
        for child in &node.children {
            flatten(
                child,
                depth + 1,
                expanded,
                if query.is_empty() { "" } else { child_query },
                rows,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BackendKind;

    #[test]
    fn preferences_restore_navigation_without_restoring_shells_or_input_and_cancel_on_interaction()
    {
        let mut original = app();
        original.open_project(Project {
            engine: 0,
            namespace: None,
            name: "shop".into(),
        });
        original.tab = Tab::Layers;
        original.performance_open = true;
        original.log_view.wrap = false;
        let preferences = original.preferences();
        let serialized = toml::to_string(&preferences).unwrap();
        assert!(!serialized.contains("unix:///test"));
        assert!(!serialized.contains("exec_command"));
        let mut restored = app();
        restored.apply_preferences(preferences.clone(), true);
        restored.restore_navigation();
        assert_eq!(restored.project.as_ref().unwrap().name, "shop");
        assert_eq!(restored.tab, Tab::Layers);
        assert!(restored.performance_open);
        assert!(!restored.log_view.wrap);
        assert!(!restored.exec_input);
        assert!(restored.sessions.is_empty());
        let mut canceled = app();
        canceled.apply_preferences(preferences, true);
        canceled.cancel_startup();
        canceled.restore_navigation();
        assert!(canceled.project.is_none());
        original.tab = Tab::Exec;
        assert_eq!(original.preferences().tab, Tab::Overview);
    }

    fn app() -> App {
        let mut app = App::new(
            vec![Connection {
                name: "test".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///test".into(),
            }],
            100,
            false,
            true,
        );
        app.update_engine(
            0,
            Ok(Snapshot {
                containers: vec![Container {
                    id: "one".into(),
                    name: "api-1".into(),
                    project: Some("shop".into()),
                    service: Some("api".into()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        app
    }

    #[test]
    fn runtime_is_parent_of_projects_and_vim_navigation_works() {
        let mut app = app();
        assert_eq!(app.rows[0].scope, Scope::Engine);
        assert_eq!(app.rows[1].title, "shop [0/1 running]");
        app.move_selection(1);
        app.expand();
        app.move_selection(1);
        app.expand();
        app.move_selection(1);
        app.expand();
        assert!(app.rows.iter().any(|r| r.title.starts_with("api-1")));
        app.collapse();
        app.collapse();
        assert!(matches!(
            app.current().unwrap().scope,
            Scope::Services { .. }
        ));
    }

    #[test]
    fn refresh_preserves_selection_and_search_finds_descendants() {
        let mut app = app();
        app.move_selection(1);
        let key = app.current().unwrap().key.clone();
        app.refresh_tree();
        assert_eq!(app.current().unwrap().key, key);
        app.tree_query = "api-1".into();
        app.refresh_tree();
        assert!(app.rows.iter().any(|r| r.title.starts_with("api-1")));
    }

    #[test]
    fn duplicate_engine_identity_does_not_duplicate_workloads() {
        let mut app = app();
        let mut connection = app.engines[0].connection.clone();
        connection.name = "alias".into();
        app.engines.push(Engine {
            connection,
            snapshot: None,
            error: None,
            alias: None,
            metrics: Default::default(),
            metrics_revision: 0,
        });
        let snapshot = Snapshot {
            engine_id: Some("same".into()),
            ..Default::default()
        };
        app.update_engine(0, Ok(snapshot.clone()));
        app.update_engine(1, Ok(snapshot));
        assert_eq!(app.engines[1].alias.as_deref(), Some("test"));
    }

    #[test]
    fn failure_preserves_last_known_snapshot_but_marks_it_stale() {
        let mut app = app();
        app.update_engine(0, Err("offline".into()));
        assert!(app.engines[0].snapshot.is_some());
        assert!(app.rows[0].title.contains("unavailable"));
    }

    #[test]
    fn next_search_match_skips_ancestors_and_wraps() {
        let mut app = app();
        app.tree_query = "api-1".into();
        app.refresh_tree();
        app.selected = 0;
        app.next_tree_match(false);
        assert!(app.current().unwrap().title.contains("api-1"));
        let selected = app.selected;
        app.next_tree_match(false);
        assert_eq!(app.selected, selected);
        app.next_tree_match(true);
        assert_eq!(app.selected, selected);
    }

    #[test]
    fn counts_distinct_services_and_running_instances() {
        let mut app = app();
        let stopped = app.engines[0].snapshot.as_ref().unwrap().containers[0].clone();
        let running = Container {
            id: "two".into(),
            state: "running".into(),
            ..stopped.clone()
        };
        let replica = Container {
            id: "three".into(),
            ..running.clone()
        };
        let postgres = Container {
            id: "four".into(),
            service: Some("postgres".into()),
            ..running.clone()
        };
        app.update_engine(
            0,
            Ok(Snapshot {
                containers: vec![stopped, running, replica, postgres],
                ..Default::default()
            }),
        );
        let tree = engine_tree(0, &app.engines[0]);
        let project = &tree.children[0];
        assert_eq!(project.row.title, "shop [3/4 running]");
        let services = &project.children[0];
        assert_eq!(services.row.title, "Services (2)");
        assert_eq!(services.children[0].row.title, "api [2/3 running]");
        assert_eq!(services.children[1].row.title, "postgres [1/1 running]");
    }

    #[test]
    fn metric_updates_are_engine_scoped_and_stale_revisions_are_ignored() {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        let row = app
            .rows
            .iter()
            .find(|row| matches!(row.scope, Scope::Engine) && row.engine == 0)
            .unwrap()
            .clone();
        let other = app
            .rows
            .iter()
            .find(|row| matches!(row.scope, Scope::Engine) && row.engine == 1)
            .unwrap()
            .clone();
        let before_other = app.metrics_for(&other).cpu_percent;
        app.engines[0].metrics_revision = 3;
        let batch = || {
            std::collections::HashMap::from([(
                "/api-1".into(),
                Ok(crate::metrics::Reading {
                    usage: crate::metrics::Usage {
                        cpu_percent: Some(123.0),
                        memory_bytes: Some(1024),
                        ..Default::default()
                    },
                    counters: None,
                    sampled_at: std::time::Instant::now(),
                }),
            )])
        };
        let before = app.metrics_for(&row).cpu_percent;
        app.apply_metrics(crate::metrics::Event {
            engine: 0,
            revision: 2,
            batch: batch(),
        });
        assert_eq!(app.metrics_for(&row).cpu_percent, before);
        app.apply_metrics(crate::metrics::Event {
            engine: 0,
            revision: 3,
            batch: batch(),
        });
        assert_ne!(app.metrics_for(&row).cpu_percent, before);
        assert_eq!(app.metrics_for(&other).cpu_percent, before_other);
        app.engines[0].error = Some("offline".into());
        assert_eq!(app.metrics_for(&row).cpu_percent, None);
    }

    #[test]
    fn known_empty_runtime_is_zero_but_disconnected_runtime_is_not() {
        let mut app = app();
        app.update_engine(0, Ok(Snapshot::default()));
        let row = app.rows[0].clone();
        assert_eq!(app.metrics_for(&row).cpu_label(), "0.0%");
        assert_eq!(app.metrics_for(&row).memory_label(), "0 B");
        app.update_engine(0, Err("offline".into()));
        assert_eq!(app.metrics_for(&row).cpu_label(), "--");
    }

    #[test]
    fn native_podman_reconciliation_keeps_machine_indices_across_restarts_and_failures() {
        use crate::discovery::podman::{Discovery, Machine};
        let mut app = app();
        let before = app.engines[0].connection.clone();
        let report = |socket: &str| Discovery {
            machines: vec![
                Machine {
                    name: "first".into(),
                    endpoint: Ok(socket.into()),
                },
                Machine {
                    name: "second".into(),
                    endpoint: Err("stopped".into()),
                },
            ],
            warning: None,
        };
        let availability = app.reconcile_podman(&report("unix:///one.sock"));
        let index = app.podman_machines["first"];
        let stopped = app.podman_machines["second"];
        assert!(availability.contains(&(index, true)));
        assert!(availability.contains(&(stopped, false)));
        app.update_engine(
            index,
            Ok(Snapshot {
                engine_id: Some("podman-id".into()),
                ..Default::default()
            }),
        );
        app.selected = app.rows.iter().position(|row| row.engine == index).unwrap();
        let selected = app.current().unwrap().key.clone();
        app.reconcile_podman(&report("unix:///two.sock"));
        assert_eq!(app.podman_machines["first"], index);
        assert_eq!(app.current().unwrap().key, selected);
        assert_eq!(app.engines[index].connection.endpoint, "unix:///two.sock");
        assert!(app.engines[index].error.is_some());
        assert_eq!(app.engines[0].connection, before);
        let failed_scan = Discovery {
            machines: vec![],
            warning: Some("invalid JSON".into()),
        };
        assert!(
            app.reconcile_podman(&failed_scan)
                .iter()
                .all(|(_, available)| !available)
        );
        assert_eq!(app.engines[index].connection.endpoint, "unix:///two.sock");
        app.reconcile_podman(&Discovery::default());
        assert!(
            app.engines[index]
                .error
                .as_ref()
                .unwrap()
                .contains("no longer listed")
        );
        assert_eq!(app.reconcile_podman(&report("unix:///three.sock")).len(), 2);
        assert_eq!(app.podman_machines["first"], index);
    }

    #[test]
    fn native_podman_and_docker_context_aliases_do_not_duplicate_metrics() {
        use crate::discovery::podman::{Discovery, Machine};
        let mut app = app();
        let identity = app.engines[0].snapshot.as_ref().unwrap().engine_id.clone();
        app.reconcile_podman(&Discovery {
            machines: vec![Machine {
                name: "fixture".into(),
                endpoint: Ok("unix:///podman.sock".into()),
            }],
            warning: None,
        });
        let index = app.podman_machines["fixture"];
        app.update_engine(
            index,
            Ok(Snapshot {
                engine_id: identity,
                ..Default::default()
            }),
        );
        // Seed the same nonempty identity explicitly, independent of the app fixture.
        app.update_engine(
            0,
            Ok(Snapshot {
                engine_id: Some("same-podman".into()),
                ..Default::default()
            }),
        );
        app.update_engine(
            index,
            Ok(Snapshot {
                engine_id: Some("same-podman".into()),
                ..Default::default()
            }),
        );
        assert!(app.engines[index].alias.is_some());
        assert_eq!(app.engine_metrics(index).cpu_percent, None);
    }

    #[test]
    fn startup_matching_waits_for_native_discovery_and_uses_machine_project_labels() {
        use crate::discovery::podman::{Discovery, Machine};
        let mut app = app();
        let cwd = std::env::current_dir().unwrap();
        app.configure_startup(cwd.clone());
        app.wait_for_machine_discovery();
        app.update_engine(0, Ok(Snapshot::default()));
        assert!(app.startup_cwd.is_some());
        app.reconcile_podman(&Discovery {
            machines: vec![Machine {
                name: "fixture".into(),
                endpoint: Ok("unix:///podman.sock".into()),
            }],
            warning: None,
        });
        let index = app.podman_machines["fixture"];
        assert!(app.project.is_none());
        app.update_engine(
            index,
            Ok(Snapshot {
                containers: vec![Container {
                    id: "native-api".into(),
                    project: Some("native-project".into()),
                    service: Some("api".into()),
                    compose_working_dir: Some(cwd),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(app.project.as_ref().unwrap().engine, index);
        assert_eq!(app.project.as_ref().unwrap().name, "native-project");
    }

    #[test]
    fn matching_podman_socket_is_aliased_even_when_the_api_omits_engine_identity() {
        use crate::discovery::podman::{Discovery, Machine};
        let mut app = app();
        app.reconcile_podman(&Discovery {
            machines: vec![Machine {
                name: "fixture".into(),
                endpoint: Ok("/test".into()),
            }],
            warning: None,
        });
        let index = app.podman_machines["fixture"];
        app.update_engine(index, Ok(Snapshot::default()));
        assert!(app.engines[index].alias.is_some());
    }

    #[test]
    fn late_native_machine_respects_pins_without_resorting_usage_or_moving_focus() {
        use crate::discovery::podman::{Discovery, Machine};
        let mut app = app();
        app.context_sort = crate::context_sort::ContextSort::Cpu;
        app.context_pins = vec!["Podman / preferred".into()];
        let selected = app.current().unwrap().key.clone();
        app.reconcile_podman(&Discovery {
            machines: vec![Machine {
                name: "preferred".into(),
                endpoint: Ok("unix:///podman.sock".into()),
            }],
            warning: None,
        });
        assert_eq!(app.context_order[0], app.podman_machines["preferred"]);
        assert_eq!(app.context_order[app.performance_scroll], 0);
        assert_eq!(app.current().unwrap().key, selected);
    }

    #[test]
    fn sorting_preserves_backend_indices_selection_expansion_and_log_targets() {
        use crate::context_sort::ContextSort;
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        let identities: Vec<_> = app
            .engines
            .iter()
            .map(|engine| engine.connection.clone())
            .collect();
        let selection = app.current().unwrap().key.clone();
        let expanded = app.expanded.clone();
        app.log_scope = Some("fixed-log-target".into());
        app.performance_scroll = app
            .context_order
            .iter()
            .position(|&engine| engine == 0)
            .unwrap();
        app.pick_context_sort();
        app.picker.as_mut().unwrap().selected = 2;
        app.choose();
        assert_eq!(app.context_sort, ContextSort::Cpu);
        assert_eq!(app.current().unwrap().key, selection);
        assert_eq!(app.expanded, expanded);
        assert_eq!(app.log_scope.as_deref(), Some("fixed-log-target"));
        assert_eq!(app.context_order[app.performance_scroll], 0);
        assert_eq!(
            app.engines
                .iter()
                .map(|engine| engine.connection.clone())
                .collect::<Vec<_>>(),
            identities
        );
        let order = app.context_order.clone();
        app.update_engine(0, Err("disconnected".into()));
        app.refresh_tree();
        assert_eq!(
            app.context_order, order,
            "usage sort must not jump on refresh"
        );
        app.pick_context_sort();
        app.picker.as_mut().unwrap().selected = 0;
        app.choose();
        assert_ne!(app.context_order, order);
    }

    #[test]
    fn project_workspace_excludes_other_engines_and_back_restores_navigation() {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        let selected = app.current().unwrap().key.clone();
        let rows = app.rows.len();
        let expanded = app.expanded.clone();
        app.open_selected();
        assert_eq!(app.project.as_ref().unwrap().name, "shop");
        assert!(app.rows.iter().all(|row| row.engine == 0));
        assert!(
            !app.rows
                .iter()
                .any(|row| matches!(row.scope, Scope::Engine | Scope::Standalone(_)))
        );
        assert_eq!(app.rows[0].depth, 0);
        assert_eq!(app.selected_containers().len(), 3);
        assert!(app.back());
        assert!(app.project.is_none());
        assert_eq!(app.rows.len(), rows);
        assert_eq!(app.current().unwrap().key, selected);
        assert_eq!(app.expanded, expanded);
    }

    #[test]
    fn workspace_includes_attached_external_network_but_not_other_workloads() {
        let mut app = app();
        let mut snapshot = app.engines[0].snapshot.as_ref().unwrap().clone();
        snapshot.containers[0].networks.push("shared".into());
        snapshot.containers.push(Container {
            id: "other".into(),
            name: "other-api".into(),
            project: Some("other".into()),
            networks: vec!["shared".into()],
            ..Default::default()
        });
        snapshot.networks.push(crate::model::Resource {
            name: "shared".into(),
            external: true,
            containers: vec!["api-1".into(), "other-api".into()],
            ..Default::default()
        });
        app.update_engine(0, Ok(snapshot));
        app.selected = app
            .rows
            .iter()
            .position(
                |row| matches!(&row.scope, Scope::Project { project, .. } if project == "shop"),
            )
            .unwrap();
        app.open_selected();
        app.tree_query = "shared".into();
        app.refresh_tree();
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(&row.scope, Scope::Resource { name, .. } if name == "shared"))
            .unwrap();
        assert!(app.current().unwrap().title.contains("shared/external"));
        assert_eq!(app.selected_containers().len(), 1);
        assert_eq!(app.selected_containers()[0].name, "api-1");
    }

    #[test]
    fn service_exec_requires_replica_choice_and_rejects_offline_engines() {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        app.demo = false;
        app.selected = app
            .rows
            .iter()
            .position(
                |row| matches!(&row.scope, Scope::Service { service, .. } if service == "api"),
            )
            .unwrap();
        app.request_exec();
        assert!(app.exec_request.is_none());
        assert_eq!(app.picker.as_ref().unwrap().choices.len(), 2);
        app.picker.as_mut().unwrap().selected = 1;
        app.choose();
        let request = app.exec_request.take().unwrap();
        assert_eq!(request.container.name, "shop-api-2");
        assert_eq!(request.connection.name, "OrbStack");
        app.engines[0].error = Some("offline".into());
        app.request_exec();
        assert!(app.exec_request.is_none());
        assert!(app.notice.contains("disconnected"));
    }

    #[test]
    fn resource_only_projects_show_zero_counts() {
        let mut app = app();
        app.update_engine(
            0,
            Ok(Snapshot {
                networks: vec![crate::model::Resource {
                    name: "empty_default".into(),
                    project: Some("empty".into()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        let tree = engine_tree(0, &app.engines[0]);
        assert_eq!(tree.children[0].row.title, "empty [0/0 running]");
        assert_eq!(tree.children[0].children[0].row.title, "Services (0)");
    }

    #[test]
    fn toggling_a_search_expanded_branch_collapses_it_and_preserves_selection() {
        let mut app = app();
        app.tree_query = "api-1".into();
        app.refresh_tree();
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Service { .. }))
            .unwrap();
        let key = app.current().unwrap().key.clone();
        app.toggle_branch();
        assert!(app.tree_query.is_empty());
        assert_eq!(app.current().unwrap().key, key);
        assert!(!app.expanded.contains(&key));
        assert!(
            !app.rows
                .iter()
                .any(|row| matches!(row.scope, Scope::Container(_)))
        );
        app.toggle_branch();
        assert!(
            app.rows
                .iter()
                .any(|row| matches!(row.scope, Scope::Container(_)))
        );
    }

    #[test]
    fn counts_do_not_mix_namespaces_or_reset_selection_on_refresh() {
        let mut app = app();
        let mut snapshot = app.engines[0].snapshot.as_ref().unwrap().clone();
        snapshot.containers[0].namespace = Some("first".into());
        let other = Container {
            namespace: Some("second".into()),
            state: "running".into(),
            ..snapshot.containers[0].clone()
        };
        snapshot.containers.push(other);
        snapshot.namespaces = vec!["first".into(), "second".into()];
        app.update_engine(0, Ok(snapshot.clone()));
        let tree = engine_tree(0, &app.engines[0]);
        assert_eq!(tree.children[0].children[0].row.title, "shop [0/1 running]");
        assert_eq!(tree.children[1].children[0].row.title, "shop [1/1 running]");
        app.selected = 1;
        app.expand();
        app.move_selection(1);
        app.expand();
        let key = app.current().unwrap().key.clone();
        snapshot.containers[0].state = "running".into();
        app.update_engine(0, Ok(snapshot));
        assert_eq!(app.current().unwrap().key, key);
        assert_eq!(app.current().unwrap().title, "shop [1/1 running]");
        assert!(app.expanded.contains(&key));
    }
}
