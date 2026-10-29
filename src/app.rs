use crate::{
    config::Connection,
    logs::LogBuffer,
    logs::view::LogView,
    model::{Condition, Container, Snapshot, Tone},
    workspace::Project,
};
use std::collections::{BTreeSet, HashSet};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Logs,
    Networks,
    Exec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
    Command,
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
    pub exec_session: Option<crate::terminal::Session>,
    pub exec_input: bool,
    pub exec_dimensions: (u16, u16),
    pub platforms: crate::platform::Cache,
    startup_cwd: Option<PathBuf>,
    startup_pending: HashSet<usize>,
}

impl App {
    pub fn new(
        connections: Vec<Connection>,
        log_capacity: usize,
        monochrome: bool,
        demo: bool,
    ) -> Self {
        let mut app = Self {
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
            exec_session: None,
            exec_input: false,
            exec_dimensions: (24, 80),
            platforms: Default::default(),
            startup_cwd: None,
            startup_pending: HashSet::new(),
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
            self.engines[i].alias = identity.and_then(|id| {
                (0..i)
                    .find(|&other| {
                        self.engines[other].error.is_none()
                            && self.engines[other]
                                .snapshot
                                .as_ref()
                                .and_then(|s| s.engine_id.as_deref())
                                == Some(id)
                    })
                    .map(|other| self.engines[other].connection.name.clone())
            });
        }
        self.refresh_tree();
        self.startup_pending.remove(&index);
        if self.startup_pending.is_empty()
            && let Some(cwd) = self.startup_cwd.take()
        {
            let projects = crate::workspace::directory_matches(&self.engines, &cwd);
            match projects.len() {
                0 => self.notice.clear(),
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

    pub fn current(&self) -> Option<&Row> {
        self.rows.get(self.selected)
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
            return Some(Condition::new(Tone::Error, "unavailable"));
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
        if let Some(session) = &mut self.exec_session
            && session.running()
        {
            self.tab = Tab::Exec;
            if session.same_target(&request) {
                session.follow();
                self.detail_focus = true;
                self.performance_focus = false;
                self.exec_input = true;
            } else {
                self.detail_focus = true;
                self.performance_focus = false;
                self.exec_input = false;
                self.notice = format!(
                    "An Exec session is already open for {}. Exit it or use :close-exec before opening another container.",
                    session.request.container.name
                );
            }
            return;
        }
        let result = crate::exec::prepare(&request).and_then(|command| {
            crate::terminal::Session::start(
                request,
                command,
                self.exec_dimensions.0,
                self.exec_dimensions.1,
            )
        });
        match result {
            Ok(session) => {
                self.exec_session = Some(session);
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
        if let Some(session) = &mut self.exec_session {
            session.close();
        }
        self.exec_input = false;
        self.notice = "Exec closed; the container was not stopped.".into();
    }

    pub fn refresh_tree(&mut self) {
        let previous = self.current().map(|r| r.key.clone());
        let mut rows = Vec::new();
        for (i, engine) in self.engines.iter().enumerate() {
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
        if self.startup_cwd.take().is_some() {
            self.notice.clear();
        }
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
        let Some(mut picker) = self.picker.take() else {
            return;
        };
        if picker.selected >= picker.choices.len() {
            return;
        }
        match picker.choices.remove(picker.selected) {
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
