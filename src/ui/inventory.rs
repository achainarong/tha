//! Selectable runtime inventories with guarded removal. Sizes are not reclaimable space.
use super::*;
use crate::{
    app::InventoryView,
    model::{Container, Image, Snapshot},
};
use crossterm::event::KeyCode;
use ratatui::widgets::TableState;

pub const IMAGE_FILTERS: &[&str] = &[
    "All",
    "Running",
    "Stopped-only",
    "Unreferenced",
    "Untagged",
    "Unknown",
];
pub const VOLUME_FILTERS: &[&str] = &[
    "Managed",
    "Running",
    "Stopped-only",
    "Unreferenced",
    "Unknown",
    "Bind mounts",
];
pub const SORTS: &[&str] = &["Usage ↓", "Name ↑", "Size ↓"];
const MAX_NAME_CELLS: usize = 82;

#[derive(Debug)]
struct Entry {
    key: String,
    name: String,
    tags: String,
    size: Option<u64>,
    status: &'static str,
    running: usize,
    stopped: usize,
    project: String,
    extra: String,
    containers: Vec<String>,
    untagged: bool,
}

fn references<'a>(image: &Image, containers: &'a [Container]) -> Vec<&'a Container> {
    containers
        .iter()
        .filter(
            |c| match c.image_id.as_deref().filter(|id| !id.is_empty()) {
                Some(id) => id == image.id,
                None => image.tags.contains(&c.image),
            },
        )
        .collect()
}

fn usage(containers: &[&Container], complete: bool) -> (usize, usize, &'static str) {
    let running = containers.iter().filter(|c| c.running()).count();
    let stopped = containers.len() - running;
    (
        running,
        stopped,
        if running > 0 {
            "running"
        } else if stopped > 0 {
            "stopped-only"
        } else if complete {
            "unreferenced"
        } else {
            "unknown"
        },
    )
}

fn filters(mode: OverviewMode) -> &'static [&'static str] {
    if mode == OverviewMode::Images {
        IMAGE_FILTERS
    } else {
        VOLUME_FILTERS
    }
}

fn image_name(tag: &str) -> (&str, &str) {
    // Registry ports are not tag separators. Digest references are kept intact.
    if tag.contains('@') {
        return (tag, "digest");
    }
    match tag.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => (name, tag),
        _ => (tag, crate::i18n::t("latest")),
    }
}

fn entries(snapshot: &Snapshot, mode: OverviewMode, binds: bool) -> Vec<Entry> {
    if mode == OverviewMode::Images {
        let complete = snapshot
            .containers
            .iter()
            .all(|c| c.image_id.as_ref().is_some_and(|id| !id.is_empty()));
        return snapshot
            .images
            .iter()
            .flatten()
            .map(|image| {
                let refs = references(image, &snapshot.containers);
                let (running, stopped, status) = usage(&refs, complete);
                let mut names = std::collections::BTreeSet::new();
                let mut tags = std::collections::BTreeSet::new();
                for tag in &image.tags {
                    let (name, tag) = image_name(tag);
                    names.insert(safe_text(name));
                    tags.insert(safe_text(tag));
                }
                Entry {
                    key: image.id.clone(),
                    name: if names.is_empty() {
                        "<untagged>".into()
                    } else {
                        names.into_iter().collect::<Vec<_>>().join(", ")
                    },
                    tags: tags.into_iter().collect::<Vec<_>>().join(", "),
                    size: image.size,
                    status,
                    running,
                    stopped,
                    project: String::new(),
                    extra: crate::tf!(
                        "Tags: {a0}",
                        a0 = image
                            .tags
                            .iter()
                            .map(|tag| safe_text(tag))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    containers: refs
                        .iter()
                        .map(|c| format!("{} ({})", safe_text(&c.name), safe_text(&c.state)))
                        .collect(),
                    untagged: image.tags.is_empty(),
                }
            })
            .collect();
    }
    if binds {
        let mut mounts = std::collections::BTreeMap::<&str, Vec<&Container>>::new();
        for container in &snapshot.containers {
            for mount in &container.bind_mounts {
                let refs = mounts.entry(mount).or_default();
                if !refs.iter().any(|c| c.key() == container.key()) {
                    refs.push(container);
                }
            }
        }
        return mounts
            .into_iter()
            .map(|(mount, refs)| {
                let (running, stopped, status) = usage(&refs, true);
                Entry {
                    key: mount.into(),
                    name: safe_text(mount),
                    tags: "bind".into(),
                    size: None,
                    status,
                    running,
                    stopped,
                    project: String::new(),
                    extra: crate::i18n::t(
                        "Host path → container destination; not a managed volume",
                    )
                    .into(),
                    containers: refs
                        .iter()
                        .map(|c| format!("{} ({})", safe_text(&c.name), safe_text(&c.state)))
                        .collect(),
                    untagged: false,
                }
            })
            .collect();
    }
    snapshot
        .volumes
        .iter()
        .map(|volume| {
            let refs: Vec<_> = snapshot
                .containers
                .iter()
                .filter(|c| c.volumes.contains(&volume.name))
                .collect();
            let (running, stopped, status) = usage(&refs, true);
            let driver = volume
                .details
                .strip_prefix("driver: ")
                .unwrap_or("unknown")
                .split(" · ")
                .next()
                .unwrap_or("unknown");
            Entry {
                key: volume.name.clone(),
                name: safe_text(&volume.name),
                tags: safe_text(driver),
                size: None,
                status,
                running,
                stopped,
                project: volume
                    .project
                    .as_deref()
                    .map(safe_text)
                    .unwrap_or_else(|| "—".into()),
                extra: safe_text(&volume.details),
                containers: refs
                    .iter()
                    .map(|c| format!("{} ({})", safe_text(&c.name), safe_text(&c.state)))
                    .collect(),
                untagged: false,
            }
        })
        .collect()
}

fn update_available(
    updates: &std::collections::HashMap<(usize, String), crate::images::UpdateStatus>,
    engine: usize,
    image: &str,
) -> bool {
    updates
        .get(&(engine, image.to_owned()))
        .is_some_and(|status| matches!(status, crate::images::UpdateStatus::Available(_)))
}

fn rank(entry: &Entry) -> usize {
    match entry.status {
        "running" => 0,
        "stopped-only" => 1,
        "unreferenced" => 2,
        _ => 3,
    }
}

fn visible(
    mut rows: Vec<Entry>,
    view: &InventoryView,
    mode: OverviewMode,
    query: &str,
) -> Vec<Entry> {
    let query = query.to_lowercase();
    rows.retain(|row| {
        let filter = match view.filter {
            1 => row.status == "running",
            2 => row.status == "stopped-only",
            3 => row.status == "unreferenced",
            4 if mode == OverviewMode::Images => row.untagged,
            4 => row.status == "unknown",
            5 if mode == OverviewMode::Images => row.status == "unknown",
            _ => true,
        };
        filter
            && (query.is_empty()
                || format!(
                    "{} {} {} {} {} {}",
                    row.name,
                    row.tags,
                    row.key,
                    row.project,
                    row.extra,
                    row.containers.join(" ")
                )
                .to_lowercase()
                .contains(&query))
    });
    rows.sort_by(|a, b| {
        let order = match view.sort {
            1 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            2 => b.size.cmp(&a.size),
            _ => rank(a).cmp(&rank(b)),
        };
        order
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.key.cmp(&b.key))
    });
    rows
}

fn reconcile(view: &mut InventoryView, rows: &[Entry]) {
    view.index = view
        .selected
        .as_ref()
        .and_then(|key| rows.iter().position(|row| &row.key == key))
        .unwrap_or(view.index)
        .min(rows.len().saturating_sub(1));
    view.selected = rows.get(view.index).map(|row| row.key.clone());
    if rows.is_empty() {
        view.details = false;
        view.offset = 0;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CleanupSelection {
    Selected,
    AllUnreferenced,
}

/// Navigation is called only after editors, dialogs and embedded shells consume keys.
pub fn begin_removal(app: &mut App) {
    begin_cleanup(app, CleanupSelection::Selected);
}

/// Freeze the entire runtime's unused inventory, independently of search and marks.
pub fn begin_prune(app: &mut App) {
    begin_cleanup(app, CleanupSelection::AllUnreferenced);
}

fn begin_cleanup(app: &mut App, selection: CleanupSelection) {
    if !matches!(
        app.overview_mode,
        OverviewMode::Images | OverviewMode::Volumes
    ) {
        app.notice =
            crate::i18n::t("Open Images or Volumes to prune unreferenced resources").into();
        return;
    }
    if app.inventory_removal.is_some() {
        app.notice =
            crate::i18n::t("Resource cleanup is already awaiting confirmation or running").into();
        return;
    }
    if app.demo {
        app.notice = crate::i18n::t("Demo mode: resource deletion is disabled").into();
        return;
    }
    let key = (app.inventory_engine, app.overview_mode);
    let view = app.inventory_views.get(&key).cloned().unwrap_or_default();
    if selection == CleanupSelection::Selected && key.1 == OverviewMode::Volumes && view.filter == 5
    {
        app.notice = crate::i18n::t("Bind mounts cannot be deleted from Tha").into();
        return;
    }
    let Some(engine) = app
        .engines
        .get(key.0)
        .filter(|engine| engine.error.is_none() && engine.alias.is_none())
    else {
        app.notice = crate::i18n::t("Select a connected primary runtime").into();
        return;
    };
    let Some(snapshot) = &engine.snapshot else {
        return;
    };
    if engine.connection.kind != crate::config::BackendKind::Docker {
        app.notice = crate::i18n::t("Resource removal is unsupported for this backend").into();
        return;
    }
    let all = entries(snapshot, key.1, false);
    let rows = if selection == CleanupSelection::AllUnreferenced {
        all
    } else {
        visible(all, &view, key.1, &view.query)
    };
    let index = view
        .selected
        .as_ref()
        .and_then(|key| rows.iter().position(|row| &row.key == key))
        .unwrap_or(view.index);
    let chosen: Vec<_> = if selection == CleanupSelection::AllUnreferenced {
        rows.iter()
            .filter(|row| row.status == "unreferenced")
            .collect()
    } else if view.marked.is_empty() {
        rows.get(index)
            .into_iter()
            .filter(|row| row.status == "unreferenced")
            .collect()
    } else {
        rows.iter()
            .filter(|row| view.marked.contains(&row.key) && row.status == "unreferenced")
            .collect()
    };
    if chosen.is_empty() {
        app.notice = if selection == CleanupSelection::AllUnreferenced {
            crate::i18n::t("No unreferenced resources to prune; running/stopped references and unknown usage are protected")
        } else {
            crate::i18n::t("Select unreferenced resources first; running/stopped references and unknown usage are protected")
        }.into();
        return;
    }
    let Some(identity) = snapshot.engine_id.clone().filter(|id| !id.is_empty()) else {
        app.notice = crate::i18n::t("Runtime identity unavailable; deletion refused").into();
        return;
    };
    let mut targets = Vec::new();
    for row in chosen {
        let request = crate::backend::RemovalRequest {
            connection: engine.connection.clone(),
            identity: identity.clone(),
            key: row.key.clone(),
            volume: key.1 == OverviewMode::Volumes,
        };
        if let Err(error) = request.validate(snapshot) {
            app.notice = format!("{error:#}");
            return;
        }
        targets.push(crate::app::InventoryRemovalTarget {
            request,
            label: format!(
                "{}{}",
                row.name,
                if row.tags.is_empty() || key.1 == OverviewMode::Volumes {
                    String::new()
                } else {
                    format!(":{}", row.tags)
                }
            ),
        });
    }
    app.inventory_removal = Some(crate::app::InventoryRemoval {
        targets,
        context: key,
        scroll: 0,
        visible: false,
        pending: false,
        dismissed: false,
    });
}

pub fn draw_removal(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(removal) = app.inventory_removal.as_mut() else {
        return;
    };
    removal.visible = false;
    if removal.dismissed {
        return;
    }
    let Some(first) = removal.targets.first() else {
        return;
    };
    let height = removal
        .targets
        .len()
        .saturating_mul(2)
        .saturating_add(13)
        .clamp(19, 26) as u16;
    let dialog = centered(area, 100, height);
    frame.render_widget(Clear, dialog);
    let block = Block::bordered().title(crate::i18n::t(" Confirm resource deletion "));
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);
    if area.width < 70 || area.height < 19 {
        frame.render_widget(
            Paragraph::new(crate::i18n::t(
                "Resize to at least 70 × 19 to review deletion. Esc cancels.",
            ))
            .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    let parts = Layout::vertical([
        Constraint::Length(6),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(inner);
    let header=Paragraph::new(crate::tf!("Runtime: {a0}\nRemove {a1} {a2}?\n{a3}\nRunning AND stopped references rechecked before EACH deletion.\nNo force deletion or parent-image pruning.", a0 = safe_text(&first.request.connection.name), a1 = removal.targets.len(), a2 = if first.request.volume {crate::i18n::t("managed volumes")} else {"images"}, a3 = if first.request.volume {crate::i18n::t("WARNING: this permanently destroys all data in the listed volumes.")} else {crate::i18n::t("Image sizes include shared layers; they are NOT reclaimable-space estimates.")})).wrap(Wrap {trim:false});
    if header.line_count(parts[0].width) > parts[0].height as usize {
        frame.render_widget(
            Paragraph::new(crate::i18n::t(
                "Resize wider/taller to review the complete deletion confirmation. Esc cancels.",
            ))
            .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    frame.render_widget(header, parts[0]);
    let names: Vec<_> = removal
        .targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            Line::from(format!(
                "{}. {}{}",
                index + 1,
                safe_text(&target.label),
                if target.label == target.request.key {
                    String::new()
                } else {
                    format!(" · {}", safe_text(&target.request.key))
                }
            ))
        })
        .collect();
    let list = Paragraph::new(names).wrap(Wrap { trim: false });
    removal.scroll = removal
        .scroll
        .min(
            list.line_count(parts[1].width)
                .saturating_sub(parts[1].height as usize),
        )
        .min(u16::MAX as usize);
    frame.render_widget(list.scroll((removal.scroll as u16, 0)), parts[1]);
    frame.render_widget(Paragraph::new(if removal.pending {crate::i18n::t("Cleanup running · Esc Hide (does not undo or cancel sent requests)\n↑/↓ or PgUp/PgDn Review names")} else {crate::i18n::t("y / Enter Confirm · n / Esc Cancel\n↑/↓ or PgUp/PgDn Review names")}).style(muted()).wrap(Wrap {trim:false}),parts[2]);
    removal.visible = true;
}

pub fn draw_removal_results(frame: &mut Frame, app: &mut App, area: Rect) {
    if !app.inventory_removal_results_visible {
        return;
    }
    let Some(report) = &app.inventory_removal_report else {
        return;
    };
    let dialog = centered(area, 110, 28);
    frame.render_widget(Clear, dialog);
    let block = Block::bordered().title(crate::i18n::t(" Resource cleanup results "));
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);
    if inner.width == 0 || inner.height < 3 {
        return;
    }
    let parts = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(Paragraph::new(report.summary()).style(muted()), parts[0]);
    let rows: Vec<_> = report
        .entries
        .iter()
        .map(|(key, status)| {
            Line::from(format!(
                "{} · {}",
                safe_text(key),
                match status {
                    crate::backend::RemovalStatus::Removed => crate::i18n::t("Removed").into(),
                    crate::backend::RemovalStatus::Skipped(reason) =>
                        crate::tf!("Skipped: {a0}", a0 = safe_text(reason)),
                    crate::backend::RemovalStatus::Failed(reason) =>
                        crate::tf!("Failed: {a0}", a0 = safe_text(reason)),
                }
            ))
        })
        .collect();
    let paragraph = Paragraph::new(rows).wrap(Wrap { trim: false });
    app.inventory_removal_results_scroll = app
        .inventory_removal_results_scroll
        .min(
            paragraph
                .line_count(parts[1].width)
                .saturating_sub(parts[1].height as usize),
        )
        .min(u16::MAX as usize);
    frame.render_widget(
        paragraph.scroll((app.inventory_removal_results_scroll as u16, 0)),
        parts[1],
    );
    frame.render_widget(
        Paragraph::new(crate::i18n::t("j/k PgUp/PgDn Scroll · Enter/Esc Close")).style(muted()),
        parts[2],
    );
}

pub fn navigate(app: &mut App, code: KeyCode, height: usize) {
    let key = (app.inventory_engine, app.overview_mode);
    let mut view = app.inventory_views.get(&key).cloned().unwrap_or_default();
    let rows = app
        .engines
        .get(key.0)
        .and_then(|engine| engine.snapshot.as_ref())
        .map(|snapshot| {
            visible(
                entries(
                    snapshot,
                    key.1,
                    key.1 == OverviewMode::Volumes && view.filter == 5,
                ),
                &view,
                key.1,
                &view.query,
            )
        })
        .unwrap_or_default();
    reconcile(&mut view, &rows);
    let amount = height.saturating_sub(9).max(1);
    if view.details
        && matches!(
            code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::Char('j' | 'k')
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End
        )
    {
        view.details_scroll = match code {
            KeyCode::Up | KeyCode::Char('k') => view.details_scroll.saturating_sub(1),
            KeyCode::PageUp => view.details_scroll.saturating_sub(amount),
            KeyCode::PageDown => view
                .details_scroll
                .saturating_add(amount)
                .min(u16::MAX as usize),
            KeyCode::Home => 0,
            KeyCode::End => u16::MAX as usize,
            _ => view.details_scroll.saturating_add(1).min(u16::MAX as usize),
        };
    } else {
        match code {
            KeyCode::Char('z') if app.security.enabled => view.zoom = !view.zoom,
            KeyCode::Char('m') if key.1 == OverviewMode::Images && app.security.enabled => {
                if let Some(row) = rows.get(view.index)
                    && !view.scan_marked.remove(&row.key)
                {
                    view.scan_marked.insert(row.key.clone());
                }
            }
            KeyCode::Char('M') if key.1 == OverviewMode::Images && app.security.enabled => {
                view.scan_marked = rows.iter().map(|row| row.key.clone()).collect();
            }
            KeyCode::Char(' ') => {
                if let Some(row) = rows.get(view.index) {
                    if row.status == "unreferenced"
                        && !(key.1 == OverviewMode::Volumes && view.filter == 5)
                    {
                        if !view.marked.remove(&row.key) {
                            view.marked.insert(row.key.clone());
                        }
                    } else {
                        app.notice =
                            crate::i18n::t("Only unreferenced resources can be marked for cleanup")
                                .into();
                    }
                }
            }
            KeyCode::Char('u') => {
                view.marked = rows
                    .iter()
                    .filter(|row| {
                        row.status == "unreferenced"
                            && !(key.1 == OverviewMode::Volumes && view.filter == 5)
                    })
                    .map(|row| row.key.clone())
                    .collect()
            }
            KeyCode::Char('c') => {
                view.marked.clear();
                view.scan_marked.clear();
            }
            KeyCode::Up | KeyCode::Char('k') => view.index = view.index.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => view.index = view.index.saturating_add(1),
            KeyCode::PageUp => view.index = view.index.saturating_sub(amount),
            KeyCode::PageDown => view.index = view.index.saturating_add(amount),
            KeyCode::Home => view.index = 0,
            KeyCode::End | KeyCode::Char('G') => view.index = rows.len().saturating_sub(1),
            KeyCode::Char('g') if view.pending_g => view.index = 0,
            KeyCode::Enter if !rows.is_empty() => {
                view.details = !view.details;
                view.details_scroll = 0;
            }
            KeyCode::Esc => view.details = false,
            KeyCode::Char('f') => {
                view.marked.clear();
                view.filter = (view.filter + 1) % filters(key.1).len();
                view.index = 0;
                view.selected = None;
                view.offset = 0;
                view.details = false;
            }
            KeyCode::Char('s') => view.sort = (view.sort + 1) % SORTS.len(),
            _ => {}
        }
        if !matches!(code, KeyCode::Char('f')) {
            view.index = view.index.min(rows.len().saturating_sub(1));
            view.selected = rows.get(view.index).map(|row| row.key.clone());
        }
    }
    view.pending_g = code == KeyCode::Char('g') && !view.pending_g;
    app.inventory_views.insert(key, view);
}

/// The connected runtime whose images the Images view is showing.
fn image_runtime(app: &App) -> Result<(&crate::app::Engine, &Snapshot), String> {
    if app.demo {
        return Err(crate::i18n::t("Demo mode does not contact registries").into());
    }
    let engine = app
        .engines
        .get(app.inventory_engine)
        .filter(|engine| engine.error.is_none() && engine.alias.is_none())
        .ok_or(crate::i18n::t("Select a connected runtime first"))?;
    if engine.connection.kind != crate::config::BackendKind::Docker {
        return Err(crate::i18n::t("Registry updates require a Docker-compatible runtime").into());
    }
    let snapshot = engine
        .snapshot
        .as_ref()
        .filter(|snapshot| snapshot.images.is_some())
        .ok_or(crate::i18n::t(
            "Image inventory is unavailable for this runtime",
        ))?;
    Ok((engine, snapshot))
}

/// Compare what the Images list shows now. Only this explicit request contacts a registry.
pub fn begin_update_check(app: &mut App) {
    let (engine, snapshot) = match image_runtime(app) {
        Ok(runtime) => runtime,
        Err(notice) => {
            app.notice = notice;
            return;
        }
    };
    let key = (app.inventory_engine, OverviewMode::Images);
    let view = app.inventory_views.get(&key).cloned().unwrap_or_default();
    let rows = visible(entries(snapshot, key.1, false), &view, key.1, &view.query);
    let targets: Vec<_> = snapshot
        .images
        .iter()
        .flatten()
        .filter(|image| !image.tags.is_empty() && rows.iter().any(|row| row.key == image.id))
        .map(|image| crate::images::Target {
            connection: engine.connection.clone(),
            engine: key.0,
            image: image.clone(),
        })
        .collect();
    if targets.is_empty() {
        app.notice = crate::i18n::t("No tagged images are visible to check").into();
        return;
    }
    app.notice = crate::tf!(
        "Checking {a0} images against their registries…",
        a0 = targets.len()
    );
    app.image_check_request = Some(targets);
}

/// Pull every tag of the selected image. Containers keep the image they already use.
pub fn begin_image_pull(app: &mut App) {
    if app.image_pull_active {
        app.notice = crate::i18n::t("Another image pull is still running").into();
        return;
    }
    let (engine, snapshot) = match image_runtime(app) {
        Ok(runtime) => runtime,
        Err(notice) => {
            app.notice = notice;
            return;
        }
    };
    let key = (app.inventory_engine, OverviewMode::Images);
    let view = app.inventory_views.get(&key).cloned().unwrap_or_default();
    let rows = visible(entries(snapshot, key.1, false), &view, key.1, &view.query);
    let selected = view
        .selected
        .as_ref()
        .and_then(|id| rows.iter().find(|row| &row.key == id))
        .or_else(|| rows.get(view.index));
    let image = selected.and_then(|row| {
        snapshot
            .images
            .iter()
            .flatten()
            .find(|image| image.id == row.key)
    });
    let Some(image) = image else {
        app.notice = crate::i18n::t("Select an image in the Images view to pull").into();
        return;
    };
    let tags: Vec<_> = image
        .tags
        .iter()
        .filter(|tag| crate::images::pullable(tag))
        .cloned()
        .collect();
    if tags.is_empty() {
        app.notice = crate::i18n::t("This image has no tag to pull").into();
        return;
    }
    let notice = crate::tf!("Pulling {a0}…", a0 = safe_text(&tags.join(", ")));
    let request = crate::images::PullRequest {
        connection: engine.connection.clone(),
        engine: key.0,
        image: image.id.clone(),
        tags,
    };
    app.notice = notice;
    app.image_pull_request = Some(request);
    app.image_pull_active = true;
}

/// Resolve only currently available images in the chosen engine, including unused ones.
pub fn security_targets(app: &App, checked: bool) -> Vec<crate::security::Target> {
    let key = (app.inventory_engine, OverviewMode::Images);
    let view = app.inventory_views.get(&key).cloned().unwrap_or_default();
    let Some(engine) = app
        .engines
        .get(key.0)
        .filter(|engine| engine.error.is_none() && engine.alias.is_none())
    else {
        return vec![];
    };
    let Some(snapshot) = engine.snapshot.as_ref() else {
        return vec![];
    };
    let Some(runtime) = snapshot.engine_id.as_ref() else {
        return vec![];
    };
    let rows = visible(entries(snapshot, key.1, false), &view, key.1, &view.query);
    let selected = view
        .selected
        .as_ref()
        .and_then(|id| rows.iter().find(|row| &row.key == id))
        .or_else(|| rows.get(view.index));
    // Scan every checked image still in inventory, including checked rows hidden by filters.
    let all = entries(snapshot, key.1, false);
    all.into_iter()
        .filter(|row| {
            if checked && !view.scan_marked.is_empty() {
                view.scan_marked.contains(&row.key)
            } else {
                selected.is_some_and(|selected| selected.key == row.key)
            }
        })
        .map(|row| crate::security::Target {
            connection: engine.connection.clone(),
            runtime: runtime.clone(),
            image: row.key,
            label: format!("{}:{}", row.name, row.tags),
        })
        .collect()
}

pub fn draw_overview(frame: &mut Frame, app: &mut App, area: Rect) {
    draw_overview_inner(frame, app, area, false);
}

pub fn draw_security_images(frame: &mut Frame, app: &mut App, area: Rect) {
    app.inventory_views
        .entry((app.inventory_engine, OverviewMode::Images))
        .or_default()
        .details = false;
    draw_overview_inner(frame, app, area, true);
}

fn all_update_count(app: &App, engine: usize, snapshot: &Snapshot) -> usize {
    snapshot
        .images
        .iter()
        .flatten()
        .filter(|image| update_available(&app.image_updates, engine, &image.id))
        .count()
}

fn draw_overview_inner(frame: &mut Frame, app: &mut App, area: Rect, scan_only: bool) {
    let parts = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    app.clamp_inventory_engine();
    let engines = app.engines_in_view();
    let names: Vec<_> = engines
        .iter()
        .map(|&index| safe_text(&app.engines[index].connection.name))
        .collect();
    app.inventory_engine = app
        .inventory_engine
        .min(app.engines.len().saturating_sub(1));
    let titles: Vec<&str> = names.iter().map(String::as_str).collect();
    for (index, region) in tab_regions(parts[0], &titles).into_iter().enumerate() {
        app.mouse_regions
            .push((region, MouseTarget::InventoryRuntime(index)));
    }
    app.mouse_regions
        .push((parts[1], MouseTarget::InventoryPane));
    frame.render_widget(
        Tabs::new(names)
            .select(
                engines
                    .iter()
                    .position(|&index| index == app.inventory_engine),
            )
            .highlight_style(selected()),
        parts[0],
    );
    let block = pane(
        app,
        if scan_only {
            crate::i18n::t(" Security images · m/M Check · Enter/Tab Findings ").into()
        } else if app.overview_mode == OverviewMode::Images {
            crate::i18n::t(" Images overview ").into()
        } else {
            crate::i18n::t(" Volumes overview ").into()
        },
        if scan_only { !app.detail_focus } else { true },
    );
    let inner = block.inner(parts[1]);
    frame.render_widget(block, parts[1]);
    draw_inner(frame, app, inner, scan_only);
}

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    draw_inner(frame, app, area, false);
}

fn draw_inner(frame: &mut Frame, app: &mut App, area: Rect, scan_only: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let key = (app.inventory_engine, app.overview_mode);
    let mut view = app.inventory_views.get(&key).cloned().unwrap_or_default();
    let Some(engine) = app.engines.get(key.0) else {
        frame.render_widget(Paragraph::new(crate::i18n::t("Select a runtime.")), area);
        return;
    };
    let Some(snapshot) = engine.snapshot.as_ref() else {
        frame.render_widget(
            Paragraph::new(crate::i18n::t(
                "Runtime unavailable; inventory cannot be read.",
            )),
            area,
        );
        return;
    };
    let binds = key.1 == OverviewMode::Volumes && view.filter == 5;
    let unavailable = if key.1 == OverviewMode::Images {
        snapshot.images.is_none()
    } else {
        !binds && !snapshot.volumes_available
    };
    let all = entries(snapshot, key.1, binds);
    let total = all.len();
    let used = all
        .iter()
        .filter(|row| row.running + row.stopped > 0)
        .count();
    let unused = all
        .iter()
        .filter(|row| row.status == "unreferenced")
        .count();
    let query = if app.input_mode == InputMode::InventorySearch {
        app.input.clone()
    } else {
        view.query.clone()
    };
    view.marked.retain(|key| {
        !unavailable
            && all
                .iter()
                .any(|row| &row.key == key && row.status == "unreferenced")
    });
    let rows = visible(all, &view, key.1, &query);
    reconcile(&mut view, &rows);
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(crate::tf!(
            "{a0}  ·  {a1}  ·  Filter: {a2}  ·  Sort: {a3}{a4}",
            a0 = safe_text(&engine.connection.name),
            a1 = if binds {
                crate::i18n::t("Bind mounts")
            } else {
                crate::i18n::t("inventory")
            },
            a2 = crate::i18n::dynamic(filters(key.1)[view.filter % filters(key.1).len()]),
            a3 = crate::i18n::dynamic(SORTS[view.sort % SORTS.len()]),
            a4 = if engine.error.is_some() {
                crate::i18n::t("  ·  STALE snapshot")
            } else {
                ""
            }
        ))
        .style(muted()),
        parts[0],
    );
    let updates = if key.1 == OverviewMode::Images {
        match all_update_count(app, key.0, snapshot) {
            0 => String::new(),
            count => crate::tf!(" · {count} ↑ update available", count = count),
        }
    } else {
        String::new()
    };
    let marks = if scan_only {
        crate::tf!("{a0} checked for scan", a0 = view.scan_marked.len())
    } else if app.security.enabled && key.1 == OverviewMode::Images {
        crate::tf!(
            "{a0} cleanup / {a1} scan checked",
            a0 = view.marked.len(),
            a1 = view.scan_marked.len()
        )
    } else {
        crate::tf!("{count} marked", count = view.marked.len())
    };
    frame.render_widget(
        Paragraph::new(crate::tf!(
            "{a0} / {total} shown · {used} used · {unused} unreferenced · {marks}{updates}{a1}",
            a0 = rows.len(),
            total = total,
            used = used,
            unused = unused,
            marks = marks,
            updates = updates,
            a1 = if query.is_empty() {
                String::new()
            } else {
                format!(" · /{}", safe_text(&query))
            }
        )),
        parts[1],
    );
    frame.render_widget(
        Paragraph::new({
            let row = if rows.is_empty() { 0 } else { view.index + 1 };
            let total = rows.len();
            if scan_only {
                t("Space/m Check · M All · s/S Scan · Enter Findings · Tab Focus · z Zoom").to_owned()
            } else if app.security.enabled && key.1 == OverviewMode::Images {
                crate::tf!(
                    "Row {row} / {total} · U Check updates · :pull-image · m/M Check scan/all · 7 Security · :scan-images · z Zoom · Space Cleanup · d Delete · p Prune all unused · Enter Details",
                    row = row,
                    total = total
                )
            } else if key.1 == OverviewMode::Images {
                crate::tf!(
                    "Row {row} / {total} · Space Mark  u Mark unused  c Clear  d Delete  p Prune all unused  U Check updates  :pull-image · / Search  f Filter  s Sort  Enter Details",
                    row = row,
                    total = total
                )
            } else {
                crate::tf!(
                    "Row {row} / {total} · Space Mark  u Mark unused  c Clear  d Delete  p Prune all unused · / Search  f Filter  s Sort  Enter Details",
                    row = row,
                    total = total
                )
            }
        })
        .style(muted()),
        parts[3],
    );
    let body = parts[2];
    if unavailable || rows.is_empty() {
        frame.render_widget(
            Paragraph::new(if unavailable {
                crate::i18n::t(
                    "Inventory unavailable for this runtime. See Containers overview warnings.",
                )
            } else if total == 0 {
                crate::i18n::t("No resources reported by this runtime.")
            } else {
                crate::i18n::t("No matches. / edits search; f changes the status filter.")
            })
            .wrap(Wrap { trim: false }),
            body,
        );
    } else {
        let (table_area, details_area) = if view.details && body.width >= 110 && !view.zoom {
            let split =
                Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .split(body);
            (Some(split[0]), Some(split[1]))
        } else if view.details {
            (None, Some(body))
        } else {
            (Some(body), None)
        };
        if let Some(table_area) = table_area {
            let images = key.1 == OverviewMode::Images;
            let wide = table_area.width >= 92;
            let headers = if wide {
                vec![
                    if images {
                        crate::i18n::t("Image")
                    } else {
                        crate::i18n::t("Volume / source")
                    },
                    if images {
                        crate::i18n::t("Tags")
                    } else {
                        crate::i18n::t("Driver")
                    },
                    crate::i18n::t("Size"),
                    crate::i18n::t("Status"),
                    crate::i18n::t("Refs"),
                    if images {
                        "ID"
                    } else {
                        crate::i18n::t("Project")
                    },
                ]
            } else if table_area.width >= 50 {
                vec![
                    if images {
                        crate::i18n::t("Image / tags")
                    } else {
                        crate::i18n::t("Volume / source")
                    },
                    crate::i18n::t("Size"),
                    crate::i18n::t("Status"),
                    crate::i18n::t("Refs"),
                ]
            } else {
                vec![crate::i18n::t("Name"), crate::i18n::t("Status")]
            };
            let available_name_width = table_area.width.saturating_sub(if wide {
                68
            } else if table_area.width >= 50 {
                36
            } else {
                17
            }) as usize;
            let name_width = if wide && !images {
                rows.iter()
                    .map(|row| row.name.width().saturating_add(4))
                    .max()
                    .unwrap_or(18)
                    .clamp(18, MAX_NAME_CELLS)
                    .min(available_name_width)
            } else if wide {
                available_name_width.min(MAX_NAME_CELLS)
            } else {
                available_name_width
            };
            let constraints = if wide {
                vec![
                    Constraint::Length(name_width as u16),
                    Constraint::Length(14),
                    Constraint::Length(10),
                    Constraint::Length(13),
                    Constraint::Length(5),
                    if images {
                        Constraint::Length(14)
                    } else {
                        Constraint::Min(14)
                    },
                ]
            } else if table_area.width >= 50 {
                vec![
                    Constraint::Min(12),
                    Constraint::Length(10),
                    Constraint::Length(13),
                    Constraint::Length(5),
                ]
            } else {
                vec![Constraint::Min(8), Constraint::Length(13)]
            };
            let display_rows = rows.iter().map(|row| {
                let size = row
                    .size
                    .map(metrics::bytes)
                    .unwrap_or_else(|| "unknown".into());
                let status_style = match row.status {
                    "running" => accent(app, Color::Green),
                    "stopped-only" => accent(app, Color::Yellow),
                    _ => muted(),
                };
                let name = if !wide && images && !row.tags.is_empty() {
                    format!("{}:{}", row.name, row.tags)
                } else {
                    row.name.clone()
                };
                // Security checks every image for scanning; cleanup marks only exist in Images.
                let mark = if scan_only {
                    if view.scan_marked.contains(&row.key) {
                        "[x] "
                    } else {
                        "[ ] "
                    }
                } else if app.security.enabled && view.scan_marked.contains(&row.key) {
                    "[✓] "
                } else if view.marked.contains(&row.key) {
                    "[x] "
                } else if row.status == "unreferenced" {
                    "[ ] "
                } else {
                    "    "
                };
                let update = if images && update_available(&app.image_updates, key.0, &row.key) {
                    "↑ "
                } else {
                    ""
                };
                let mut cells = vec![Cell::from(ellipsize_line(
                    Line::from(format!("{mark}{update}{name}")),
                    name_width,
                ))];
                if wide {
                    cells.push(Cell::from(ellipsize_line(Line::from(row.tags.clone()), 14)));
                }
                if table_area.width >= 50 {
                    cells.push(Cell::from(
                        Line::from(crate::i18n::dynamic(&size).to_owned())
                            .alignment(Alignment::Right),
                    ));
                }
                cells.push(Cell::from(crate::i18n::dynamic(row.status)).style(status_style));
                if table_area.width >= 50 {
                    cells.push(Cell::from(
                        Line::from((row.running + row.stopped).to_string())
                            .alignment(Alignment::Right),
                    ));
                }
                if wide {
                    cells.push(Cell::from(if images {
                        safe_text(&row.key)
                            .trim_start_matches("sha256:")
                            .chars()
                            .take(12)
                            .collect::<String>()
                    } else {
                        row.project.clone()
                    }));
                }
                TableRow::new(cells)
            });
            let mut state = TableState::default()
                .with_offset(view.offset)
                .with_selected(Some(view.index));
            frame.render_stateful_widget(
                Table::new(display_rows, constraints)
                    .header(
                        TableRow::new(headers)
                            .style(Style::default().add_modifier(Modifier::BOLD))
                            .bottom_margin(1),
                    )
                    .column_spacing(2)
                    .row_highlight_style(selected())
                    .highlight_symbol("▸ "),
                table_area,
                &mut state,
            );
            view.offset = state.offset();
            // The header row and its margin sit above the first table row.
            for offset in 0..table_area.height.saturating_sub(2) {
                let index = view.offset + offset as usize;
                if index < rows.len() {
                    app.mouse_regions.push((
                        Rect::new(table_area.x, table_area.y + 2 + offset, table_area.width, 1),
                        MouseTarget::InventoryRow(index),
                    ));
                }
            }
        }
        if let Some(details_area) = details_area {
            let row = &rows[view.index];
            let block = Block::bordered()
                .title(crate::i18n::t(" Details · Esc Back "))
                .border_style(muted());
            let inner = block.inner(details_area);
            frame.render_widget(block, details_area);
            if inner.width > 0 {
                let mut lines = vec![
                    Line::styled(
                        row.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Line::from(crate::tf!("ID / key: {a0}", a0 = safe_text(&row.key))),
                    Line::from(row.extra.clone()),
                    Line::from(crate::tf!(
                        "Status: {a0} · {a1} running / {a2} stopped",
                        a0 = row.status,
                        a1 = row.running,
                        a2 = row.stopped
                    )),
                    Line::from(crate::tf!(
                        "Size: {a0}",
                        a0 = row
                            .size
                            .map(metrics::bytes)
                            .unwrap_or_else(
                                || crate::i18n::t("unknown (no filesystem scan)").into()
                            )
                    )),
                ];
                if key.1 == OverviewMode::Images {
                    lines.push(Line::from(crate::tf!(
                        "Update: {a0}",
                        a0 = app
                            .image_updates
                            .get(&(key.0, row.key.clone()))
                            .map_or_else(
                                || crate::i18n::t("not checked · U compares with the registry")
                                    .to_owned(),
                                |status| safe_text(&status.describe())
                            )
                    )));
                }
                lines.push(Line::from(crate::i18n::t("Container references:")));
                lines.extend(
                    row.containers
                        .iter()
                        .map(|name| Line::from(format!("  {name}"))),
                );
                if row.containers.is_empty() {
                    lines.push(Line::from(crate::i18n::t("  None reported")));
                }
                lines.push(Line::styled(if key.1 == OverviewMode::Images { crate::i18n::t("Sizes may include shared layers, not reclaimable space. Untagged does not imply unused.") } else { crate::i18n::t("Unreferenced does not mean safe to delete. Bind mounts are not managed volumes.") }, muted()));
                let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
                view.details_scroll = view
                    .details_scroll
                    .min(
                        paragraph
                            .line_count(inner.width)
                            .saturating_sub(inner.height as usize),
                    )
                    .min(u16::MAX as usize);
                frame.render_widget(paragraph.scroll((view.details_scroll as u16, 0)), inner);
            }
        }
    }
    app.inventory_views.insert(key, view);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn prune_freezes_all_unused_resources_ignoring_search_filter_and_marks() {
        for mode in [OverviewMode::Images, OverviewMode::Volumes] {
            let mut app = App::new(crate::demo::connections(), 100, true, true);
            app.demo = false;
            app.overview_mode = mode;
            app.engines[0].snapshot = Some(Snapshot {
                engine_id: Some("fixture".into()),
                containers: vec![
                    Container {
                        image_id: Some("running".into()),
                        volumes: vec!["running".into()],
                        state: "running".into(),
                        ..Default::default()
                    },
                    Container {
                        image_id: Some("stopped".into()),
                        volumes: vec!["stopped".into()],
                        state: "exited".into(),
                        ..Default::default()
                    },
                ],
                images: Some(
                    ["running", "stopped", "unused-one", "unused-two"]
                        .into_iter()
                        .map(|id| Image {
                            id: id.into(),
                            tags: vec![format!("{id}:latest")],
                            digests: vec![],
                            size: None,
                        })
                        .collect(),
                ),
                volumes_available: true,
                volumes: ["running", "stopped", "unused-one", "unused-two"]
                    .into_iter()
                    .map(|name| crate::model::Resource {
                        name: name.into(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            });
            let view = app.inventory_views.entry((0, mode)).or_default();
            view.filter = 1;
            view.query = "no matches".into();
            view.marked.insert("unused-one".into());

            begin_prune(&mut app);

            let removal = app.inventory_removal.as_ref().unwrap();
            assert_eq!(
                removal
                    .targets
                    .iter()
                    .map(|target| target.request.key.as_str())
                    .collect::<Vec<_>>(),
                ["unused-one", "unused-two"]
            );
            assert!(!removal.pending);
            assert!(app.inventory_removal_request.is_none());
            app.engines[0]
                .snapshot
                .as_mut()
                .unwrap()
                .volumes
                .push(crate::model::Resource {
                    name: "new-unused".into(),
                    ..Default::default()
                });
            assert_eq!(app.inventory_removal.as_ref().unwrap().targets.len(), 2);
        }
    }

    #[test]
    fn prune_protects_unknown_image_usage_and_demo_mode() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.overview_mode = OverviewMode::Images;
        begin_prune(&mut app);
        assert!(app.inventory_removal.is_none());
        app.demo = false;
        begin_prune(&mut app);
        assert!(app.inventory_removal.is_none());
        assert!(app.notice.contains("No unreferenced resources"));
    }

    #[test]
    fn marking_is_unused_only_query_scoped_and_confirmation_targets_are_frozen() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.demo = false;
        app.overview_mode = OverviewMode::Volumes;
        let key = (0, OverviewMode::Volumes);
        app.engines[0]
            .snapshot
            .as_mut()
            .unwrap()
            .volumes
            .push(crate::model::Resource {
                name: "extra_cache".into(),
                ..Default::default()
            });
        navigate(&mut app, KeyCode::Home, 25);
        navigate(&mut app, KeyCode::Char(' '), 25);
        assert!(
            app.inventory_views[&key].marked.is_empty(),
            "A referenced row cannot be marked"
        );
        navigate(&mut app, KeyCode::Char('u'), 25);
        assert_eq!(app.inventory_views[&key].marked.len(), 2);
        let view = app.inventory_views.get_mut(&key).unwrap();
        view.details = true;
        view.index = usize::MAX;
        view.selected = None;
        navigate(&mut app, KeyCode::Char(' '), 25);
        assert_eq!(
            app.inventory_views[&key].marked.len(),
            1,
            "Space toggles the row while Details is open"
        );
        navigate(&mut app, KeyCode::Char(' '), 25);
        assert_eq!(app.inventory_views[&key].marked.len(), 2);
        assert!(app.inventory_views[&key].details);
        app.inventory_views.get_mut(&key).unwrap().details = false;
        navigate(&mut app, KeyCode::Char('c'), 25);
        app.inventory_views.get_mut(&key).unwrap().query = "old_cache".into();
        navigate(&mut app, KeyCode::Char('u'), 25);
        assert_eq!(app.inventory_views[&key].marked.len(), 1);
        begin_removal(&mut app);
        assert_eq!(app.inventory_removal.as_ref().unwrap().targets.len(), 1);
        assert_eq!(
            app.inventory_removal.as_ref().unwrap().targets[0]
                .request
                .key,
            "old_cache"
        );
        app.engines[0]
            .snapshot
            .as_mut()
            .unwrap()
            .volumes
            .push(crate::model::Resource {
                name: "old_cache_new".into(),
                ..Default::default()
            });
        assert_eq!(
            app.inventory_removal.as_ref().unwrap().targets.len(),
            1,
            "Newly appearing resources must never join an existing confirmation"
        );
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal
            .draw(|frame| draw_removal(frame, &mut app, frame.area()))
            .unwrap();
        assert!(app.inventory_removal.as_ref().unwrap().visible);
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("y / Enter Confirm"));
        assert!(text.contains("n / Esc Cancel"));
        assert!(!text.contains("old_cache_new"));
        let mut tiny = Terminal::new(TestBackend::new(50, 12)).unwrap();
        tiny.draw(|frame| draw_removal(frame, &mut app, frame.area()))
            .unwrap();
        assert!(!app.inventory_removal.as_ref().unwrap().visible);
        app.inventory_removal = None;
        navigate(&mut app, KeyCode::Char('f'), 25);
        assert!(
            app.inventory_views[&key].marked.is_empty(),
            "Filter changes must clear marks"
        );
        app.overview_mode = OverviewMode::Images;
        navigate(&mut app, KeyCode::Char('u'), 25);
        assert!(
            app.inventory_views[&(0, OverviewMode::Images)]
                .marked
                .is_empty(),
            "Unknown image usage cannot be bulk marked"
        );
    }
    #[test]
    fn large_confirmation_and_partial_result_lists_remain_scrollable() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.demo = false;
        app.overview_mode = OverviewMode::Volumes;
        app.engines[0].snapshot.as_mut().unwrap().volumes = (0..120)
            .map(|i| crate::model::Resource {
                name: format!("cache-{i:03}"),
                ..Default::default()
            })
            .collect();
        navigate(&mut app, KeyCode::Char('u'), 25);
        begin_removal(&mut app);
        assert_eq!(app.inventory_removal.as_ref().unwrap().targets.len(), 120);
        app.inventory_removal.as_mut().unwrap().scroll = u16::MAX as usize;
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal
            .draw(|frame| draw_removal(frame, &mut app, frame.area()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("cache-119"));
        assert!(text.contains("Remove 120 managed volumes?"));
        app.inventory_removal_report = Some(crate::backend::RemovalReport {
            entries: vec![
                ("cache-one".into(), crate::backend::RemovalStatus::Removed),
                (
                    "cache-two".into(),
                    crate::backend::RemovalStatus::Skipped("newly referenced".into()),
                ),
                (
                    "cache-three".into(),
                    crate::backend::RemovalStatus::Failed(
                        "connection lost; refresh before retrying".into(),
                    ),
                ),
            ],
        });
        app.inventory_removal_results_visible = true;
        terminal
            .draw(|frame| draw_removal_results(frame, &mut app, frame.area()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("1 removed, 1 skipped, 1 failed"));
        assert!(text.contains("newly referenced"));
    }
    #[test]
    fn ids_override_retagged_names_and_missing_identity_is_unknown() {
        let image = Image {
            id: "sha256:old".into(),
            tags: vec!["app:latest".into()],
            digests: vec![],
            size: None,
        };
        let containers = vec![
            Container {
                image_id: Some("sha256:new".into()),
                image: "app:latest".into(),
                state: "running".into(),
                ..Default::default()
            },
            Container {
                image_id: Some(image.id.clone()),
                state: "exited".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            usage(&references(&image, &containers), true),
            (0, 1, "stopped-only")
        );
        assert_eq!(usage(&[], false), (0, 0, "unknown"));
        assert_eq!(
            image_name("localhost:5055/app:dev"),
            ("localhost:5055/app", "dev")
        );
    }

    #[test]
    fn scan_checks_include_used_images_and_are_independent_of_cleanup_and_filters() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        app.security.enabled = true;
        crate::demo::populate(&mut app);
        app.overview_mode = OverviewMode::Images;
        let snapshot = app.engines[0].snapshot.as_mut().unwrap();
        snapshot.engine_id = Some("fixture-engine".into());
        snapshot.images = Some(vec![
            Image {
                id: "sha256:used".into(),
                tags: vec!["used:latest".into()],
                digests: vec![],
                size: None,
            },
            Image {
                id: "sha256:unused".into(),
                tags: vec!["unused:latest".into()],
                digests: vec![],
                size: None,
            },
        ]);
        snapshot.containers[0].image_id = Some("sha256:used".into());
        navigate(&mut app, KeyCode::Char('M'), 25);
        assert_eq!(security_targets(&app, true).len(), 2);
        let view = app
            .inventory_views
            .get_mut(&(0, OverviewMode::Images))
            .unwrap();
        assert!(view.marked.is_empty());
        view.query = "used:latest".into();
        assert_eq!(
            security_targets(&app, true).len(),
            2,
            "Hidden checks remain in the scan queue"
        );
        app.engines[0]
            .snapshot
            .as_mut()
            .unwrap()
            .images
            .as_mut()
            .unwrap()
            .retain(|image| image.id == "sha256:used");
        assert_eq!(
            security_targets(&app, true).len(),
            1,
            "Vanished checks are skipped"
        );
    }
    #[test]
    fn sorting_filtering_search_and_stable_selection_cover_full_metadata() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        let snapshot = app.engines[0].snapshot.as_mut().unwrap();
        for c in &mut snapshot.containers {
            c.image_id = Some(
                if c.image == "redis:7" {
                    "sha256:demo-redis"
                } else {
                    "sha256:demo-api"
                }
                .into(),
            );
        }
        let mut view = InventoryView::default();
        let all = || entries(snapshot, OverviewMode::Images, false);
        let rows = visible(all(), &view, OverviewMode::Images, "");
        assert_eq!(rows[0].status, "running");
        assert_eq!(rows[1].status, "stopped-only");
        assert_eq!(rows[2].status, "unreferenced");
        assert_eq!(
            visible(all(), &view, OverviewMode::Images, "SCRATCH-REDIS").len(),
            1
        );
        assert_eq!(
            visible(all(), &view, OverviewMode::Images, "sha256:unused").len(),
            1
        );
        view.filter = 4;
        assert_eq!(visible(all(), &view, OverviewMode::Images, "").len(), 1);
        view.filter = 0;
        view.selected = Some("sha256:demo-api".into());
        view.sort = 2;
        let rows = visible(all(), &view, OverviewMode::Images, "");
        reconcile(&mut view, &rows);
        assert_eq!(rows[view.index].key, "sha256:demo-api");
        let volumes = visible(
            entries(snapshot, OverviewMode::Volumes, false),
            &InventoryView::default(),
            OverviewMode::Volumes,
            "",
        );
        assert_eq!(volumes[0].status, "running");
        assert_eq!(volumes[1].name, "old_cache");
        assert_eq!(entries(snapshot, OverviewMode::Volumes, true).len(), 1);
    }
    #[test]
    fn selection_follows_identity_across_refresh_and_table_scrolls_to_last_row() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.overview_mode = OverviewMode::Images;
        let images: Vec<_> = (0..100)
            .map(|i| Image {
                id: format!("id-{i}"),
                tags: vec![format!("image{i:03}:latest")],
                digests: vec![],
                size: Some(i),
            })
            .collect();
        app.engines[0].snapshot = Some(Snapshot {
            images: Some(images),
            ..Default::default()
        });
        navigate(&mut app, KeyCode::End, 15);
        let key = (0, OverviewMode::Images);
        let mut terminal = Terminal::new(TestBackend::new(100, 15)).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut app, frame.area()))
            .unwrap();
        assert_eq!(app.inventory_views[&key].selected.as_deref(), Some("id-99"));
        assert!(app.inventory_views[&key].offset > 0);
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("image099"));
        app.engines[0]
            .snapshot
            .as_mut()
            .unwrap()
            .images
            .as_mut()
            .unwrap()
            .push(Image {
                id: "inserted".into(),
                tags: vec!["aaa:new".into()],
                digests: vec![],
                size: None,
            });
        terminal
            .draw(|frame| draw(frame, &mut app, frame.area()))
            .unwrap();
        assert_eq!(app.inventory_views[&key].selected.as_deref(), Some("id-99"));
        assert_eq!(app.inventory_views[&key].index, 100);
        app.engines[0]
            .snapshot
            .as_mut()
            .unwrap()
            .images
            .as_mut()
            .unwrap()
            .clear();
        terminal
            .draw(|frame| draw(frame, &mut app, frame.area()))
            .unwrap();
        assert!(app.inventory_views[&key].selected.is_none());
        assert_eq!(app.inventory_views[&key].offset, 0);
    }
    #[test]
    fn bind_mounts_are_grouped_without_duplicate_container_references() {
        let snapshot = Snapshot {
            containers: vec![
                Container {
                    id: "one".into(),
                    name: "web".into(),
                    state: "running".into(),
                    bind_mounts: vec!["/host → /app".into(), "/host → /app".into()],
                    ..Default::default()
                },
                Container {
                    id: "two".into(),
                    name: "old-web".into(),
                    state: "exited".into(),
                    bind_mounts: vec!["/host → /app".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let rows = entries(&snapshot, OverviewMode::Volumes, true);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].running, rows[0].stopped), (1, 1));
        assert_eq!(
            visible(
                rows,
                &InventoryView {
                    filter: 5,
                    ..Default::default()
                },
                OverviewMode::Volumes,
                "OLD-WEB"
            )
            .len(),
            1
        );
    }
    #[test]
    fn tables_details_empty_and_unavailable_views_survive_resize() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        for mode in [OverviewMode::Images, OverviewMode::Volumes] {
            app.overview_mode = mode;
            for size in [(150, 40), (65, 20), (20, 8), (1, 1)] {
                for details in [false, true] {
                    app.inventory_views.entry((0, mode)).or_default().details = details;
                    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
                    terminal
                        .draw(|frame| draw(frame, &mut app, frame.area()))
                        .unwrap();
                }
            }
        }
        app.engines[0].snapshot.as_mut().unwrap().volumes_available = false;
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut app, frame.area()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Inventory unavailable"));
    }

    #[test]
    fn volume_names_are_content_sized_capped_and_leave_room_for_projects() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.overview_mode = OverviewMode::Volumes;
        let project = "project_with_more_than_fourteen_cells";
        for name in ["zam_database".to_owned(), "x".repeat(100), "界".repeat(40)] {
            let snapshot = app.engines[0].snapshot.as_mut().unwrap();
            snapshot.volumes = vec![crate::model::Resource {
                name: name.clone(),
                project: Some(project.into()),
                details: "driver: local".into(),
                ..Default::default()
            }];
            snapshot.volumes_available = true;
            for width in [150, 220] {
                let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
                terminal
                    .draw(|frame| draw(frame, &mut app, frame.area()))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let lines: Vec<String> = (0..20)
                    .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
                    .collect();
                let header = lines
                    .iter()
                    .find(|line| line.contains("Volume / source"))
                    .unwrap();
                let driver = header.find("Driver").unwrap();
                let volume = header.find("Volume / source").unwrap();
                let expected_width = (name.width() + 4)
                    .clamp(18, 82)
                    .min(usize::from(width) - 68);
                assert_eq!(driver - volume, expected_width + 2);
                let visible_project = if width >= 220 {
                    project
                } else {
                    &project[..14]
                };
                assert!(
                    lines.iter().any(|line| line.contains(visible_project)),
                    "{lines:#?}"
                );
                if name.width() + 4 > expected_width {
                    assert!(lines.iter().any(|line| line.contains('…')));
                    let header_y = lines.iter().position(|line| line == header).unwrap();
                    let row_y = header_y as u16 + 2;
                    // A double-width grapheme may leave one spare cell before
                    // the next column; the ellipsis must still stay inside Name.
                    assert!((3..=4).any(|offset| {
                        buffer[(driver as u16 - offset, row_y)].symbol() == "…"
                    }));
                    assert_eq!(buffer[(driver as u16, row_y)].symbol(), "l");
                } else {
                    assert!(lines.iter().any(|line| line.contains(&name)));
                }
            }
        }
    }

    #[test]
    fn image_names_are_capped_at_82_cells_and_shrink_with_the_table() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.overview_mode = OverviewMode::Images;
        app.engines[0].snapshot.as_mut().unwrap().images = Some(vec![Image {
            id: format!("sha256:{}", "a".repeat(64)),
            tags: vec![format!("registry.example/{}:latest", "x".repeat(100))],
            digests: vec![],
            size: Some(1024),
        }]);
        for width in [92, 120, 150, 220] {
            let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
            terminal
                .draw(|frame| draw(frame, &mut app, frame.area()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let lines: Vec<String> = (0..20)
                .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            let header = lines
                .iter()
                .find(|line| line.contains("Image") && line.contains("Tags"))
                .unwrap();
            let name = header.find("Image").unwrap();
            let tags = header.find("Tags").unwrap();
            assert_eq!(tags - name, (usize::from(width) - 68).min(82) + 2);
            assert!(lines.iter().any(|line| line.contains('…')));
            assert!(lines.iter().any(|line| line.contains("latest")));
            let row_y = lines
                .iter()
                .position(|line| line.contains("registry.example/"))
                .unwrap();
            assert_eq!(buffer[(tags as u16 - 3, row_y as u16)].symbol(), "…");
            assert_eq!(buffer[(tags as u16, row_y as u16)].symbol(), "l");
        }
    }
}
