use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use futures_util::StreamExt;
use std::{io::IsTerminal, path::PathBuf, time::Duration};
use tha::{
    app::{App, InputMode, MouseTarget, OverviewMode, Tab},
    backend,
    cli::Cli,
    config::{BackendKind, Config, Connection, parse_connection},
    demo, discovery,
    logs::{self, LogEvent},
    metrics,
    model::Snapshot,
    shortcuts::{self, ActionId as A, NavigationContext},
    ui,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

#[tokio::main]
async fn main() {
    if let Err(error) = start().await {
        eprintln!("tha: {error:#}");
        std::process::exit(1);
    }
}

async fn start() -> Result<()> {
    let cli = Cli::parse();
    if let Some(tha::cli::Command::RemoteExec { socket, request }) = &cli.command {
        let code = tha::remote_exec::client(socket, request).await?;
        std::process::exit(code);
    }
    if let Some(tha::cli::Command::RemoteShell { remote, control }) = &cli.command {
        let code = tha::remote_exec::host_client(&serde_json::from_str(remote)?, control).await?;
        std::process::exit(code);
    }
    let mut config = Config::load(cli.config.as_ref())?;
    tha::history::set_retention_hours(config.history_retention_hours);
    config.monochrome |= cli.monochrome || std::env::var_os("NO_COLOR").is_some();
    if let Some(command) = &cli.command {
        match command {
            tha::cli::Command::Ps { json }
            | tha::cli::Command::Images { json }
            | tha::cli::Command::Volumes { json } => {
                let kind = match command {
                    tha::cli::Command::Ps { .. } => tha::query::Kind::Containers,
                    tha::cli::Command::Images { .. } => tha::query::Kind::Images,
                    _ => tha::query::Kind::Volumes,
                };
                let found = connections(&cli, &config, false)?;
                if !tha::query::run(kind, *json, found).await? {
                    std::process::exit(1);
                }
                return Ok(());
            }
            tha::cli::Command::Update { check } => {
                tha::update::run(*check).await?;
                return Ok(());
            }
            tha::cli::Command::RemoteExec { .. } => unreachable!(),
            tha::cli::Command::RemoteShell { .. } => unreachable!(),
            tha::cli::Command::Module { action } => {
                anyhow::ensure!(
                    !cli.standalone && !cli.demo,
                    "Modules extend service mode; omit --standalone and --demo"
                );
                match action {
                    tha::cli::ModuleAction::Install { name } => {
                        anyhow::ensure!(
                            name == "security",
                            "Unknown module '{name}'; available modules: security"
                        );
                        let database = tha::history::path(&config)?;
                        let mut ready = false;
                        for _ in 0..50 {
                            if tha::history::collector_running(&database)? {
                                ready = true;
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        anyhow::ensure!(
                            ready,
                            "Security requires service mode: run tha service install and tha service start first"
                        );
                        tha::security::install(name).await?
                    }
                    tha::cli::ModuleAction::Enable { name } => {
                        tha::security::set_enabled(name, true)?;
                        println!("{name}: enabled (service module)");
                    }
                    tha::cli::ModuleAction::Disable { name } => {
                        tha::security::set_enabled(name, false)?;
                        println!("{name}: disabled; saved reports and resources retained");
                    }
                    tha::cli::ModuleAction::Status => {
                        println!(
                            "security: {}",
                            if tha::security::enabled() {
                                "enabled (service module)"
                            } else if tha::security::installed() {
                                "disabled (resources retained)"
                            } else {
                                "not installed"
                            }
                        );
                        if tha::security::enabled() {
                            println!(
                                "security worker: {}",
                                match tha::security::service_status().await {
                                    Ok(true) => "running · queue active",
                                    Ok(false) => "running · idle",
                                    Err(_) => "unavailable; start/restart the per-user service",
                                }
                            );
                        }
                    }
                }
                return Ok(());
            }
            tha::cli::Command::Daemon => {
                if cli.demo || cli.standalone {
                    bail!("daemon cannot be combined with --demo or --standalone");
                }
                let connections = connections(&cli, &config, false)?;
                return tha::daemon::run(
                    config,
                    connections,
                    tha::remote::managed_path(cli.config.as_deref()),
                )
                .await;
            }
            tha::cli::Command::Service { action } => {
                return tha::service::run(*action, cli.config.as_deref()).await;
            }
            tha::cli::Command::History { action } => {
                let path = tha::history::path(&config)?;
                match action {
                    tha::cli::HistoryAction::Status => {
                        println!("{}\n{}", path.display(), tha::history::status(&path)?)
                    }
                    tha::cli::HistoryAction::Compact => {
                        let budget = config.history_disk_mb * 1024 * 1024;
                        tokio::task::spawn_blocking(move || -> Result<()> {
                            let store = tha::history::Store::open(&path, budget)?;
                            while store.prune_batch(tha::history::now_ms())? > 0 {}
                            store.compact()?;
                            println!(
                                "Compacted {} ({} bytes)",
                                path.display(),
                                store.size_bytes()
                            );
                            Ok(())
                        })
                        .await??;
                    }
                }
                return Ok(());
            }
        }
    }
    let connections = if cli.demo {
        demo::connections()
    } else {
        connections(&cli, &config, !cli.list_connections)?
    };
    if cli.list_connections {
        let discovery = if cli.demo || !config.discover_connections {
            Default::default()
        } else {
            discovery::podman::discover().await
        };
        let native = discovery.machines.into_iter().filter_map(|machine| {
            machine.endpoint.ok().map(|endpoint| Connection {
                name: format!("Podman / {}", machine.name),
                kind: BackendKind::Docker,
                endpoint,
            })
        });
        for connection in discovery::deduplicate(connections.into_iter().chain(native).collect()) {
            println!(
                "{}\t{:?}\t{}",
                tha::model::safe_text(&connection.name),
                connection.kind,
                tha::model::safe_text(&connection.endpoint)
            );
        }
        if let Some(warning) = discovery.warning {
            eprintln!("tha: {}", tha::model::safe_text(&warning));
        }
        return Ok(());
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!(
            "The UI needs an interactive terminal. Run tha in a terminal, or use --list-connections."
        );
    }
    let mut app = App::new(
        connections,
        config.log_capacity,
        config.monochrome,
        cli.demo,
    );
    app.discover_connections = config.discover_connections;
    app.service_modules = !cli.standalone;
    app.security.enabled = app.service_modules && tha::security::enabled();
    if cli.demo {
        demo::populate(&mut app);
        if cli.demo_servers {
            demo::populate_servers(&mut app);
        }
    }
    if !cli.demo && !cli.standalone {
        let path = tha::history::path(&config)?;
        app.auto_history_database = Some(path.clone());
        if path.exists() {
            app.history_database = Some(path.clone());
        }
        if tha::history::collector_running(&path)? {
            app.metrics_database = Some(path);
        }
    }
    app.exec_command = config.exec_command;
    app.sessions.set_limit(config.max_exec_sessions)?;
    app.local_metrics_refresh_seconds = config.metrics_refresh_seconds;
    app.metrics_refresh_seconds = if app.metrics_database.is_some() {
        1
    } else {
        config.metrics_refresh_seconds
    };
    app.context_pins = config.context_order;
    if !cli.demo {
        let path = tha::remote::managed_path(cli.config.as_deref());
        let managed = path
            .as_deref()
            .map_or_else(|| Ok(Vec::new()), tha::remote::load_managed);
        let path = path.filter(|_| config.save_preferences);
        app.load_machines(std::mem::take(&mut config.remotes), managed, path);
    }
    app.refresh_tree();
    if !cli.demo {
        app.selected = 0;
    }
    if !cli.demo && !cli.overview {
        app.configure_startup(std::env::current_dir().context("Cannot read current directory")?);
        if app.discover_connections {
            app.wait_for_machine_discovery();
        }
    }
    tha::keymap::Keymap::parse(&config.bindings)?;
    app.custom_bindings = config.bindings;
    app.alert_rules = config.alerts;
    app.vscode_path = config.vscode_path;
    app.check_updates = config.check_updates && !cli.demo;
    app.config_path = cli
        .config
        .clone()
        .or_else(tha::config::default_path)
        .filter(|_| !cli.demo);
    app.config_modified = app.config_path.as_ref().and_then(|path| {
        std::fs::metadata(path)
            .and_then(|data| data.modified())
            .ok()
    });
    app.apply_base_settings(tha::settings::BaseSettings {
        theme: config.theme,
        keyboard: config.keyboard,
        mouse: config.mouse,
        log_wrap: config.log_wrap,
        language: config.language,
        remote_connect: config.remote_connect,
        editor: config.editor,
    });
    if config.save_preferences && !cli.demo {
        app.saved_views_path = tha::saved_views::default_path();
        if let Some(path) = &app.saved_views_path {
            match tha::saved_views::load(path) {
                Ok(views) => app.saved_views = views,
                Err(error) => {
                    eprintln!("tha: saved views ignored (existing file preserved): {error:#}");
                    app.saved_views_path = None;
                }
            }
        }
        app.preferences_path = tha::preferences::default_path();
        if let Some(path) = &app.preferences_path {
            match tha::preferences::load(path) {
                Ok(Some(preferences)) => {
                    app.apply_preferences(preferences, !cli.overview);
                    app.finish_preference_restore();
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("tha: preferences ignored (existing file preserved): {error:#}");
                    app.preferences_path = None;
                }
            }
        }
    }
    run(app, config.refresh_seconds).await
}

/// `allow_empty` is for the UI, where servers can be the only engines or be added later.
fn connections(cli: &Cli, config: &Config, allow_empty: bool) -> Result<Vec<Connection>> {
    let mut connections = config.connections.clone();
    for value in &cli.connection {
        connections.push(parse_connection(value, BackendKind::Docker)?);
    }
    for value in &cli.containerd {
        connections.push(parse_connection(value, BackendKind::Containerd)?);
    }
    if let Ok(endpoint) = std::env::var("DOCKER_HOST")
        && !endpoint.is_empty()
    {
        connections.push(Connection {
            name: "Docker / DOCKER_HOST".into(),
            kind: BackendKind::Docker,
            endpoint,
        });
    }
    if config.discover_connections {
        connections.extend(tha::collector::discover_local());
    }
    if connections.is_empty() {
        if !config.discover_connections {
            if allow_empty {
                return Ok(connections);
            }
            bail!("No connections configured; add [[connections]] or enable discover_connections");
        }
        connections.push(Connection {
            name: "Docker / local".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///var/run/docker.sock".into(),
        });
    }
    Ok(discovery::deduplicate(connections))
}

enum EditEvent {
    Fetched {
        location: Box<tha::compose_edit::Location>,
        fetched: tha::compose_edit::Fetched,
    },
    Notice(String),
    Saved {
        location: Box<tha::compose_edit::Location>,
        message: String,
    },
    OpenFailed {
        location: Box<tha::compose_edit::Location>,
        message: String,
    },
    NetworkTest(Result<tha::files::NetTest, String>),
}

/// The explorer tree a file being edited belongs to, and its path there.
fn edited_file(location: &tha::compose_edit::Location) -> Option<(String, &str)> {
    use tha::{compose_edit::Location, files::Source};
    match location {
        Location::Local(_) => None,
        Location::Container { source, path } => Some((source.key(), path)),
        Location::Server {
            remote,
            control,
            path,
        } => {
            let source = Source::Server {
                remote: remote.clone(),
                control: control.clone(),
                label: remote.name.clone(),
            };
            Some((source.key(), path))
        }
    }
}

/// Gives the terminal to the editor until it exits. Other work keeps running, and the screen and
/// key handling come back whatever happened.
async fn hand_off(
    terminal: &mut ratatui::DefaultTerminal,
    events: &mut Option<EventStream>,
    editor: tha::editor::Editor,
    vscode_path: Option<&std::path::Path>,
    path: &std::path::Path,
    mouse: bool,
) -> Result<()> {
    let Some(mut command) = editor.command(path, vscode_path) else {
        bail!(
            "{}",
            tha::tf!(
                "{editor} is not installed; choose another editor in Settings (,)",
                editor = editor.label()
            )
        );
    };
    *events = None;
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste
    );
    ratatui::restore();
    if editor == tha::editor::Editor::VsCode {
        println!(
            "{}",
            tha::i18n::t("Waiting for VS Code. Close the file's tab to return to Tha.")
        );
    }
    let status = tokio::task::spawn_blocking(move || command.status()).await;
    // Key input comes back first, whatever else fails: with none, the event loop would quit.
    *events = Some(EventStream::new());
    // The same terminal object: making a new one would also read the cursor position.
    crossterm::terminal::enable_raw_mode().context("Cannot restore the terminal")?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableBracketedPaste
    )?;
    if mouse {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    }
    // Forces a full repaint. `Terminal::clear` would read the cursor position first, which can
    // time out here, and a repaint does not need it.
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    )?;
    terminal.current_buffer_mut().reset();
    terminal.swap_buffers();
    terminal.current_buffer_mut().reset();
    let status = status
        .context("The editor did not finish")?
        .context("Cannot start the editor")?;
    anyhow::ensure!(
        status.success(),
        "{}",
        tha::tf!("{editor} exited with an error", editor = editor.label())
    );
    Ok(())
}

struct TerminalRestore;
impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste
        );
        ratatui::restore();
    }
}

struct AbortTask(JoinHandle<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct SnapshotEvent {
    engine: usize,
    generation: u64,
    result: Result<Snapshot, String>,
}

#[derive(Default)]
struct EngineWorkers {
    connection: Option<Connection>,
    generation: u64,
    tasks: Vec<AbortTask>,
}

impl EngineWorkers {
    fn accepts(&self, generation: u64) -> bool {
        self.connection.is_some() && self.generation == generation
    }
}

struct WorkerChannels {
    snapshots: mpsc::Sender<SnapshotEvent>,
    metrics: mpsc::Sender<metrics::Event>,
    refresh: watch::Receiver<u64>,
}

fn sync_workers(
    app: &mut App,
    availability: &[(usize, bool)],
    workers: &mut Vec<EngineWorkers>,
    targets: &mut Vec<watch::Sender<Option<metrics::Target>>>,
    channels: &WorkerChannels,
    refresh_seconds: u64,
) {
    while workers.len() < app.engines.len() {
        workers.push(EngineWorkers::default());
        targets.push(watch::channel(None).0);
    }
    for &(index, available) in availability {
        let engine = &mut app.engines[index];
        let slot = &mut workers[index];
        let connection = available.then_some(&engine.connection);
        if slot.connection.as_ref() == connection {
            continue;
        }
        slot.tasks.clear(); // Drop aborts both old-endpoint workers.
        slot.generation = slot.generation.wrapping_add(1);
        slot.connection = connection.cloned();
        engine.metrics_revision = engine.metrics_revision.wrapping_add(1);
        targets[index].send_replace(None);
        if !available {
            continue;
        }
        let connection = engine.connection.clone();
        // The daemon never collects from servers, so their metrics are always sampled here.
        let server_seconds = tha::remote::server_of(&connection)
            .and_then(|server| app.machines.iter().find(|m| m.remote.name == server))
            .map(|machine| machine.remote.metrics_seconds());
        if let Some(seconds) = server_seconds {
            slot.tasks.push(AbortTask(tokio::spawn(metrics::worker(
                index,
                connection.clone(),
                targets[index].subscribe(),
                channels.metrics.clone(),
                seconds,
            ))));
        } else if let Some(path) = &app.metrics_database {
            slot.tasks
                .push(AbortTask(tokio::spawn(tha::daemon::metrics_worker(
                    index,
                    connection.clone(),
                    targets[index].subscribe(),
                    channels.metrics.clone(),
                    path.clone(),
                ))));
        } else {
            slot.tasks.push(AbortTask(tokio::spawn(metrics::worker(
                index,
                connection.clone(),
                targets[index].subscribe(),
                channels.metrics.clone(),
                app.metrics_refresh_seconds,
            ))));
        }
        let sender = channels.snapshots.clone();
        let mut refresh = channels.refresh.clone();
        let generation = slot.generation;
        slot.tasks.push(AbortTask(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(refresh_seconds));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { _ = interval.tick() => {}, changed = refresh.changed() => { if changed.is_err() { break; } } }
                let result = backend::snapshot(&connection).await.map_err(|error| format!("{error:#}"));
                if sender.send(SnapshotEvent { engine: index, generation, result }).await.is_err() { break; }
            }
        })));
    }
}

/// Output while typing should echo at once; a noisy command stays batched like logs do.
const EXEC_ECHO_GAP: Duration = Duration::from_millis(16);
const EXEC_BATCH_GAP: Duration = Duration::from_millis(70);
const EXEC_ECHO_WINDOW: Duration = Duration::from_millis(200);

/// Reads every session's output. Returns whether the screen needs drawing.
fn poll_exec_sessions(app: &mut App) -> bool {
    let mut redraw = false;
    for notice in app.sessions.poll() {
        redraw = true;
        if app.local_tool_finished(notice.id) {
            continue;
        }
        if Some(notice.id) == app.sessions.active_id() {
            app.exec_input = false;
        }
        app.notice = tha::tf!(
            "Exec #{a0} {a1} · container was not stopped. F6 switches retained sessions.",
            a0 = notice.id,
            a1 = notice.status
        );
    }
    if app.tab == Tab::Exec && app.sessions.take_active_output_changed() {
        redraw = true;
    }
    redraw
}

async fn run(mut app: App, refresh_seconds: u64) -> Result<()> {
    // Not `ratatui::try_init`: its panic hook would restore the terminal for a panic on any
    // thread, leaving this loop drawing on a terminal it no longer has. See `tha::crash`.
    let _restore = TerminalRestore;
    crossterm::terminal::enable_raw_mode().context("Cannot initialize terminal")?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)
        .context("Cannot initialize terminal")?;
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))
            .context("Cannot initialize terminal")?;
    let crash_log = tha::crash::log_path();
    tha::crash::install(crash_log.clone());
    let mut background_panics = 0;
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    let mut mouse_captured = app.mouse_enabled;
    if mouse_captured {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    }
    // Dropped while an editor has the terminal, or both would read the same keystrokes.
    let mut events = Some(EventStream::new());
    let (edit_tx, mut edit_rx) = mpsc::channel::<EditEvent>(4);
    let mut edit_tasks: Vec<AbortTask> = Vec::new();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    // The terminal closed or the SSH session dropped. Without this nothing would clean up, and
    // a download in progress would be left behind.
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let (snapshot_tx, mut snapshot_rx) = mpsc::channel::<SnapshotEvent>(32);
    let (refresh_tx, refresh_rx) = watch::channel(0u64);
    let (log_tx, mut log_rx) = mpsc::channel::<LogEvent>(512);
    let (metrics_tx, mut metrics_rx) = mpsc::channel::<metrics::Event>(32);
    let (platform_tx, mut platform_rx) = mpsc::channel::<tha::platform::Event>(16);
    let mut platform_task: Option<(tha::platform::Key, AbortTask)> = None;
    let (layers_tx, mut layers_rx) = mpsc::channel::<tha::layers::Event>(4);
    let mut layers_task: Option<AbortTask> = None;
    let (diagnostics_tx, mut diagnostics_rx) = mpsc::channel::<tha::diagnostics::Event>(4);
    let (downloads_tx, mut downloads_rx) = mpsc::channel::<tha::downloads::Event>(64);
    let mut download_lists: Vec<AbortTask> = Vec::new();
    // Inspecting and counting for the dialog, the preview, and an explicit file search.
    let mut download_dialog: [Option<AbortTask>; 4] = [None, None, None, None];
    let mut download_jobs: std::collections::HashMap<u64, AbortTask> = Default::default();
    if app.check_updates {
        let sender = edit_tx.clone();
        edit_tasks.push(AbortTask(tokio::spawn(async move {
            if let Ok(latest) = tha::update::latest().await
                && tha::update::newer(&latest, tha::update::current())
            {
                let message = tha::tf!(
                    "Tha {version} is available; run tha update",
                    version = latest
                );
                let _ = sender.send(EditEvent::Notice(message)).await;
            }
        })));
    }
    let mut alerts_checked_at = std::time::Instant::now();
    let mut alerts_read_at: Option<std::time::Instant> = None;
    let mut alert_reads: Vec<AbortTask> = Vec::new();
    let (fleet_tx, mut fleet_rx) = mpsc::channel::<tha::fleet::Vulnerabilities>(1);
    let (diagnostic_logs_tx, mut diagnostic_logs_rx) =
        mpsc::channel::<tha::diagnostics::LogLoadEvent>(1);
    // One per side, so a new peer or a refresh aborts what it replaces. No other inspect runs.
    let mut diagnostics_tasks: [Option<AbortTask>; 5] = [None, None, None, None, None];
    let (security_tx, mut security_rx) = mpsc::channel::<tha::security::Event>(16);
    let (security_notice_tx, mut security_notice_rx) = mpsc::channel::<String>(4);
    let (schedule_tx, mut schedule_rx) = mpsc::channel::<tha::security::ScheduleEvent>(4);
    let mut security_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut security_cancel: Option<watch::Sender<bool>> = None;
    let (export_tx, mut export_rx) = mpsc::channel::<String>(4);
    let (removal_tx, mut removal_rx) = mpsc::channel::<tha::backend::RemovalReport>(1);
    let mut removal_task: Option<AbortTask> = None;
    let mut export_task: Option<AbortTask> = None;
    let (compose_tx, mut compose_rx) = mpsc::channel::<tha::compose::Event>(64);
    let mut compose_task: Option<AbortTask> = None;
    let (event_tx, mut event_rx) = mpsc::channel::<tha::events::Update>(64);
    let mut event_watchers = std::collections::HashMap::<String, AbortTask>::new();
    let mut event_read: Option<AbortTask> = None;
    let mut event_read_at = std::time::Instant::now() - Duration::from_secs(60);
    let (image_tx, mut image_rx) = mpsc::channel::<tha::images::Event>(32);
    let mut image_check_task: Option<AbortTask> = None;
    let mut image_pull_task: Option<AbortTask> = None;
    let (action_tx, mut action_rx) = mpsc::channel::<tha::actions::Update>(64);
    let mut action_tasks = std::collections::BTreeMap::<u64, AbortTask>::new();
    let mut metric_targets = Vec::new();
    let mut workers = Vec::new();
    let channels = WorkerChannels {
        snapshots: snapshot_tx,
        metrics: metrics_tx,
        refresh: refresh_rx.clone(),
    };
    let (discovery_tx, mut discovery_rx) = mpsc::channel(1);
    let mut discovery_task = None;
    let (remote_tx, mut remote_rx) = mpsc::channel::<tha::remote::Event>(16);
    let mut remote_tasks = std::collections::HashMap::<String, AbortTask>::new();
    if !app.demo {
        if !app.machines.is_empty() {
            tha::remote::sweep_stale_sessions().await;
        }
        app.connect_startup_machines();
        let available = app.worker_availability();
        sync_workers(
            &mut app,
            &available,
            &mut workers,
            &mut metric_targets,
            &channels,
            refresh_seconds,
        );
        if app.discover_connections {
            let mut refresh = refresh_rx;
            discovery_task = Some(AbortTask(tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(refresh_seconds));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! { _ = interval.tick() => {}, changed = refresh.changed() => { if changed.is_err() { break; } } }
                    if discovery_tx
                        .send(discovery::podman::discover().await)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            })));
        }
    }
    let mut log_task: Option<AbortTask> = None;
    let (history_tx, mut history_rx) = mpsc::channel::<tha::history::Event>(1);
    let mut history_task: Option<AbortTask> = None;
    let mut history_request: Option<(String, String, usize, Option<i64>)> = None;
    let mut history_updated = std::time::Instant::now() - Duration::from_secs(2);
    let mut source_checked = std::time::Instant::now();
    let log_capacity = app.logs.capacity();
    let mut redraw = tokio::time::interval(Duration::from_millis(70));
    redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let frame_started = std::time::Instant::now();
    terminal.draw(|frame| ui::draw(frame, &mut app))?;
    let mut visual_deadline = app.next_visual_deadline(frame_started);
    let mut security_refresh = tokio::time::interval(Duration::from_secs(5));
    let exec_output = app.sessions.output_ready();
    let mut exec_poll_at: Option<std::time::Instant> = None;
    let mut exec_polled = std::time::Instant::now();
    let mut exec_typed = std::time::Instant::now() - EXEC_ECHO_WINDOW;
    loop {
        let mut redraw_now = false;
        if tha::crash::background_panics() != background_panics {
            background_panics = tha::crash::background_panics();
            app.notice = tha::tf!(
                "Part of Tha stopped unexpectedly; restart Tha if something looks wrong. Details: {path}",
                path = crash_log
                    .as_deref()
                    .unwrap_or(std::path::Path::new("crash.log"))
                    .display()
            );
            redraw_now = true;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            _ = hangup.recv() => break,
            _ = security_refresh.tick() => { redraw_now = app.security.refresh_saved(); },
            event = async { events.as_mut()?.next().await } => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    exec_typed = std::time::Instant::now();
                    let height = terminal.size()?.height.saturating_sub(5 + ui::footer_height(&app)) as usize;
                    if handle_key(&mut app, key, height, &refresh_tx) { break; }
                    redraw_now = true;
                },
                Some(Ok(Event::Mouse(mouse))) if mouse.kind != MouseEventKind::Moved => {
                    let height = terminal.size()?.height.saturating_sub(5 + ui::footer_height(&app)) as usize;
                    if handle_mouse(&mut app, mouse, height, &refresh_tx) { break; }
                    redraw_now = true;
                },
                Some(Ok(Event::Paste(text))) => {
                    exec_typed = std::time::Instant::now();
                    redraw_now = true;
                    app.cancel_startup();
                    if app.servers_modal_visible() {
                        app.servers_paste(&text);
                    } else if let Some(confirmation) = &mut app.action_confirmation {
                        if confirmation.request.action == tha::actions::Action::Delete {
                            if text.chars().any(char::is_control) || confirmation.typed.len() + text.len() > 1024 {
                                confirmation.error = Some(tha::i18n::t("Paste contains control characters or exceeds 1024 bytes; nothing pasted").into());
                            } else { confirmation.typed.push_str(&text); }
                        }
                    } else if app.exec_input && app.tab == Tab::Exec {
                        if let Some(session) = app.sessions.active_mut() && let Err(error) = session.paste(&text) { app.notice = tha::tf!("{error}", error = format!("{error:#}")); }
                    } else if app.downloads.open.is_some() && app.downloads.search_input.is_some() {
                        app.downloads.append_search(&text);
                    } else if app.downloads.open.is_some() && app.downloads.new_file_input.is_some() {
                        app.downloads.append_new_file(&text);
                    } else if app.input_mode != InputMode::Normal {
                        let text = tha::model::safe_text(&text);
                        for ch in text.chars() { if app.input.len() + ch.len_utf8() > 1024 { break; } app.input.push(ch); }
                    }
                },
                Some(Err(error)) => return Err(error.into()),
                Some(Ok(Event::Resize(_, _))) => { app.action_review_visible = false; app.log_export_review_visible = false; redraw_now = true; },
                None => break,
                _ => {}
            },
            _ = exec_output.notified() => {
                if app.tab == Tab::Exec && exec_poll_at.is_none() {
                    let gap = if exec_typed.elapsed() < EXEC_ECHO_WINDOW { EXEC_ECHO_GAP } else { EXEC_BATCH_GAP };
                    exec_poll_at = Some(exec_polled + gap);
                }
            },
            _ = async { tokio::time::sleep_until(exec_poll_at.unwrap_or_else(std::time::Instant::now).into()).await }, if exec_poll_at.is_some() => {
                exec_poll_at = None;
                exec_polled = std::time::Instant::now();
                redraw_now = poll_exec_sessions(&mut app);
            },
            Some(event) = snapshot_rx.recv() => {
                if !workers.get(event.engine).is_some_and(|slot| slot.accepts(event.generation)) { continue; }
                app.update_engine(event.engine, event.result);
                publish_metric_targets(&mut app, &metric_targets);
                sync_event_watchers(&app, &mut event_watchers, &event_tx);
                redraw_now = true;
            },
            Some(event) = remote_rx.recv() => {
                app.apply_remote_event(event);
                redraw_now = true;
            },
            Some(discovery) = discovery_rx.recv() => {
                if let Some(warning) = &discovery.warning { app.notice = warning.clone(); }
                let availability = app.reconcile_podman(&discovery);
                sync_workers(&mut app, &availability, &mut workers, &mut metric_targets, &channels, refresh_seconds);
                publish_metric_targets(&mut app, &metric_targets);
                redraw_now = true;
            },
            Some(event) = metrics_rx.recv() => { app.apply_metrics(event); redraw_now = true; },
            Some(event) = history_rx.recv() => {
                if event.generation == app.history_view.generation {
                    app.history_view.loading = false;
                    match event.data { Ok(data) => { app.history_view.data = data; app.history_view.error=None; }, Err(error) => app.history_view.error=Some(error) }
                    history_task=None;
                    redraw_now=true;
                }
            },
            Some(event) = compose_rx.recv() => {
                redraw_now = true;
                if let Some(job) = &mut app.compose_job {
                    match event {
                        tha::compose::Event::Line(line) => job.push(line),
                        tha::compose::Event::Finished(outcome) => {
                            app.notice = match &outcome {
                                Ok(()) => tha::tf!("Finished: {a0}", a0 = job.title),
                                Err(error) => tha::tf!("Failed: {a0} · {error} · :compose-output shows details", a0 = job.title, error = error),
                            };
                            job.outcome = Some(outcome);
                            compose_task = None;
                            refresh_tx.send_modify(|value| *value = value.wrapping_add(1));
                        }
                    }
                }
            },
            Some(update) = event_rx.recv() => {
                if let Some(engine) = app.engines.iter_mut().find(|engine| engine.connection.endpoint == update.endpoint) {
                    for event in update.events { engine.events.add(event); }
                }
                redraw_now = matches!(app.tab, Tab::Events | Tab::History);
            },
            Some(event) = image_rx.recv() => {
                redraw_now = true;
                match event {
                    tha::images::Event::Checked { engine, image, status } => {
                        app.image_updates.insert((engine, image), status);
                    }
                    tha::images::Event::CheckFinished(summary) => {
                        app.notice = summary;
                        image_check_task = None;
                    }
                    tha::images::Event::PullProgress(text) => app.notice = text,
                    tha::images::Event::PullFinished { engine, image, result } => {
                        app.image_pull_active = false;
                        app.notice = result;
                        app.image_updates.remove(&(engine, image));
                        image_pull_task = None;
                        refresh_tx.send_modify(|value| *value = value.wrapping_add(1));
                    }
                }
            },
            Some(result) = export_rx.recv() => { app.log_export_busy = false; app.notice = result; export_task = None; redraw_now = true; },
            Some(report)=removal_rx.recv()=> {
                if let Some(removal)=app.inventory_removal.take() && let Some(view)=app.inventory_views.get_mut(&removal.context) {
                    for target in removal.targets {view.marked.remove(&target.request.key);}
                }
                let (_,skipped,failed)=report.counts();
                app.notice=report.summary();
                app.inventory_removal_results_visible=skipped+failed>0;
                app.inventory_removal_results_scroll=0;
                app.inventory_removal_report=Some(report);
                removal_task=None;
                refresh_tx.send_modify(|value|*value=value.wrapping_add(1));
                redraw_now=true;
            },
            Some(update) = action_rx.recv() => {
                let operation = update.operation;
                let finished = matches!(&update.event, tha::actions::Event::Finished);
                if app.apply_action_update(update) {
                    redraw_now = true;
                    if finished { action_tasks.remove(&operation); }
                    refresh_tx.send_modify(|generation| *generation = generation.wrapping_add(1));
                }
            },
            Some(event) = platform_rx.recv() => { app.platforms.insert(event.key, match event.result { Ok(platform) => tha::platform::State::Ready(platform), Err(error) => tha::platform::State::Unavailable(error) }); redraw_now = true; },
            Some(event) = layers_rx.recv() => {
                app.layers.complete(event);
                redraw_now = true;
            },
            Some(event) = diagnostics_rx.recv() => {
                let now = tha::history::now_ms();
                if event.role == tha::diagnostics::Role::Overview {
                    if let Some(side) = &mut app.overview_details {
                        side.complete(event, now);
                    }
                } else if let Some(view) = &mut app.diagnostics {
                    view.complete(event, now);
                }
                redraw_now = true;
            },
            Some(event) = downloads_rx.recv() => {
                if let tha::downloads::Event::Finished { job, .. } = &event {
                    download_jobs.remove(job);
                }
                app.downloads.apply(event, std::time::Instant::now());
                redraw_now = true;
            },
            Some(event) = edit_rx.recv() => {
                match event {
                    EditEvent::Fetched { location, mut fetched } => {
                        let editor = app.editor;
                        let vscode = app.vscode_path.clone();
                        if !fetched.writable {
                            app.notice = tha::i18n::t(
                                "You cannot write this file there, so it opened read-only",
                            )
                            .into();
                        }
                        fetched.preserve_on_drop();
                        match hand_off(&mut terminal, &mut events, editor, vscode.as_deref(), &fetched.path, mouse_captured).await {
                            Err(error) => {
                                app.notice = tha::tf!(
                                    "Not saved: {error}. Your copy is kept in {folder}",
                                    error = tha::model::safe_text(&format!("{error:#}")),
                                    folder = fetched.recovery_folder().display().to_string()
                                );
                            }
                            Ok(()) if !fetched.writable => fetched.discard_on_drop(),
                            Ok(()) => {
                                let sender = edit_tx.clone();
                                app.notice = tha::i18n::t("Sending the file back…").into();
                                edit_tasks.push(AbortTask(tokio::spawn(async move {
                                    let result = tha::compose_edit::send_back(&location, &fetched).await;
                                    if result.is_ok() {
                                        fetched.discard_on_drop();
                                    }
                                    let message = match result {
                                        Ok(true) => tha::i18n::t("Saved. Nothing was restarted or applied.").to_owned(),
                                        Ok(false) => tha::i18n::t("No changes, so nothing was sent.").to_owned(),
                                        Err(error) => {
                                            // The edits must not be lost with the temporary folder.
                                            tha::tf!(
                                                "Not saved: {error}. Your copy is kept in {folder}",
                                                error = tha::model::safe_text(&format!("{error:#}")),
                                                folder = fetched.recovery_folder().display().to_string()
                                            )
                                        }
                                    };
                                    let _ = sender.send(EditEvent::Saved { location, message }).await;
                                })));
                            }
                        }
                    }
                    EditEvent::Saved { location, message } => {
                        if let Some((key, path)) = edited_file(&location) {
                            app.downloads.invalidate_preview(&key, path);
                            app.downloads.action_failed(&key, path, &message);
                        }
                        if app.downloads.open.is_some() {
                            app.downloads.message = Some(message.clone());
                        }
                        app.notice = message;
                    }
                    EditEvent::OpenFailed { location, message } => {
                        if let Some((key, path)) = edited_file(&location) {
                            app.downloads.action_failed(&key, path, &message);
                        }
                        if app.downloads.open.is_some() {
                            app.downloads.message = Some(message.clone());
                        }
                        app.notice = message;
                    }
                    EditEvent::Notice(message) => app.notice = message,
                    EditEvent::NetworkTest(result) => {
                        if let Some(view) = &mut app.network_test {
                            view.result = Some(result);
                        }
                    }
                }
                redraw_now = true;
            },
            Some(found) = fleet_rx.recv() => {
                if let Some(view) = &mut app.fleet_view {
                    view.vulnerabilities = found;
                }
                redraw_now = true;
            },
            Some(event) = diagnostic_logs_rx.recv() => {
                if let Some(view) = &mut app.diagnostics {
                    view.complete_logs(event);
                }
                redraw_now = true;
            },
            Some(event) = security_rx.recv() => {
                if let tha::security::Outcome::Finished(Err(error)) = &event.result {
                    app.notice = tha::tf!("Security scan failed for {a0}: {a1}", a0 = tha::model::safe_text(&event.target.label), a1 = tha::model::safe_text(error));
                }
                app.security.complete(event);
                redraw_now = true;
            },
            Some(notice) = security_notice_rx.recv() => { app.notice = notice; redraw_now = true; },
            Some(event) = schedule_rx.recv() => { app.apply_schedule_event(event); redraw_now = true; },
            _ = redraw.tick() => {
                if source_checked.elapsed() >= Duration::from_secs(1) {
                    app.security.enabled = app.service_modules && tha::security::enabled();
                    if !app.security.enabled && app.overview_mode == tha::app::OverviewMode::Security { app.overview_mode = tha::app::OverviewMode::Containers; }
                    source_checked=std::time::Instant::now();
                    if let Some(path)=app.auto_history_database.clone() {
                        if path.exists() { app.history_database=Some(path.clone()); }
                        match tha::history::collector_running(&path) {
                            Ok(running) if running != app.metrics_database.is_some() => {
                                app.metrics_database=running.then_some(path);
                                app.metrics_refresh_seconds=if running {1} else {app.local_metrics_refresh_seconds};
                                for worker in &mut workers { worker.connection=None; }
                                let available=app.worker_availability();
                                sync_workers(&mut app,&available,&mut workers,&mut metric_targets,&channels,refresh_seconds);
                                publish_metric_targets(&mut app,&metric_targets);
                                app.notice=if running { tha::i18n::t("Connected to daemon metrics; local stats polling stopped") } else { tha::i18n::t("Daemon stopped; local stats sampling resumed") }.into();
                            }
                            Err(error)=>app.notice=tha::tf!("Cannot check daemon: {error}", error = format!("{error:#}")),
                            _=>{}
                        }
                    }
                }
                redraw_now = visual_deadline.is_some_and(|at| std::time::Instant::now() >= at);
                redraw_now |= app.tab == Tab::History && history_updated.elapsed() >= Duration::from_secs(1);
                // Also catches exits, which print nothing, and sessions on other tabs. Skipped just
                // after a poll, or a noisy command would be read twice as often as it is batched.
                if exec_polled.elapsed() >= EXEC_BATCH_GAP - Duration::from_millis(10) {
                    exec_polled = std::time::Instant::now();
                    redraw_now |= poll_exec_sessions(&mut app);
                }
                // Batch log updates so a noisy container cannot trigger unlimited redraws.
                for _ in 0..512 {
                    let Ok(event) = log_rx.try_recv() else { break; };
                    if event.generation == app.log_generation {
                        app.logs.push(&event.source, &event.text, event.error);
                        redraw_now |= app.tab == Tab::Logs;
                    }
                }
            }
        }
        for command in app.take_remote_commands() {
            match command {
                tha::machines::RemoteCommand::Connect { remote, generation } => {
                    let name = remote.name.clone();
                    // Replacing the entry drops the old session, which closes its SSH process.
                    remote_tasks.insert(
                        name,
                        AbortTask(tokio::spawn(tha::remote::supervise(
                            remote,
                            generation,
                            remote_tx.clone(),
                        ))),
                    );
                }
                tha::machines::RemoteCommand::Disconnect(name) => {
                    remote_tasks.remove(&name);
                }
            }
        }
        let availability = app.take_remote_availability();
        if !availability.is_empty() {
            sync_workers(
                &mut app,
                &availability,
                &mut workers,
                &mut metric_targets,
                &channels,
                refresh_seconds,
            );
            publish_metric_targets(&mut app, &metric_targets);
            sync_event_watchers(&app, &mut event_watchers, &event_tx);
        }
        if let Some(text) = app.clipboard_request.take() {
            // OSC 52: the terminal decides whether to allow it, so the form says "if allowed".
            let _ = std::io::Write::write_all(
                &mut std::io::stdout(),
                tha::remote::osc52(&text).as_bytes(),
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        if let Some(request) = app.exec_request.take() {
            app.start_exec(request);
        }
        if let Some(request) = app.action_request.take() {
            action_tasks.insert(
                request.operation,
                AbortTask(tokio::spawn(tha::actions::run_tracked(
                    request,
                    action_tx.clone(),
                ))),
            );
        }
        app.update_overview_details();
        if app.diagnostics.is_none() {
            diagnostics_tasks[0] = None;
            diagnostics_tasks[1] = None;
        }
        if app.overview_details.is_none() {
            diagnostics_tasks[2] = None;
        }
        for request in app.diagnostics_requests.drain(..) {
            let sender = diagnostics_tx.clone();
            diagnostics_tasks[request.role as usize] = Some(AbortTask(tokio::spawn(async move {
                // Only Overview shows what was overridden, so only it reads the image.
                let with_image = request.role == tha::diagnostics::Role::Overview;
                let result =
                    tha::diagnostics::inspect(&request.connection, &request.target.id, with_image)
                        .await
                        .map_err(|error| tha::tf!("{error}", error = format!("{error:#}")));
                let _ = sender
                    .send(tha::diagnostics::Event {
                        role: request.role,
                        generation: request.generation,
                        target: request.target,
                        result,
                    })
                    .await;
            })));
        }
        if let Some(targets) = app.fleet_vulnerability_request.take() {
            let sender = fleet_tx.clone();
            diagnostics_tasks[4] = Some(AbortTask(tokio::spawn(async move {
                let found = tokio::task::spawn_blocking(move || {
                    targets
                        .into_iter()
                        .filter_map(|target| {
                            let count = tha::security::saved_critical(&target).ok().flatten()?;
                            Some(((target.connection.endpoint, target.image), count))
                        })
                        .collect()
                })
                .await;
                if let Ok(found) = found {
                    let _ = sender.send(found).await;
                }
            })));
        }
        if app.fleet_view.is_none() {
            diagnostics_tasks[4] = None;
        }
        if let Some(request) = app.edit_request.take() {
            edit_tasks.retain(|task| !task.0.is_finished());
            match request.location {
                tha::compose_edit::Location::Local(path) => {
                    let before = std::fs::read(&path).ok();
                    let editor = app.editor;
                    let vscode = app.vscode_path.clone();
                    app.notice = match hand_off(
                        &mut terminal,
                        &mut events,
                        editor,
                        vscode.as_deref(),
                        &path,
                        mouse_captured,
                    )
                    .await
                    {
                        Err(error) => tha::tf!("{error}", error = format!("{error:#}")),
                        Ok(()) if std::fs::read(&path).ok() != before => {
                            tha::i18n::t("Saved. Nothing was restarted or applied.").to_owned()
                        }
                        Ok(()) => tha::i18n::t("No changes.").to_owned(),
                    };
                }
                location => {
                    app.notice = tha::tf!("Getting {file}…", file = request.label);
                    let sender = edit_tx.clone();
                    edit_tasks.push(AbortTask(tokio::spawn(async move {
                        let event = match tha::compose_edit::fetch(&location).await {
                            Ok(fetched) => EditEvent::Fetched {
                                location: Box::new(location),
                                fetched,
                            },
                            Err(error) => EditEvent::OpenFailed {
                                location: Box::new(location),
                                message: tha::tf!(
                                    "Could not open the file: {error}",
                                    error = tha::model::safe_text(&format!("{error:#}"))
                                ),
                            },
                        };
                        let _ = sender.send(event).await;
                    })));
                }
            }
        }
        app.update_compose_review();
        if app.network_test_request {
            app.network_test_request = false;
            if let Some(view) = &app.network_test {
                let (source, host, port) = (view.source.clone(), view.host.clone(), view.port);
                let sender = edit_tx.clone();
                edit_tasks.push(AbortTask(tokio::spawn(async move {
                    let result = tokio::time::timeout(
                        Duration::from_secs(10),
                        tha::files::network_test(&source, &host, port),
                    )
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!(tha::i18n::t("Timed out"))))
                    .map_err(|error| tha::model::safe_text(&format!("{error:#}")));
                    let _ = sender.send(EditEvent::NetworkTest(result)).await;
                })));
            }
        }
        if let Some((path, bundle)) = app.diagnostic_export_request.take() {
            let sender = edit_tx.clone();
            edit_tasks.push(AbortTask(tokio::spawn(async move {
                let shown = path.display().to_string();
                let written = tokio::task::spawn_blocking(move || {
                    use std::io::Write;
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&path)?;
                    file.write_all(bundle.as_bytes())?;
                    file.sync_all()
                })
                .await;
                let message = match written {
                    Ok(Ok(())) => tha::tf!("Diagnostics saved to {path}", path = shown),
                    Ok(Err(error)) => tha::tf!(
                        "Diagnostics not saved: {error}",
                        error = tha::model::safe_text(&error.to_string())
                    ),
                    Err(error) => tha::tf!(
                        "Diagnostics not saved: {error}",
                        error = tha::model::safe_text(&error.to_string())
                    ),
                };
                let _ = sender.send(EditEvent::Notice(message)).await;
            })));
        }
        if alerts_checked_at.elapsed() >= Duration::from_secs(5) {
            alerts_checked_at = std::time::Instant::now();
            app.reload_config();
            app.evaluate_alerts();
            // Servers keep events while Tha is closed, so read what they saved now and then.
            if app.alert_rules.enabled
                && alerts_read_at.is_none_or(|at| at.elapsed() >= Duration::from_secs(60))
            {
                alerts_read_at = Some(std::time::Instant::now());
                alert_reads.retain(|task| !task.0.is_finished());
                let since = tha::history::now_ms() - tha::history::RETENTION_MS;
                for engine in &app.engines {
                    let Some(socket) = tha::remote::local_socket(&engine.connection.endpoint)
                    else {
                        continue;
                    };
                    if engine.snapshot.is_none() || engine.error.is_some() {
                        continue;
                    }
                    let (endpoint, sender) = (engine.connection.endpoint.clone(), event_tx.clone());
                    alert_reads.push(AbortTask(tokio::spawn(async move {
                        if let Ok(events) = tha::events::read_remote(&socket, since).await {
                            let _ = sender.send(tha::events::Update { endpoint, events }).await;
                        }
                    })));
                }
            }
        }
        app.downloads.release_idle(std::time::Instant::now());
        redraw_now |= app.downloads.update_preview(std::time::Instant::now());
        for request in app.take_download_requests() {
            run_download_request(
                request,
                &downloads_tx,
                &mut download_lists,
                &mut download_dialog,
                &mut download_jobs,
            );
        }
        // Slots 0–2 follow `Role`, 3 loads logs for the open view, and 4 reads saved scans for fleet.
        if app.diagnostics.is_none() {
            diagnostics_tasks[3] = None;
        }
        if let Some(request) = app.diagnostics_log_request.take() {
            let sender = diagnostic_logs_tx.clone();
            diagnostics_tasks[3] = Some(AbortTask(tokio::spawn(async move {
                let result = tokio::time::timeout(
                    tha::diagnostics::DEADLINE,
                    logs::load_window(
                        &request.connection,
                        &request.container,
                        request.since,
                        request.until,
                    ),
                )
                .await
                .map_err(|_| tha::i18n::t("Loading logs timed out after 5s").to_owned())
                .and_then(|result| {
                    result.map_err(|error| tha::model::safe_text(&format!("{error:#}")))
                });
                let _ = sender
                    .send(tha::diagnostics::LogLoadEvent {
                        generation: request.generation,
                        target: request.target,
                        result,
                    })
                    .await;
            })));
        }
        if let Some(request) = app.log_export_request.take() {
            let sender = export_tx.clone();
            export_task = Some(AbortTask(tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || request.write()).await;
                let message = match result {
                    Ok(Ok(message)) => message,
                    Ok(Err(error)) => {
                        tha::tf!("Log export failed: {error}", error = format!("{error:#}"))
                    }
                    Err(error) => tha::tf!("Log export worker failed: {error}", error = error),
                };
                let _ = sender.send(message).await;
            })));
        }
        if redraw_now {
            if app.tab == Tab::History {
                if let Some((index, container)) = app.performance_container() {
                    let engine = &app.engines[index];
                    let runtime = tha::history::runtime_key(
                        &engine.connection,
                        &engine
                            .snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.engine_id.clone()),
                    );
                    let target = (runtime.clone(), container.key());
                    app.history_view.select_target(target.clone());
                    let request = (
                        runtime,
                        container.key(),
                        app.history_view.range,
                        app.history_view.end_ms,
                    );
                    let changed = history_request.as_ref() != Some(&request);
                    if changed
                        || history_task.is_none()
                            && history_updated.elapsed() >= Duration::from_secs(1)
                            && app.history_view.end_ms.is_none()
                    {
                        history_task = None;
                        if changed {
                            app.history_view.data = Default::default();
                            if history_request.as_ref().is_none_or(|old| {
                                old.0 != request.0 || old.1 != request.1 || old.2 != request.2
                            }) {
                                app.history_view.cursor = None;
                            }
                            app.history_view.error = None;
                        }
                        app.history_view.generation = app.history_view.generation.wrapping_add(1);
                        let generation = app.history_view.generation;
                        let (start, end) = app.history_view.bounds();
                        history_request = Some(request);
                        history_updated = std::time::Instant::now();
                        if tha::remote::is_remote(&engine.connection) {
                            let sender = history_tx.clone();
                            let connection = engine.connection.clone();
                            let mut fallback =
                                engine.metrics.local_history(&container.key(), start, end);
                            let seconds = tha::remote::server_of(&connection)
                                .and_then(|server| {
                                    app.machines.iter().find(|m| m.remote.name == server)
                                })
                                .map_or(tha::remote::DEFAULT_METRICS_SECONDS, |m| {
                                    m.remote.metrics_seconds()
                                });
                            fallback.sample_interval_ms = Some(seconds as i64 * 1000);
                            app.history_view.loading = true;
                            history_task = Some(AbortTask(tokio::spawn(async move {
                                let data = match tha::remote_history::query(
                                    &connection,
                                    &target.0,
                                    &target.1,
                                    (start, end),
                                    600,
                                )
                                .await
                                {
                                    Ok(Some(data)) => data,
                                    Ok(None) => {
                                        fallback.source = "Warning: Agent history unavailable; upgrade/restart tha-agent for persistent history · session only".into();
                                        fallback
                                    }
                                    Err(error) => {
                                        fallback.source = format!(
                                            "Warning: {} · session only",
                                            tha::layers::text(&format!("{error:#}"))
                                        );
                                        fallback
                                    }
                                };
                                let _ = sender
                                    .send(tha::history::Event {
                                        generation,
                                        data: Ok(data),
                                    })
                                    .await;
                            })));
                        } else if let Some(path) = app.history_database.clone() {
                            let sender = history_tx.clone();
                            let session =
                                engine.metrics.local_samples(&container.key(), start, end);
                            app.history_view.loading = true;
                            history_task = Some(AbortTask(tokio::spawn(async move {
                                let data = tha::live::history_query_with_session(
                                    path,
                                    target.0,
                                    target.1,
                                    (start, end),
                                    600,
                                    session,
                                )
                                .await
                                .map_err(|error| tha::tf!("{error}", error = format!("{error:#}")));
                                let _ = sender.send(tha::history::Event { generation, data }).await;
                            })));
                        } else {
                            app.history_view.data =
                                engine.metrics.local_history(&container.key(), start, end);
                            app.history_view.loading = false;
                        }
                    }
                }
            } else {
                history_task = None;
                history_request = None;
            }
            // Input/operation feedback is not delayed until the next animation tick.
            // Resolve view targets first so navigation never paints the previous target's layers.
            if let Some(request) = app.inventory_removal_request.take() {
                let sender = removal_tx.clone();
                removal_task = Some(AbortTask(tokio::spawn(async move {
                    let report = tha::backend::remove_unused_batch(request).await;
                    let _ = sender.send(report).await;
                })));
            }
            if let Some(request) = app.compose_request.take() {
                compose_task = Some(AbortTask(tokio::spawn(tha::compose::run(
                    request,
                    compose_tx.clone(),
                ))));
            }
            if (matches!(app.tab, Tab::Events | Tab::History) || app.timeline_view.is_some())
                && event_read_at.elapsed() >= Duration::from_secs(5)
            {
                event_read_at = std::time::Instant::now();
                event_read = read_saved_events(&app, &event_tx);
            }
            if let Some(targets) = app.image_check_request.take() {
                // A newer check replaces any earlier one for this session.
                image_check_task = Some(AbortTask(tokio::spawn(tha::images::check(
                    targets,
                    image_tx.clone(),
                ))));
            }
            if let Some(request) = app.image_pull_request.take() {
                image_pull_task = Some(AbortTask(tokio::spawn(tha::images::pull(
                    request,
                    image_tx.clone(),
                ))));
            }
            update_layers(&mut app, &mut layers_task, &layers_tx);
            if app.security.cancel {
                app.security.cancel = false;
                if app
                    .security
                    .scan_target
                    .as_ref()
                    .or(app.security.target.as_ref())
                    .is_some_and(|target| tha::remote::is_remote(&target.connection))
                {
                    if let Some(cancel) = &security_cancel {
                        let _ = cancel.send(true);
                    }
                    app.notice =
                        "Stopped following the remote scan; the agent owns its schedule".into();
                } else {
                    let sender = security_notice_tx.clone();
                    tokio::spawn(async move {
                        let notice = match tha::security::cancel_remote().await {
                            Ok(()) => tha::i18n::t("Security queue cancellation requested").into(),
                            Err(error) => tha::tf!(
                                "Cannot cancel security queue: {error}",
                                error = format!("{error:#}")
                            ),
                        };
                        let _ = sender.send(notice).await;
                    });
                }
            }
            if app.security.request {
                if security_task
                    .as_ref()
                    .is_some_and(|task| !task.is_finished())
                {
                    app.notice =
                        tha::i18n::t("Security queue is still running or cleaning up").into();
                } else if let Some(target) = app.security.target.clone() {
                    let mut targets = std::mem::take(&mut app.security.queue);
                    if targets.is_empty() {
                        targets.push(target);
                    }
                    app.security.start(targets.len());
                    let (cancel_tx, cancel_rx) = watch::channel(false);
                    security_cancel = Some(cancel_tx);
                    security_task = Some(tokio::spawn(tha::security::submit(
                        targets,
                        app.security.generation,
                        app.security.update,
                        security_tx.clone(),
                        cancel_rx,
                    )));
                }
                app.security.request = false;
                app.security.update = false;
            }
            if let Some(connection) = app.security.schedule_load.take() {
                let sender = schedule_tx.clone();
                tokio::spawn(async move {
                    let result = tha::security::remote::schedule(&connection)
                        .await
                        .map_err(|error| tha::layers::text(&format!("{error:#}")));
                    let _ = sender
                        .send(tha::security::ScheduleEvent::Loaded(connection, result))
                        .await;
                });
            }
            if let Some((connection, cron, ignore)) = app.security.schedule_save.take() {
                let sender = schedule_tx.clone();
                tokio::spawn(async move {
                    let result = tha::security::remote::set_schedule(&connection, &cron, ignore)
                        .await
                        .map_err(|error| tha::layers::text(&format!("{error:#}")));
                    let _ = sender
                        .send(tha::security::ScheduleEvent::Saved(result))
                        .await;
                });
            }
            update_platform(&mut app, &mut platform_task, &platform_tx);
            update_logs(&mut app, &mut log_task, &log_tx, log_capacity);
            if mouse_captured != app.mouse_enabled {
                mouse_captured = app.mouse_enabled;
                if mouse_captured {
                    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
                } else {
                    crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
                }
            }
            let frame_started = std::time::Instant::now();
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            visual_deadline = app.next_visual_deadline(frame_started);
        }
    }
    drop(log_task);
    drop(security_cancel);
    if let Some(task) = security_task {
        task.abort(); // Disconnect only: the service owns the queue and persists its reports.
        let _ = task.await;
    }
    drop(history_task);
    drop(action_tasks);
    drop(layers_task);
    drop(removal_task);
    drop(export_task);
    drop(compose_task);
    drop(event_watchers);
    drop(event_read);
    drop(image_check_task);
    drop(image_pull_task);
    drop(terminal);
    drop(_restore);
    if let Some(path) = &app.preferences_path
        && let Err(error) = tha::preferences::save(path, &app.preferences())
    {
        eprintln!("tha: could not save preferences: {error:#}");
    }
    drop(discovery_task);
    drop(workers);
    drop(remote_tasks);
    tha::remote::stop_private_agent();
    tha::remote::remove_session_directory();
    Ok(())
}

/// Runs what the download manager asked for. Dropping a task cancels it, and a cancelled
/// download removes its own partial file.
fn run_download_request(
    request: tha::downloads::Request,
    tx: &mpsc::Sender<tha::downloads::Event>,
    lists: &mut Vec<AbortTask>,
    dialog: &mut [Option<AbortTask>; 4],
    jobs: &mut std::collections::HashMap<u64, AbortTask>,
) {
    use tha::downloads::{Event, Request};
    let message = |error: anyhow::Error| tha::model::safe_text(&format!("{error:#}"));
    const QUICK: Duration = Duration::from_secs(15);
    let sender = tx.clone();
    match request {
        Request::Search(request) => {
            dialog[3] = Some(AbortTask(tokio::spawn(tha::file_search::run(
                request, sender,
            ))));
        }
        Request::CancelSearch => dialog[3] = None,
        Request::List { key, source, path } => {
            lists.retain(|task| !task.0.is_finished());
            lists.push(AbortTask(tokio::spawn(async move {
                let result = tokio::time::timeout(QUICK, tha::files::list(&source, &path))
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!(tha::i18n::t("Timed out"))))
                    .map_err(message);
                let _ = sender.send(Event::Listed { key, path, result }).await;
            })));
        }
        Request::Inspect {
            dialog: id,
            source,
            path,
        } => {
            dialog[0] = Some(AbortTask(tokio::spawn(async move {
                let result = tokio::time::timeout(QUICK, tha::files::stat(&source, &path))
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!(tha::i18n::t("Timed out"))))
                    .map_err(message);
                let _ = sender.send(Event::Inspected { dialog: id, result }).await;
            })));
        }
        Request::Count {
            dialog: id,
            source,
            path,
        } => {
            dialog[1] = Some(AbortTask(tokio::spawn(async move {
                let running = sender.clone();
                let mut last = tha::files::Totals::default();
                let result = tha::files::count(&source, &path, |totals| {
                    last = totals;
                    // A running total may be skipped when busy. The final one is sent below.
                    let _ = running.try_send(Event::Counted {
                        dialog: id,
                        result: Ok(totals),
                    });
                })
                .await;
                let result = result.map(|()| last).map_err(message);
                let _ = sender.send(Event::Counted { dialog: id, result }).await;
            })));
        }
        Request::Start {
            job,
            source,
            path,
            destination,
            folder,
        } => {
            jobs.insert(
                job,
                AbortTask(tokio::spawn(async move {
                    let running = sender.clone();
                    let result =
                        tha::files::download(&source, &path, &destination, folder, |bytes| {
                            let _ = running.try_send(Event::Progress { job, bytes });
                        })
                        .await
                        .map_err(message);
                    let _ = sender.send(Event::Finished { job, result }).await;
                })),
            );
        }
        Request::Upload {
            job,
            source,
            local,
            folder,
        } => {
            jobs.insert(
                job,
                AbortTask(tokio::spawn(async move {
                    let running = sender.clone();
                    let result = tha::files::upload(&source, &local, &folder, move |bytes| {
                        let _ = running.try_send(Event::Progress { job, bytes });
                    })
                    .await
                    .map_err(message);
                    let _ = sender.send(Event::Finished { job, result }).await;
                })),
            );
        }
        // Longer than a read, and in `lists` so that closing the panel does not interrupt it. A
        // timeout says nothing about whether it happened, so the folder is read afterwards anyway.
        Request::CreateFile {
            source,
            folder,
            name,
        } => {
            lists.retain(|task| !task.0.is_finished());
            lists.push(AbortTask(tokio::spawn(async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(30),
                    tha::files::create_file(&source, &folder, &name),
                )
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(tha::i18n::t("Timed out"))))
                .map_err(message);
                let _ = sender
                    .send(Event::FileCreated {
                        key: source.key(),
                        path: tha::files::join(&folder, &name),
                        result,
                    })
                    .await;
            })));
        }
        Request::DeleteFile { source, path } => {
            lists.retain(|task| !task.0.is_finished());
            lists.push(AbortTask(tokio::spawn(async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(30),
                    tha::files::delete_file(&source, &path),
                )
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(tha::i18n::t("Timed out"))))
                .map_err(message);
                let _ = sender
                    .send(Event::FileDeleted {
                        key: source.key(),
                        path,
                        result,
                    })
                    .await;
            })));
        }
        Request::Cancel { job } => {
            jobs.remove(&job);
        }
        Request::Preview {
            generation,
            source,
            path,
        } => {
            // Replacing the task stops reading the file that was selected before.
            dialog[2] = Some(AbortTask(tokio::spawn(async move {
                let result = tokio::time::timeout(
                    QUICK,
                    tha::files::read_start(&source, &path, tha::downloads::PREVIEW_LIMIT),
                )
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(tha::i18n::t("Timed out"))))
                .map_err(message);
                let _ = sender
                    .send(Event::Previewed {
                        key: source.key(),
                        generation,
                        path,
                        result,
                    })
                    .await;
            })));
        }
        Request::CloseDialog => {
            dialog[0] = None;
            dialog[1] = None;
        }
    }
}

fn update_layers(
    app: &mut App,
    task: &mut Option<AbortTask>,
    tx: &mpsc::Sender<tha::layers::Event>,
) {
    app.reconcile_detail_tab();
    let target = app.layers_target();
    let key = target.as_ref().map(|(key, _, _)| key.clone());
    if key.is_none() {
        *task = None;
    }
    if app.layers.key != key {
        *task = None;
        app.layers.select(key);
        app.detail_scroll = 0;
    }
    let Some((key, connection, container)) = target else {
        return;
    };
    if !matches!(app.layers.state, tha::layers::State::Loading) || task.is_some() {
        return;
    }
    if app.demo {
        app.layers.state = tha::layers::State::Ready(tha::layers::Image {
            id: format!("{} [demo metadata]", container.image),
            filesystem: vec![
                "demo:base-diff-id".into(),
                "demo:application-diff-id".into(),
            ],
            history: vec![
                tha::layers::History {
                    instruction: "COPY application /app".into(),
                    comment: "Seeded demo instruction".into(),
                    size: Some(4096),
                    empty_layer: Some(false),
                },
                tha::layers::History {
                    instruction: "ENV APP_MODE=production".into(),
                    comment: "Explicit metadata-only demo step".into(),
                    size: Some(0),
                    empty_layer: Some(true),
                },
            ],
            warnings: vec!["Demo data; not inspected from a real image".into()],
        });
        return;
    }
    let sender = tx.clone();
    let generation = app.layers.generation;
    *task = Some(AbortTask(tokio::spawn(async move {
        let result = tha::layers::lookup(&connection, &container)
            .await
            .map_err(|error| tha::layers::text(&tha::tf!("{error}", error = format!("{error:#}"))));
        let _ = sender
            .send(tha::layers::Event {
                key,
                generation,
                result,
            })
            .await;
    })));
}

fn update_platform(
    app: &mut App,
    task: &mut Option<(tha::platform::Key, AbortTask)>,
    tx: &mpsc::Sender<tha::platform::Event>,
) {
    let target = app.platform_target();
    if task.as_ref().map(|(key, _)| key) != target.as_ref().map(|(key, _, _)| key)
        && let Some((key, _)) = task.take()
    {
        app.platforms.cancel_loading(&key);
    }
    let Some((key, connection, container)) = target else {
        return;
    };
    if !app.platforms.needs_lookup(&key)
        || task
            .as_ref()
            .is_some_and(|(_, handle)| !handle.0.is_finished())
    {
        return;
    }
    app.platforms
        .insert(key.clone(), tha::platform::State::Loading);
    let sender = tx.clone();
    let event_key = key.clone();
    let handle = tokio::spawn(async move {
        let result = tha::platform::lookup(&connection, &container)
            .await
            .map_err(|error| tha::tf!("{error}", error = format!("{error:#}")));
        let _ = sender
            .send(tha::platform::Event {
                key: event_key,
                result,
            })
            .await;
    });
    *task = Some((key, AbortTask(handle)));
}

fn publish_metric_targets(app: &mut App, targets: &[watch::Sender<Option<metrics::Target>>]) {
    for (engine, tx) in app.engines.iter_mut().zip(targets) {
        let previous = tx.borrow().clone();
        let available = engine.error.is_none() && engine.alias.is_none();
        let next = engine.snapshot.as_ref().filter(|_| available);
        let unchanged = match (&previous, next) {
            (None, None) => true,
            (Some(previous), Some(snapshot)) => {
                previous.same_workload(&snapshot.engine_id, &snapshot.containers)
            }
            _ => false,
        };
        if unchanged {
            continue;
        }
        engine.metrics_revision = engine.metrics_revision.wrapping_add(1);
        tx.send_replace(next.map(|snapshot| metrics::Target {
            revision: engine.metrics_revision,
            identity: snapshot.engine_id.clone(),
            containers: snapshot.containers.clone(),
        }));
    }
}

/// One watcher per connected, primary Docker-compatible endpoint, for as long as Tha is open.
fn sync_event_watchers(
    app: &App,
    watchers: &mut std::collections::HashMap<String, AbortTask>,
    tx: &mpsc::Sender<tha::events::Update>,
) {
    if app.demo {
        return;
    }
    let wanted: std::collections::HashMap<_, _> = app
        .engines
        .iter()
        .filter(|engine| {
            engine.alias.is_none()
                && engine.snapshot.is_some()
                && engine.connection.kind == tha::config::BackendKind::Docker
        })
        .map(|engine| (engine.connection.endpoint.clone(), &engine.connection))
        .collect();
    watchers.retain(|endpoint, _| wanted.contains_key(endpoint));
    for (endpoint, connection) in wanted {
        watchers.entry(endpoint).or_insert_with(|| {
            AbortTask(tokio::spawn(tha::events::watch(
                connection.clone(),
                tx.clone(),
            )))
        });
    }
}

/// Add what the service recorded while Tha was closed for the selected runtime.
fn read_saved_events(app: &App, tx: &mpsc::Sender<tha::events::Update>) -> Option<AbortTask> {
    let engine = &app.engines[app.current()?.engine];
    let endpoint = engine.connection.endpoint.clone();
    let tx = tx.clone();
    let since = tha::history::now_ms() - tha::history::RETENTION_MS;
    if let Some(socket) = tha::remote::local_socket(&engine.connection.endpoint) {
        return Some(AbortTask(tokio::spawn(async move {
            // A server that cannot answer leaves the live timeline working.
            if let Ok(events) = tha::events::read_remote(&socket, since).await {
                let _ = tx.send(tha::events::Update { endpoint, events }).await;
            }
        })));
    }
    let path = tha::events::database_path(app.history_database.as_ref()?)?;
    let runtime = tha::history::runtime_key(
        &engine.connection,
        &engine.snapshot.as_ref()?.engine_id.clone(),
    );
    Some(AbortTask(tokio::spawn(async move {
        let events = tokio::task::spawn_blocking(move || {
            tha::events::read(&path, &runtime, since, tha::events::MAX_IN_MEMORY)
        })
        .await;
        // An unreadable events file leaves the live timeline working.
        if let Ok(Ok(events)) = events {
            let _ = tx.send(tha::events::Update { endpoint, events }).await;
        }
    })))
}

fn update_logs(
    app: &mut App,
    task: &mut Option<AbortTask>,
    tx: &mpsc::Sender<LogEvent>,
    capacity: usize,
) {
    if app.tab != Tab::Logs {
        *task = None;
        app.log_scope = None;
        return;
    }
    let Some(row) = app.current().cloned() else {
        *task = None;
        return;
    };
    let scope = log_scope_key(
        &row,
        &app.engines[row.engine],
        app.selected_container_refs(),
        app.log_since,
    );
    if app.log_scope.as_deref() == Some(&scope) {
        return;
    }
    // Ownership is needed by the subscription task, not by unchanged frame checks.
    let containers = app.selected_containers();
    let engine = &app.engines[row.engine];
    *task = None;
    app.log_generation += 1;
    app.log_scope = Some(scope);
    app.log_scope_targets = Some((
        row.engine,
        containers.iter().map(|container| container.key()).collect(),
    ));
    let pretty_json = app.logs.pretty_json;
    app.logs = logs::LogBuffer::new(capacity);
    app.logs.pretty_json = pretty_json;
    app.log_view.reset_content();
    if engine.error.is_some() || engine.alias.is_some() {
        app.logs.push(
            "Tha",
            tha::i18n::t(
                "Connection unavailable or aliased. Browse a connected engine to view logs.",
            ),
            true,
        );
        return;
    }
    if containers.is_empty() {
        app.logs
            .push("Tha", tha::i18n::t("No containers in this scope."), true);
        return;
    }
    if app.demo {
        let tx = tx.clone();
        let generation = app.log_generation;
        *task = Some(AbortTask(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(800));
            for n in 0..60u64 {
                let source = containers[n as usize % containers.len()].name.clone();
                let text = if n % 7 == 0 {
                    format!(
                        "2026-10-29T14:32:{:02}Z {{\"level\":\"warn\",\"message\":\"upstream retry\",\"attempt\":2}}",
                        n % 60
                    )
                } else if n % 3 == 0 {
                    // Enough variety to try the text filter: `backend1 500` needs both words.
                    let (upstream, status) =
                        [("backend1", 500), ("backend1", 200), ("backend2", 500)]
                            [(n / 3) as usize % 3];
                    format!(
                        "2026-10-29T14:32:{:02}Z ERROR upstream={upstream} status={status} path=/checkout",
                        n % 60
                    )
                } else {
                    format!(
                        "2026-10-29T14:32:{:02}Z INFO request completed method=GET path=/health duration=2ms",
                        n % 60
                    )
                };
                if tx
                    .send(LogEvent {
                        generation,
                        source,
                        text,
                        error: false,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                if n > 20 {
                    interval.tick().await;
                }
            }
        })));
    } else {
        *task = Some(AbortTask(logs::subscribe(
            engine.connection.clone(),
            containers,
            app.log_generation,
            app.log_since,
            capacity,
            tx.clone(),
        )));
    }
}

fn log_scope_key<'a>(
    row: &tha::app::Row,
    engine: &tha::app::Engine,
    containers: impl IntoIterator<Item = &'a tha::model::Container>,
    since: Option<i64>,
) -> String {
    format!(
        "{}:{:?}:{:?}:{:?}:{:?}",
        row.key,
        engine.connection.endpoint,
        engine.error,
        since,
        containers
            .into_iter()
            .map(|container| container.key())
            .collect::<Vec<_>>()
    )
}

fn queue_log_export(app: &mut App, path: &str) -> Result<()> {
    if !app.log_export_review_visible {
        bail!(tha::i18n::t(
            "Review the export dialog in a terminal at least 60 × 18 first"
        ));
    }
    if app.log_export_busy {
        bail!(tha::i18n::t("Another export is already running"));
    }
    let row = app
        .current()
        .context(tha::i18n::t("Log selection disappeared"))?;
    let engine = &app.engines[row.engine];
    let key = log_scope_key(row, engine, app.selected_container_refs(), app.log_since);
    if app.tab != Tab::Logs
        || engine.error.is_some()
        || engine.alias.is_some()
        || app.log_scope.as_ref() != Some(&key)
        || app.log_export_scope.as_ref() != Some(&key)
    {
        bail!(tha::i18n::t(
            "Log target changed or is unavailable; cancel and reopen export after reviewing Logs"
        ));
    }
    let request =
        logs::export::Request::snapshot(PathBuf::from(path), &app.logs, &app.log_view.filter)?;
    app.log_export_request = Some(request);
    app.log_export_busy = true;
    Ok(())
}

#[path = "main_input.rs"]
mod input;
use input::*;

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
