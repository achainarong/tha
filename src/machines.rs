//! Servers as machines in the Explorer: their connection state, and the Servers modal.
//!
//! Engine indices never move, so a server keeps one hidden anchor engine for its machine row
//! and its real engines stay in `App::engines` while disconnected, without workers.
use crate::{
    app::{App, Engine, OverviewMode, Scope, Tab},
    config::{BackendKind, Connection},
    remote::{self, ConnectMode, Remote},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The machine name of everything reached without a server.
pub const LOCAL_MACHINE: &str = "";

const LOCAL_TOOL_PREFIX: &str = "local-tool:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineState {
    NotConnected,
    Connecting,
    Connected,
    Offline { reason: String, retrying: bool },
}

#[derive(Debug, Clone)]
pub struct Machine {
    pub remote: Remote,
    /// Saved through the modal, as opposed to hand-written in `config.toml`.
    pub managed: bool,
    pub state: MachineState,
    pub allow_delete: bool,
    pub anchor: usize,
    pub generation: u64,
}

impl Machine {
    pub fn connected(&self) -> bool {
        self.state == MachineState::Connected
    }

    /// Anything other than an idle or stopped session, which `Enter` would then close.
    fn session_active(&self) -> bool {
        matches!(
            self.state,
            MachineState::Connected
                | MachineState::Connecting
                | MachineState::Offline { retrying: true, .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCommand {
    Connect { remote: Remote, generation: u64 },
    Disconnect(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormRow {
    Name,
    Host,
    User,
    Port,
    Key,
    Connect,
    CreateKey,
    CopySetup,
    Test,
    Save,
}

impl FormRow {
    pub const ALL: [Self; 10] = [
        Self::Name,
        Self::Host,
        Self::User,
        Self::Port,
        Self::Key,
        Self::Connect,
        Self::CreateKey,
        Self::CopySetup,
        Self::Test,
        Self::Save,
    ];
}

#[derive(Debug, Clone)]
pub struct Form {
    pub editing: Option<String>,
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: String,
    pub key: String,
    pub connect: Option<ConnectMode>,
    metrics_refresh_seconds: Option<u64>,
    pub focus: usize,
    pub dirty: bool,
    pub error: Option<String>,
    pub note: Option<String>,
    /// The admin command, or why it cannot be shown yet.
    pub setup: Result<String, String>,
    pub confirm_discard: bool,
    pub confirm_reconnect: bool,
}

impl Form {
    fn new() -> Self {
        let mut form = Self {
            editing: None,
            name: String::new(),
            host: String::new(),
            user: String::new(),
            port: String::new(),
            key: String::new(),
            connect: None,
            metrics_refresh_seconds: None,
            focus: 0,
            dirty: false,
            error: None,
            note: None,
            setup: Ok(String::new()),
            confirm_discard: false,
            confirm_reconnect: false,
        };
        form.refresh_setup();
        form
    }

    fn from_remote(remote: &Remote) -> Self {
        let mut form = Self {
            editing: Some(remote.name.clone()),
            name: remote.name.clone(),
            host: remote.host.clone(),
            user: remote.user.clone(),
            port: remote.port.map(|port| port.to_string()).unwrap_or_default(),
            key: remote.identity_file.clone(),
            connect: remote.connect,
            metrics_refresh_seconds: remote.metrics_refresh_seconds,
            ..Self::new()
        };
        form.refresh_setup();
        form
    }

    pub fn row(&self) -> FormRow {
        FormRow::ALL[self.focus.min(FormRow::ALL.len() - 1)]
    }

    fn text(&mut self) -> Option<&mut String> {
        match self.row() {
            FormRow::Name => Some(&mut self.name),
            FormRow::Host => Some(&mut self.host),
            FormRow::User => Some(&mut self.user),
            FormRow::Port => Some(&mut self.port),
            FormRow::Key => Some(&mut self.key),
            _ => None,
        }
    }

    pub fn remote(&self) -> anyhow::Result<Remote> {
        let port = match self.port.trim() {
            "" => None,
            port => Some(port.parse::<u16>().map_err(|_| {
                anyhow::anyhow!(crate::i18n::t("Port must be between 1 and 65535"))
            })?),
        };
        let remote = Remote {
            name: self.name.trim().to_owned(),
            host: self.host.trim().to_owned(),
            user: self.user.trim().to_owned(),
            identity_file: self.key.trim().to_owned(),
            port,
            connect: self.connect,
            metrics_refresh_seconds: self.metrics_refresh_seconds,
        };
        remote.validate()?;
        Ok(remote)
    }

    fn refresh_setup(&mut self) {
        let candidate = Remote {
            name: "setup".into(),
            host: "setup".into(),
            user: self.user.trim().to_owned(),
            identity_file: self.key.trim().to_owned(),
            port: None,
            connect: None,
            metrics_refresh_seconds: None,
        };
        self.setup = if self.key.trim().is_empty() {
            Err(crate::i18n::t("Choose an SSH key to see the command for the server.").into())
        } else {
            candidate
                .validate()
                .and_then(|()| remote::setup_command(&candidate))
                .map_err(|error| crate::model::safe_text(&format!("{error:#}")))
        };
    }
}

#[derive(Debug, Clone)]
pub enum ModalView {
    List,
    Form(Box<Form>),
    ConfirmRemove(String),
}

#[derive(Debug, Clone)]
pub struct RemotesModal {
    pub selected: usize,
    pub view: ModalView,
    /// Set while a test or key creation runs in the embedded terminal, keeping the typed form.
    pub hidden: bool,
    pub message: Option<String>,
}

impl App {
    pub fn is_anchor(&self, index: usize) -> bool {
        remote::parse_endpoint(&self.engines[index].connection.endpoint)
            .is_some_and(|(_, engine)| engine.is_empty())
    }

    /// Which machine runs this engine: a server name, or `LOCAL_MACHINE`.
    pub fn machine_name(&self, index: usize) -> &str {
        remote::server_of(&self.engines[index].connection).unwrap_or(LOCAL_MACHINE)
    }

    pub fn machine(&self, name: &str) -> Option<&Machine> {
        self.machines.iter().find(|m| m.remote.name == name)
    }

    fn machine_mut(&mut self, name: &str) -> Option<&mut Machine> {
        self.machines.iter_mut().find(|m| m.remote.name == name)
    }

    pub fn engine_visible(&self, index: usize) -> bool {
        if !remote::is_remote(&self.engines[index].connection) {
            return true;
        }
        !self.is_anchor(index)
            && self
                .machine(self.machine_name(index))
                .is_some_and(Machine::connected)
    }

    /// Engines that have something to poll. A server's engines only do while connected, so a
    /// disconnected server produces no traffic.
    pub fn worker_availability(&self) -> Vec<(usize, bool)> {
        (0..self.engines.len())
            .map(|index| (index, self.engine_visible(index)))
            .collect()
    }

    pub fn configured_engine_count(&self) -> usize {
        (0..self.engines.len())
            .filter(|&index| !self.is_anchor(index))
            .filter(|&index| self.engine_visible(index))
            .count()
    }

    pub fn machine_engines(&self, name: &str) -> Vec<usize> {
        self.context_order
            .iter()
            .copied()
            .filter(|&index| self.machine_name(index) == name && self.engine_visible(index))
            .collect()
    }

    /// Requests for the supervisor tasks, which live with the other workers in the main loop.
    pub fn take_remote_commands(&mut self) -> Vec<RemoteCommand> {
        std::mem::take(&mut self.remote_commands)
    }

    pub fn take_remote_availability(&mut self) -> Vec<(usize, bool)> {
        std::mem::take(&mut self.remote_availability)
    }

    /// Later entries lose on a duplicate name, so `config.toml` wins and nothing is overwritten.
    pub fn load_machines(
        &mut self,
        configured: Vec<Remote>,
        managed: anyhow::Result<Vec<Remote>>,
        path: Option<std::path::PathBuf>,
    ) {
        for remote in configured {
            self.add_machine(remote, false);
        }
        self.remotes_path = path;
        match managed {
            Ok(remotes) => {
                for remote in remotes {
                    if self.machine(&remote.name).is_some() {
                        self.remotes_problem = Some(crate::tf!(
                            "Skipped saved server {name}: the name is already in config.toml",
                            name = crate::model::safe_text(&remote.name)
                        ));
                    } else {
                        self.add_machine(remote, true);
                    }
                }
            }
            Err(error) => {
                // Never overwrite a file that could not be understood.
                self.remotes_path = None;
                self.remotes_problem = Some(crate::model::safe_text(&format!("{error:#}")));
            }
        }
        self.refresh_tree();
    }

    fn add_machine(&mut self, remote: Remote, managed: bool) {
        let anchor = self.anchor_for(&remote.name);
        self.expanded.insert(format!(
            "{anchor}/{:?}",
            Scope::Machine(remote.name.clone())
        ));
        self.machines.push(Machine {
            remote,
            managed,
            state: MachineState::NotConnected,
            allow_delete: false,
            anchor,
            generation: 0,
        });
    }

    fn anchor_for(&mut self, name: &str) -> usize {
        let endpoint = remote::endpoint(name, "");
        if let Some(index) = self
            .engines
            .iter()
            .position(|engine| engine.connection.endpoint == endpoint)
        {
            return index;
        }
        self.push_engine(Connection {
            name: name.to_owned(),
            kind: BackendKind::Docker,
            endpoint,
        })
    }

    fn push_engine(&mut self, connection: Connection) -> usize {
        let index = self.engines.len();
        self.engines.push(Engine {
            connection,
            runtime: None,
            snapshot: None,
            error: Some(crate::i18n::t("Not connected").into()),
            alias: None,
            metrics: Default::default(),
            metrics_revision: 0,
            events: Default::default(),
        });
        self.context_order.push(index);
        index
    }

    /// Called once the UI starts, and never from the daemon, which has no machines.
    pub fn connect_startup_machines(&mut self) {
        if self.demo {
            return;
        }
        let names: Vec<_> = self
            .machines
            .iter()
            .filter(|machine| {
                machine.remote.connect.unwrap_or(self.remote_connect) == ConnectMode::Startup
            })
            .map(|machine| machine.remote.name.clone())
            .collect();
        for name in names {
            self.connect_machine(&name);
        }
    }

    pub fn connect_machine(&mut self, name: &str) {
        if self.demo {
            self.notice = crate::i18n::t("Demo mode: servers are not contacted").into();
            return;
        }
        let Some(machine) = self.machine_mut(name) else {
            return;
        };
        machine.generation = machine.generation.wrapping_add(1);
        machine.state = MachineState::Connecting;
        let command = RemoteCommand::Connect {
            remote: machine.remote.clone(),
            generation: machine.generation,
        };
        self.remote_commands.push(command);
        self.refresh_tree();
    }

    /// Stops the session and every worker, so the server gets no further traffic.
    pub fn disconnect_machine(&mut self, name: &str) {
        let Some(machine) = self.machine_mut(name) else {
            return;
        };
        machine.generation = machine.generation.wrapping_add(1);
        machine.state = MachineState::NotConnected;
        self.remote_commands
            .push(RemoteCommand::Disconnect(name.to_owned()));
        self.release_engines(name, crate::i18n::t("Not connected"));
        self.refresh_tree();
    }

    /// The server's data must not stay on screen looking current, or stay actionable.
    fn release_engines(&mut self, name: &str, reason: &str) {
        for index in 0..self.engines.len() {
            if self.machine_name(index) != name || self.is_anchor(index) {
                continue;
            }
            let engine = &mut self.engines[index];
            engine.snapshot = None;
            engine.error = Some(reason.to_owned());
            engine.metrics = Default::default();
            engine.metrics_revision = engine.metrics_revision.wrapping_add(1);
            self.remote_availability.push((index, false));
        }
    }

    pub fn apply_remote_event(&mut self, event: remote::Event) {
        // A late event from a replaced session must not overwrite the newer state.
        if self
            .machine(&event.server)
            .is_none_or(|machine| machine.generation != event.generation)
        {
            return;
        }
        match event.update {
            remote::Update::Connecting => {
                if let Some(machine) = self.machine_mut(&event.server) {
                    machine.state = MachineState::Connecting;
                }
            }
            remote::Update::Connected {
                engines,
                allow_delete,
            } => {
                if let Some(machine) = self.machine_mut(&event.server) {
                    machine.state = MachineState::Connected;
                    machine.allow_delete = allow_delete;
                }
                self.attach_engines(&event.server, &engines);
            }
            remote::Update::Failed { reason, retrying } => {
                self.release_engines(&event.server, &reason);
                if let Some(machine) = self.machine_mut(&event.server) {
                    machine.state = MachineState::Offline { reason, retrying };
                }
            }
        }
        self.refresh_tree();
    }

    fn attach_engines(&mut self, server: &str, engines: &[String]) {
        for engine in engines {
            let connection = remote::connection(server, engine);
            let index = match self
                .engines
                .iter()
                .position(|existing| existing.connection.endpoint == connection.endpoint)
            {
                Some(index) => index,
                None => self.push_engine(connection),
            };
            self.engines[index].error = None;
            self.expanded.insert(format!("{index}/{:?}", Scope::Engine));
            self.remote_availability.push((index, true));
        }
        // An engine the server stopped exposing gets no more requests.
        for index in 0..self.engines.len() {
            let Some((name, engine)) =
                remote::parse_endpoint(&self.engines[index].connection.endpoint)
            else {
                continue;
            };
            if name == server && !engine.is_empty() && !engines.iter().any(|e| e == engine) {
                self.engines[index].snapshot = None;
                self.remote_availability.push((index, false));
            }
        }
    }

    /// `Enter` on a machine row. A server that is not connected connects as it opens.
    pub fn open_machine(&mut self, name: &str) {
        if self.machine(name).is_some_and(|machine| {
            matches!(
                machine.state,
                MachineState::NotConnected
                    | MachineState::Offline {
                        retrying: false,
                        ..
                    }
            )
        }) {
            self.connect_machine(name);
        }
        if self.machine.as_deref() != Some(name) {
            self.push_navigation();
            self.machine = Some(name.to_owned());
            self.project = None;
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
    }

    pub fn selected_machine(&self) -> Option<String> {
        if let Some(Scope::Machine(name)) = self.current().map(|row| &row.scope) {
            return Some(name.clone());
        }
        let name = self.machine_name(self.current()?.engine);
        Some(name.to_owned())
    }

    /// The disconnect action: the selected server, from its machine row or anything inside it.
    pub fn toggle_selected_connection(&mut self) {
        let name = self.selected_machine().unwrap_or_default();
        match self.machine(&name).map(Machine::session_active) {
            Some(true) => self.disconnect_machine(&name),
            Some(false) => self.connect_machine(&name),
            None => {
                self.notice = crate::i18n::t("Select a server first").into();
            }
        }
    }

    pub fn open_servers(&mut self) {
        match &mut self.remotes_modal {
            Some(modal) => modal.hidden = false,
            None => {
                self.remotes_modal = Some(RemotesModal {
                    selected: 0,
                    view: ModalView::List,
                    hidden: false,
                    message: self.remotes_problem.clone(),
                });
            }
        }
    }

    pub fn servers_modal_visible(&self) -> bool {
        self.remotes_modal.as_ref().is_some_and(|m| !m.hidden)
    }

    pub fn select_server_row(&mut self, index: usize) {
        let count = self.machines.len();
        let Some(modal) = &mut self.remotes_modal else {
            return;
        };
        match &mut modal.view {
            ModalView::List if index < count => modal.selected = index,
            ModalView::Form(form) if index < FormRow::ALL.len() => form.focus = index,
            _ => {}
        }
    }

    /// Every key stays inside the modal, so typing a name can never trigger an Explorer action.
    pub fn servers_key(&mut self, key: KeyEvent) {
        let Some(modal) = self.remotes_modal.as_ref() else {
            return;
        };
        match &modal.view {
            ModalView::List => self.servers_list_key(key),
            ModalView::ConfirmRemove(name) => {
                let name = name.clone();
                self.servers_confirm_key(key, &name);
            }
            ModalView::Form(_) => self.servers_form_key(key),
        }
    }

    fn servers_list_key(&mut self, key: KeyEvent) {
        let count = self.machines.len();
        let selected = self
            .remotes_modal
            .as_ref()
            .map_or(0, |modal| modal.selected.min(count.saturating_sub(1)));
        let chosen = self.machines.get(selected).cloned();
        let Some(modal) = &mut self.remotes_modal else {
            return;
        };
        modal.message = None;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.remotes_modal = None,
            KeyCode::Char('j') | KeyCode::Down => {
                modal.selected = (selected + 1).min(count.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => modal.selected = selected.saturating_sub(1),
            KeyCode::Char('a') => modal.view = ModalView::Form(Box::new(Form::new())),
            KeyCode::Char('e') => {
                if let Some(machine) = chosen {
                    if machine.managed {
                        modal.view = ModalView::Form(Box::new(Form::from_remote(&machine.remote)));
                    } else {
                        modal.message = Some(
                            crate::i18n::t("This server is in config.toml; edit it there.").into(),
                        );
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(machine) = chosen {
                    if machine.managed {
                        modal.view = ModalView::ConfirmRemove(machine.remote.name);
                    } else {
                        modal.message = Some(
                            crate::i18n::t("This server is in config.toml; remove it there.")
                                .into(),
                        );
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(machine) = chosen {
                    if machine.session_active() {
                        self.disconnect_machine(&machine.remote.name);
                    } else {
                        self.connect_machine(&machine.remote.name);
                    }
                }
            }
            _ => {}
        }
    }

    fn servers_confirm_key(&mut self, key: KeyEvent, name: &str) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.remove_server(name);
                if let Some(modal) = &mut self.remotes_modal {
                    modal.view = ModalView::List;
                    modal.selected = modal.selected.min(self.machines.len().saturating_sub(1));
                }
            }
            KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                if let Some(modal) = &mut self.remotes_modal {
                    modal.view = ModalView::List;
                }
            }
            _ => {}
        }
    }

    fn remove_server(&mut self, name: &str) {
        self.disconnect_machine(name);
        self.machines.retain(|machine| machine.remote.name != name);
        self.context_pins
            .retain(|pin| !pin.starts_with(&format!("{name} / ")));
        if self.machine.as_deref() == Some(name) {
            self.machine = None;
        }
        let saved = self.persist_servers();
        if let Some(modal) = &mut self.remotes_modal {
            modal.message = Some(saved.unwrap_or_else(|| {
                crate::tf!(
                    "Removed {name}. Its key still works on the server until removed from that user's authorized_keys.",
                    name = crate::model::safe_text(name)
                )
            }));
        }
        self.refresh_tree();
    }

    /// `Some` is what the user needs to know: saving failed, or is off for this session.
    fn persist_servers(&self) -> Option<String> {
        let Some(path) = &self.remotes_path else {
            return Some(self.remotes_problem.clone().unwrap_or_else(|| {
                crate::i18n::t("Changes last for this session: saving is turned off.").into()
            }));
        };
        let managed: Vec<_> = self
            .machines
            .iter()
            .filter(|machine| machine.managed)
            .map(|machine| machine.remote.clone())
            .collect();
        remote::save_managed(path, &managed).err().map(|error| {
            crate::tf!(
                "Could not save servers: {error}",
                error = crate::model::safe_text(&format!("{error:#}"))
            )
        })
    }

    fn servers_form_key(&mut self, key: KeyEvent) {
        let Some(modal) = &mut self.remotes_modal else {
            return;
        };
        let ModalView::Form(form) = &mut modal.view else {
            return;
        };
        if form.confirm_discard {
            if matches!(key.code, KeyCode::Char('y')) {
                modal.view = ModalView::List;
            } else {
                form.confirm_discard = false;
            }
            return;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if form.dirty {
                    form.confirm_discard = true;
                } else {
                    modal.view = ModalView::List;
                }
            }
            KeyCode::Tab | KeyCode::Down => move_focus(form, 1),
            KeyCode::BackTab | KeyCode::Up => move_focus(form, -1),
            KeyCode::Char('s') if control => self.save_form(),
            KeyCode::Char('u') if control => {
                if let Some(text) = form.text() {
                    text.clear();
                    form.dirty = true;
                    form.confirm_reconnect = false;
                }
            }
            KeyCode::Enter => match form.row() {
                FormRow::Connect => cycle_connect(form, true),
                FormRow::CreateKey => self.create_key(),
                FormRow::CopySetup => self.copy_setup(),
                FormRow::Test => self.test_connection(),
                FormRow::Save => self.save_form(),
                _ => move_focus(form, 1),
            },
            KeyCode::Left if form.row() == FormRow::Connect => cycle_connect(form, false),
            KeyCode::Right | KeyCode::Char(' ') if form.row() == FormRow::Connect => {
                cycle_connect(form, true)
            }
            KeyCode::Backspace => {
                if let Some(text) = form.text() {
                    text.pop();
                    form.dirty = true;
                    form.confirm_reconnect = false;
                }
            }
            KeyCode::Char(character) if !control && !character.is_control() => {
                if let Some(text) = form.text() {
                    text.push(character);
                    form.dirty = true;
                    form.confirm_reconnect = false;
                }
            }
            _ => {}
        }
    }

    /// Bracketed paste: never pressed as keys, and refused if it holds control characters.
    pub fn servers_paste(&mut self, pasted: &str) {
        let Some(modal) = &mut self.remotes_modal else {
            return;
        };
        let ModalView::Form(form) = &mut modal.view else {
            return;
        };
        let pasted = pasted.trim_end_matches(['\n', '\r']);
        if pasted.chars().any(char::is_control) || pasted.len() > 1024 {
            form.error = Some(
                crate::i18n::t(
                    "Paste contains control characters or exceeds 1024 bytes; nothing pasted",
                )
                .into(),
            );
            return;
        }
        if let Some(text) = form.text() {
            text.push_str(pasted);
            form.dirty = true;
            form.confirm_reconnect = false;
            form.refresh_setup();
        }
    }

    fn form_mut(&mut self) -> Option<&mut Form> {
        match &mut self.remotes_modal.as_mut()?.view {
            ModalView::Form(form) => Some(form),
            _ => None,
        }
    }

    fn save_form(&mut self) {
        let Some(form) = self.form_mut() else {
            return;
        };
        let editing = form.editing.clone();
        let confirmed = form.confirm_reconnect;
        let remote = match form.remote() {
            Ok(remote) => remote,
            Err(error) => {
                form.error = Some(format!("{error:#}"));
                return;
            }
        };
        if self
            .machines
            .iter()
            .any(|m| m.remote.name == remote.name && Some(&m.remote.name) != editing.as_ref())
        {
            if let Some(form) = self.form_mut() {
                form.error = Some(crate::i18n::t("A server with this name already exists").into());
            }
            return;
        }
        let existing = editing
            .as_deref()
            .and_then(|name| self.machine(name))
            .cloned();
        let reconnect = existing.as_ref().is_some_and(|machine| {
            machine.session_active()
                && (machine.remote.name != remote.name || !machine.remote.same_transport(&remote))
        });
        if reconnect && !confirmed {
            if let Some(form) = self.form_mut() {
                form.confirm_reconnect = true;
                form.error = None;
                form.note = Some(
                    crate::i18n::t("Saving reconnects this server. Save again to continue.").into(),
                );
            }
            return;
        }
        if let Some(previous) = &existing {
            if reconnect {
                self.disconnect_machine(&previous.remote.name);
            }
            self.rename_machine(&previous.remote.name, &remote.name);
            if let Some(machine) = self.machine_mut(&previous.remote.name) {
                machine.remote = remote.clone();
            } else if let Some(machine) = self.machine_mut(&remote.name) {
                machine.remote = remote.clone();
            }
        } else {
            self.add_machine(remote.clone(), true);
        }
        if reconnect {
            self.connect_machine(&remote.name);
        }
        let problem = self.persist_servers();
        if let Some(modal) = &mut self.remotes_modal {
            modal.message = problem;
            modal.view = ModalView::List;
            modal.selected = self
                .machines
                .iter()
                .position(|machine| machine.remote.name == remote.name)
                .unwrap_or(0);
        }
        self.refresh_tree();
    }

    fn rename_machine(&mut self, old: &str, new: &str) {
        if old == new {
            return;
        }
        let Some(position) = self.machines.iter().position(|m| m.remote.name == old) else {
            return;
        };
        let anchor = self.anchor_for(new);
        self.expanded
            .insert(format!("{anchor}/{:?}", Scope::Machine(new.to_owned())));
        self.machines[position].anchor = anchor;
        self.machines[position].remote.name = new.to_owned();
        let prefix = format!("{old} / ");
        for pin in &mut self.context_pins {
            if let Some(engine) = pin.strip_prefix(&prefix) {
                *pin = format!("{new} / {engine}");
            }
        }
        if self.machine.as_deref() == Some(old) {
            self.machine = Some(new.to_owned());
        }
    }

    fn copy_setup(&mut self) {
        let Some(form) = self.form_mut() else {
            return;
        };
        form.refresh_setup();
        match form.setup.clone() {
            Ok(command) => {
                form.note = Some(
                    crate::i18n::t("Sent to the clipboard, if your terminal allows it.").into(),
                );
                form.error = None;
                self.clipboard_request = Some(command);
            }
            Err(reason) => form.error = Some(reason),
        }
    }

    fn create_key(&mut self) {
        let Some(form) = self.form_mut() else {
            return;
        };
        let path = form.key.trim().to_owned();
        let command = if path.is_empty() {
            Err(anyhow::anyhow!(crate::i18n::t(
                "Type the path for the new key first, such as ~/.ssh/tha_prod"
            )))
        } else {
            remote::keygen_command(&path)
        };
        let title = crate::tf!(
            "Create SSH key {path}",
            path = crate::model::safe_text(&path)
        );
        self.open_local_tool(&title, "create-key", command);
    }

    fn test_connection(&mut self) {
        let Some(form) = self.form_mut() else {
            return;
        };
        let (title, command) = match form.remote() {
            Ok(remote) => (
                crate::tf!("Test connection to {name}", name = remote.name),
                remote::test_command(&remote),
            ),
            Err(error) => (String::new(), Err(error)),
        };
        self.open_local_tool(&title, "test", command);
    }

    /// OpenSSH needs a real terminal for host key and passphrase prompts, so these run in the
    /// embedded one. The form waits, hidden, and comes back when the command succeeds.
    fn open_local_tool(
        &mut self,
        title: &str,
        kind: &str,
        command: anyhow::Result<std::process::Command>,
    ) {
        let opened = command.and_then(|command| {
            self.sessions
                .open_local_tool(title, kind, command, self.exec_dimensions)
        });
        match opened {
            Ok(_) => {
                if let Some(modal) = &mut self.remotes_modal {
                    modal.hidden = true;
                }
                self.overview_mode = OverviewMode::Containers;
                self.tab = Tab::Exec;
                self.detail_focus = true;
                self.performance_focus = false;
                self.exec_input = true;
                self.notice.clear();
            }
            Err(error) => {
                if let Some(form) = self.form_mut() {
                    form.error = Some(crate::model::safe_text(&format!("{error:#}")));
                }
            }
        }
    }

    /// Handles a local tool that just ended. Returns false for an ordinary exec session.
    pub fn local_tool_finished(&mut self, id: u64) -> bool {
        let Some((_, session)) = self.sessions.iter().find(|(found, _)| *found == id) else {
            return false;
        };
        let Some(kind) = local_tool_kind(&session.request) else {
            return false;
        };
        let kind = kind.to_owned();
        let succeeded = matches!(session.state, crate::terminal::State::Exited(0));
        let status = session.status();
        let screen = session.screen().contents();
        self.exec_input = false;
        if !succeeded {
            self.notice = crate::tf!(
                "{status}. Read the output above, then :servers returns to the form.",
                status = status
            );
            return true;
        }
        // Success has nothing left to read, so return to the form and free the session slot.
        if self.sessions.select(id) {
            self.sessions.close_active();
        }
        self.detail_focus = false;
        self.tab = Tab::Overview;
        let note = if kind == "test" {
            let engines = screen
                .lines()
                .rev()
                .find_map(|line| remote::parse_listing(line.trim().as_bytes()).ok());
            match engines {
                Some(listing) => crate::tf!(
                    "Connection works. Engines: {engines}",
                    engines = listing.engines.join(", ")
                ),
                None => crate::i18n::t("Connection works.").into(),
            }
        } else {
            crate::i18n::t("Key created. Run the setup command on the server.").into()
        };
        if let Some(modal) = &mut self.remotes_modal {
            modal.hidden = false;
        }
        if let Some(form) = self.form_mut() {
            form.refresh_setup();
            form.error = None;
            form.note = Some(note);
        }
        true
    }
}

fn move_focus(form: &mut Form, amount: isize) {
    let count = FormRow::ALL.len() as isize;
    // Leaving a field is when the key path or user may have changed the setup command.
    form.refresh_setup();
    form.focus = (form.focus as isize + amount).rem_euclid(count) as usize;
}

fn cycle_connect(form: &mut Form, forward: bool) {
    let options = [None, Some(ConnectMode::Manual), Some(ConnectMode::Startup)];
    let position = options.iter().position(|o| *o == form.connect).unwrap_or(0);
    let step = if forward { 1 } else { options.len() - 1 };
    form.connect = options[(position + step) % options.len()];
    form.dirty = true;
}

pub fn local_tool_request(title: &str, kind: &str) -> crate::exec::Request {
    crate::exec::Request {
        connection: Connection {
            name: crate::i18n::t("Servers").into(),
            kind: BackendKind::Docker,
            endpoint: String::new(),
        },
        container: crate::model::Container {
            id: format!("{LOCAL_TOOL_PREFIX}{kind}"),
            name: title.to_owned(),
            ..Default::default()
        },
        command: Vec::new(),
    }
}

pub fn local_tool_kind(request: &crate::exec::Request) -> Option<&str> {
    request.container.id.strip_prefix(LOCAL_TOOL_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Snapshot, remote::Update};

    fn app() -> App {
        App::new(
            vec![Connection {
                name: "Docker / local".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///local.sock".into(),
            }],
            100,
            false,
            false,
        )
    }

    fn remote(name: &str) -> Remote {
        Remote {
            name: name.into(),
            host: "10.0.0.5".into(),
            user: "tha".into(),
            identity_file: "/keys/k".into(),
            port: None,
            connect: None,
            metrics_refresh_seconds: None,
        }
    }

    fn with_servers(names: &[&str]) -> App {
        let mut app = app();
        app.load_machines(
            Vec::new(),
            Ok(names.iter().map(|name| remote(name)).collect()),
            None,
        );
        app
    }

    fn connected(app: &mut App, name: &str) {
        app.connect_machine(name);
        let generation = app.machine(name).unwrap().generation;
        app.apply_remote_event(remote::Event {
            server: name.into(),
            generation,
            update: Update::Connected {
                engines: vec!["docker".into()],
                allow_delete: false,
            },
        });
    }

    fn press(app: &mut App, code: KeyCode) {
        app.servers_key(KeyEvent::from(code));
    }

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyCode::Char(character));
        }
    }

    fn titles(app: &App) -> Vec<String> {
        app.rows.iter().map(|row| row.title.clone()).collect()
    }

    #[test]
    fn manual_servers_contact_nothing_at_startup() {
        let mut app = with_servers(&["prod"]);
        app.connect_startup_machines();
        assert!(app.take_remote_commands().is_empty());
        assert!(!app.worker_availability()[1].1, "the anchor has no workers");
    }

    #[test]
    fn startup_mode_connects_and_a_server_can_override_it() {
        let mut app = with_servers(&["manual", "startup"]);
        app.remote_connect = ConnectMode::Startup;
        app.machines[0].remote.connect = Some(ConnectMode::Manual);
        app.connect_startup_machines();
        let commands = app.take_remote_commands();
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0],
            RemoteCommand::Connect { remote, .. } if remote.name == "startup"
        ));
    }

    #[test]
    fn demo_mode_never_contacts_a_server() {
        let mut app = App::new(Vec::new(), 100, false, true);
        app.load_machines(vec![remote("prod")], Ok(Vec::new()), None);
        app.remote_connect = ConnectMode::Startup;
        app.connect_startup_machines();
        app.connect_machine("prod");
        assert!(app.take_remote_commands().is_empty());
    }

    #[test]
    fn disconnecting_stops_every_worker_and_clears_server_data() {
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        let index = app.machine_engines("prod")[0];
        app.update_engine(index, Ok(Snapshot::default()));
        app.take_remote_commands();
        app.take_remote_availability();
        app.disconnect_machine("prod");
        assert_eq!(
            app.take_remote_commands(),
            [RemoteCommand::Disconnect("prod".into())]
        );
        assert_eq!(app.take_remote_availability(), [(index, false)]);
        assert!(app.engines[index].snapshot.is_none());
        assert!(!app.worker_availability()[index].1);
    }

    #[test]
    fn a_late_event_from_a_replaced_session_is_ignored() {
        let mut app = with_servers(&["prod"]);
        app.connect_machine("prod");
        let old = app.machine("prod").unwrap().generation;
        app.disconnect_machine("prod");
        app.apply_remote_event(remote::Event {
            server: "prod".into(),
            generation: old,
            update: Update::Connected {
                engines: vec!["docker".into()],
                allow_delete: true,
            },
        });
        assert_eq!(
            app.machine("prod").unwrap().state,
            MachineState::NotConnected
        );
        assert!(app.machine_engines("prod").is_empty());
    }

    #[test]
    fn a_lost_connection_removes_data_that_could_look_current() {
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        let index = app.machine_engines("prod")[0];
        app.update_engine(index, Ok(Snapshot::default()));
        let generation = app.machine("prod").unwrap().generation;
        app.apply_remote_event(remote::Event {
            server: "prod".into(),
            generation,
            update: Update::Failed {
                reason: "gone".into(),
                retrying: true,
            },
        });
        assert!(app.engines[index].snapshot.is_none());
        assert!(!app.engine_visible(index));
    }

    #[test]
    fn engine_indices_stay_stable_across_reconnects() {
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        let first = app.machine_engines("prod")[0];
        app.disconnect_machine("prod");
        connected(&mut app, "prod");
        assert_eq!(app.machine_engines("prod"), [first]);
    }

    #[test]
    fn config_servers_win_a_duplicate_and_stay_read_only() {
        let mut app = app();
        app.load_machines(vec![remote("prod")], Ok(vec![remote("prod")]), None);
        assert_eq!(app.machines.len(), 1);
        assert!(app.remotes_problem.is_some());
        app.open_servers();
        press(&mut app, KeyCode::Char('e'));
        assert!(matches!(
            app.remotes_modal.as_ref().unwrap().view,
            ModalView::List
        ));
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(
            app.remotes_modal.as_ref().unwrap().view,
            ModalView::List
        ));
        assert!(app.remotes_modal.as_ref().unwrap().message.is_some());
    }

    #[test]
    fn an_unreadable_servers_file_is_never_overwritten() {
        let mut app = app();
        app.load_machines(
            Vec::new(),
            Err(anyhow::anyhow!("broken")),
            Some("/tmp/remotes.toml".into()),
        );
        assert!(app.remotes_path.is_none());
        assert!(app.remotes_problem.as_deref().unwrap().contains("broken"));
    }

    fn fill_form(app: &mut App, name: &str) {
        type_text(app, name);
        press(app, KeyCode::Tab);
        type_text(app, "10.0.0.5");
        press(app, KeyCode::Tab);
        type_text(app, "tha");
        press(app, KeyCode::Tab);
        press(app, KeyCode::Tab);
        type_text(app, "/keys/k");
    }

    #[test]
    fn typed_letters_stay_in_the_field() {
        let mut app = app();
        app.open_servers();
        press(&mut app, KeyCode::Char('a'));
        // Each of these would do something in the list or the Explorer.
        type_text(&mut app, "aedqjkxs");
        let modal = app.remotes_modal.as_ref().unwrap();
        let ModalView::Form(form) = &modal.view else {
            panic!("the form must stay open");
        };
        assert_eq!(form.name, "aedqjkxs");
        assert!(app.take_remote_commands().is_empty());
    }

    #[test]
    fn adding_saves_a_private_file_and_leaves_the_config_alone() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("remotes.toml");
        let config = directory.path().join("config.toml");
        std::fs::write(&config, "# comment\n").unwrap();
        let mut app = app();
        app.load_machines(Vec::new(), Ok(Vec::new()), Some(path.clone()));
        app.open_servers();
        press(&mut app, KeyCode::Char('a'));
        fill_form(&mut app, "prod");
        app.servers_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(app.machines.len(), 1);
        assert_eq!(remote::load_managed(&path).unwrap()[0].name, "prod");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "# comment\n");
        assert!(
            app.take_remote_commands().is_empty(),
            "saving never connects"
        );
    }

    #[test]
    fn invalid_input_keeps_the_form_open_with_the_reason() {
        let mut app = app();
        app.open_servers();
        press(&mut app, KeyCode::Char('a'));
        type_text(&mut app, "prod");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "-oProxyCommand=sh");
        app.servers_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let ModalView::Form(form) = &app.remotes_modal.as_ref().unwrap().view else {
            panic!("the form must stay open");
        };
        assert!(form.error.is_some());
        assert!(app.machines.is_empty());
    }

    #[test]
    fn duplicate_names_are_refused() {
        let mut app = with_servers(&["prod"]);
        app.open_servers();
        press(&mut app, KeyCode::Char('a'));
        fill_form(&mut app, "prod");
        app.servers_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(app.machines.len(), 1);
    }

    #[test]
    fn changing_a_connected_servers_key_warns_then_reconnects() {
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        app.take_remote_commands();
        app.open_servers();
        press(&mut app, KeyCode::Char('e'));
        for _ in 0..4 {
            press(&mut app, KeyCode::Tab);
        }
        type_text(&mut app, "2");
        let save = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        app.servers_key(save);
        assert!(
            app.take_remote_commands().is_empty(),
            "first save only warns"
        );
        assert_eq!(app.machines[0].remote.identity_file, "/keys/k");
        app.servers_key(save);
        assert_eq!(app.machines[0].remote.identity_file, "/keys/k2");
        let commands = app.take_remote_commands();
        assert_eq!(commands[0], RemoteCommand::Disconnect("prod".into()));
        assert!(matches!(
            &commands[1],
            RemoteCommand::Connect { remote, .. } if remote.identity_file == "/keys/k2"
        ));
    }

    #[test]
    fn renaming_moves_pins_and_focus() {
        let mut app = with_servers(&["prod"]);
        app.context_pins = vec!["prod / docker".into()];
        app.machine = Some("prod".into());
        app.open_servers();
        press(&mut app, KeyCode::Char('e'));
        type_text(&mut app, "2");
        app.servers_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(app.machines[0].remote.name, "prod2");
        assert_eq!(app.context_pins, ["prod2 / docker"]);
        assert_eq!(app.machine.as_deref(), Some("prod2"));
    }

    #[test]
    fn escape_asks_before_discarding_typed_text() {
        let mut app = app();
        app.open_servers();
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Esc);
        assert!(
            matches!(app.remotes_modal.as_ref().unwrap().view, ModalView::List),
            "nothing typed, nothing to lose"
        );
        press(&mut app, KeyCode::Char('a'));
        type_text(&mut app, "x");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('n'));
        let ModalView::Form(form) = &app.remotes_modal.as_ref().unwrap().view else {
            panic!("any other key keeps the form");
        };
        assert_eq!(form.name, "x");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            app.remotes_modal.as_ref().unwrap().view,
            ModalView::List
        ));
    }

    #[test]
    fn removing_disconnects_and_notes_the_key_on_the_server() {
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        app.machine = Some("prod".into());
        app.take_remote_commands();
        app.open_servers();
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.machines.is_empty());
        assert!(app.machine.is_none());
        assert_eq!(
            app.take_remote_commands(),
            [RemoteCommand::Disconnect("prod".into())]
        );
    }

    #[test]
    fn pasted_control_characters_are_refused() {
        let mut app = app();
        app.open_servers();
        press(&mut app, KeyCode::Char('a'));
        app.servers_paste("a\u{1b}[31m");
        app.servers_paste("prod\n");
        let ModalView::Form(form) = &app.remotes_modal.as_ref().unwrap().view else {
            panic!("the form must stay open");
        };
        assert_eq!(form.name, "prod");
    }

    #[test]
    fn create_key_never_overwrites_an_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let existing = directory.path().join("key");
        std::fs::write(&existing, "secret").unwrap();
        let mut app = app();
        app.open_servers();
        press(&mut app, KeyCode::Char('a'));
        for _ in 0..4 {
            press(&mut app, KeyCode::Tab);
        }
        type_text(&mut app, existing.to_str().unwrap());
        for _ in 0..2 {
            press(&mut app, KeyCode::Tab);
        }
        press(&mut app, KeyCode::Enter);
        let modal = app.remotes_modal.as_ref().unwrap();
        assert!(!modal.hidden);
        let ModalView::Form(form) = &modal.view else {
            panic!("the form must stay open");
        };
        assert!(form.error.as_deref().unwrap().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "secret");
        assert!(app.sessions.is_empty());
    }

    #[tokio::test]
    async fn a_successful_local_tool_returns_to_the_form_and_frees_its_slot() {
        let mut app = app();
        app.open_servers();
        press(&mut app, KeyCode::Char('a'));
        let mut command = std::process::Command::new("/bin/sh");
        command.args([
            "-c",
            r#"printf '%s\n' '{"engines":["docker"],"allow_delete":false}'"#,
        ]);
        app.open_local_tool("Test", "test", Ok(command));
        assert!(!app.servers_modal_visible());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let id = loop {
            if let Some(notice) = app.sessions.poll().pop() {
                break notice.id;
            }
            assert!(std::time::Instant::now() < deadline, "the tool must end");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert!(app.local_tool_finished(id));
        assert!(app.servers_modal_visible());
        assert!(app.sessions.is_empty());
        let ModalView::Form(form) = &app.remotes_modal.as_ref().unwrap().view else {
            panic!("the form must return");
        };
        assert!(
            form.note.as_deref().unwrap().contains("docker"),
            "{:?}",
            form.note
        );
    }

    #[tokio::test]
    async fn a_failed_local_tool_stays_readable() {
        let mut app = app();
        app.open_servers();
        press(&mut app, KeyCode::Char('a'));
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "echo denied; exit 255"]);
        app.open_local_tool("Test", "test", Ok(command));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let id = loop {
            if let Some(notice) = app.sessions.poll().pop() {
                break notice.id;
            }
            assert!(std::time::Instant::now() < deadline, "the tool must end");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert!(app.local_tool_finished(id));
        assert!(!app.servers_modal_visible(), "the output stays on screen");
        assert_eq!(app.sessions.len(), 1);
        app.open_servers();
        assert!(app.servers_modal_visible());
    }

    #[test]
    fn the_tree_is_unchanged_without_servers() {
        let mut app = app();
        app.refresh_tree();
        assert_eq!(titles(&app), ["Docker / local [connecting]"]);
        assert_eq!(app.rows[0].depth, 0);
        assert_eq!(app.rows[0].scope, Scope::Engine);
    }

    #[test]
    fn machines_group_their_engines_with_local_first() {
        let mut app = with_servers(&["prod", "staging"]);
        connected(&mut app, "prod");
        let rows = &app.rows;
        assert_eq!(rows[0].scope, Scope::Machine(LOCAL_MACHINE.into()));
        assert_eq!(rows[1].title, "Docker / local [connecting]");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].scope, Scope::Machine("prod".into()));
        assert_eq!(rows[3].title, "docker [connecting]");
        assert_eq!(rows[3].depth, 1);
        assert_eq!(rows[4].scope, Scope::Machine("staging".into()));
        assert!(rows[4].title.contains("not connected"));
    }

    #[test]
    fn no_local_engines_means_no_local_row_and_no_panic() {
        let mut app = App::new(Vec::new(), 100, false, false);
        app.load_machines(Vec::new(), Ok(vec![remote("prod")]), None);
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.rows[0].scope, Scope::Machine("prod".into()));
        assert!(app.condition_for(&app.rows[0].clone()).is_some());
    }

    #[test]
    fn enter_focuses_only_that_machine_and_back_restores_the_view() {
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        let position = app
            .rows
            .iter()
            .position(|row| row.scope == Scope::Machine("prod".into()))
            .unwrap();
        app.selected = position;
        app.open_selected();
        assert_eq!(app.machine.as_deref(), Some("prod"));
        assert_eq!(titles(&app), ["docker [connecting]"]);
        assert!(app.back());
        assert!(app.machine.is_none());
        assert_eq!(app.selected, position);
    }

    #[test]
    fn enter_on_a_disconnected_server_connects_and_focuses() {
        let mut app = with_servers(&["prod"]);
        app.selected = app
            .rows
            .iter()
            .position(|row| row.scope == Scope::Machine("prod".into()))
            .unwrap();
        app.open_selected();
        assert_eq!(app.machine.as_deref(), Some("prod"));
        assert_eq!(app.take_remote_commands().len(), 1);
        assert_eq!(app.rows.len(), 1, "the machine row shows its state");
        assert_eq!(app.rows[0].scope, Scope::Machine("prod".into()));
    }

    #[test]
    fn enter_on_this_machine_focuses_local_engines_and_contacts_nothing() {
        let mut app = with_servers(&["prod"]);
        app.selected = 0;
        app.open_selected();
        assert_eq!(app.machine.as_deref(), Some(LOCAL_MACHINE));
        assert_eq!(titles(&app), ["Docker / local [connecting]"]);
        assert!(app.take_remote_commands().is_empty());
    }

    #[test]
    fn a_focused_server_stays_focused_while_offline() {
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        app.selected = app
            .rows
            .iter()
            .position(|row| row.scope == Scope::Machine("prod".into()))
            .unwrap();
        app.open_selected();
        let generation = app.machine("prod").unwrap().generation;
        app.apply_remote_event(remote::Event {
            server: "prod".into(),
            generation,
            update: Update::Failed {
                reason: "lost".into(),
                retrying: true,
            },
        });
        assert_eq!(app.machine.as_deref(), Some("prod"));
        assert_eq!(app.rows[0].scope, Scope::Machine("prod".into()));
    }

    #[test]
    fn the_machine_row_never_acts_on_an_engine() {
        use crate::actions::Action;
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        let index = app.machine_engines("prod")[0];
        app.update_engine(
            index,
            Ok(Snapshot {
                engine_id: Some("id".into()),
                containers: vec![crate::model::Container {
                    id: "a".into(),
                    name: "a".into(),
                    state: "running".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        for scope in [
            Scope::Machine("prod".into()),
            Scope::Machine(LOCAL_MACHINE.into()),
        ] {
            app.selected = app.rows.iter().position(|row| row.scope == scope).unwrap();
            for action in [Action::Start, Action::Stop, Action::Restart, Action::Delete] {
                app.request_action(action);
                assert!(app.action_confirmation.is_none(), "{scope:?} {action:?}");
                assert!(app.action_request.is_none(), "{scope:?} {action:?}");
            }
            assert!(app.selected_containers().is_empty(), "{scope:?}");
        }
    }

    #[test]
    fn filtering_keeps_the_machine_parent_visible() {
        let mut app = with_servers(&["prod", "staging"]);
        connected(&mut app, "prod");
        app.tree_query = "docker".into();
        app.refresh_tree();
        assert!(
            app.rows
                .iter()
                .any(|row| row.scope == Scope::Machine("prod".into()))
        );
        assert!(
            !app.rows
                .iter()
                .any(|row| row.scope == Scope::Machine("staging".into()))
        );
    }

    #[test]
    fn saved_focus_restores_without_connecting_and_ignores_unknown_names() {
        let mut app = with_servers(&["prod"]);
        let mut preferences = crate::preferences::Preferences {
            machine: Some("prod".into()),
            ..Default::default()
        };
        app.apply_preferences(preferences.clone(), true);
        app.finish_preference_restore();
        assert_eq!(app.machine.as_deref(), Some("prod"));
        assert!(app.take_remote_commands().is_empty());

        let mut app = with_servers(&["prod"]);
        preferences.machine = Some("gone".into());
        app.apply_preferences(preferences, true);
        app.finish_preference_restore();
        assert!(app.machine.is_none());
    }

    #[test]
    fn compose_is_refused_on_a_server_before_any_local_path_check() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("compose.yaml");
        std::fs::write(&file, "services: {}\n").unwrap();
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        let index = app.machine_engines("prod")[0];
        app.update_engine(
            index,
            Ok(Snapshot {
                engine_id: Some("id".into()),
                containers: vec![crate::model::Container {
                    id: "a".into(),
                    name: "shop-api-1".into(),
                    state: "running".into(),
                    project: Some("shop".into()),
                    service: Some("api".into()),
                    compose_working_dir: Some(directory.path().to_path_buf()),
                    compose_config_files: vec![file],
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        app.selected = app
            .rows
            .iter()
            .position(
                |row| matches!(&row.scope, Scope::Project { project, .. } if project == "shop"),
            )
            .expect("the project row");
        app.request_compose(crate::compose::Action::Up);
        assert!(app.compose_confirmation.is_none());
        assert!(
            app.notice.contains("not available on servers"),
            "{}",
            app.notice
        );
    }

    #[test]
    fn the_startup_project_match_ignores_servers() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = with_servers(&["prod"]);
        connected(&mut app, "prod");
        let index = app.machine_engines("prod")[0];
        app.update_engine(
            index,
            Ok(Snapshot {
                containers: vec![crate::model::Container {
                    id: "a".into(),
                    project: Some("shop".into()),
                    compose_working_dir: Some(directory.path().to_path_buf()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert!(crate::workspace::directory_matches(&app.engines, directory.path()).is_empty());
    }
}
