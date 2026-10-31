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
    let mut config = Config::load(cli.config.as_ref())?;
    config.monochrome |= cli.monochrome || std::env::var_os("NO_COLOR").is_some();
    if let Some(command) = &cli.command {
        match command {
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
                let connections = connections(&cli, &config)?;
                return tha::daemon::run(config, connections).await;
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
        connections(&cli, &config)?
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
    app.log_view.wrap = config.log_wrap;
    app.exec_command = config.exec_command;
    app.sessions.set_limit(config.max_exec_sessions)?;
    app.local_metrics_refresh_seconds = config.metrics_refresh_seconds;
    app.metrics_refresh_seconds = if app.metrics_database.is_some() {
        1
    } else {
        config.metrics_refresh_seconds
    };
    app.context_pins = config.context_order;
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
    app.keymap = tha::keymap::Keymap::parse(&config.bindings)?;
    app.custom_bindings = config.bindings;
    if config.save_preferences && !cli.demo {
        app.preferences_path = tha::preferences::default_path();
        if let Some(path) = &app.preferences_path {
            match tha::preferences::load(path) {
                Ok(Some(preferences)) => app.apply_preferences(preferences, !cli.overview),
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
    if config.discover_connections {
        connections.extend(tha::collector::discover_local());
    }
    if connections.is_empty() {
        if !config.discover_connections {
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
        if let Some(path) = &app.metrics_database {
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
    let (layers_tx, mut layers_rx) = mpsc::channel::<tha::layers::Event>(4);
    let mut layers_task: Option<AbortTask> = None;
    let (security_tx, mut security_rx) = mpsc::channel::<tha::security::Event>(16);
    let (security_notice_tx, mut security_notice_rx) = mpsc::channel::<String>(4);
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
    loop {
        let mut redraw_now = false;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    let height = terminal.size()?.height.saturating_sub(5 + ui::footer_height(&app)) as usize;
                    if handle_key(&mut app, key, height, &refresh_tx) { break; }
                    redraw_now = true;
                },
                Some(Ok(Event::Paste(text))) => {
                    redraw_now = true;
                    app.cancel_startup();
                    if let Some(confirmation) = &mut app.action_confirmation {
                        if confirmation.request.action == tha::actions::Action::Delete {
                            if text.chars().any(char::is_control) || confirmation.typed.len() + text.len() > 1024 {
                                confirmation.error = Some("Paste contains control characters or exceeds 1024 bytes; nothing pasted".into());
                            } else { confirmation.typed.push_str(&text); }
                        }
                    } else if app.exec_input && app.tab == Tab::Exec {
                        if let Some(session) = app.sessions.active_mut() && let Err(error) = session.paste(&text) { app.notice = format!("{error:#}"); }
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
            Some(event) = snapshot_rx.recv() => {
                if !workers.get(event.engine).is_some_and(|slot| slot.accepts(event.generation)) { continue; }
                app.update_engine(event.engine, event.result);
                publish_metric_targets(&mut app, &metric_targets);
                sync_event_watchers(&app, &mut event_watchers, &event_tx);
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
                                Ok(()) => format!("Finished: {}", job.title),
                                Err(error) => format!("Failed: {} · {error} · :compose-output shows details", job.title),
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
            Some(event) = security_rx.recv() => {
                if let tha::security::Outcome::Finished(Err(error)) = &event.result {
                    app.notice = format!("Security scan failed for {}: {}", tha::model::safe_text(&event.target.label), tha::model::safe_text(error));
                }
                app.security.complete(event);
                redraw_now = true;
            },
            Some(notice) = security_notice_rx.recv() => { app.notice = notice; redraw_now = true; },
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
                                let available: Vec<_>=(0..app.engines.len()).map(|index|(index,true)).collect();
                                sync_workers(&mut app,&available,&mut workers,&mut metric_targets,&channels,refresh_seconds);
                                publish_metric_targets(&mut app,&metric_targets);
                                app.notice=if running { "Connected to daemon metrics; local stats polling stopped" } else { "Daemon stopped; local stats sampling resumed" }.into();
                            }
                            Err(error)=>app.notice=format!("Cannot check daemon: {error:#}"),
                            _=>{}
                        }
                    }
                }
                redraw_now = visual_deadline.is_some_and(|at| std::time::Instant::now() >= at)
                    || app.tab == Tab::Exec && app.sessions.active().is_some();
                redraw_now |= app.tab == Tab::History && history_updated.elapsed() >= Duration::from_secs(1);
                for notice in app.sessions.poll() {
                    redraw_now = true;
                    if Some(notice.id) == app.sessions.active_id() { app.exec_input = false; }
                    app.notice = format!("Exec #{} {} · container was not stopped. F6 switches retained sessions.", notice.id, notice.status);
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
        if let Some(request) = app.log_export_request.take() {
            let sender = export_tx.clone();
            export_task = Some(AbortTask(tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || request.write()).await;
                let message = match result {
                    Ok(Ok(message)) => message,
                    Ok(Err(error)) => format!("Log export failed: {error:#}"),
                    Err(error) => format!("Log export worker failed: {error}"),
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
                        if let Some(path) = app.history_database.clone() {
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
                                .map_err(|error| format!("{error:#}"));
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
            if matches!(app.tab, Tab::Events | Tab::History)
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
                let sender = security_notice_tx.clone();
                tokio::spawn(async move {
                    let notice = match tha::security::cancel_remote().await {
                        Ok(()) => "Security queue cancellation requested".into(),
                        Err(error) => format!("Cannot cancel security queue: {error:#}"),
                    };
                    let _ = sender.send(notice).await;
                });
            }
            if app.security.request {
                if security_task
                    .as_ref()
                    .is_some_and(|task| !task.is_finished())
                {
                    app.notice = "Security queue is still running or cleaning up".into();
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
            update_platform(&mut app, &mut platform_task, &platform_tx);
            update_logs(&mut app, &mut log_task, &log_tx, log_capacity);
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
    Ok(())
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
            .map_err(|error| tha::layers::text(&format!("{error:#}")));
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
    let path = tha::events::database_path(app.history_database.as_ref()?)?;
    let engine = &app.engines[app.current()?.engine];
    let runtime = tha::history::runtime_key(
        &engine.connection,
        &engine.snapshot.as_ref()?.engine_id.clone(),
    );
    let endpoint = engine.connection.endpoint.clone();
    let tx = tx.clone();
    Some(AbortTask(tokio::spawn(async move {
        let since = tha::history::now_ms() - tha::history::RETENTION_MS;
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
    let containers = app.selected_containers();
    let engine = &app.engines[row.engine];
    let scope = log_scope_key(&row, engine, &containers, app.log_since);
    if app.log_scope.as_deref() == Some(&scope) {
        return;
    }
    *task = None;
    app.log_generation += 1;
    app.log_scope = Some(scope);
    let pretty_json = app.logs.pretty_json;
    app.logs = logs::LogBuffer::new(capacity);
    app.logs.pretty_json = pretty_json;
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
                        "2026-10-29T14:32:{:02}Z {{\"level\":\"warn\",\"message\":\"upstream retry\",\"attempt\":2}}",
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

fn log_scope_key(
    row: &tha::app::Row,
    engine: &tha::app::Engine,
    containers: &[tha::model::Container],
    since: Option<i64>,
) -> String {
    format!(
        "{}:{:?}:{:?}:{:?}:{:?}",
        row.key,
        engine.connection.endpoint,
        engine.error,
        since,
        containers
            .iter()
            .map(|container| container.key())
            .collect::<Vec<_>>()
    )
}

fn queue_log_export(app: &mut App, path: &str) -> Result<()> {
    if !app.log_export_review_visible {
        bail!("Review the export dialog in a terminal at least 60 × 18 first");
    }
    if app.log_export_busy {
        bail!("Another export is already running");
    }
    let row = app.current().context("Log selection disappeared")?;
    let engine = &app.engines[row.engine];
    let key = log_scope_key(row, engine, &app.selected_containers(), app.log_since);
    if app.tab != Tab::Logs
        || engine.error.is_some()
        || engine.alias.is_some()
        || app.log_scope.as_ref() != Some(&key)
        || app.log_export_scope.as_ref() != Some(&key)
    {
        bail!(
            "Log target changed or is unavailable; cancel and reopen export after reviewing Logs"
        );
    }
    let request =
        logs::export::Request::snapshot(PathBuf::from(path), &app.logs, &app.log_view.filter)?;
    app.log_export_request = Some(request);
    app.log_export_busy = true;
    Ok(())
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
        } else if let Some(session) = app.sessions.active_mut()
            && let Err(error) = session.key(key)
        {
            app.notice = format!("{error:#}");
        }
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }
    if let Some(removal) = app.inventory_removal.as_mut()
        && !removal.dismissed
    {
        if !removal.pending {
            match key.code {
                KeyCode::Esc | KeyCode::Char('n') => app.inventory_removal = None,
                KeyCode::Enter | KeyCode::Char('y')
                    if removal.visible
                        && !removal.targets.is_empty()
                        && key.kind == KeyEventKind::Press
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    let current = removal.targets.iter().all(|target| {
                        app.engines.iter().any(|engine| {
                            engine.error.is_none()
                                && engine.alias.is_none()
                                && engine.connection.endpoint == target.request.connection.endpoint
                                && engine
                                    .snapshot
                                    .as_ref()
                                    .and_then(|snapshot| snapshot.engine_id.as_deref())
                                    == Some(target.request.identity.as_str())
                        })
                    });
                    if current {
                        removal.pending = true;
                        app.notice = format!(
                            "Deleting {} resources · Esc hides progress; no automatic retries",
                            removal.targets.len()
                        );
                        app.inventory_removal_request = Some(
                            removal
                                .targets
                                .iter()
                                .map(|target| target.request.clone())
                                .collect(),
                        );
                    } else {
                        app.inventory_removal = None;
                        app.notice = "Runtime changed; deletion cancelled".into();
                    }
                }
                KeyCode::Up | KeyCode::PageUp => {
                    removal.scroll = removal
                        .scroll
                        .saturating_sub(if key.code == KeyCode::PageUp {
                            height.max(1)
                        } else {
                            1
                        })
                }
                KeyCode::Down | KeyCode::PageDown => {
                    removal.scroll = removal
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
        } else {
            match key.code {
                KeyCode::Esc => removal.dismissed = true,
                KeyCode::Up | KeyCode::PageUp => {
                    removal.scroll = removal
                        .scroll
                        .saturating_sub(if key.code == KeyCode::PageUp {
                            height.max(1)
                        } else {
                            1
                        })
                }
                KeyCode::Down | KeyCode::PageDown => {
                    removal.scroll = removal
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
        }
        return false;
    }
    if app.inventory_removal_results_visible {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                app.inventory_removal_results_visible = false
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => {
                app.inventory_removal_results_scroll = app
                    .inventory_removal_results_scroll
                    .saturating_sub(if key.code == KeyCode::PageUp {
                        height.max(1)
                    } else {
                        1
                    })
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => {
                app.inventory_removal_results_scroll = app
                    .inventory_removal_results_scroll
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
    if app.compose_confirmation.is_some() {
        let typing = app
            .compose_confirmation
            .as_ref()
            .unwrap()
            .request
            .action
            .requires_typing();
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => app.compose_confirmation = None,
            KeyCode::Enter if key.kind == KeyEventKind::Press && plain => app.confirm_compose(),
            KeyCode::Char('n') if !typing => app.compose_confirmation = None,
            KeyCode::Char('y') if !typing && plain => app.confirm_compose(),
            KeyCode::Backspace if typing => {
                app.compose_confirmation.as_mut().unwrap().typed.pop();
            }
            KeyCode::Char(ch) if typing && plain => {
                let confirmation = app.compose_confirmation.as_mut().unwrap();
                if !ch.is_control() && confirmation.typed.len() + ch.len_utf8() <= 1024 {
                    confirmation.typed.push(ch);
                }
            }
            _ => {}
        }
        return false;
    }
    if app.compose_output_visible {
        if let Some(job) = &mut app.compose_job {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    app.compose_output_visible = false;
                }
                KeyCode::Up | KeyCode::Char('k') => job.scroll = job.scroll.saturating_add(1),
                KeyCode::Down | KeyCode::Char('j') => job.scroll = job.scroll.saturating_sub(1),
                KeyCode::PageUp => job.scroll = job.scroll.saturating_add(height.max(1)),
                KeyCode::PageDown => job.scroll = job.scroll.saturating_sub(height.max(1)),
                KeyCode::End | KeyCode::Char('G') => job.scroll = 0,
                _ => {}
            }
        } else {
            app.compose_output_visible = false;
        }
        return false;
    }
    if app.action_confirmation.is_some() {
        use tha::actions::Action;
        let deleting = app.action_confirmation.as_ref().unwrap().request.action == Action::Delete;
        match key.code {
            KeyCode::Esc => app.action_confirmation = None,
            KeyCode::Enter if deleting => app.confirm_action(),
            KeyCode::Enter
                if key.kind == KeyEventKind::Press
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                app.confirm_action()
            }
            KeyCode::Char('n') if !deleting => app.action_confirmation = None,
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
    if app.action_results_visible {
        let Some(progress) = app.action_progress_mut() else {
            app.action_results_visible = false;
            return false;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                app.action_results_visible = false
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
    if app.input_mode == InputMode::InventorySearch {
        let view = app
            .inventory_views
            .entry((
                app.inventory_engine,
                if app.overview_mode == tha::app::OverviewMode::Security {
                    tha::app::OverviewMode::Images
                } else {
                    app.overview_mode
                },
            ))
            .or_default();
        match key.code {
            KeyCode::Esc => {
                app.input_mode = InputMode::Normal;
                app.input.clear();
            }
            KeyCode::Enter => {
                view.query = std::mem::take(&mut app.input);
                view.marked.clear();
                view.index = 0;
                view.selected = None;
                view.offset = 0;
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.input.clear()
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && app.input.len() + ch.len_utf8() <= 1024 =>
            {
                app.input.push(ch)
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
                let mode = app.input_mode;
                let input = if mode == InputMode::ExportLogs {
                    app.input.clone()
                } else {
                    app.input.trim().to_owned()
                };
                app.input_mode = InputMode::Normal;
                app.input.clear();
                if mode == InputMode::Command {
                    return execute(app, &input, refresh);
                }
                if mode == InputMode::LogFilter {
                    match logs::filter::Filter::parse(&input) {
                        Ok(filter) => {
                            app.log_view.filter = filter;
                            app.log_filter_text = input;
                            app.notice = "Applied retained-log filter; entries without required metadata do not match. :clear-log-filter resets it.".into();
                        }
                        Err(error) => {
                            app.notice = format!("Invalid log filter: {error:#}");
                            app.input_mode = mode;
                            app.input = input;
                        }
                    }
                } else if mode == InputMode::ExportLogs {
                    if let Err(error) = queue_log_export(app, &input) {
                        app.notice = format!("{error:#}");
                        app.input_mode = mode;
                        app.input = input;
                    }
                } else if app.detail_focus && app.tab == Tab::Logs {
                    if let Err(error) = app.log_view.check_query(&input) {
                        app.notice = tha::model::safe_text(&format!("Invalid regex: {error}"))
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        app.input_mode = mode;
                        app.input = input;
                    } else {
                        if app.notice.starts_with("Invalid regex") {
                            app.notice.clear();
                        }
                        app.log_view.set_query(input, &mut app.logs);
                    }
                } else {
                    app.tree_query = input;
                    app.refresh_tree();
                }
            }
            KeyCode::Char('r')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && app.input_mode == InputMode::Search
                    && app.detail_focus
                    && app.tab == Tab::Logs =>
            {
                app.log_view.regex_search = !app.log_view.regex_search;
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
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(app.input_mode, InputMode::LogFilter | InputMode::ExportLogs) =>
            {
                app.input.clear()
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && app.input.len() + ch.len_utf8() <= 1024 =>
            {
                app.input.push(ch);
            }
            _ => {}
        }
        return false;
    }
    // Only navigation reaches this point. Editors, confirmations and PTYs retain raw keys.
    let context = NavigationContext::of(app);
    let Some(action) = shortcuts::resolve(context, key, &app.keymap, app.security.enabled) else {
        app.pending_g = false;
        return false;
    };
    if !action.info().scope.includes(context) {
        app.notice = format!(
            "{} is unavailable in {}",
            action.info().label,
            context.label()
        );
        return false;
    }
    if app.keymap.action(key).is_some()
        && let Some(reason) = shortcuts::unavailable(app, action)
    {
        app.notice = reason;
        return false;
    }
    dispatch_navigation(app, action, key, height, refresh)
}

fn dispatch_navigation(
    app: &mut App,
    action: A,
    key: KeyEvent,
    height: usize,
    refresh: &watch::Sender<u64>,
) -> bool {
    // Command-only actions use the same implementation and safeguards as the palette.
    if matches!(
        action,
        A::AllRuntimes
            | A::ClearLogFilter
            | A::ExportLogs
            | A::ReconnectLogs
            | A::LogsSince
            | A::ComposeUpdate
            | A::ComposePull
            | A::ComposeUp
            | A::ComposeRecreate
            | A::ComposeDown
            | A::ComposeOutput
            | A::PrettyJson
            | A::CheckUpdates
            | A::PullImage
            | A::CloseExec
            | A::UpdateSecurity
            | A::ReloadLayers
            | A::Refresh
            | A::ActionResults
            | A::RemovalResults
            | A::Monochrome
            | A::Quit
    ) {
        return execute(app, action.info().commands[0], refresh);
    }
    if matches!(action, A::PreviousView | A::NextView) {
        use tha::app::OverviewMode::*;
        let forward = action == A::NextView;
        let modes: &[tha::app::OverviewMode] = if app.security.enabled {
            &[Containers, Images, Volumes, Security]
        } else {
            &[Containers, Images, Volumes]
        };
        let index = modes
            .iter()
            .position(|mode| *mode == app.overview_mode)
            .unwrap_or(0);
        let next = modes[if forward {
            (index + 1) % modes.len()
        } else {
            (index + modes.len() - 1) % modes.len()
        }];
        switch_overview(app, next);
        return false;
    }
    match action {
        A::ContainersView => switch_overview(app, tha::app::OverviewMode::Containers),
        A::ImagesView => switch_overview(app, tha::app::OverviewMode::Images),
        A::VolumesView => switch_overview(app, tha::app::OverviewMode::Volumes),
        A::SecurityView => switch_overview(app, tha::app::OverviewMode::Security),
        A::Help => {
            app.help = true;
            app.help_scroll = 0;
        }
        A::Commands => {
            app.input_mode = InputMode::Command;
            app.input.clear();
        }
        A::Views => {
            return execute(app, "views", refresh);
        }
        _ => {}
    }
    if matches!(
        action,
        A::ContainersView
            | A::ImagesView
            | A::VolumesView
            | A::SecurityView
            | A::Help
            | A::Commands
            | A::Views
    ) {
        return false;
    }
    // Detail shortcuts explicitly return from an inventory to workload details.
    if matches!(
        action,
        A::Overview | A::Logs | A::Networks | A::Exec | A::Metrics | A::Layers | A::Events
    ) {
        switch_overview(app, tha::app::OverviewMode::Containers);
    }
    if app.overview_mode == tha::app::OverviewMode::Security {
        let finding_list =
            !app.security.expanded && matches!(app.security.state, tha::security::State::Ready(_));
        match action {
            A::ScanImage => app.request_security_scan(false),
            A::ScanImages => app.request_security_scan(true),
            A::CancelScan => app.security.cancel = true,
            A::CheckSecurityImage | A::CheckImage | A::CheckImages => {
                app.overview_mode = tha::app::OverviewMode::Images;
                ui::inventory::navigate(
                    app,
                    if action == A::CheckImages {
                        KeyCode::Char('M')
                    } else {
                        KeyCode::Char('m')
                    },
                    height,
                );
                app.overview_mode = tha::app::OverviewMode::Security;
            }
            A::ClearScanChecks => {
                app.inventory_views
                    .entry((app.inventory_engine, tha::app::OverviewMode::Images))
                    .or_default()
                    .scan_marked
                    .clear();
            }
            A::SecurityFocusNext => app.detail_focus = !app.detail_focus,
            A::Zoom => app.security.zoom = !app.security.zoom,
            A::Open if !app.detail_focus => app.open_security(),
            A::Open if matches!(&app.security.state, tha::security::State::Ready(report) if !report.findings.is_empty()) =>
            {
                app.security.expanded = !app.security.expanded;
                app.detail_scroll = 0;
            }
            A::Open => {}
            A::Escape if app.security.expanded => {
                app.security.expanded = false;
                app.detail_scroll = 0;
            }
            A::Escape if app.security.zoom => app.security.zoom = false,
            A::Escape if app.detail_focus => app.detail_focus = false,
            A::Down if app.detail_focus && finding_list => app.security.move_by(1),
            A::Up if app.detail_focus && finding_list => app.security.move_by(-1),
            A::First if app.detail_focus && finding_list => app.security.move_by(isize::MIN),
            A::Last if app.detail_focus && finding_list => app.security.move_by(isize::MAX),
            A::PageDown if app.detail_focus && finding_list => {
                app.security.move_by(height as isize)
            }
            A::PageUp if app.detail_focus && finding_list => {
                app.security.move_by(-(height as isize))
            }
            A::Down if app.detail_focus => app.detail_scroll = app.detail_scroll.saturating_add(1),
            A::Up if app.detail_focus => app.detail_scroll = app.detail_scroll.saturating_sub(1),
            A::PageDown if app.detail_focus => {
                app.detail_scroll = app.detail_scroll.saturating_add(height)
            }
            A::PageUp if app.detail_focus => {
                app.detail_scroll = app.detail_scroll.saturating_sub(height)
            }
            A::First if app.detail_focus => app.detail_scroll = 0,
            A::Last if app.detail_focus => app.detail_scroll = u16::MAX as usize,
            _ => {
                app.overview_mode = tha::app::OverviewMode::Images;
                let quit = dispatch_navigation(app, action, key, height, refresh);
                if app.overview_mode == tha::app::OverviewMode::Images {
                    app.overview_mode = tha::app::OverviewMode::Security;
                }
                return quit;
            }
        }
        return false;
    }
    if app.overview_mode != tha::app::OverviewMode::Containers {
        match action {
            A::RemoveResources => ui::inventory::begin_removal(app),
            A::PruneResources => ui::inventory::begin_prune(app),
            A::Escape
                if app
                    .inventory_views
                    .get(&(app.inventory_engine, app.overview_mode))
                    .is_some_and(|view| view.details) =>
            {
                ui::inventory::navigate(app, KeyCode::Esc, height)
            }
            A::Escape => switch_overview(app, tha::app::OverviewMode::Containers),
            A::Back => return true,
            A::PreviousRuntime
            | A::NextRuntime
            | A::SecurityPreviousRuntime
            | A::SecurityNextRuntime => {
                app.inventory_engine =
                    if matches!(action, A::PreviousRuntime | A::SecurityPreviousRuntime) {
                        app.inventory_engine.saturating_sub(1)
                    } else {
                        app.inventory_engine
                            .saturating_add(1)
                            .min(app.engines.len().saturating_sub(1))
                    };
            }
            A::Search => {
                let view = app
                    .inventory_views
                    .entry((app.inventory_engine, app.overview_mode))
                    .or_default();
                app.input = view.query.clone();
                view.details = false;
                app.input_mode = InputMode::InventorySearch;
            }
            A::Up
            | A::Down
            | A::PageUp
            | A::PageDown
            | A::First
            | A::Last
            | A::Open
            | A::InventoryFilter
            | A::InventorySort
            | A::Mark
            | A::MarkUnused
            | A::ClearMarks
            | A::Zoom
            | A::CheckImage
            | A::CheckImages => {
                // The existing inventory operation accepts a canonical key, not
                // a user binding. Never feed it the original configured key.
                let code = if action == A::First
                    && (key.code == KeyCode::Home || app.keymap.action(key).is_some())
                {
                    KeyCode::Home
                } else if action == A::Last {
                    KeyCode::End
                } else {
                    let binding = action.info().bindings.first().expect("inventory binding");
                    let event: KeyEvent = binding
                        .key
                        .parse::<crokey::KeyCombination>()
                        .expect("catalog key")
                        .into();
                    event.code
                };
                ui::inventory::navigate(app, code, height)
            }
            A::RefreshInventory => refresh.send_modify(|value| *value += 1),
            A::Security => app.open_security(),
            _ => {}
        }
        return false;
    }
    if app.detail_focus && app.tab == Tab::Security {
        let handled = match action {
            A::Zoom => {
                app.security.zoom = !app.security.zoom;
                true
            }
            A::Open => {
                app.security.expanded = !app.security.expanded;
                app.detail_scroll = 0;
                true
            }
            A::Escape if app.security.expanded => {
                app.security.expanded = false;
                app.detail_scroll = 0;
                true
            }
            A::Escape if app.security.zoom => {
                app.security.zoom = false;
                true
            }
            A::Down if !app.security.expanded => {
                app.security.move_by(1);
                true
            }
            A::Up if !app.security.expanded => {
                app.security.move_by(-1);
                true
            }
            A::First if !app.security.expanded => {
                app.security.move_by(isize::MIN);
                true
            }
            A::Last if !app.security.expanded => {
                app.security.move_by(isize::MAX);
                true
            }
            A::PageDown if !app.security.expanded => {
                app.security.move_by(height as isize);
                true
            }
            A::PageUp if !app.security.expanded => {
                app.security.move_by(-(height as isize));
                true
            }
            _ => false,
        };
        if handled {
            return false;
        }
    }
    if app.detail_focus && app.tab == Tab::History {
        let view = &mut app.history_view;
        let handled = match action {
            A::PreviousRange => {
                view.range_by(-1);
                true
            }
            A::NextRange => {
                view.range_by(1);
                true
            }
            A::MetricsLeft | A::MetricsRight
                if view.cursor.is_none()
                    && (matches!(key.code, KeyCode::Left | KeyCode::Right)
                        || app.keymap.action(key).is_some()) =>
            {
                view.pan(if action == A::MetricsLeft { -1 } else { 1 });
                true
            }
            A::MetricsLeft => {
                view.cursor = Some(
                    view.cursor
                        .unwrap_or(view.data.points.len().saturating_sub(1))
                        .saturating_sub(1),
                );
                view.end_ms.get_or_insert_with(tha::history::now_ms);
                true
            }
            A::MetricsRight => {
                view.cursor = Some(
                    view.cursor
                        .unwrap_or(0)
                        .saturating_add(1)
                        .min(view.data.points.len().saturating_sub(1)),
                );
                view.end_ms.get_or_insert_with(tha::history::now_ms);
                true
            }
            A::MetricsCursor => {
                view.cursor = if view.cursor.is_some() {
                    None
                } else {
                    Some(view.data.points.len().saturating_sub(1))
                };
                view.end_ms.get_or_insert_with(tha::history::now_ms);
                true
            }
            A::MetricsLive => {
                view.end_ms = None;
                view.cursor = None;
                true
            }
            A::Zoom => {
                view.expanded = !view.expanded;
                true
            }
            A::Down => {
                view.metric = (view.metric + 1) % 4;
                true
            }
            A::Up => {
                view.metric = (view.metric + 3) % 4;
                true
            }
            A::Escape if view.expanded => {
                view.expanded = false;
                true
            }
            A::Escape if view.cursor.is_some() => {
                view.cursor = None;
                true
            }
            _ => false,
        };
        if handled {
            return false;
        }
    }
    let pending_g = app.pending_g;
    app.pending_g = false;
    match action {
        A::FilterLogs => {
            app.input_mode = InputMode::LogFilter;
            app.input = app.log_filter_text.clone();
        }
        A::Start | A::Stop | A::Restart | A::DeleteContainer => {
            app.request_action(action.lifecycle().expect("lifecycle action"))
        }
        A::Back => {
            if app.detail_zoom == Some(app.tab) {
                app.detail_zoom = None;
            } else if app.tab == Tab::History && app.history_view.expanded {
                app.history_view.expanded = false;
            } else if app.performance_focus {
                app.performance_focus = false;
            } else if app.detail_focus {
                app.detail_focus = false;
            } else if !app.back() {
                return true;
            }
        }
        A::Escape => {
            if app.detail_zoom == Some(app.tab) {
                app.detail_zoom = None;
            } else if app.tab == Tab::History && app.history_view.expanded {
                app.history_view.expanded = false;
            } else if app.performance_open && app.performance_expanded {
                app.performance_expanded = false;
            } else if app.tab == Tab::Layers && app.detail_focus && app.layers.expanded {
                app.layers.expanded = false;
                app.detail_scroll = 0;
            } else if app.performance_focus {
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
        A::WorkspaceBack => {
            app.back();
        }
        A::OpenExec => app.request_exec(),
        A::Performance | A::PerformanceSpace => app.toggle_performance(),
        A::PerformanceScope => app.switch_performance_scope(),
        A::Zoom if app.performance_focus => {
            app.performance_expanded = !app.performance_expanded;
        }
        A::Zoom if app.detail_focus => {
            app.detail_zoom = if app.detail_zoom == Some(app.tab) {
                None
            } else {
                Some(app.tab)
            };
        }
        A::SortContexts => app.pick_context_sort(),
        A::ExecSessions => app.pick_sessions(),
        A::Actions => app.open_actions(),
        A::Search => {
            app.input_mode = InputMode::Search;
            app.input.clear();
        }
        A::FocusNext | A::FocusPrevious => app.cycle_focus(action == A::FocusPrevious),
        A::Overview => {
            app.tab = Tab::Overview;
            app.detail_scroll = 0;
        }
        A::Events => {
            app.tab = Tab::Events;
            app.detail_focus = true;
            app.performance_focus = false;
            app.detail_scroll = 0;
        }
        A::Logs => {
            app.tab = Tab::Logs;
            app.detail_scroll = 0;
        }
        A::Networks => {
            app.tab = Tab::Networks;
            app.detail_scroll = 0;
        }
        A::Exec => {
            app.tab = Tab::Exec;
            app.detail_focus = true;
            app.performance_focus = false;
            app.exec_input = false;
        }
        A::Layers => {
            if !app.layers_available() {
                return false;
            }
            app.tab = Tab::Layers;
            app.detail_focus = true;
            app.performance_focus = false;
            app.layers.expanded = false;
            app.detail_scroll = 0;
        }
        A::Security => app.open_security(),
        A::Metrics if app.performance_container().is_some() => {
            app.tab = Tab::History;
            app.detail_focus = true;
            app.performance_focus = false;
        }
        A::Open => {
            if app.performance_focus
                && app.performance_scope == tha::app::PerformanceScope::Container
            {
                if app.performance_container().is_some() {
                    app.performance_focus = false;
                    app.detail_focus = true;
                    app.tab = Tab::Overview;
                }
            } else if app.performance_focus {
                let engine_index = app.performance_engine();
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
            } else if app.detail_focus && app.tab == Tab::Layers {
                if matches!(&app.layers.state, tha::layers::State::Ready(image) if image.rows() > 0)
                {
                    app.layers.expanded = !app.layers.expanded;
                    app.detail_scroll = 0;
                }
            } else if app.detail_focus && app.tab == Tab::Exec {
                if let Some(session) = app.sessions.active_mut()
                    && session.running()
                {
                    session.follow();
                    app.exec_input = true;
                } else if app.sessions.active().is_none() {
                    app.request_exec();
                }
            } else if !app.detail_focus {
                app.open_selected();
            }
        }
        A::ToggleBranch => app.toggle_branch(),
        A::Down => move_by(app, 1, height),
        A::Up => move_by(app, -1, height),
        A::HalfPageDown => move_by(app, (height / 2).max(1) as isize, height),
        A::HalfPageUp => move_by(app, -((height / 2).max(1) as isize), height),
        A::PageDown => move_by(app, height.max(1) as isize, height),
        A::PageUp => move_by(app, -(height.max(1) as isize), height),
        A::Left => {
            if !app.detail_focus && !app.performance_focus {
                app.collapse();
            } else if app.tab == Tab::Logs && !app.log_view.wrap {
                app.log_view.horizontal = app.log_view.horizontal.saturating_sub(8);
            }
        }
        A::Right => {
            if !app.detail_focus && !app.performance_focus {
                app.expand();
            } else if app.tab == Tab::Logs && !app.log_view.wrap {
                app.log_view.horizontal = (app.log_view.horizontal + 8).min(16_384);
            }
        }
        A::First => {
            if pending_g || key.code == KeyCode::Home || app.keymap.action(key).is_some() {
                if app.performance_focus {
                    app.move_performance_selection(isize::MIN);
                } else if !app.detail_focus {
                    app.selected = 0;
                } else if app.tab == Tab::Logs {
                    app.log_view.first(&mut app.logs);
                } else if app.tab == Tab::Layers && !app.layers.expanded {
                    app.layers.selected = 0;
                    app.detail_scroll = 0;
                } else if app.tab == Tab::Exec {
                    if let Some(session) = app.sessions.active_mut() {
                        session.scroll(500);
                    }
                } else {
                    app.detail_scroll = 0;
                }
            } else {
                app.pending_g = true;
            }
        }
        A::Last => {
            if app.performance_focus {
                app.move_performance_selection(isize::MAX);
            } else if !app.detail_focus {
                app.selected = app.rows.len().saturating_sub(1);
            } else if app.tab == Tab::Logs {
                app.logs.follow = true;
            } else if app.tab == Tab::Layers && !app.layers.expanded {
                app.layers.move_by(isize::MAX);
            } else if app.tab == Tab::Exec {
                if let Some(session) = app.sessions.active_mut() {
                    session.follow();
                }
            } else {
                app.detail_scroll = u16::MAX as usize;
            }
        }
        A::Follow => {
            if app.logs.follow {
                app.log_view.scroll(&mut app.logs, 0);
            } else {
                app.logs.follow = true;
            }
        }
        A::Wrap => app.log_view.toggle_wrap(&app.logs),
        A::NextMatch | A::PreviousMatch => {
            let backwards = action == A::PreviousMatch;
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
        app.move_performance_selection(amount);
    } else if !app.detail_focus {
        app.move_selection(amount);
        app.detail_scroll = 0;
    } else if app.tab == Tab::Logs {
        app.log_view.scroll(&mut app.logs, amount);
    } else if app.tab == Tab::Layers && !app.layers.expanded {
        app.layers.move_by(amount);
    } else if app.tab == Tab::Exec {
        if let Some(session) = app.sessions.active_mut() {
            session.scroll(-amount);
        }
    } else {
        app.detail_scroll = app
            .detail_scroll
            .saturating_add_signed(amount)
            .min(u16::MAX as usize);
    }
}

fn switch_overview(app: &mut App, mode: tha::app::OverviewMode) {
    if mode == tha::app::OverviewMode::Security && !app.security.enabled {
        return;
    }
    if app.overview_mode == tha::app::OverviewMode::Containers
        && mode != tha::app::OverviewMode::Containers
    {
        app.inventory_engine = app.current().map(|row| row.engine).unwrap_or(0);
    }
    app.overview_mode = mode;
    if mode == tha::app::OverviewMode::Security {
        app.detail_focus = false;
    }
}

fn execute(app: &mut App, command: &str, refresh: &watch::Sender<u64>) -> bool {
    if matches!(
        command,
        "overview" | "logs" | "networks" | "layers" | "history" | "metrics" | "exec" | "events"
    ) {
        switch_overview(app, tha::app::OverviewMode::Containers);
    }
    app.notice.clear();
    if let Some(argument) = command.strip_prefix("logs-since ") {
        match logs::parse_since(argument, time::OffsetDateTime::now_utc()) {
            Ok(since) => {
                app.log_since = since;
                app.tab = Tab::Logs;
                app.detail_focus = true;
                app.performance_focus = false;
                if app.demo {
                    app.notice = "Demo mode: fixture logs ignore the start time.".into();
                }
            }
            Err(error) => {
                app.notice = format!("{error:#}");
                app.input_mode = InputMode::Command;
                app.input = command.into();
            }
        }
        return false;
    }
    match command {
        "events" => {
            app.tab = Tab::Events;
            app.detail_focus = true;
            app.performance_focus = false;
            app.detail_scroll = 0;
        }
        "compose-update" => app.request_compose(tha::compose::Action::Update),
        "compose-pull" => app.request_compose(tha::compose::Action::Pull),
        "compose-up" => app.request_compose(tha::compose::Action::Up),
        "compose-recreate" => app.request_compose(tha::compose::Action::Recreate),
        "compose-down" => app.request_compose(tha::compose::Action::Down),
        "compose-output" => {
            if app.compose_job.is_some() {
                app.compose_output_visible = true;
            } else {
                app.notice = "No Compose operation has run in this session".into();
            }
        }
        "logs-since" => {
            app.input_mode = InputMode::Command;
            app.input = "logs-since ".into();
        }
        "json" => {
            app.log_view.toggle_pretty_json(&mut app.logs);
        }
        "check-updates" => ui::inventory::begin_update_check(app),
        "pull-image" => ui::inventory::begin_image_pull(app),
        "security" => app.open_security(),
        "scan-image" => app.request_security_scan(false),
        "scan-images" => app.request_security_scan(true),
        "prune" => ui::inventory::begin_prune(app),
        "cancel-scan" if app.overview_mode == tha::app::OverviewMode::Containers => {
            app.notice = "Scans run from the Security view (Ctrl+→ or Alt+4)".into();
        }
        "cancel-scan" => {
            app.security.cancel = true;
            app.notice = "Cancelling security queue and removing scanner…".into();
        }
        "update-security-db" => {
            app.request_security_scan(false);
            if app.security.request {
                app.security.update = true;
            }
        }
        "removal-results" => {
            if app.inventory_removal_report.is_some() {
                app.inventory_removal_results_visible = true;
                app.inventory_removal_results_scroll = 0;
            } else {
                app.notice = "No image/volume cleanup results yet".into();
            }
        }
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
        "layers" => {
            if !app.layers_available() {
                app.notice = "Layers requires a container or single-container service".into();
                return false;
            }
            app.tab = Tab::Layers;
            app.detail_focus = true;
            app.performance_focus = false;
            app.detail_scroll = 0;
        }
        "images" | "volumes" => {
            let mode = if command == "images" {
                tha::app::OverviewMode::Images
            } else {
                tha::app::OverviewMode::Volumes
            };
            switch_overview(app, mode);
        }
        "history" | "metrics" => {
            if app.performance_container().is_some() {
                app.tab = Tab::History;
                app.detail_focus = true;
                app.performance_focus = false;
            } else {
                app.notice = "Select a container to open Metrics".into();
            }
        }
        "reload-layers" => {
            app.layers.select(None);
            app.detail_scroll = 0;
        }
        "actions" => app.open_actions(),
        "action-results" => app.open_action_results(),
        "filter-logs" => {
            app.tab = Tab::Logs;
            app.detail_focus = true;
            app.performance_focus = false;
            app.input_mode = InputMode::LogFilter;
            app.input = app.log_filter_text.clone();
        }
        "clear-log-filter" => {
            app.log_view.filter = Default::default();
            app.log_filter_text = "{}".into();
        }
        "export-logs" => {
            if app.tab != Tab::Logs || app.log_scope.is_none() {
                app.notice =
                    "Open and review Logs for the desired workload before exporting".into();
            } else if app.log_export_busy {
                app.notice = "Another export is already running".into();
            } else {
                app.log_export_scope = app.log_scope.clone();
                app.log_export_review_visible = false;
                app.input_mode = InputMode::ExportLogs;
                app.input.clear();
                app.notice.clear();
            }
        }
        "close-exec" => app.close_exec(),
        "exec-sessions" => app.pick_sessions(),
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
    #[test]
    fn shared_navigation_routes_every_pane_without_stealing_modal_input() {
        use tha::app::OverviewMode as O;
        let (tx, _) = watch::channel(0);
        for (overview, focus, performance, tab, details, expanded) in [
            (O::Containers, false, false, Tab::Overview, false, false),
            (O::Containers, false, true, Tab::Overview, false, false),
            (O::Containers, true, false, Tab::Overview, false, false),
            (O::Containers, true, false, Tab::Logs, false, false),
            (O::Containers, true, false, Tab::Exec, false, false),
            (O::Containers, true, false, Tab::History, false, false),
            (O::Containers, true, false, Tab::Layers, false, false),
            (O::Images, false, false, Tab::Overview, false, false),
            (O::Images, false, false, Tab::Overview, true, false),
            (O::Volumes, false, false, Tab::Overview, false, false),
            (O::Volumes, false, false, Tab::Overview, true, false),
            (O::Security, false, false, Tab::Security, false, false),
            (O::Security, true, false, Tab::Security, false, false),
            (O::Security, true, false, Tab::Security, false, true),
        ] {
            let mut app = app();
            app.overview_mode = overview;
            app.detail_focus = focus;
            app.performance_focus = performance;
            app.tab = tab;
            app.security.enabled = true;
            app.security.expanded = expanded;
            app.inventory_views
                .entry((0, overview))
                .or_default()
                .details = details;
            let context = NavigationContext::of(&app);
            assert_eq!(
                shortcuts::resolve(context, KeyEvent::from(KeyCode::Down), &app.keymap, true),
                Some(A::Down)
            );
            handle_key(&mut app, KeyEvent::from(KeyCode::Down), 20, &tx);
            handle_key(&mut app, KeyEvent::from(KeyCode::F(1)), 20, &tx);
            assert!(app.help, "{context:?}");
            assert!(app.action_request.is_none());
            assert!(app.exec_request.is_none());
            assert!(!app.security.request);
        }
    }
    #[test]
    fn history_navigation_is_container_only_and_does_not_steal_editor_or_shell_input() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.selected = 0;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('6')), 20, &tx);
        assert_eq!(app.tab, Tab::Overview);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, tha::app::Scope::Container(_)))
            .unwrap();
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('6')), 20, &tx);
        assert_eq!(app.tab, Tab::History);
        assert!(app.detail_focus);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char(']')), 20, &tx);
        assert_eq!(app.history_view.range, 1);
        handle_key(&mut app, KeyEvent::from(KeyCode::Left), 20, &tx);
        assert!(app.history_view.end_ms.is_some());
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('z')), 20, &tx);
        assert!(app.history_view.expanded);
        app.detail_focus = false;
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc), 20, &tx);
        assert!(!app.history_view.expanded);
        app.detail_focus = true;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('f')), 20, &tx);
        assert!(app.history_view.end_ms.is_none());
        app.input_mode = InputMode::Search;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char(']')), 20, &tx);
        assert_eq!(app.input, "]");
        assert_eq!(app.history_view.range, 1);
        app.input_mode = InputMode::Normal;
        app.selected = 0;
        app.reconcile_detail_tab();
        assert_eq!(app.tab, Tab::Overview);
    }
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
    fn invalid_log_regex_keeps_the_prompt_and_current_search_and_says_why() {
        let (tx, _) = watch::channel(0);
        let mut app = App::new(tha::demo::connections(), 100, true, true);
        tha::demo::populate(&mut app);
        app.tab = Tab::Logs;
        app.detail_focus = true;
        app.logs.push("api", "status=500", false);
        app.input_mode = InputMode::Search;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            20,
            &tx,
        );
        assert!(app.log_view.regex_search);
        app.input = r"5\d\d".into();
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
        assert_eq!(app.log_view.query(), r"5\d\d");
        assert_eq!(app.input_mode, InputMode::Normal);
        app.input_mode = InputMode::Search;
        app.input = "(bad".into();
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
        assert_eq!(app.input_mode, InputMode::Search);
        assert_eq!(app.input, "(bad");
        assert!(app.notice.starts_with("Invalid regex"), "{}", app.notice);
        assert_eq!(app.log_view.query(), r"5\d\d", "The current search stays");
        app.input = "ok".into();
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
        assert!(app.notice.is_empty());
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
    fn scanning_checked_images_preserves_focus_and_navigation() {
        use tha::app::OverviewMode;
        for shortcut in ['s', 'S'] {
            let mut app = app();
            app.demo = false;
            app.security.enabled = true;
            app.overview_mode = OverviewMode::Images;
            app.detail_focus = false;
            let snapshot = app.engines[0].snapshot.as_mut().unwrap();
            snapshot.engine_id = Some("fixture-engine".into());
            snapshot.images = Some(
                ['a', 'b', 'c']
                    .into_iter()
                    .map(|ch| tha::model::Image {
                        id: format!("sha256:{}", ch.to_string().repeat(64)),
                        tags: vec![format!("image-{ch}:latest")],
                        size: None,
                        ..Default::default()
                    })
                    .collect(),
            );
            ui::inventory::navigate(&mut app, KeyCode::Char('M'), 25);
            app.overview_mode = OverviewMode::Security;
            app.inventory_views
                .get_mut(&(0, OverviewMode::Images))
                .unwrap()
                .query = "image-b".into();
            let (tx, _) = watch::channel(0);
            handle_key(&mut app, KeyEvent::from(KeyCode::Char(shortcut)), 25, &tx);
            assert!(app.security.request);
            assert_eq!(
                app.security.target.as_ref().unwrap().image,
                format!("sha256:{}", "b".repeat(64))
            );
            assert_eq!(
                app.security.queue.len(),
                3,
                "Hidden checks must be scanned too"
            );
            assert!(!app.detail_focus);
            app.security.busy = true;
            let generation = app.security.generation;
            handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 25, &tx);
            assert!(app.detail_focus);
            assert_eq!(app.security.generation, generation);
            handle_key(&mut app, KeyEvent::from(KeyCode::Tab), 25, &tx);
            assert!(!app.detail_focus);
            app.inventory_views
                .get_mut(&(0, OverviewMode::Images))
                .unwrap()
                .query
                .clear();
            let before = app.inventory_views[&(0, OverviewMode::Images)].index;
            handle_key(&mut app, KeyEvent::from(KeyCode::Down), 25, &tx);
            assert_ne!(
                app.inventory_views[&(0, OverviewMode::Images)].index,
                before
            );
            assert_eq!(app.security.generation, generation);
        }
    }

    #[test]
    fn scanning_without_checks_preserves_findings_focus() {
        use tha::app::OverviewMode;
        let mut app = app();
        app.demo = false;
        app.security.enabled = true;
        app.overview_mode = OverviewMode::Security;
        app.detail_focus = true;
        app.engines[0].snapshot.as_mut().unwrap().engine_id = Some("fixture-engine".into());
        let target = ui::inventory::security_targets(&app, false).remove(0);
        app.security.select(Some(target.clone()));
        app.request_security_scan(false);
        assert!(app.security.request);
        assert!(app.detail_focus);
        assert_eq!(app.security.target, Some(target));
        assert!(app.security.queue.is_empty());
    }

    #[test]
    fn container_security_tab_stays_in_place_and_cannot_start_or_cancel_scans() {
        let (tx, _) = watch::channel(0);
        let mut app = app();
        app.security.enabled = true;
        app.engines[0].snapshot.as_mut().unwrap().engine_id = Some("engine".into());
        for container in &mut app.engines[0].snapshot.as_mut().unwrap().containers {
            container.image_id = Some(format!("sha256:{}", container.id));
        }
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, tha::app::Scope::Container(_)))
            .unwrap();
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('8')), 20, &tx);
        assert_eq!(
            app.overview_mode,
            tha::app::OverviewMode::Containers,
            "No view change"
        );
        assert_eq!(app.tab, Tab::Security);
        assert!(app.detail_focus);
        assert!(
            app.security.target.is_some(),
            "Shows this container's image"
        );
        for key in ['s', 'S', 'x', 'm', 'M'] {
            handle_key(&mut app, KeyEvent::from(KeyCode::Char(key)), 20, &tx);
        }
        for command in [
            "scan-image",
            "scan-images",
            "update-security-db",
            "cancel-scan",
        ] {
            execute(&mut app, command, &tx);
            assert!(
                app.notice.contains("Security view"),
                "{command}: {}",
                app.notice
            );
        }
        assert!(!app.security.request);
        assert!(!app.security.update);
        assert!(!app.security.cancel);
        assert!(app.security.queue.is_empty());
        assert_eq!(app.overview_mode, tha::app::OverviewMode::Containers);
        // The tab follows the selection and leaves when no single image is selected.
        let image = app.security.target.clone();
        app.selected = app
            .rows
            .iter()
            .rposition(|row| matches!(row.scope, tha::app::Scope::Container(_)))
            .unwrap();
        app.reconcile_detail_tab();
        assert_eq!(app.tab, Tab::Security);
        assert_ne!(app.security.target, image);
        app.selected = 0;
        app.reconcile_detail_tab();
        assert_eq!(app.tab, Tab::Overview);
        // Scanning still works from the Security view's own keys.
        app.demo = false;
        assert!(tha::shortcuts::unavailable(&app, A::ScanImage).is_some());
        app.overview_mode = tha::app::OverviewMode::Security;
        assert!(tha::shortcuts::unavailable(&app, A::ScanImage).is_none());
    }

    #[test]
    fn security_zoom_and_advisory_details_are_navigation_only_and_escape_restores() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.tab = Tab::Security;
        app.detail_focus = true;
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        handle_key(&mut app, key(KeyCode::Char('z')), 20, &tx);
        assert!(app.security.zoom);
        handle_key(&mut app, key(KeyCode::Enter), 20, &tx);
        assert!(app.security.expanded);
        handle_key(&mut app, key(KeyCode::Esc), 20, &tx);
        assert!(!app.security.expanded);
        assert!(app.security.zoom);
        handle_key(&mut app, key(KeyCode::Esc), 20, &tx);
        assert!(!app.security.zoom);
        app.input_mode = InputMode::Command;
        handle_key(&mut app, key(KeyCode::Char('z')), 20, &tx);
        assert!(!app.security.zoom);
        assert_eq!(app.input, "z");
    }

    #[test]
    fn enabled_security_is_the_fourth_top_level_overview_and_is_hidden_without_the_module() {
        use tha::app::OverviewMode;
        let mut app = app();
        let (refresh, _) = watch::channel(0);
        app.security.enabled = false;
        app.overview_mode = OverviewMode::Volumes;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
            25,
            &refresh,
        );
        assert_eq!(app.overview_mode, OverviewMode::Containers);
        app.security.enabled = true;
        app.overview_mode = OverviewMode::Volumes;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
            25,
            &refresh,
        );
        assert_eq!(app.overview_mode, OverviewMode::Security);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            25,
            &refresh,
        );
        assert!(app.security.zoom);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
            25,
            &refresh,
        );
        assert_eq!(app.overview_mode, OverviewMode::Containers);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT),
            25,
            &refresh,
        );
        assert_eq!(app.overview_mode, OverviewMode::Security);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT),
            25,
            &refresh,
        );
        assert_eq!(app.overview_mode, OverviewMode::Security);
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
    fn performance_navigation_and_enter_use_only_connected_non_alias_runtimes() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.engines[2].alias = Some("OrbStack".into());
        app.context_order = vec![3, 1, 0, 2];
        app.performance_open = true;
        app.performance_focus = true;
        app.detail_focus = false;
        assert_eq!(app.performance_engines(), [1, 0]);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('G')), 20, &tx);
        assert_eq!(app.performance_engine(), Some(0));
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('k')), 20, &tx);
        assert_eq!(app.performance_engine(), Some(1));
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
        assert_eq!(app.current().unwrap().engine, 1);
        assert!(app.detail_focus);
        assert_eq!(app.tab, Tab::Overview);

        app.performance_focus = true;
        app.detail_focus = false;
        for engine in &mut app.engines {
            engine.error = Some("offline".into());
        }
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('G')), 20, &tx);
        assert_eq!(app.performance_scroll, 0);
        let selected = app.selected;
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
        assert_eq!(app.selected, selected);
        assert!(app.performance_focus);
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
        let expected = app.performance_engines()[0];
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
    fn inner_tabs_zoom_toggle_and_restore_without_losing_focus_or_search() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.detail_focus = true;
        app.tree_query = "preserve search".into();
        for tab in [
            Tab::Overview,
            Tab::Logs,
            Tab::Networks,
            Tab::Exec,
            Tab::Layers,
        ] {
            app.tab = tab;
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('z')), 20, &tx);
            assert_eq!(app.detail_zoom, Some(tab));
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('z')), 20, &tx);
            assert_eq!(app.detail_zoom, None);
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('z')), 20, &tx);
            handle_key(&mut app, KeyEvent::from(KeyCode::Esc), 20, &tx);
            assert_eq!(app.detail_zoom, None);
            assert!(app.detail_focus);
            assert_eq!(app.tree_query, "preserve search");
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('z')), 20, &tx);
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('q')), 20, &tx);
            assert_eq!(app.detail_zoom, None);
            assert!(app.detail_focus);
        }
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
            KeyCode::Char('z'),
            KeyCode::Char('4'),
            KeyCode::Esc,
            KeyCode::Char('q'),
            KeyCode::F(6),
        ] {
            assert!(!handle_key(
                &mut app,
                KeyEvent::new(code, KeyModifiers::NONE),
                20,
                &tx
            ));
            assert!(app.exec_input);
            assert_eq!(app.detail_zoom, None);
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
    fn lifecycle_background_work_keeps_navigation_and_retains_results() {
        use tha::actions::{Action, Status};
        let (tx, _) = watch::channel(0);
        let mut app = lifecycle_app("running");
        app.request_action(Action::Restart);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('n')), 20, &tx);
        assert!(app.action_confirmation.is_none());
        assert!(app.action_request.is_none());

        app.request_action(Action::Restart);
        app.action_review_visible = true;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('y')), 20, &tx);
        assert!(app.action_confirmation.is_none());
        assert!(!app.action_results_visible);
        assert!(app.action_status().unwrap().contains("0/1 completed"));
        app.action_request.take(); // Simulate worker startup.
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        terminal
            .draw(|frame| tha::ui::draw(frame, &mut app))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!rendered.contains("Restart · Running"));
        assert!(rendered.contains("0/1 completed"));
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('2')), 20, &tx);
        assert_eq!(app.tab, tha::app::Tab::Logs);
        app.request_action(Action::Stop);
        assert!(app.action_confirmation.is_none());
        assert!(app.notice.contains("already active"));

        execute(&mut app, "action-results", &tx);
        assert!(app.action_results_visible);
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc), 20, &tx);
        assert!(!app.action_results_visible);
        assert!(!app.action_progress().unwrap().finished);
        let progress = app.action_progress_mut().unwrap();
        progress.results[0] = Some(Status::Unknown("lost response".into()));
        progress.finished = true;
        assert!(app.action_status().unwrap().contains("1 unknown"));
        execute(&mut app, "action-results", &tx);
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
        assert!(!app.action_results_visible);
        assert!(matches!(
            app.action_progress().unwrap().results[0],
            Some(Status::Unknown(_))
        ));

        // Retained results must neither block a new confirmation nor be cleared by cancellation.
        app.request_action(Action::Restart);
        assert!(app.action_confirmation.is_some());
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('n')), 20, &tx);
        assert!(app.action_status().unwrap().contains("1 unknown"));
    }

    #[test]
    fn parent_lifecycle_shortcuts_confirm_only_the_selected_workloads() {
        use tha::{actions::Action, app::Scope};
        let (tx, _) = watch::channel(0);
        for (scope, expected) in [
            (
                Scope::Services {
                    namespace: None,
                    project: "fixture-project".into(),
                },
                vec!["fixture", "worker"],
            ),
            (Scope::Standalone(None), vec!["standalone"]),
        ] {
            for (key, state, action) in [
                ('S', "exited", Action::Start),
                ('s', "running", Action::Stop),
                ('r', "running", Action::Restart),
            ] {
                let mut app = lifecycle_app(state);
                let mut snapshot = app.engines[0].snapshot.clone().unwrap();
                let original = snapshot.containers[0].clone();
                for (id, project, service) in [
                    ("worker", Some("fixture-project"), Some("worker")),
                    ("other", Some("other-project"), Some("api")),
                    ("standalone", None, None),
                ] {
                    let mut member = original.clone();
                    member.id = id.into();
                    member.name = id.into();
                    member.project = project.map(Into::into);
                    member.service = service.map(Into::into);
                    snapshot.containers.push(member);
                }
                app.update_engine(0, Ok(snapshot));
                app.selected = app
                    .rows
                    .iter()
                    .position(|row| matches!(row.scope, Scope::Project { .. }))
                    .unwrap();
                app.expand();
                app.selected = app.rows.iter().position(|row| row.scope == scope).unwrap();
                let row_key = app.current().unwrap().key.clone();
                assert_eq!(
                    app.action_availability(action, &row_key).unwrap(),
                    expected.len()
                );
                assert!(
                    app.action_availability(Action::Delete, &row_key)
                        .unwrap_err()
                        .contains("Delete is disabled")
                );
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
                    20,
                    &tx,
                );
                let confirmation = app.action_confirmation.as_ref().unwrap();
                assert_eq!(confirmation.request.action, action);
                let ids = confirmation
                    .request
                    .containers
                    .iter()
                    .map(|container| container.id.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(ids.len(), expected.len());
                assert!(expected.iter().all(|id| ids.contains(id)));
                assert!(app.action_request.is_none());
                let name = confirmation.request.name.clone();
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
                terminal
                    .draw(|frame| tha::ui::draw(frame, &mut app))
                    .unwrap();
                assert!(app.action_review_visible);
                let rendered = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>();
                assert!(rendered.contains(&name));
                assert!(rendered.contains(&format!("{} target(s)", expected.len())));
                assert!(rendered.contains("n / Esc Cancel"));
                // A refresh and changed selection must not add targets to this review.
                let mut updated = app.engines[0].snapshot.clone().unwrap();
                let mut replica = original;
                replica.id = "new-replica".into();
                replica.name = "new-replica".into();
                updated.containers.push(replica);
                app.update_engine(0, Ok(updated));
                app.selected = 0;
                app.action_review_visible = true;
                handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
                    20,
                    &tx,
                );
                let request = app.action_request.as_ref().unwrap();
                assert_eq!(request.containers.len(), expected.len());
                assert!(
                    request
                        .containers
                        .iter()
                        .all(|container| container.id != "new-replica")
                );
            }
        }
    }

    #[test]
    fn lifecycle_shortcuts_require_confirmation_and_n_cancels() {
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
                    KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
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
        assert!(app.action_progress().is_some());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.action_confirmation.is_none());
    }

    #[test]
    fn enter_and_y_confirm_start_stop_restart_only_after_visible_review() {
        use tha::actions::Action;
        let (tx, _) = watch::channel(0);
        for (shortcut, state, action) in [
            ('S', "exited", Action::Start),
            ('s', "running", Action::Stop),
            ('r', "running", Action::Restart),
        ] {
            for confirm in [KeyCode::Enter, KeyCode::Char('y')] {
                let mut app = lifecycle_app(state);
                handle_key(&mut app, KeyEvent::from(KeyCode::Char(shortcut)), 20, &tx);
                handle_key(&mut app, KeyEvent::from(confirm), 20, &tx);
                assert!(app.action_request.is_none());
                assert!(app.action_confirmation.is_some());
                app.action_review_visible = true;
                for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
                    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, modifiers), 20, &tx);
                    assert!(app.action_request.is_none());
                }
                let mut repeat = KeyEvent::from(KeyCode::Enter);
                repeat.kind = KeyEventKind::Repeat;
                handle_key(&mut app, repeat, 20, &tx);
                assert!(app.action_request.is_none());
                handle_key(&mut app, KeyEvent::from(confirm), 20, &tx);
                assert_eq!(app.action_request.as_ref().unwrap().action, action);
                assert!(app.action_confirmation.is_none());
                assert!(!app.action_results_visible);
            }
            for cancel in [KeyCode::Char('n'), KeyCode::Esc] {
                let mut app = lifecycle_app(state);
                handle_key(&mut app, KeyEvent::from(KeyCode::Char(shortcut)), 20, &tx);
                handle_key(&mut app, KeyEvent::from(cancel), 20, &tx);
                assert!(app.action_confirmation.is_none());
                assert!(app.action_request.is_none());
            }
        }
        let mut app = lifecycle_app("running");
        app.selected = 0;
        app.engines[0].runtime = Some(tha::runtime::Target::Colima {
            profile: "fixture".into(),
        });
        app.engines[0].error = Some("offline".into());
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('S')), 20, &tx);
        app.action_review_visible = true;
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
        assert!(app.action_request.as_ref().unwrap().runtime.is_some());
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
            "details", "shell", "search", "control", "repeat", "demo", "offline", "alias",
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
                "control" => key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                "repeat" => key.kind = KeyEventKind::Repeat,
                "demo" => app.demo = true,
                "offline" => app.engines[0].error = Some("offline".into()),
                _ => app.engines[0].alias = Some("primary".into()),
            }
            handle_key(&mut app, key, 20, &tx);
            assert!(app.action_confirmation.is_none(), "{mode}");
            assert!(app.action_request.is_none());
        }
    }

    #[test]
    fn performance_actions_target_the_displayed_runtime_or_container_and_keep_confirmations() {
        use tha::{
            actions::Action,
            app::{PerformanceScope, Scope},
            runtime::Target,
        };
        let (tx, _) = watch::channel(0);
        for (key, state, action) in [
            ('S', "exited", Action::Start),
            ('s', "running", Action::Stop),
            ('r', "running", Action::Restart),
        ] {
            for scope in [PerformanceScope::Runtime, PerformanceScope::Container] {
                let mut app = lifecycle_app(state);
                app.engines[0].runtime = Some(Target::Colima {
                    profile: "fixture".into(),
                });
                app.tree_query = "fixture-api".into();
                app.refresh_tree();
                let container_index = app
                    .rows
                    .iter()
                    .position(|row| matches!(row.scope, Scope::Container(_)))
                    .unwrap();
                app.selected = if scope == PerformanceScope::Container {
                    container_index
                } else {
                    1
                };
                app.performance_scope = scope;
                app.performance_open = true;
                app.performance_focus = true;
                app.performance_expanded = true;
                handle_key(&mut app, KeyEvent::from(KeyCode::Char(key)), 20, &tx);
                let request = &app.action_confirmation.as_ref().unwrap().request;
                assert_eq!(request.action, action);
                assert_eq!(
                    request.runtime.is_some(),
                    scope == PerformanceScope::Runtime
                );
                if scope == PerformanceScope::Container {
                    assert_eq!(request.containers[0].id, "fixture");
                    assert_eq!(request.containers.len(), 1);
                }
                assert!(
                    app.action_request.is_none(),
                    "Opening the panel never bypasses confirmation"
                );
                handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
                assert!(
                    app.action_request.is_none(),
                    "Review must actually be rendered"
                );
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
                terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
                assert!(app.action_review_visible);
                handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
                assert_eq!(app.action_request.as_ref().unwrap().action, action);
                assert!(app.performance_focus && app.performance_expanded);
                handle_key(&mut app, KeyEvent::from(KeyCode::Char(key)), 20, &tx);
                assert!(
                    app.action_confirmation.is_none(),
                    "Conflicting work is still blocked"
                );
            }
        }
        // A focused container view must never turn a group selection into a bulk action.
        let mut app = lifecycle_app("running");
        app.performance_scope = PerformanceScope::Container;
        app.performance_focus = true;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('r')), 20, &tx);
        assert!(app.action_confirmation.is_none());
        assert!(app.action_request.is_none());
        assert!(app.notice.contains("Select a target"));
    }

    #[test]
    fn performance_container_actions_keep_delete_and_input_safeguards() {
        use tha::app::{PerformanceScope, Scope};
        let (tx, _) = watch::channel(0);
        for mode in [
            "delete", "offline", "alias", "demo", "control", "repeat", "search",
        ] {
            let mut app = lifecycle_app("exited");
            app.tree_query = "fixture-api".into();
            app.refresh_tree();
            app.selected = app
                .rows
                .iter()
                .position(|row| matches!(row.scope, Scope::Container(_)))
                .unwrap();
            app.performance_open = true;
            app.performance_focus = true;
            app.performance_scope = PerformanceScope::Container;
            let mut key = KeyEvent::from(KeyCode::Char('d'));
            match mode {
                "offline" => app.engines[0].error = Some("offline".into()),
                "alias" => app.engines[0].alias = Some("primary".into()),
                "demo" => app.demo = true,
                "control" => key.modifiers = KeyModifiers::CONTROL,
                "repeat" => key.kind = KeyEventKind::Repeat,
                "search" => app.input_mode = InputMode::Search,
                _ => {}
            }
            handle_key(&mut app, key, 20, &tx);
            assert!(app.action_request.is_none(), "{mode}");
            if mode == "delete" {
                assert!(app.action_confirmation.is_some());
                app.action_review_visible = true;
                handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
                assert!(app.action_request.is_none());
                let confirmation = app.action_confirmation.as_mut().unwrap();
                confirmation.typed = confirmation.request.name.clone();
                handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
                assert_eq!(
                    app.action_request.as_ref().unwrap().containers[0].id,
                    "fixture"
                );
            } else {
                assert!(app.action_confirmation.is_none(), "{mode}");
            }
        }
    }

    #[test]
    fn runtime_performance_actions_do_not_follow_an_unrelated_explorer_selection() {
        use tha::{actions::Action, app::PerformanceScope, runtime::Target};
        let (tx, _) = watch::channel(0);
        let mut app = lifecycle_app("running");
        app.engines[0].runtime = Some(Target::Colima {
            profile: "fixture".into(),
        });
        let mut other = tha::app::Engine {
            connection: app.engines[0].connection.clone(),
            runtime: None,
            snapshot: app.engines[0].snapshot.clone(),
            error: None,
            alias: None,
            metrics: Default::default(),
            metrics_revision: 0,
            events: Default::default(),
        };
        other.connection.name = "other runtime".into();
        other.connection.endpoint = "unix:///other-fixture".into();
        other.runtime = Some(Target::Colima {
            profile: "other".into(),
        });
        other.snapshot.as_mut().unwrap().engine_id = Some("other-engine".into());
        app.engines.push(other);
        app.context_order.push(1);
        app.refresh_tree();
        app.performance_scope = PerformanceScope::Runtime;
        app.performance_open = true;
        app.performance_focus = true;
        app.performance_scroll = 1;
        assert_eq!(app.current().unwrap().engine, 0);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('r')), 20, &tx);
        let request = &app.action_confirmation.as_ref().unwrap().request;
        assert_eq!(request.engine, 1);
        assert_eq!(request.runtime.as_ref(), app.engines[1].runtime.as_ref());
        assert_eq!(request.action, Action::Restart);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('n')), 20, &tx);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('a')), 20, &tx);
        assert!(app.picker.as_ref().unwrap().title.contains("other runtime"));
        app.choose();
        assert_eq!(app.action_confirmation.as_ref().unwrap().request.engine, 1);
    }

    #[test]
    fn focused_container_navigation_skips_groups_and_inspects_the_shown_container() {
        use tha::app::{PerformanceScope, Scope};
        let (tx, _) = watch::channel(0);
        let mut app = app();
        app.tree_query = "-".into();
        app.refresh_tree();
        app.performance_open = true;
        app.performance_focus = true;
        app.performance_scope = PerformanceScope::Container;
        let indices: Vec<_> = app
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                matches!(row.scope, Scope::Container(_)) && app.engines[row.engine].alias.is_none()
            })
            .map(|(index, _)| index)
            .collect();
        assert!(indices.len() >= 3);
        for expanded in [false, true] {
            app.performance_expanded = expanded;
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('g')), 20, &tx);
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('g')), 20, &tx);
            assert_eq!(app.selected, indices[0]);
            handle_key(&mut app, KeyEvent::from(KeyCode::Down), 20, &tx);
            assert_eq!(app.selected, indices[1]);
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('j')), 20, &tx);
            assert_eq!(app.selected, indices[2]);
            handle_key(&mut app, KeyEvent::from(KeyCode::Up), 20, &tx);
            assert_eq!(app.selected, indices[1]);
            handle_key(&mut app, KeyEvent::from(KeyCode::PageDown), 2, &tx);
            assert_eq!(app.selected, indices[3.min(indices.len() - 1)]);
            handle_key(&mut app, KeyEvent::from(KeyCode::PageUp), 2, &tx);
            assert_eq!(app.selected, indices[1]);
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('G')), 20, &tx);
            assert_eq!(app.selected, *indices.last().unwrap());
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('j')), 20, &tx);
            assert_eq!(app.selected, *indices.last().unwrap());
        }
        let key = app.current().unwrap().key.clone();
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
        assert!(app.detail_focus && !app.performance_focus);
        assert_eq!(app.current().unwrap().key, key);
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

    #[tokio::test]
    async fn performance_scope_zoom_escape_and_container_inspection_are_navigation_only() {
        use tha::app::PerformanceScope;
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, tha::app::Scope::Container(_)))
            .unwrap();
        let selected = app.current().unwrap().key.clone();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT),
            20,
            &tx,
        );
        assert_eq!(app.performance_scope, PerformanceScope::Container);
        assert!(app.performance_open && app.performance_focus);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('z')), 20, &tx);
        assert!(app.performance_expanded);
        app.tree_query = "preserve this search".into();
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc), 20, &tx);
        assert!(!app.performance_expanded);
        assert!(app.performance_focus && app.performance_open);
        assert_eq!(app.tree_query, "preserve this search");
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 20, &tx);
        assert!(app.detail_focus && !app.performance_focus);
        assert_eq!(app.current().unwrap().key, selected);
        assert_eq!(app.tab, Tab::Overview);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('z')), 20, &tx);
        assert!(!app.performance_expanded, "Zoom is Performance-focus-only");
        app.input_mode = InputMode::Search;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('P')), 20, &tx);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('z')), 20, &tx);
        assert_eq!(app.input, "Pz");
        assert_eq!(app.performance_scope, PerformanceScope::Container);
        app.input_mode = InputMode::Normal;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('P')), 20, &tx);
        assert_eq!(app.performance_scope, PerformanceScope::Runtime);
        assert!(app.action_request.is_none());
    }

    #[tokio::test]
    async fn prune_shortcut_and_command_require_confirmation_and_allow_cancellation() {
        use tha::app::OverviewMode;
        let mut app = app();
        let (refresh, _) = watch::channel(0);
        app.demo = false;
        switch_overview(&mut app, OverviewMode::Volumes);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('p')), 25, &refresh);
        assert!(app.inventory_removal.is_some());
        assert!(app.inventory_removal_request.is_none());
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 25, &refresh);
        assert!(app.inventory_removal_request.is_none());
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc), 25, &refresh);
        assert!(app.inventory_removal.is_none());

        execute(&mut app, "prune", &refresh);
        app.inventory_removal.as_mut().unwrap().visible = true;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('y')), 25, &refresh);
        assert_eq!(app.inventory_removal_request.as_ref().unwrap().len(), 1);
        assert!(app.inventory_removal.as_ref().unwrap().pending);
    }

    #[tokio::test]
    async fn unused_resource_removal_requires_visible_yes_no_confirmation_and_protects_used_volumes()
     {
        use tha::app::OverviewMode;
        let mut app = app();
        let (refresh, _) = watch::channel(0);
        app.demo = false;
        switch_overview(&mut app, OverviewMode::Volumes);
        ui::inventory::navigate(&mut app, KeyCode::Home, 25);
        handle_key(&mut app, KeyEvent::from(KeyCode::Delete), 25, &refresh);
        assert!(
            app.inventory_removal.is_none(),
            "Used volumes must be refused"
        );
        ui::inventory::navigate(&mut app, KeyCode::End, 25);
        handle_key(&mut app, KeyEvent::from(KeyCode::Delete), 25, &refresh);
        assert_eq!(
            app.inventory_removal.as_ref().unwrap().targets[0]
                .request
                .key,
            "old_cache"
        );
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 25, &refresh);
        assert!(
            app.inventory_removal_request.is_none(),
            "An unrendered or tiny confirmation cannot confirm"
        );
        app.inventory_removal.as_mut().unwrap().visible = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
            25,
            &refresh,
        );
        assert_eq!(
            app.overview_mode,
            OverviewMode::Volumes,
            "Dialogs retain keyboard ownership"
        );
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('n')), 25, &refresh);
        assert!(app.inventory_removal.is_none());
        handle_key(&mut app, KeyEvent::from(KeyCode::Delete), 25, &refresh);
        app.inventory_removal.as_mut().unwrap().visible = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            25,
            &refresh,
        );
        assert!(app.inventory_removal_request.is_none());
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('y')), 25, &refresh);
        assert_eq!(
            app.inventory_removal_request.as_ref().unwrap()[0].key,
            "old_cache"
        );
        assert!(app.inventory_removal.as_ref().unwrap().pending);
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc), 25, &refresh);
        assert!(app.inventory_removal.as_ref().unwrap().dismissed);
    }

    #[tokio::test]
    async fn inventory_table_navigation_search_details_and_overview_cycle_are_isolated() {
        use tha::app::OverviewMode;
        let mut app = app();
        let (refresh, _) = watch::channel(0);
        let selected = app.selected;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT),
            25,
            &refresh,
        );
        assert_eq!(app.overview_mode, OverviewMode::Volumes);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
            25,
            &refresh,
        );
        assert_eq!(app.overview_mode, OverviewMode::Containers);
        handle_key(&mut app, KeyEvent::from(KeyCode::F(8)), 25, &refresh);
        assert_eq!(app.overview_mode, OverviewMode::Images);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('G')), 25, &refresh);
        let view_key = (app.inventory_engine, app.overview_mode);
        assert_eq!(app.inventory_views[&view_key].index, 2);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('g')), 25, &refresh);
        assert_eq!(app.inventory_views[&view_key].index, 2);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('g')), 25, &refresh);
        assert_eq!(app.inventory_views[&view_key].index, 0);
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 25, &refresh);
        assert!(app.inventory_views[&view_key].details);
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc), 25, &refresh);
        assert!(!app.inventory_views[&view_key].details);
        assert_eq!(app.overview_mode, OverviewMode::Images);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')), 25, &refresh);
        for c in "redis".chars() {
            handle_key(&mut app, KeyEvent::from(KeyCode::Char(c)), 25, &refresh);
        }
        handle_key(&mut app, KeyEvent::from(KeyCode::F(8)), 25, &refresh);
        assert_eq!(
            app.overview_mode,
            OverviewMode::Images,
            "Function keys must not switch while editing"
        );
        assert_eq!(app.input_mode, InputMode::InventorySearch);
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), 25, &refresh);
        assert_eq!(app.inventory_views[&view_key].query, "redis");
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('/')), 25, &refresh);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            25,
            &refresh,
        );
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc), 25, &refresh);
        assert_eq!(
            app.inventory_views[&view_key].query, "redis",
            "Cancel must restore the applied query"
        );
        handle_key(&mut app, KeyEvent::from(KeyCode::F(8)), 25, &refresh);
        assert_eq!(app.overview_mode, OverviewMode::Volumes);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('f')), 25, &refresh);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('s')), 25, &refresh);
        handle_key(&mut app, KeyEvent::from(KeyCode::F(7)), 25, &refresh);
        assert_eq!(app.inventory_views[&view_key].query, "redis");
        handle_key(&mut app, KeyEvent::from(KeyCode::F(7)), 25, &refresh);
        assert_eq!(app.overview_mode, OverviewMode::Containers);
        assert_eq!(app.selected, selected);
        assert!(app.action_request.is_none());
        handle_key(&mut app, KeyEvent::from(KeyCode::F(7)), 25, &refresh);
        assert_eq!(app.overview_mode, OverviewMode::Volumes);
        assert_eq!(app.inventory_views[&(0, OverviewMode::Volumes)].filter, 1);
    }

    #[tokio::test]
    async fn inventory_shortcuts_and_commands_switch_read_only_runtime_views() {
        use tha::app::OverviewMode;
        let mut app = app();
        let (refresh, _) = watch::channel(0);
        let selected = app.selected;
        let tab = app.tab;
        app.detail_scroll = 99;
        for (key, command, mode) in [
            ('2', "images", OverviewMode::Images),
            ('3', "volumes", OverviewMode::Volumes),
        ] {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(key), KeyModifiers::ALT),
                20,
                &refresh,
            );
            assert_eq!(app.overview_mode, mode);
            assert_eq!(app.tab, tab);
            assert_eq!(app.detail_scroll, 99);
            handle_key(&mut app, KeyEvent::from(KeyCode::Down), 20, &refresh);
            assert_eq!(app.selected, selected);
            assert_eq!(app.inventory_views[&(app.inventory_engine, mode)].index, 1);
            handle_key(&mut app, KeyEvent::from(KeyCode::Right), 20, &refresh);
            assert_eq!(app.inventory_engine, 1);
            assert!(
                !app.inventory_views
                    .contains_key(&(app.inventory_engine, mode))
            );
            handle_key(&mut app, KeyEvent::from(KeyCode::Char('a')), 20, &refresh);
            assert!(
                app.picker.is_none(),
                "Inventory navigation must not open hidden workload actions"
            );
            assert!(app.action_request.is_none());
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT),
                20,
                &refresh,
            );
            assert_eq!(app.overview_mode, OverviewMode::Containers);
            execute(&mut app, command, &refresh);
            assert_eq!(app.overview_mode, mode);
            app.input_mode = InputMode::Search;
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(key), KeyModifiers::ALT),
                20,
                &refresh,
            );
            assert_eq!(app.input, key.to_string());
            assert_eq!(app.overview_mode, mode);
            app.input.clear();
            app.input_mode = InputMode::Normal;
            handle_key(&mut app, KeyEvent::from(KeyCode::Esc), 20, &refresh);
            assert_eq!(app.overview_mode, OverviewMode::Containers);
            assert_eq!(app.selected, selected);
            assert_eq!(app.detail_scroll, 99);
        }
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('8')), 20, &refresh);
        assert_eq!(app.overview_mode, OverviewMode::Containers);
        assert_eq!(app.tab, tab, "Security is hidden without its module");
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('5')), 20, &refresh);
        assert_eq!(app.overview_mode, OverviewMode::Containers);
        assert_eq!(app.tab, Tab::Events);
        execute(&mut app, "images", &refresh);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('3')), 20, &refresh);
        assert_eq!(app.overview_mode, OverviewMode::Containers);
        assert_eq!(app.tab, Tab::Networks);
    }

    #[tokio::test]
    async fn unsupported_layers_shortcuts_are_blocked_and_pending_lookup_is_cancelled() {
        let mut app = app();
        let (refresh, _) = watch::channel(0);
        app.selected = 0;
        assert!(!app.layers_available());
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('7')), 20, &refresh);
        assert_eq!(app.tab, Tab::Overview);
        execute(&mut app, "layers", &refresh);
        assert_eq!(app.tab, Tab::Overview);
        let pending = tokio::spawn(std::future::pending::<()>());
        let abort = pending.abort_handle();
        let mut task = Some(AbortTask(pending));
        let (sender, _) = mpsc::channel(4);
        app.tab = Tab::Layers;
        update_layers(&mut app, &mut task, &sender);
        tokio::task::yield_now().await;
        assert!(abort.is_finished());
        assert!(task.is_none());
        assert_eq!(app.tab, Tab::Overview);
    }

    #[tokio::test]
    async fn number_six_loads_demo_layers_and_enter_and_escape_toggle_read_only_details() {
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, tha::app::Scope::Container(_)))
            .unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.tab, Tab::Layers);
        assert!(app.detail_focus);
        let (layers_tx, _) = mpsc::channel(4);
        let mut task = None;
        update_layers(&mut app, &mut task, &layers_tx);
        assert!(matches!(app.layers.state, tha::layers::State::Ready(_)));
        assert!(task.is_none());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.layers.selected > 0);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.layers.expanded);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(!app.layers.expanded);
        assert!(app.detail_focus);
        assert!(app.action_request.is_none());
    }

    #[test]
    fn action_menu_reuses_safety_checks_and_keeps_disabled_actions_open() {
        let mut app = lifecycle_app("running");
        let (tx, _) = watch::channel(0);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.picker.is_some());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        ); // Start is disabled.
        assert!(app.picker.is_some());
        assert!(app.action_request.is_none());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            20,
            &tx,
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.picker.is_none());
        assert!(app.action_confirmation.is_some());
        assert!(app.action_request.is_none());
    }

    #[test]
    fn configurable_zoom_views_metrics_and_command_actions_use_normal_routing() {
        use tha::app::OverviewMode as O;
        let mut app = app();
        let (tx, _) = watch::channel(0);
        app.keymap = tha::keymap::Keymap::parse(&std::collections::BTreeMap::from([
            ("x".into(), "zoom".into()),
            ("z".into(), "none".into()),
            ("alt-left".into(), "previous-view".into()),
            ("alt-right".into(), "next-view".into()),
            ("alt-n".into(), "focus-next".into()),
            ("alt-m".into(), "monochrome".into()),
            ("alt-r".into(), "next-range".into()),
        ]))
        .unwrap();
        app.detail_focus = true;
        app.tab = Tab::Logs;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('z')), 20, &tx);
        assert_eq!(app.detail_zoom, None);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('x')), 20, &tx);
        assert_eq!(app.detail_zoom, Some(Tab::Logs));
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('x')), 20, &tx);
        assert_eq!(app.detail_zoom, None);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL),
            20,
            &tx,
        );
        assert_eq!(app.overview_mode, O::Images);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::ALT),
            20,
            &tx,
        );
        assert_eq!(app.overview_mode, O::Volumes);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
            20,
            &tx,
        );
        assert_eq!(app.overview_mode, O::Images);
        let colors = app.monochrome;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT),
            20,
            &tx,
        );
        assert_eq!(app.monochrome, !colors);
        app.overview_mode = O::Security;
        app.security.enabled = true;
        app.detail_focus = false;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT),
            20,
            &tx,
        );
        assert!(app.detail_focus);
        app.overview_mode = O::Containers;
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, tha::app::Scope::Container(_)))
            .unwrap();
        app.tab = Tab::History;
        let range = app.history_view.range;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
            20,
            &tx,
        );
        assert_eq!(app.history_view.range, range + 1);
        app.input_mode = InputMode::Search;
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('x')), 20, &tx);
        assert_eq!(app.input, "x");
        assert!(!app.history_view.expanded);
    }

    #[test]
    fn rebound_lifecycle_keys_still_require_visible_confirmation_and_normal_scope() {
        let mut app = lifecycle_app("running");
        let (tx, _) = watch::channel(0);
        app.keymap = tha::keymap::Keymap::parse(&std::collections::BTreeMap::from([(
            "alt-s".into(),
            "stop".into(),
        )]))
        .unwrap();
        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        handle_key(
            &mut app,
            KeyEvent {
                kind: KeyEventKind::Repeat,
                ..key
            },
            20,
            &tx,
        );
        assert!(app.action_confirmation.is_none());
        handle_key(&mut app, key, 20, &tx);
        assert!(app.action_confirmation.is_some());
        assert!(app.action_request.is_none());
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('n')), 20, &tx);
        assert!(app.action_confirmation.is_none());
        app.detail_focus = true;
        app.tab = Tab::Logs;
        handle_key(&mut app, key, 20, &tx);
        assert!(app.action_confirmation.is_none());
        assert!(app.action_request.is_none());
    }

    #[test]
    fn custom_keys_are_navigation_only_and_never_change_editor_or_confirmation_input() {
        let mut app = lifecycle_app("running");
        let (tx, _) = watch::channel(0);
        app.keymap = tha::keymap::Keymap::parse(&std::collections::BTreeMap::from([
            ("x".into(), "logs".into()),
            ("y".into(), "layers".into()),
        ]))
        .unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.tab, Tab::Logs);
        app.input_mode = InputMode::Search;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.input, "x");
        app.input_mode = InputMode::Normal;
        app.tab = Tab::Overview;
        app.request_action(tha::actions::Action::Stop);
        app.action_review_visible = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.tab, Tab::Overview);
        assert!(app.action_request.is_some());
    }

    #[test]
    fn log_filter_editor_validates_without_destroying_retained_logs_and_export_pins_scope() {
        let (tx, _) = watch::channel(0);
        let mut app = lifecycle_app("running");
        app.tab = Tab::Logs;
        app.detail_focus = true;
        app.logs.push("api", "INFO before", false);
        app.logs.push("api", "ERROR failed", false);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.input_mode, InputMode::LogFilter);
        app.input = "invalid".into();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.input_mode, InputMode::LogFilter);
        assert!(!app.log_view.filter.active());
        app.input = r#"{"level":"error"}"#.into();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.log_view.filter.active());
        assert_eq!(app.logs.entries.len(), 2);
        let row = app.current().unwrap();
        app.log_scope = Some(log_scope_key(
            row,
            &app.engines[row.engine],
            &app.selected_containers(),
            app.log_since,
        ));
        execute(&mut app, "export-logs", &tx);
        assert_eq!(app.input_mode, InputMode::ExportLogs);
        app.input = "/never-written-by-this-test.jsonl".into();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.log_export_request.is_none());
        app.log_export_review_visible = true;
        app.selected = 0;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert!(app.log_export_request.is_none());
        assert!(app.notice.contains("Log target changed"));
        app.selected = 1;
        app.log_export_review_visible = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            20,
            &tx,
        );
        assert_eq!(app.log_export_request.as_ref().unwrap().entries.len(), 1);
        assert!(app.log_export_busy);
    }
}
