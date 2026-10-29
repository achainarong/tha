use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use std::{io::IsTerminal, path::PathBuf, time::Duration};
use tha::{
    app::{App, InputMode, Tab},
    backend,
    cli::Cli,
    config::{BackendKind, Config, Connection, parse_connection},
    demo, discovery,
    logs::{self, LogEvent},
    metrics,
    model::Snapshot,
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
    let mut config = Config::load(cli.config.as_ref())?;
    config.monochrome |= cli.monochrome || std::env::var_os("NO_COLOR").is_some();
    let connections = if cli.demo {
        demo::connections()
    } else {
        connections(&cli, &config)?
    };
    if cli.list_connections {
        let discovery = if cli.demo {
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
    if cli.demo {
        demo::populate(&mut app);
    }
    app.log_view.wrap = config.log_wrap;
    app.exec_command = config.exec_command;
    app.metrics_refresh_seconds = config.metrics_refresh_seconds;
    app.context_pins = config.context_order;
    app.refresh_tree();
    if !cli.demo {
        app.selected = 0;
    }
    if !cli.demo && !cli.overview {
        app.configure_startup(std::env::current_dir().context("Cannot read current directory")?);
        app.wait_for_machine_discovery();
    }
    run(app, config.refresh_seconds).await
}

fn connections(cli: &Cli, config: &Config) -> Result<Vec<Connection>> {
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
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let docker_config = std::env::var_os("DOCKER_CONFIG").map(PathBuf::from);
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    connections.extend(discovery::discover(
        home.as_deref(),
        docker_config.as_deref(),
        runtime_dir.as_deref(),
    ));
    if connections.is_empty() {
        connections.push(Connection {
            name: "Docker / local".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///var/run/docker.sock".into(),
        });
    }
    Ok(discovery::deduplicate(connections))
}

struct TerminalRestore;
impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
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
        slot.tasks.push(AbortTask(tokio::spawn(metrics::worker(
            index,
            connection.clone(),
            targets[index].subscribe(),
            channels.metrics.clone(),
            app.metrics_refresh_seconds,
        ))));
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

async fn run(mut app: App, refresh_seconds: u64) -> Result<()> {
    // Ratatui installs a panic hook that restores raw mode and the alternate screen.
    let mut terminal = ratatui::try_init().context("Cannot initialize terminal")?;
    let _restore = TerminalRestore;
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    let mut events = EventStream::new();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let (snapshot_tx, mut snapshot_rx) = mpsc::channel::<SnapshotEvent>(32);
    let (refresh_tx, refresh_rx) = watch::channel(0u64);
    let (log_tx, mut log_rx) = mpsc::channel::<LogEvent>(512);
    let (metrics_tx, mut metrics_rx) = mpsc::channel::<metrics::Event>(32);
    let (platform_tx, mut platform_rx) = mpsc::channel::<tha::platform::Event>(16);
    let mut platform_task: Option<(tha::platform::Key, AbortTask)> = None;
    let (action_tx, mut action_rx) = mpsc::channel(64);
    let mut action_task: Option<AbortTask> = None;
    let mut metric_targets = Vec::new();
    let mut workers = Vec::new();
    let channels = WorkerChannels {
        snapshots: snapshot_tx,
        metrics: metrics_tx,
        refresh: refresh_rx.clone(),
    };
    let (discovery_tx, mut discovery_rx) = mpsc::channel(1);
    let mut discovery_task = None;
    if !app.demo {
        let available: Vec<_> = (0..app.engines.len()).map(|index| (index, true)).collect();
        sync_workers(
            &mut app,
            &available,
            &mut workers,
            &mut metric_targets,
            &channels,
            refresh_seconds,
        );
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
    let mut log_task: Option<AbortTask> = None;
    let log_capacity = app.logs.capacity();
    let mut redraw = tokio::time::interval(Duration::from_millis(70));
    redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    terminal.draw(|frame| ui::draw(frame, &mut app))?;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    let height = terminal.size()?.height.saturating_sub(7) as usize;
                    if handle_key(&mut app, key, height, &refresh_tx) { break; }
                },
                Some(Ok(Event::Paste(text))) => {
                    if let Some(confirmation) = &mut app.action_confirmation {
                        if confirmation.request.action == tha::actions::Action::Delete {
                            if text.chars().any(char::is_control) || confirmation.typed.len() + text.len() > 1024 {
                                confirmation.error = Some("Paste contains control characters or exceeds 1024 bytes; nothing pasted".into());
                            } else { confirmation.typed.push_str(&text); }
                        }
                    } else if app.exec_input && app.tab == Tab::Exec {
                        if let Some(session) = &mut app.exec_session && let Err(error) = session.paste(&text) { app.notice = format!("{error:#}"); }
                    } else if app.input_mode != InputMode::Normal {
                        let text = tha::model::safe_text(&text);
                        for ch in text.chars() { if app.input.len() + ch.len_utf8() > 1024 { break; } app.input.push(ch); }
                    }
                },
                Some(Err(error)) => return Err(error.into()),
                Some(Ok(Event::Resize(_, _))) => app.action_review_visible = false,
                None => break,
                _ => {}
            },
            Some(event) = snapshot_rx.recv() => {
                if !workers.get(event.engine).is_some_and(|slot| slot.accepts(event.generation)) { continue; }
                app.update_engine(event.engine, event.result);
                publish_metric_targets(&mut app, &metric_targets);
            },
            Some(discovery) = discovery_rx.recv() => {
                if let Some(warning) = &discovery.warning { app.notice = warning.clone(); }
                let availability = app.reconcile_podman(&discovery);
                sync_workers(&mut app, &availability, &mut workers, &mut metric_targets, &channels, refresh_seconds);
                publish_metric_targets(&mut app, &metric_targets);
            },
            Some(event) = metrics_rx.recv() => app.apply_metrics(event),
            Some(event) = action_rx.recv() => {
                if let Some(progress) = &mut app.action_progress {
                    match event {
                        tha::actions::Event::Result { index, status } => {
                            if let Some(result) = progress.results.get_mut(index) { *result = Some(status); }
                        }
                        tha::actions::Event::Finished => {
                            progress.finished = true;
                            app.notice = format!("{} finished; inspect per-container results. No automatic retry or rollback.", progress.request.action.label());
                            action_task = None;
                            refresh_tx.send_modify(|generation| *generation = generation.wrapping_add(1));
                        }
                    }
                }
            },
            Some(event) = platform_rx.recv() => app.platforms.insert(event.key, match event.result { Ok(platform) => tha::platform::State::Ready(platform), Err(error) => tha::platform::State::Unavailable(error) }),
            _ = redraw.tick() => {
                update_platform(&mut app, &mut platform_task, &platform_tx);
                if let Some(session) = &mut app.exec_session && session.poll() {
                    app.exec_input = false;
                    app.notice = format!("Exec {} · container was not stopped. Press e to open another shell.", session.status());
                }
                update_logs(&mut app, &mut log_task, &log_tx, log_capacity);
                // Batch log updates so a noisy container cannot trigger unlimited redraws.
                for _ in 0..512 {
                    let Ok(event) = log_rx.try_recv() else { break; };
                    if event.generation == app.log_generation { app.logs.push(&event.source, &event.text, event.error); }
                }
                terminal.draw(|frame| ui::draw(frame, &mut app))?;
            }
        }
        if let Some(request) = app.exec_request.take() {
            app.start_exec(request);
        }
        if let Some(request) = app.action_request.take() {
            action_task = Some(AbortTask(tokio::spawn(tha::actions::run(
                request,
                action_tx.clone(),
            ))));
        }
    }
    drop(log_task);
    drop(action_task);
    drop(discovery_task);
    drop(workers);
    Ok(())
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
            .map_err(|error| format!("{error:#}"));
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
    let containers = app.selected_containers();
    let engine = &app.engines[row.engine];
    let scope = format!(
        "{}:{:?}:{:?}:{:?}",
        row.key,
        engine.connection.endpoint,
        engine.error,
        containers.iter().map(|c| c.key()).collect::<Vec<_>>()
    );
    if app.log_scope.as_deref() == Some(&scope) {
        return;
    }
    *task = None;
    app.log_generation += 1;
    app.log_scope = Some(scope);
    app.logs = logs::LogBuffer::new(capacity);
    app.log_view.reset_content();
    if engine.error.is_some() || engine.alias.is_some() {
        app.logs.push(
            "Tha",
            "Connection unavailable or aliased. Browse a connected engine to view logs.",
            true,
        );
        return;
    }
    if containers.is_empty() {
        app.logs.push("Tha", "No containers in this scope.", true);
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
                        "14:32:{:02} {{\"level\":\"warn\",\"message\":\"upstream retry\",\"attempt\":2}}",
                        n % 60
                    )
                } else {
                    format!(
                        "14:32:{:02} INFO request completed method=GET path=/health duration=2ms",
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
            tx.clone(),
        )));
    }
}

fn handle_key(app: &mut App, key: KeyEvent, height: usize, refresh: &watch::Sender<u64>) -> bool {
    app.cancel_startup();
    if app.exec_input && app.detail_focus && app.tab == Tab::Exec {
        // Legacy terminals encode Ctrl-] as 0x1d, which Crossterm reports as Ctrl-5.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
        {
            app.exec_input = false;
            app.detail_focus = false;
            app.performance_focus = false;
            app.notice = "Exec is still connected · navigation mode · e refocuses the same container, or select 4 then Enter.".into();
        } else if let Some(session) = &mut app.exec_session
            && let Err(error) = session.key(key)
        {
            app.notice = format!("{error:#}");
        }
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }
    if app.action_confirmation.is_some() {
        use tha::actions::Action;
        let deleting = app.action_confirmation.as_ref().unwrap().request.action == Action::Delete;
        match key.code {
            KeyCode::Esc => app.action_confirmation = None,
            KeyCode::Enter if deleting => app.confirm_action(),
            KeyCode::Enter => app.action_confirmation = None, // Cancel is the default.
            KeyCode::Char('y')
                if !deleting
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                app.confirm_action()
            }
            KeyCode::Up | KeyCode::PageUp => {
                let confirmation = app.action_confirmation.as_mut().unwrap();
                confirmation.scroll =
                    confirmation
                        .scroll
                        .saturating_sub(if key.code == KeyCode::PageUp {
                            height.max(1)
                        } else {
                            1
                        });
            }
            KeyCode::Down | KeyCode::PageDown => {
                let confirmation = app.action_confirmation.as_mut().unwrap();
                confirmation.scroll = confirmation
                    .scroll
                    .saturating_add(if key.code == KeyCode::PageDown {
                        height.max(1)
                    } else {
                        1
                    })
                    .min(u16::MAX as usize);
            }
            KeyCode::Backspace if deleting => {
                app.action_confirmation.as_mut().unwrap().typed.pop();
            }
            KeyCode::Char(ch)
                if deleting
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let confirmation = app.action_confirmation.as_mut().unwrap();
                if !ch.is_control() && confirmation.typed.len() + ch.len_utf8() <= 1024 {
                    confirmation.typed.push(ch);
                }
            }
            _ => {}
        }
        return false;
    }
    if let Some(progress) = &mut app.action_progress {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') if progress.finished => {
                app.action_progress = None
            }
            KeyCode::Up | KeyCode::PageUp => {
                progress.scroll = progress
                    .scroll
                    .saturating_sub(if key.code == KeyCode::PageUp {
                        height.max(1)
                    } else {
                        1
                    })
            }
            KeyCode::Down | KeyCode::PageDown => {
                progress.scroll = progress
                    .scroll
                    .saturating_add(if key.code == KeyCode::PageDown {
                        height.max(1)
                    } else {
                        1
                    })
                    .min(u16::MAX as usize)
            }
            _ => {}
        }
        return false;
    }
    if app.help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::F(1) => {
                app.help = false
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.help_scroll = app.help_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => app.help_scroll = app.help_scroll.saturating_sub(1),
            KeyCode::PageDown => app.help_scroll = app.help_scroll.saturating_add(height.max(1)),
            KeyCode::PageUp => app.help_scroll = app.help_scroll.saturating_sub(height.max(1)),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.help_scroll = app.help_scroll.saturating_add((height / 2).max(1))
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.help_scroll = app.help_scroll.saturating_sub((height / 2).max(1))
            }
            KeyCode::Char('g') | KeyCode::Home => app.help_scroll = 0,
            KeyCode::Char('G') | KeyCode::End => app.help_scroll = u16::MAX as usize,
            _ => {}
        }
        return false;
    }
    if app.picker.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.picker = None,
            KeyCode::Enter => app.choose(),
            KeyCode::Char('j') | KeyCode::Down => {
                let picker = app.picker.as_mut().unwrap();
                picker.selected = (picker.selected + 1).min(picker.choices.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let picker = app.picker.as_mut().unwrap();
                picker.selected = picker.selected.saturating_sub(1);
            }
            _ => {}
        }
        return false;
    }
    if app.input_mode != InputMode::Normal {
        match key.code {
            KeyCode::Esc => {
                app.input_mode = InputMode::Normal;
                app.input.clear();
            }
            KeyCode::Enter => {
                let input = app.input.trim().to_owned();
                let mode = app.input_mode;
                app.input_mode = InputMode::Normal;
                app.input.clear();
                if mode == InputMode::Command {
                    return execute(app, &input, refresh);
                }
                if app.detail_focus && app.tab == Tab::Logs {
                    app.log_view.set_query(input, &mut app.logs);
                } else {
                    app.tree_query = input;
                    app.refresh_tree();
                }
            }
            KeyCode::Tab if app.input_mode == InputMode::Command => {
                if let Some((command, _)) = ui::COMMANDS
                    .iter()
                    .find(|(command, _)| command.starts_with(&app.input))
                {
                    app.input = (*command).into();
                }
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL) && app.input.len() < 1024 =>
            {
                app.input.push(ch);
            }
            _ => {}
        }
        return false;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let lifecycle_key = !app.detail_focus
        && !app.performance_focus
        && !key.modifiers.intersects(
            KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER
                | KeyModifiers::HYPER
                | KeyModifiers::META,
        )
        && key.kind == KeyEventKind::Press;
    let pending_g = app.pending_g;
    app.pending_g = false;
    match key.code {
        KeyCode::Char('S') if lifecycle_key => app.request_action(tha::actions::Action::Start),
        KeyCode::Char('s') if lifecycle_key => app.request_action(tha::actions::Action::Stop),
        KeyCode::Char('r') if lifecycle_key => app.request_action(tha::actions::Action::Restart),
        KeyCode::Char('d') if lifecycle_key => app.request_action(tha::actions::Action::Delete),
        KeyCode::Char('q') => {
            if app.performance_focus {
                app.performance_focus = false;
            } else if app.detail_focus {
                app.detail_focus = false;
            } else if !app.back() {
                return true;
            }
        }
        KeyCode::Esc => {
            if app.performance_focus {
                app.performance_focus = false;
            } else if app.detail_focus && app.tab == Tab::Logs && !app.log_view.query().is_empty() {
                app.log_view.set_query(String::new(), &mut app.logs);
            } else if !app.tree_query.is_empty() {
                app.tree_query.clear();
                app.refresh_tree();
            } else if app.detail_focus {
                app.detail_focus = false;
            } else {
                app.back();
            }
        }
        KeyCode::Backspace => {
            app.back();
        }
        KeyCode::Char('?') | KeyCode::F(1) => {
            app.help = true;
            app.help_scroll = 0;
        }
        KeyCode::Char('e') => app.request_exec(),
        KeyCode::Char('p') => app.toggle_performance(),
        KeyCode::Char('o') => app.pick_context_sort(),
        KeyCode::Char(':') => {
            app.input_mode = InputMode::Command;
            app.input.clear();
        }
        KeyCode::Char('/') => {
            app.input_mode = InputMode::Search;
            app.input.clear();
        }
        KeyCode::Tab | KeyCode::BackTab => app.cycle_focus(key.code == KeyCode::BackTab),
        KeyCode::Char('1') => {
            app.tab = Tab::Overview;
            app.detail_scroll = 0;
        }
        KeyCode::Char('2') => {
            app.tab = Tab::Logs;
            app.detail_scroll = 0;
        }
        KeyCode::Char('3') => {
            app.tab = Tab::Networks;
            app.detail_scroll = 0;
        }
        KeyCode::Char('4') => {
            app.tab = Tab::Exec;
            app.detail_focus = true;
            app.performance_focus = false;
            app.exec_input = false;
        }
        KeyCode::Enter => {
            if app.performance_focus {
                let engine_index = app.context_order.get(app.performance_scroll).copied();
                let Some(engine_index) = engine_index else {
                    return false;
                };
                while app.back() {}
                app.performance_focus = false;
                app.tree_query.clear();
                app.refresh_tree();
                app.selected = app
                    .rows
                    .iter()
                    .position(|row| {
                        row.engine == engine_index && row.scope == tha::app::Scope::Engine
                    })
                    .unwrap_or(0);
                app.tab = Tab::Overview;
                app.detail_focus = true;
            } else if app.detail_focus && app.tab == Tab::Exec {
                if let Some(session) = &mut app.exec_session
                    && session.running()
                {
                    session.follow();
                    app.exec_input = true;
                } else if app.exec_session.is_none() {
                    app.request_exec();
                }
            } else if !app.detail_focus {
                app.open_selected();
            }
        }
        KeyCode::Char(' ') if app.performance_focus => app.toggle_performance(),
        KeyCode::Char(' ') if !app.detail_focus => app.toggle_branch(),
        KeyCode::Char('j') | KeyCode::Down => move_by(app, 1, height),
        KeyCode::Char('k') | KeyCode::Up => move_by(app, -1, height),
        KeyCode::Char('d') if ctrl => move_by(app, (height / 2).max(1) as isize, height),
        KeyCode::Char('u') if ctrl => move_by(app, -((height / 2).max(1) as isize), height),
        KeyCode::PageDown => move_by(app, height.max(1) as isize, height),
        KeyCode::PageUp => move_by(app, -(height.max(1) as isize), height),
        KeyCode::Char('h') | KeyCode::Left => {
            if !app.detail_focus && !app.performance_focus {
                app.collapse();
            } else if app.tab == Tab::Logs && !app.log_view.wrap {
                app.log_view.horizontal = app.log_view.horizontal.saturating_sub(8);
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            if !app.detail_focus && !app.performance_focus {
                app.expand();
            } else if app.tab == Tab::Logs && !app.log_view.wrap {
                app.log_view.horizontal = (app.log_view.horizontal + 8).min(16_384);
            }
        }
        KeyCode::Char('g') => {
            if pending_g {
                if app.performance_focus {
                    app.performance_scroll = 0;
                } else if !app.detail_focus {
                    app.selected = 0;
                } else if app.tab == Tab::Logs {
                    app.log_view.first(&mut app.logs);
                } else if app.tab == Tab::Exec {
                    if let Some(session) = &mut app.exec_session {
                        session.scroll(500);
                    }
                } else {
                    app.detail_scroll = 0;
                }
            } else {
                app.pending_g = true;
            }
        }
        KeyCode::Char('G') => {
            if app.performance_focus {
                app.performance_scroll = app.engines.len().saturating_sub(1);
            } else if !app.detail_focus {
                app.selected = app.rows.len().saturating_sub(1);
            } else if app.tab == Tab::Logs {
                app.logs.follow = true;
            } else if app.tab == Tab::Exec {
                if let Some(session) = &mut app.exec_session {
                    session.follow();
                }
            } else {
                app.detail_scroll = u16::MAX as usize;
            }
        }
        KeyCode::Char('f') if app.tab == Tab::Logs && app.detail_focus => {
            if app.logs.follow {
                app.log_view.scroll(&mut app.logs, 0);
            } else {
                app.logs.follow = true;
            }
        }
        KeyCode::Char('w') if app.tab == Tab::Logs && app.detail_focus => {
            app.log_view.toggle_wrap(&app.logs)
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            let backwards = key.code == KeyCode::Char('N');
            if app.detail_focus && app.tab == Tab::Logs {
                app.log_view.next_match(&mut app.logs, backwards);
            } else {
                app.next_tree_match(backwards);
            }
        }
        _ => {}
    }
    false
}

fn move_by(app: &mut App, amount: isize, _height: usize) {
    if app.performance_focus {
        app.performance_scroll = app
            .performance_scroll
            .saturating_add_signed(amount)
            .min(app.engines.len().saturating_sub(1));
    } else if !app.detail_focus {
        app.move_selection(amount);
        app.detail_scroll = 0;
    } else if app.tab == Tab::Logs {
        app.log_view.scroll(&mut app.logs, amount);
    } else if app.tab == Tab::Exec {
        if let Some(session) = &mut app.exec_session {
            session.scroll(-amount);
        }
    } else {
        app.detail_scroll = app
            .detail_scroll
            .saturating_add_signed(amount)
            .min(u16::MAX as usize);
    }
}

fn execute(app: &mut App, command: &str, refresh: &watch::Sender<u64>) -> bool {
    app.notice.clear();
    match command {
        "quit" | "q" => return true,
        "overview" => {
            app.tab = Tab::Overview;
            app.detail_focus = true;
            app.detail_scroll = 0;
        }
        "logs" => {
            app.tab = Tab::Logs;
            app.detail_focus = true;
        }
        "networks" => {
            app.tab = Tab::Networks;
            app.detail_focus = true;
            app.detail_scroll = 0;
        }
        "refresh" => {
            refresh.send_modify(|value| *value += 1);
            if app.demo {
                app.notice = "Demo mode: fixture data is unchanged.".into();
            } else {
                app.notice = "Refreshing all engines…".into();
            }
        }
        "reconnect-logs" => {
            app.log_scope = None;
            app.tab = Tab::Logs;
            app.detail_focus = true;
        }
        "wrap" => app.log_view.toggle_wrap(&app.logs),
        "exec" => app.request_exec(),
        "close-exec" => app.close_exec(),
        "performance" => app.toggle_performance(),
        "sort-contexts" => app.pick_context_sort(),
        "back" => {
            app.back();
        }
        "all" => while app.back() {},
        "monochrome" => app.monochrome = !app.monochrome,
        "help" => {
            app.help = true;
            app.help_scroll = 0;
        }
        "" => {}
        _ => {
            app.notice =
                format!("Unknown command: {command}. Press : then Tab to discover commands.")
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> App {
        let mut a = App::new(demo::connections(), 100, false, true);
        demo::populate(&mut a);
        a
    }

    #[test]
    fn search_input_does_not_trigger_navigation_or_quit() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.input_mode = InputMode::Search;
        let selected = app.selected;
        for ch in "q jkl".chars() {
            assert!(!handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                20,
                &tx
            ));
        }
        assert_eq!(app.input, "q jkl");
        assert_eq!(app.selected, selected);
    }

    #[test]
    fn command_palette_executes_and_q_returns_before_quitting() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        assert!(!execute(&mut app, "logs", &tx));
        assert_eq!(app.tab, Tab::Logs);
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(!handle_key(&mut app, q, 20, &tx));
        assert!(handle_key(&mut app, q, 20, &tx));
    }

    #[test]
    fn space_toggles_branches_without_moving_selection_and_ignores_details() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let key = app.current().unwrap().key.clone();
        assert!(app.expanded.contains(&key));
        let expanded_rows = app.rows.len();
        handle_key(&mut app, space, 20, &tx);
        assert!(!app.expanded.contains(&key));
        assert!(app.rows.len() < expanded_rows);
        assert_eq!(app.current().unwrap().key, key);
        handle_key(&mut app, space, 20, &tx);
        assert!(app.expanded.contains(&key));
        assert_eq!(app.rows.len(), expanded_rows);
        app.detail_focus = true;
        handle_key(&mut app, space, 20, &tx);
        assert!(app.expanded.contains(&key));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(!app.detail_focus);
        assert!(app.expanded.contains(&key));
    }

    #[test]
    fn space_on_a_container_does_not_collapse_its_parent() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, tha::app::Scope::Container(_)))
            .unwrap();
        let key = app.current().unwrap().key.clone();
        let expanded = app.expanded.clone();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.current().unwrap().key, key);
        assert_eq!(app.expanded, expanded);
    }

    #[test]
    fn wrap_shortcut_toggles_only_in_logs_and_does_not_fire_while_typing() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        let w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE);
        handle_key(&mut app, w, 20, &tx);
        assert!(app.log_view.wrap);
        app.detail_focus = true;
        app.tab = Tab::Logs;
        handle_key(&mut app, w, 20, &tx);
        assert!(!app.log_view.wrap);
        handle_key(&mut app, w, 20, &tx);
        assert!(app.log_view.wrap);
        app.input_mode = InputMode::Search;
        handle_key(&mut app, w, 20, &tx);
        assert!(app.log_view.wrap);
        assert_eq!(app.input, "w");
    }

    #[test]
    fn cheatsheet_opens_from_shortcuts_and_scrolls_without_moving_tree() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        let selected = app.selected;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.help);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.selected, selected);
        assert_eq!(app.help_scroll, 1);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(!app.help);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.help);
    }

    #[test]
    fn enter_focuses_compose_and_backspace_or_escape_returns_to_overview() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        let selected = app.current().unwrap().key.clone();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.project.is_some());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.project.is_none());
        assert_eq!(app.current().unwrap().key, selected);
        app.open_selected();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.project.is_none());
    }

    #[test]
    fn metric_targets_ignore_aliases_and_do_not_reset_on_unchanged_inventory() {
        let mut app = app();
        let targets: Vec<_> = (0..app.engines.len())
            .map(|_| watch::channel::<Option<metrics::Target>>(None).0)
            .collect();
        publish_metric_targets(&mut app, &targets);
        let revision = app.engines[0].metrics_revision;
        assert!(targets[0].borrow().is_some());
        assert!(targets[3].borrow().is_none());
        publish_metric_targets(&mut app, &targets);
        assert_eq!(app.engines[0].metrics_revision, revision);
        app.engines[1].alias = Some("OrbStack".into());
        publish_metric_targets(&mut app, &targets);
        assert!(targets[1].borrow().is_none());
        app.engines[0].error = Some("offline".into());
        publish_metric_targets(&mut app, &targets);
        assert!(targets[0].borrow().is_none());
        assert!(app.engines[0].metrics_revision > revision);
    }

    #[test]
    fn performance_panel_defaults_closed_and_is_part_of_focus_cycle_when_open() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        assert!(!app.performance_open);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.performance_open);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.performance_focus);
        assert!(!app.detail_focus);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.detail_focus);
        assert!(!app.performance_focus);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(!app.performance_open);
    }

    #[test]
    fn context_sort_shortcut_and_command_open_picker_and_enter_uses_display_order() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.picker.is_some());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            20,
            &tx,
        );
        execute(&mut app, "sort-contexts", &tx);
        app.picker.as_mut().unwrap().selected = 1;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.context_sort, tha::context_sort::ContextSort::Name);
        app.performance_open = true;
        app.performance_focus = true;
        app.performance_scroll = 0;
        let expected = app.context_order[0];
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.current().unwrap().engine, expected);
        assert!(app.detail_focus);
    }

    #[test]
    fn exec_input_owns_control_keys_and_ctrl_bracket_returns_to_navigation() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.tab = Tab::Exec;
        app.detail_focus = true;
        app.exec_input = true;
        for code in [
            KeyCode::Char('p'),
            KeyCode::Char('4'),
            KeyCode::Esc,
            KeyCode::Char('q'),
        ] {
            assert!(!handle_key(
                &mut app,
                KeyEvent::new(code, KeyModifiers::NONE),
                20,
                &tx
            ));
            assert!(app.exec_input);
            assert!(!app.performance_open);
        }
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            20,
            &tx
        ));
        assert!(app.exec_input);
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL),
            20,
            &tx
        ));
        assert!(!app.exec_input);
        assert!(!app.detail_focus);
        app.exec_input = true;
        app.detail_focus = true;
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL),
            20,
            &tx
        ));
        assert!(!app.exec_input);
    }

    #[test]
    fn number_four_selects_exec_and_enter_requests_a_shell_when_none_exists() {
        let mut app = app();
        app.demo = false;
        let (tx, _) = watch::channel(0);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(&row.scope, tha::app::Scope::Container(_)))
            .unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.tab, Tab::Exec);
        assert!(app.detail_focus);
        assert!(!app.exec_input);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.exec_request.is_some());
    }

    #[tokio::test]
    async fn workers_are_replaced_on_socket_change_and_reject_old_or_stopped_events() {
        let mut app = app();
        app.engines[0].connection.endpoint = "unix:///nonexistent/tha-before.sock".into();
        let (snapshots, mut rx) = mpsc::channel(16);
        let (metrics, _) = mpsc::channel(16);
        let (_refresh, refresh) = watch::channel(0);
        let channels = WorkerChannels {
            snapshots,
            metrics,
            refresh,
        };
        let mut workers = Vec::new();
        let mut targets = Vec::new();
        sync_workers(
            &mut app,
            &[(0, true)],
            &mut workers,
            &mut targets,
            &channels,
            5,
        );
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(workers[0].accepts(first.generation));
        sync_workers(
            &mut app,
            &[(0, true)],
            &mut workers,
            &mut targets,
            &channels,
            5,
        );
        assert_eq!(workers[0].generation, first.generation);
        app.engines[0].connection.endpoint = "unix:///nonexistent/tha-after.sock".into();
        sync_workers(
            &mut app,
            &[(0, true)],
            &mut workers,
            &mut targets,
            &channels,
            5,
        );
        assert!(!workers[0].accepts(first.generation));
        let next = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(workers[0].accepts(next.generation));
        assert!(next.result.is_err());
        sync_workers(
            &mut app,
            &[(0, false)],
            &mut workers,
            &mut targets,
            &channels,
            5,
        );
        assert!(!workers[0].accepts(next.generation));
        assert!(workers[0].tasks.is_empty());
        assert!(targets[0].borrow().is_none());
    }

    #[tokio::test]
    async fn log_subscription_generation_changes_when_the_socket_changes() {
        let mut app = app();
        app.tab = Tab::Logs;
        let (tx, _) = mpsc::channel(8);
        let mut task = None;
        update_logs(&mut app, &mut task, &tx, 100);
        let before = app.log_generation;
        let engine = app.current().unwrap().engine;
        app.engines[engine].connection.endpoint = "unix:///new-podman.sock".into();
        update_logs(&mut app, &mut task, &tx, 100);
        assert!(app.log_generation > before);
    }

    fn lifecycle_app(state: &str) -> App {
        let mut app = App::new(
            vec![Connection {
                name: "mock lifecycle".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///mock-only".into(),
            }],
            100,
            false,
            false,
        );
        app.update_engine(
            0,
            Ok(Snapshot {
                engine_id: Some("mock-engine".into()),
                containers: vec![tha::model::Container {
                    id: "fixture".into(),
                    name: "fixture-api".into(),
                    project: Some("fixture-project".into()),
                    service: Some("api".into()),
                    state: state.into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        app.selected = 1;
        app
    }

    #[test]
    fn lifecycle_shortcuts_require_confirmation_and_enter_defaults_to_cancel() {
        let (tx, _) = watch::channel(0);
        for (key, state, action) in [
            ('S', "exited", tha::actions::Action::Start),
            ('s', "running", tha::actions::Action::Stop),
            ('r', "running", tha::actions::Action::Restart),
            ('d', "exited", tha::actions::Action::Delete),
        ] {
            let mut app = lifecycle_app(state);
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
                20,
                &tx,
            );
            assert_eq!(
                app.action_confirmation.as_ref().unwrap().request.action,
                action
            );
            assert!(app.action_request.is_none());
            if key != 'd' {
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    20,
                    &tx,
                );
                assert!(app.action_confirmation.is_none());
                assert!(app.action_request.is_none());
            }
        }
        let mut app = lifecycle_app("running");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            20,
            &tx,
        );
        app.action_review_visible = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.action_request.is_some());
        assert!(app.action_progress.is_some());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.action_confirmation.is_none());
    }

    #[test]
    fn deletion_requires_exact_name_and_stale_connections_cannot_be_confirmed() {
        let (tx, _) = watch::channel(0);
        let mut app = lifecycle_app("exited");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            20,
            &tx,
        );
        app.action_review_visible = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.action_request.is_none());
        assert!(app.action_confirmation.as_ref().unwrap().error.is_some());
        app.action_confirmation.as_mut().unwrap().typed = "fixture-project".into();
        app.engines[0].connection.endpoint = "unix:///changed-socket".into();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.action_request.is_none());
        assert!(app.notice.contains("Connection changed"));
        let mut app = lifecycle_app("exited");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            20,
            &tx,
        );
        app.action_review_visible = true;
        for character in "fixture-project".chars() {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                20,
                &tx,
            );
        }
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(
            app.action_request.as_ref().unwrap().action,
            tha::actions::Action::Delete
        );
    }

    #[test]
    fn lifecycle_keys_do_not_fire_in_details_shell_search_or_with_control_modifiers() {
        let (tx, _) = watch::channel(0);
        for mode in [
            "details",
            "shell",
            "search",
            "performance",
            "control",
            "repeat",
            "demo",
            "context",
            "offline",
            "alias",
        ] {
            let mut app = lifecycle_app("running");
            let mut key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
            match mode {
                "details" => app.detail_focus = true,
                "shell" => {
                    app.detail_focus = true;
                    app.tab = Tab::Exec;
                    app.exec_input = true;
                }
                "search" => app.input_mode = InputMode::Search,
                "performance" => app.performance_focus = true,
                "control" => key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                "repeat" => key.kind = KeyEventKind::Repeat,
                "demo" => app.demo = true,
                "context" => app.selected = 0,
                "offline" => app.engines[0].error = Some("offline".into()),
                _ => app.engines[0].alias = Some("primary".into()),
            }
            handle_key(&mut app, key, 20, &tx);
            assert!(app.action_confirmation.is_none(), "{mode}");
            assert!(app.action_request.is_none());
        }
    }

    #[test]
    fn lifecycle_confirmation_does_not_expand_to_new_replicas_or_follow_selection() {
        let mut app = lifecycle_app("running");
        app.request_action(tha::actions::Action::Stop);
        let mut snapshot = app.engines[0].snapshot.clone().unwrap();
        let mut replica = snapshot.containers[0].clone();
        replica.id = "new-replica".into();
        replica.name = "fixture-api-2".into();
        snapshot.containers.push(replica);
        app.update_engine(0, Ok(snapshot));
        app.selected = 0;
        app.action_review_visible = true;
        app.confirm_action();
        let request = app.action_request.as_ref().unwrap();
        assert_eq!(request.engine, 0);
        assert_eq!(request.containers.len(), 1);
        assert_eq!(request.containers[0].id, "fixture");
    }
}
