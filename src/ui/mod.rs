use crate::{
    app::{
        App, Choice, InputMode, MouseTarget, NetworkTarget, OverviewMode, PerformanceScope,
        ResourceKind, Scope, Tab,
    },
    config::BackendKind,
    logs::view::{self as log_view, Field},
    metrics,
    model::{Condition, Tone, safe_text},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, Gauge, LineGauge, Paragraph, Row as TableRow, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Sparkline, Table, Tabs, Wrap,
    },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
mod compose;
mod events;
mod history;
pub mod inventory;
mod security;

pub const COMMANDS: &[(&str, &str)] = &[
    (
        "action-results",
        "Review background lifecycle progress and results",
    ),
    ("overview", "Show workload overview"),
    (
        "prune",
        "Prune all unreferenced resources in Images/Volumes (confirmation required)",
    ),
    (
        "removal-results",
        "Review the last image/volume cleanup results",
    ),
    (
        "metrics",
        "Analyze the selected container's CPU, RAM, disk and network metrics (6)",
    ),
    (
        "images",
        "Switch to Images overview (Alt/Option+2; guarded unused-image removal)",
    ),
    (
        "volumes",
        "Switch to Volumes overview (Alt/Option+3; guarded unused-volume removal)",
    ),
    (
        "history",
        "Alias for metrics: live and historical container readings (6)",
    ),
    ("logs", "Open logs for the selected workload"),
    ("networks", "Show network membership"),
    ("refresh", "Refresh all engine snapshots"),
    (
        "reconnect-logs",
        "Reload the current logs now; streams also reconnect automatically",
    ),
    (
        "logs-since",
        "Load logs from a start time: 15m, 2h, 1d, RFC 3339, or off",
    ),
    ("wrap", "Toggle log wrapping (w in logs)"),
    ("json", "Pretty-print or restore JSON logs (J in logs)"),
    ("exec", "Open a shell in the selected container"),
    (
        "layers",
        "Inspect resolved image filesystem layers and build history (7)",
    ),
    ("reload-layers", "Retry image layers lookup (read-only)"),
    (
        "security",
        "Show the selected image's saved security report (8; z expands)",
    ),
    (
        "scan-image",
        "Scan checked images, or the selected image when none are checked",
    ),
    (
        "scan-images",
        "Scan checked images sequentially; continue after image errors",
    ),
    (
        "cancel-scan",
        "Cancel the security queue and remove its scanner",
    ),
    (
        "update-security-db",
        "Refresh scanner/database and rescan checked or selected images",
    ),
    (
        "actions",
        "Show contextual actions and disabled reasons (a)",
    ),
    ("close-exec", "Close the embedded shell (not the container)"),
    (
        "exec-sessions",
        "Switch retained Exec sessions (F6 in navigation)",
    ),
    (
        "filter-logs",
        "Edit retained-log JSON/source/severity/time filters (F3)",
    ),
    (
        "clear-log-filter",
        "Clear retained-log filters without changing search",
    ),
    (
        "export-logs",
        "Export matching retained logs to a new private JSONL file",
    ),
    (
        "check-updates",
        "Compare visible tagged images with their registries (U in Images)",
    ),
    (
        "pull-image",
        "Pull the selected image's tags; running containers are unchanged",
    ),
    (
        "events",
        "Show container starts, exits, OOM kills and health changes (5)",
    ),
    (
        "compose-update",
        "Pull newer images, then apply them with up -d (confirmed)",
    ),
    ("compose-pull", "Pull the project's images (confirmed)"),
    (
        "compose-up",
        "Apply Compose configuration with up -d (confirmed)",
    ),
    (
        "compose-recreate",
        "Recreate every container with --force-recreate (confirmed)",
    ),
    (
        "compose-down",
        "Remove containers and networks, keeping volumes (type name)",
    ),
    ("compose-output", "Show the last Compose operation's output"),
    ("performance", "Expand/collapse global performance (p)"),
    (
        "sort-contexts",
        "Sort contexts by connection, name, CPU or memory (o)",
    ),
    ("back", "Return to the previous workspace"),
    ("all", "Return to all runtimes"),
    ("monochrome", "Toggle terminal colors"),
    ("help", "Show keyboard shortcuts"),
    ("quit", "Exit Tha"),
];

fn accent(app: &App, color: Color) -> Style {
    if app.monochrome {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    }
}

fn muted() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}
fn selected() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

fn padded(area: Rect) -> Rect {
    area.inner(Margin {
        horizontal: if area.width >= 44 {
            2
        } else if area.width >= 12 {
            1
        } else {
            0
        },
        vertical: u16::from(area.height >= 10),
    })
}

fn condition_style(app: &App, condition: Condition) -> Style {
    match condition.tone {
        Tone::Good => accent(app, Color::Green),
        Tone::Warning => accent(app, Color::Yellow),
        Tone::Error => accent(app, Color::Red),
        Tone::Neutral => muted(),
    }
}

/// Cell ranges of `Tabs` titles: one space of padding each side and a one-cell divider.
fn tab_regions(area: Rect, titles: &[&str]) -> Vec<Rect> {
    let mut x = area.x;
    titles
        .iter()
        .map(|title| {
            let width = title.width() as u16 + 2;
            let region = Rect::new(x, area.y, width, 1).intersection(area);
            x = x.saturating_add(width + 1);
            region
        })
        .collect()
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.mouse_regions.clear();
    app.reconcile_detail_tab();
    app.action_review_visible = false;
    if let Some(removal) = app.inventory_removal.as_mut() {
        removal.visible = false;
    }
    app.log_export_review_visible = false;
    let area = frame.area();
    if area.width < 32 || area.height < 10 {
        frame.render_widget(
            Paragraph::new(
                "Tha\nTerminal too small. Resize to at least 32 × 10.\nPress q to quit.",
            ),
            area,
        );
        return;
    }
    let inventory_zoom = app.overview_mode != OverviewMode::Containers
        && app
            .inventory_views
            .get(&(app.inventory_engine, app.overview_mode))
            .is_some_and(|view| view.zoom);
    let layout = Layout::vertical([
        Constraint::Length(if inventory_zoom { 0 } else { 2 }),
        Constraint::Min(1),
        Constraint::Length(footer_height(app)),
    ])
    .split(area);
    let connected = app
        .engines
        .iter()
        .filter(|engine| {
            engine.snapshot.is_some() && engine.error.is_none() && engine.alias.is_none()
        })
        .count();
    let mut overview_x = layout[0].x + if app.demo { 15 } else { 8 };
    for (mode, label, available) in [
        (OverviewMode::Containers, "Containers", true),
        (OverviewMode::Images, "Images", true),
        (OverviewMode::Volumes, "Volumes", true),
        (OverviewMode::Security, "Security", app.security.enabled),
    ] {
        if !available || layout[0].height == 0 {
            continue;
        }
        let width = label.width() as u16 + if app.overview_mode == mode { 2 } else { 0 };
        app.mouse_regions.push((
            Rect::new(overview_x, layout[0].y, width, 1).intersection(layout[0]),
            MouseTarget::Overview(mode),
        ));
        overview_x += width + 3;
    }
    let header = Line::from(vec![
        Span::styled(" ⊤ Tha ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if app.demo { " Demo · " } else { " " }),
        Span::styled(
            format!(
                "{} · {} · {}{}  ",
                if app.overview_mode == OverviewMode::Containers {
                    "[Containers]"
                } else {
                    "Containers"
                },
                if app.overview_mode == OverviewMode::Images {
                    "[Images]"
                } else {
                    "Images"
                },
                if app.overview_mode == OverviewMode::Volumes {
                    "[Volumes]"
                } else {
                    "Volumes"
                },
                if app.security.enabled {
                    if app.overview_mode == OverviewMode::Security {
                        " · [Security]"
                    } else {
                        " · Security"
                    }
                } else {
                    ""
                }
            ),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{connected} connected / {} configured", app.engines.len()),
            muted(),
        ),
    ]);
    let breadcrumb = if app.overview_mode != OverviewMode::Containers {
        " Ctrl+←/→ or F7/F8 Overview · ←/→ Runtime · ↑/↓ Select · / Search · Enter Details".into()
    } else if let Some(project) = &app.project {
        format!(
            " All runtimes › {}{} › {}  · Backspace Back",
            safe_text(&app.engines[project.engine].connection.name),
            project
                .namespace
                .as_ref()
                .map(|ns| format!(" › {}", safe_text(ns)))
                .unwrap_or_default(),
            safe_text(&project.name)
        )
    } else {
        " All runtimes · Enter a Compose project to focus it".into()
    };
    frame.render_widget(
        Paragraph::new(vec![header, Line::styled(breadcrumb, muted())]),
        layout[0],
    );
    let security_zoom = app.overview_mode == OverviewMode::Containers
        && app.tab == Tab::Security
        && app.security.zoom;
    let history_zoom = app.overview_mode == OverviewMode::Containers
        && app.tab == Tab::History
        && app.history_view.expanded;
    let detail_zoom = app.overview_mode == OverviewMode::Containers
        && app.detail_focus
        && !app.performance_focus
        && app.detail_zoom == Some(app.tab);
    let explorer_zoom = app.explorer_zoomed();
    let expanded = app.overview_mode == OverviewMode::Containers
        && !explorer_zoom
        && !history_zoom
        && !security_zoom
        && !detail_zoom
        && app.performance_open
        && app.performance_expanded
        && layout[1].height >= 20;
    let body = if expanded {
        let parts = Layout::vertical([
            Constraint::Min(8),
            Constraint::Length((layout[1].height / 2).max(12)),
        ])
        .split(layout[1]);
        draw_performance_panel(frame, app, parts[1]);
        parts[0]
    } else {
        layout[1]
    };
    if app.overview_mode == OverviewMode::Security {
        security::draw_overview(frame, app, body);
    } else if app.overview_mode != OverviewMode::Containers {
        inventory::draw_overview(frame, app, body);
    } else if explorer_zoom {
        draw_tree(frame, app, body);
    } else if security_zoom {
        security::draw(frame, app, body);
    } else if history_zoom {
        history::draw(frame, app, body);
    } else if detail_zoom {
        draw_details(frame, app, body);
    } else if area.width >= 110 {
        let tree_width = (area.width / 2).clamp(52, 72);
        let panes =
            Layout::horizontal([Constraint::Length(tree_width), Constraint::Min(1)]).split(body);
        if expanded {
            draw_tree(frame, app, panes[0]);
        } else {
            draw_sidebar(frame, app, panes[0]);
        }
        draw_details(frame, app, panes[1]);
    } else if app.detail_focus {
        draw_details(frame, app, body);
    } else {
        if expanded {
            draw_tree(frame, app, body);
        } else {
            draw_sidebar(frame, app, body);
        }
    }
    draw_footer(frame, app, layout[2]);
    if app.help {
        draw_help(frame, app, area);
    }
    if app.input_mode == InputMode::Command {
        draw_palette(frame, app, area);
    }
    if matches!(app.input_mode, InputMode::LogFilter | InputMode::ExportLogs) {
        draw_log_editor(frame, app, area);
    }
    if app.picker.is_some() {
        draw_picker(frame, app, area);
    }
    if app.action_confirmation.is_some() || app.action_results_visible {
        draw_action(frame, app, area);
    }
    compose::draw(frame, app, area);
    inventory::draw_removal(frame, app, area);
    inventory::draw_removal_results(frame, app, area);
    if app.inventory_removal_results_visible
        || app
            .inventory_removal
            .as_ref()
            .is_some_and(|removal| !removal.dismissed)
    {
        app.action_review_visible = false;
        app.log_export_review_visible = false;
    }
}

fn draw_log_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    let exporting = app.input_mode == InputMode::ExportLogs;
    let review_visible = area.width >= 60 && area.height >= 18;
    let area = centered(area, 100, 20);
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(if exporting {
        " Export retained logs "
    } else {
        " Filter retained logs · JSON "
    });
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    if !review_visible {
        frame.render_widget(
            Paragraph::new("Resize to review this dialog (at least 60 × 18). Esc cancels.")
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    app.log_export_review_visible = exporting;
    let parts = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(1),
        Constraint::Length(3),
        Constraint::Length(2),
    ])
    .split(inner);
    let hint = if exporting {
        let matched = app
            .logs
            .entries
            .iter()
            .filter(|entry| app.log_view.filter.matches(entry))
            .count();
        format!(
            "{matched} matching / {} retained entries in the current scope.\nLogs may contain SECRETS. Choose an explicit destination filename.\nNew JSONL file only; mode 0600; existing files/symlinks refused.\nNo history fetch, no automatic redaction, no silent truncation.\nSnapshot limit: 16 MiB; destination directory must already exist.",
            app.logs.entries.len()
        )
    } else {
        "{} clears filters. Supported keys: source, level, min_level, fields, since, until.\nExample: {\"source\":\"api\",\"min_level\":\"warn\",\"fields\":{\"/http/status\":500}}\nTime bounds: RFC3339, e.g. \"since\":\"2026-10-29T10:00:00Z\".\nMissing required metadata does not match. Search remains separate.\nOnly retained entries are filtered; no older logs are fetched.".into()
    };
    frame.render_widget(Paragraph::new(hint).wrap(Wrap { trim: false }), parts[0]);
    let input = Paragraph::new(format!("> {}▏", safe_text(&app.input))).wrap(Wrap { trim: false });
    let scroll = input
        .line_count(parts[1].width)
        .saturating_sub(parts[1].height as usize)
        .min(u16::MAX as usize);
    frame.render_widget(input.scroll((scroll as u16, 0)), parts[1]);
    frame.render_widget(
        Paragraph::new(safe_text(&app.notice))
            .wrap(Wrap { trim: false })
            .style(accent(app, Color::Yellow)),
        parts[2],
    );
    frame.render_widget(
        Paragraph::new(if exporting {
            "Enter Export snapshot · Esc Cancel · Ctrl-u Clear"
        } else {
            "Enter Apply · Esc Cancel · Ctrl-u Clear; paste appends JSON"
        })
        .wrap(Wrap { trim: false }),
        parts[3],
    );
}

fn draw_action(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.action_confirmation.is_some() {
        draw_action_confirmation(frame, app, area);
        return;
    }
    use crate::actions::{Action, Status};
    let dialog = centered(area, 100, area.height.saturating_sub(2).min(28));
    frame.render_widget(Clear, dialog);
    if area.width < 60 || area.height < 18 {
        frame.render_widget(Paragraph::new("Resize to at least 60 × 18 to review lifecycle targets.\nEsc cancels confirmation; running actions continue.").wrap(Wrap { trim: false }).block(Block::bordered().title(" Lifecycle ")), dialog);
        return;
    }
    let Some(progress) = app.action_progress() else {
        return;
    };
    let request = &progress.request;
    let scroll = progress.scroll;
    let title = format!(
        " {} · {} ",
        request.action.label(),
        if progress.finished {
            "Results"
        } else {
            "Running"
        }
    );
    let block = Block::bordered()
        .title(title)
        .title_bottom(" ↑/↓ PgUp/PgDn Review targets ");
    let inner = padded(block.inner(dialog));
    frame.render_widget(block, dialog);
    let parts = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Runtime: {}", safe_text(&request.connection.name))),
            Line::from(if request.runtime.is_some() {
                format!("{} RUNTIME · ALL workloads", request.action.label())
            } else {
                format!(
                    "{}: {} · {} target(s), {} skipped",
                    request.action.label(),
                    safe_text(&request.name),
                    request.target_count(),
                    request.skipped
                )
            }),
        ]),
        parts[0],
    );
    let mut lines = vec![
        Line::from(format!(
            "Endpoint: {}",
            if request.connection.endpoint.is_empty() {
                "not currently available".into()
            } else {
                safe_text(&request.connection.endpoint)
            }
        )),
        Line::from(format!(
            "{}: {}",
            if request.runtime.is_some() {
                "Provider target"
            } else {
                "Runtime ID"
            },
            safe_text(&request.identity)
        )),
    ];
    if let Some(target) = &request.runtime {
        lines.push(Line::styled(
            "RUNTIME-WIDE: affects ALL workloads, including outside the focused project.",
            accent(app, Color::Yellow),
        ));
        if let Ok(commands) = target.commands(request.action) {
            for (program, args) in commands {
                lines.push(Line::from(format!("Command: {program} {}", args.join(" "))));
            }
        }
        lines.push(Line::from(
            "Runtime configuration is retained; stopped runtimes remain (offline).",
        ));
        lines.push(Line::from(safe_text(
            &progress.results[0]
                .as_ref()
                .map(Status::text)
                .unwrap_or_else(|| "Pending / checking runtime".into()),
        )));
    } else {
        lines.push(Line::styled(
            "Existing containers only; no Compose dependency orchestration.",
            muted(),
        ));
    }
    if request.action == Action::Delete {
        lines.push(Line::styled(
            "Container writable data is lost. Networks, ALL volumes and images stay.",
            accent(app, Color::Yellow),
        ));
    } else if request.runtime.is_none() && matches!(request.action, Action::Stop | Action::Restart)
    {
        lines.push(Line::styled(
            "10s stop grace period, then runtime may kill the process.",
            accent(app, Color::Yellow),
        ));
    }
    for (index, container) in request.containers.iter().enumerate() {
        lines.push(Line::from(format!(
            "{} · {}",
            index + 1,
            safe_text(&container.name)
        )));
        lines.push(Line::styled(
            format!("  ID: {}", safe_text(&container.id)),
            muted(),
        ));
        let (text, style) = match &progress.results[index] {
            Some(Status::Succeeded) => ("Succeeded".into(), accent(app, Color::Green)),
            Some(status @ Status::Failed(_)) => (status.text(), accent(app, Color::Red)),
            Some(status) => (status.text(), accent(app, Color::Yellow)),
            None => ("Pending / checking safety".into(), muted()),
        };
        lines.push(Line::styled(safe_text(&text), style));
        lines.push(Line::from(""));
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(parts[1].width)
        .saturating_sub(parts[1].height as usize);
    let scroll = scroll.min(max_scroll).min(u16::MAX as usize);
    frame.render_widget(paragraph.scroll((scroll as u16, 0)), parts[1]);
    let prompt = if progress.finished {
        "Enter/Esc Close · Inventory refresh requested\nNo automatic retry or rollback; inspect unknown outcomes."
    } else {
        "Enter/Esc Close · Actions continue in the background.\nQuitting cannot undo a request already sent to the runtime."
    };
    frame.render_widget(Paragraph::new(prompt).wrap(Wrap { trim: false }), parts[2]);
    app.action_progress_mut().unwrap().scroll = scroll;
}

fn draw_action_confirmation(frame: &mut Frame, app: &mut App, area: Rect) {
    use crate::actions::Action;
    if area.width < 44 || area.height < 10 {
        let dialog = centered(area, 56, 5);
        frame.render_widget(Clear, dialog);
        frame.render_widget(
            Paragraph::new("Resize to at least 44 × 10 to confirm.\nEsc Cancel")
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" Confirm action ")),
            dialog,
        );
        return;
    }
    let confirmation = app.action_confirmation.as_ref().unwrap();
    let request = &confirmation.request;
    let runtime_name = Line::from(format!("Runtime: {}", safe_text(&request.connection.name)));
    let mut lines = Vec::new();
    if request.runtime.is_some() {
        lines.push(Line::styled(
            match request.action {
                Action::Start => "Starts this runtime; workloads may auto-start.",
                Action::Stop => "Stops this runtime and all workloads.",
                _ => "Restarts this runtime and all workloads.",
            },
            accent(app, Color::Yellow),
        ));
        if matches!(request.runtime, Some(crate::runtime::Target::OrbStack)) {
            lines.push(Line::from("Includes all OrbStack Linux machines."));
        }
        lines.push(runtime_name);
    } else {
        if request.action == Action::Delete {
            lines.push(Line::styled(
                "Writable data is lost; all volumes/images stay.",
                accent(app, Color::Yellow),
            ));
        }
        lines.push(runtime_name);
        lines.push(Line::from(format!(
            "{} target(s), {} skipped",
            request.target_count(),
            request.skipped
        )));
        for container in &request.containers {
            lines.push(Line::from(format!("• {}", safe_text(&container.name))));
        }
    }
    let title = format!(" {} {}? ", request.action.label(), safe_text(&request.name));
    let hint = if request.action == Action::Delete {
        format!(
            "Type {} then Enter to DELETE\n> {} · Esc Cancel",
            safe_text(&request.name),
            safe_text(&confirmation.typed)
        )
    } else {
        "y / Enter Confirm    n / Esc Cancel".into()
    };
    let mut prompt = hint;
    if let Some(error) = &confirmation.error {
        prompt.push_str(&format!("\n{}", safe_text(error)));
    }
    let width = area.width.saturating_sub(2).min(56);
    let content_width = width.saturating_sub(4);
    if UnicodeWidthStr::width(title.as_str()) > width.saturating_sub(2) as usize {
        lines.push(Line::from(format!("Target: {}", safe_text(&request.name))));
    }
    let body = Paragraph::new(lines).wrap(Wrap { trim: false });
    let footer = Paragraph::new(prompt).wrap(Wrap { trim: false });
    let footer_height = footer.line_count(content_width).min(4) as u16;
    let desired = (body.line_count(content_width).min(9) as u16)
        .saturating_add(footer_height)
        .saturating_add(3);
    let dialog = centered(
        area,
        width,
        desired.min(12).min(area.height.saturating_sub(2)),
    );
    let block = Block::bordered().title(ellipsize_line(
        Line::from(title),
        dialog.width.saturating_sub(2) as usize,
    ));
    let inner = block
        .inner(dialog)
        .inner(ratatui::layout::Margin::new(1, 0));
    frame.render_widget(Clear, dialog);
    frame.render_widget(block, dialog);
    let parts = Layout::vertical([
        Constraint::Min(2),
        Constraint::Length(1),
        Constraint::Length(footer_height),
    ])
    .split(inner);
    let max_scroll = body
        .line_count(parts[0].width)
        .saturating_sub(parts[0].height as usize);
    let scroll = confirmation.scroll.min(max_scroll).min(u16::MAX as usize);
    frame.render_widget(body.scroll((scroll as u16, 0)), parts[0]);
    if max_scroll > 0 {
        frame.render_widget(
            Paragraph::new("↑/↓ Review targets").style(muted()),
            parts[1],
        );
    }
    frame.render_widget(footer, parts[2]);
    let confirmation = app.action_confirmation.as_mut().unwrap();
    confirmation.scroll = scroll;
    app.action_review_visible = parts[0].height >= 2 && parts[2].height == footer_height;
}

fn performance_height(app: &App, available: u16) -> u16 {
    if app.performance_open && available >= 12 {
        let count = if app.performance_scope == PerformanceScope::Container {
            1
        } else {
            app.performance_engines().len().min(3) as u16
        };
        if count == 0 {
            return 6;
        }
        (count + 13).max(6).min((available / 2).clamp(6, 18))
    } else {
        1
    }
}

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let height = performance_height(app, area.height);
    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(height)]).split(area);
    draw_tree(frame, app, parts[0]);
    if height == 1 {
        app.mouse_regions
            .push((parts[1], MouseTarget::CollapsedPerformance));
        frame.render_widget(
            Paragraph::new(if app.performance_scope == PerformanceScope::Container {
                " ▸ Performance · Selected container (p/P)"
            } else {
                " ▸ Performance · Connected runtimes (p/P)"
            })
            .style(if app.performance_focus {
                selected()
            } else {
                muted()
            }),
            parts[1],
        );
        return;
    }
    draw_performance_panel(frame, app, parts[1]);
}

fn draw_performance_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = pane(
        app,
        if app.performance_scope == PerformanceScope::Container {
            " ▾ Performance · Selected container · P Runtime ".into()
        } else {
            " ▾ Performance · Connected runtimes · P Container ".into()
        },
        app.performance_focus,
    )
    .title_bottom(" j/k Select · Enter Inspect · z Zoom · p Close ");
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    app.mouse_regions.push((area, MouseTarget::Performance));
    if app.performance_scope == PerformanceScope::Container {
        if let Some((index, _)) = app.performance_container() {
            draw_performance_history(frame, app, index, inner);
        } else {
            frame.render_widget(
                Paragraph::new("Select a container in Explorer · P Runtime")
                    .style(muted())
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }
        return;
    }
    let engines = app.performance_engines();
    app.performance_scroll = app.performance_scroll.min(engines.len().saturating_sub(1));
    if engines.is_empty() {
        app.performance_offset = 0;
        frame.render_widget(
            Paragraph::new("No connected runtimes")
                .style(muted())
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    if inner.height >= 9 && inner.width >= 26 {
        let index = engines[app.performance_scroll];
        let graph_height = inner
            .height
            .saturating_sub(engines.len().saturating_sub(1).min(2) as u16);
        draw_performance_history(
            frame,
            app,
            index,
            Rect::new(inner.x, inner.y, inner.width, graph_height),
        );
        let others: Vec<_> = engines
            .iter()
            .copied()
            .filter(|&other| other != index)
            .collect();
        let capacity = inner.height.saturating_sub(graph_height) as usize;
        app.performance_offset = app.performance_scroll.saturating_sub(capacity);
        for (offset, &other) in others
            .iter()
            .skip(
                app.performance_offset
                    .min(others.len().saturating_sub(capacity)),
            )
            .take(capacity)
            .enumerate()
        {
            if let Some(position) = engines.iter().position(|&engine| engine == other) {
                app.mouse_regions.push((
                    Rect::new(
                        inner.x,
                        inner.y + graph_height + offset as u16,
                        inner.width,
                        1,
                    ),
                    MouseTarget::PerformanceEngine(position),
                ));
            }
            draw_usage(
                frame,
                app,
                other,
                Rect::new(
                    inner.x,
                    inner.y + graph_height + offset as u16,
                    inner.width,
                    1,
                ),
            );
        }
        return;
    }
    let item_height = if inner.height >= 4 { 4 } else { 1 };
    let capacity = (inner.height / item_height).max(1) as usize;
    app.performance_offset = app
        .performance_offset
        .min(engines.len().saturating_sub(capacity));
    if app.performance_scroll < app.performance_offset {
        app.performance_offset = app.performance_scroll;
    } else if app.performance_scroll >= app.performance_offset + capacity {
        app.performance_offset = app.performance_scroll + 1 - capacity;
    }
    let mut y = inner.y;
    for (position, &index) in engines
        .iter()
        .enumerate()
        .skip(app.performance_offset)
        .take(capacity)
    {
        if y + item_height > inner.bottom() {
            break;
        }
        app.mouse_regions.push((
            Rect::new(inner.x, y, inner.width, item_height),
            MouseTarget::PerformanceEngine(position),
        ));
        draw_usage(
            frame,
            app,
            index,
            Rect::new(inner.x, y, inner.width, item_height),
        );
        y += item_height;
    }
}

fn pane(app: &App, title: String, focused: bool) -> Block<'static> {
    let style = if focused {
        accent(app, Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        muted()
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            ratatui::widgets::BorderType::Thick
        } else {
            ratatui::widgets::BorderType::Plain
        })
        .title(title)
        .title_style(style)
        .border_style(style)
}

/// Truncate styled text by terminal cells, without splitting a grapheme cluster.
fn ellipsize_line(mut line: Line<'_>, width: usize) -> Line<'_> {
    if line.width() <= width {
        return line;
    }
    let original = std::mem::take(&mut line.spans);
    if width == 0 {
        return line;
    }
    let mut remaining = width - UnicodeWidthStr::width("…");
    for span in original {
        let mut end = 0;
        for (offset, grapheme) in span.content.grapheme_indices(true) {
            let cells = UnicodeWidthStr::width(grapheme);
            if cells > remaining {
                if end > 0 {
                    line.spans
                        .push(Span::styled(span.content[..end].to_owned(), span.style));
                }
                line.spans.push(Span::styled("…", span.style));
                return line;
            }
            remaining -= cells;
            end = offset + grapheme.len();
        }
        line.spans.push(span);
    }
    line
}

fn draw_tree(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = pane(
        app,
        if app.tree_query.is_empty() {
            " Explorer ".into()
        } else {
            format!(" Explorer / {} ", safe_text(&app.tree_query))
        },
        !app.detail_focus && !app.performance_focus,
    );
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    app.mouse_regions.push((area, MouseTarget::Explorer));
    let show_metrics = inner.width >= 44;
    // Match the table's fixed metric columns (9 + 11 cells) and two 1-cell gaps.
    let workload_width = if show_metrics {
        inner.width.saturating_sub(22)
    } else {
        inner.width
    };
    let height = inner.height.saturating_sub(u16::from(show_metrics)) as usize;
    let now = std::time::Instant::now();
    if app.selected < app.tree_offset {
        app.tree_offset = app.selected;
    }
    if app.selected >= app.tree_offset + height {
        app.tree_offset = app.selected.saturating_sub(height.saturating_sub(1));
    }
    app.prepare_row_presentations(app.tree_offset, height, now, show_metrics);
    let rows: Vec<_> = app
        .rows
        .iter()
        .enumerate()
        .skip(app.tree_offset)
        .take(height)
        .map(|(i, row)| {
            let arrow = if row.branch {
                if app.expanded.contains(&row.key) || !app.tree_query.is_empty() {
                    "▾ "
                } else {
                    "▸ "
                }
            } else {
                "  "
            };
            let presentation = app.row_presentation(&row.key);
            let condition = presentation.condition;
            let feedback = app.row_action_feedback(row, now);
            let feedback_style = feedback
                .as_ref()
                .map(|feedback| condition_style(app, Condition::new(feedback.tone, "")));
            let style = if i == app.selected {
                selected()
            } else if row.running == Some(false)
                && condition.is_some_and(|state| state.tone == Tone::Neutral)
            {
                muted()
            } else {
                Style::default()
            };
            let mut name = vec![Span::raw(format!("{}{}", "  ".repeat(row.depth), arrow))];
            if let Some(feedback) = feedback.as_ref().filter(|feedback| feedback.active) {
                name.push(Span::styled(
                    format!("{} ", feedback.symbol),
                    feedback_style.unwrap(),
                ));
            } else if let Some(condition) = condition {
                name.push(Span::styled(
                    format!("{} ", condition.symbol()),
                    condition_style(app, condition).remove_modifier(Modifier::REVERSED),
                ));
            }
            name.push(Span::raw(safe_text(&row.title)));
            if let Some(condition) = condition.filter(|condition| {
                matches!(condition.tone, Tone::Error | Tone::Warning)
                    && !matches!(condition.label, "errors" | "workload issues")
                    && (!matches!(row.scope, Scope::Engine)
                        || app.engines[row.engine].error.is_none()
                            && app.engines[row.engine].snapshot.is_some())
            }) {
                name.push(Span::styled(
                    format!(" [{}]", condition.label),
                    condition_style(app, condition),
                ));
            }
            let width = workload_width as usize;
            let label = if let Some(feedback) = feedback {
                let suffix = Line::from(Span::styled(
                    if feedback.active {
                        format!(" · {}", feedback.label)
                    } else {
                        format!(" · {} {}", feedback.symbol, feedback.label)
                    },
                    feedback_style.unwrap(),
                ));
                // Reserve room for the action even when the workload name is long.
                let name_width = width.saturating_sub(suffix.width()).max(width.min(8));
                let mut label = ellipsize_line(Line::from(name), name_width);
                let suffix = ellipsize_line(suffix, width.saturating_sub(label.width()));
                label.spans.extend(suffix.spans);
                label
            } else {
                ellipsize_line(Line::from(name), width)
            };
            let mut cells = vec![Cell::from(label)];
            if show_metrics {
                let (cpu, memory) = (presentation.cpu.clone(), presentation.memory.clone());
                cells.push(Cell::from(Line::from(cpu).alignment(Alignment::Right)));
                cells.push(Cell::from(Line::from(memory).alignment(Alignment::Right)));
            }
            TableRow::new(cells).style(style)
        })
        .collect();
    let first_row = inner.y + u16::from(show_metrics);
    for (offset, row) in app
        .rows
        .iter()
        .skip(app.tree_offset)
        .take(height)
        .enumerate()
    {
        let index = app.tree_offset + offset;
        let y = first_row + offset as u16;
        app.mouse_regions.push((
            Rect::new(inner.x, y, workload_width, 1),
            MouseTarget::TreeRow(index),
        ));
        if row.branch {
            app.mouse_regions.push((
                Rect::new(inner.x + row.depth as u16 * 2, y, 2, 1).intersection(inner),
                MouseTarget::TreeArrow(index),
            ));
        }
    }
    if app.rows.is_empty() {
        frame.render_widget(
            Paragraph::new("No matching workloads. Esc clears search."),
            inner,
        );
        return;
    }
    if show_metrics {
        let header = TableRow::new([
            Cell::from("Workload"),
            Cell::from(Line::from("CPU").alignment(Alignment::Right)),
            Cell::from(Line::from("Mem").alignment(Alignment::Right)),
        ])
        .style(muted());
        frame.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Min(1),
                    Constraint::Length(9),
                    Constraint::Length(11),
                ],
            )
            .column_spacing(1)
            .header(header),
            inner,
        );
    } else {
        frame.render_widget(Table::new(rows, [Constraint::Min(1)]), inner);
    }
}

fn draw_details(frame: &mut Frame, app: &mut App, area: Rect) {
    let label = match app.tab {
        Tab::Overview => "1 Overview",
        Tab::Logs => "2 Logs",
        Tab::Networks => "3 Networks",
        Tab::Exec => "4 Exec",
        Tab::Events => "5 Events",
        Tab::History => "6 Metrics",
        Tab::Layers => "7 Layers",
        Tab::Security => "8 Security",
    };
    let exec_context = if app.tab == Tab::Exec {
        app.sessions
            .active()
            .map(|session| {
                format!(
                    " #{} · {}",
                    app.sessions.active_id().unwrap_or(0),
                    safe_text(&session.request.connection.name)
                )
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let block = pane(app, format!(" {label}{exec_context} "), app.detail_focus);
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    app.mouse_regions.push((area, MouseTarget::Details));
    if inner.height < 3 || inner.width < 2 || app.tab == Tab::Exec && inner.height < 4 {
        return;
    }
    let sections = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if app.tab == Tab::Exec { 2 } else { 1 }),
        Constraint::Min(1),
    ])
    .split(inner);
    let (mut names, exec_name) = if inner.width >= 82 {
        (vec!["1 Overview", "2 Logs", "3 Networks"], "4 Exec")
    } else if inner.width >= 70 {
        (vec!["1 Info", "2 Logs", "3 Net"], "4 Exec")
    } else {
        (vec!["1", "2", "3"], "4")
    };
    let mut bar_tabs = vec![Tab::Overview, Tab::Logs, Tab::Networks];
    let exec_index = names.len();
    if app.exec_tab_available() {
        names.push(exec_name);
        bar_tabs.push(Tab::Exec);
    }
    bar_tabs.push(Tab::Events);
    names.push(if inner.width >= 56 { "5 Events" } else { "5" });
    let events_index = names.len() - 1;
    let metrics_index = names.len();
    if app.performance_container().is_some() {
        names.push(if inner.width >= 44 { "6 Metrics" } else { "6" });
        bar_tabs.push(Tab::History);
    }
    let layers_index = names.len();
    if app.layers_available() {
        names.push(if inner.width >= 44 { "7 Layers" } else { "7" });
        bar_tabs.push(Tab::Layers);
    }
    let security_index = names.len();
    if app.security_tab_available() {
        names.push(if inner.width >= 44 { "8 Security" } else { "8" });
        bar_tabs.push(Tab::Security);
    }
    for (region, tab) in tab_regions(sections[0], &names).into_iter().zip(bar_tabs) {
        app.mouse_regions
            .push((region, MouseTarget::DetailTab(tab)));
    }
    let active = match app.tab {
        Tab::Overview => 0,
        Tab::Logs => 1,
        Tab::Networks => 2,
        Tab::Exec => exec_index,
        Tab::Layers => layers_index,
        Tab::History => metrics_index,
        Tab::Security => security_index,
        Tab::Events => events_index,
    };
    frame.render_widget(
        Tabs::new(names)
            .select(active)
            .highlight_style(selected().add_modifier(Modifier::BOLD)),
        sections[0],
    );
    let context = if app.tab == Tab::Exec {
        app.sessions
            .active()
            .map(|session| {
                format!(
                    "{}{}\n{} · {}",
                    session
                        .request
                        .container
                        .namespace
                        .as_ref()
                        .map(|ns| format!("{}/", safe_text(ns)))
                        .unwrap_or_default(),
                    safe_text(&session.request.container.name),
                    if app.exec_input {
                        "SHELL INPUT"
                    } else if session.running() {
                        "NAVIGATION"
                    } else {
                        "OUTPUT RETAINED"
                    },
                    safe_text(&session.status()),
                )
            })
            .unwrap_or_else(|| "No session\nNAVIGATION · Select a container and press e".into())
    } else {
        format!(
            "{} · {}",
            app.current()
                .map(|row| safe_text(&row.title))
                .unwrap_or_else(|| "No selection".into()),
            if app.detail_focus {
                "Focused"
            } else {
                "Navigation"
            }
        )
    };
    frame.render_widget(
        Paragraph::new(
            context
                .lines()
                .map(|line| Line::from(safe_text(line)))
                .collect::<Vec<_>>(),
        )
        .style(muted()),
        sections[1],
    );
    let body = sections[2];
    app.exec_dimensions = (body.height, body.width);
    if let Some(session) = app.sessions.active_mut()
        && let Err(error) = session.resize(body.height, body.width)
    {
        app.notice = format!("Exec resize failed: {error:#}");
    }
    match app.tab {
        Tab::Logs => draw_logs(frame, app, body),
        Tab::Exec => draw_exec(frame, app, body),
        Tab::Layers => draw_layers(frame, app, body),
        Tab::History => history::draw(frame, app, body),
        Tab::Events => events::draw(frame, app, body),
        Tab::Security => security::draw_report(frame, app, body),
        Tab::Networks => draw_networks(frame, app, body),
        Tab::Overview => {
            let paragraph = Paragraph::new(overview_lines(app)).wrap(Wrap { trim: false });
            let maximum = paragraph
                .line_count(body.width)
                .saturating_sub(body.height as usize);
            app.detail_scroll = app.detail_scroll.min(maximum).min(u16::MAX as usize);
            frame.render_widget(paragraph.scroll((app.detail_scroll as u16, 0)), body);
        }
    }
}

fn draw_layers(frame: &mut Frame, app: &mut App, area: Rect) {
    use crate::layers::State;
    let image = match &app.layers.state {
        State::Idle => {
            frame.render_widget(Paragraph::new("Select one connected container or a one-container service.\nRead-only image metadata; no pulls, extraction, or builds.").wrap(Wrap { trim:false }), area);
            return;
        }
        State::Loading => {
            frame.render_widget(
                Paragraph::new("Loading resolved image layers… (read-only)"),
                area,
            );
            return;
        }
        State::Failed(error) => {
            frame.render_widget(
                Paragraph::new(format!(
                    "Layers unavailable: {}\n:reload-layers retries; no image will be pulled.",
                    safe_text(error)
                ))
                .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        State::Ready(image) => image,
    };
    if !app.layers.expanded {
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
        frame.render_widget(
            Paragraph::new(ellipsize_line(
                Line::from(format!("Image: {}", image.id)),
                area.width as usize,
            ))
            .style(muted()),
            parts[0],
        );
        frame.render_widget(Paragraph::new(format!("Filesystem layers ({}) · Build history ({})\nBase → top · separate sections, not a guessed layer/step mapping",image.filesystem.len(),image.history.len())).style(muted()),parts[1]);
        let mut rows = Vec::new();
        for (index, id) in image.filesystem.iter().enumerate() {
            rows.push(TableRow::new(vec![
                Cell::from((index + 1).to_string()),
                Cell::from("Filesystem"),
                Cell::from("unknown"),
                Cell::from(ellipsize_line(
                    Line::from(safe_text(id)),
                    parts[2].width.saturating_sub(32) as usize,
                )),
            ]));
        }
        for (index, step) in image.history.iter().enumerate() {
            rows.push(TableRow::new(vec![
                Cell::from((index + 1).to_string()),
                Cell::from(if step.empty_layer == Some(true) {
                    "Metadata"
                } else {
                    "Build step"
                }),
                Cell::from(
                    step.size
                        .map(metrics::bytes)
                        .unwrap_or_else(|| "unknown".into()),
                ),
                Cell::from("instruction hidden · Enter details"),
            ]));
        }
        app.layers.selected = app.layers.selected.min(image.rows().saturating_sub(1));
        let mut state = ratatui::widgets::TableState::default()
            .with_offset(app.layers.offset)
            .with_selected((image.rows() > 0).then_some(app.layers.selected));
        frame.render_stateful_widget(
            Table::new(
                rows,
                [
                    Constraint::Length(3),
                    Constraint::Length(10),
                    Constraint::Length(10),
                    Constraint::Min(1),
                ],
            )
            .header(
                TableRow::new(["#", "Kind", "Size", "Diff ID / instruction"])
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            )
            .column_spacing(1)
            .row_highlight_style(selected())
            .highlight_symbol("▸ "),
            parts[2],
            &mut state,
        );
        app.layers.offset = state.offset();
        for offset in 0..parts[2].height.saturating_sub(1) {
            let index = app.layers.offset + offset as usize;
            if index < image.rows() {
                app.mouse_regions.push((
                    Rect::new(parts[2].x, parts[2].y + 1 + offset, parts[2].width, 1),
                    MouseTarget::LayerRow(index),
                ));
            }
        }
        frame.render_widget(
            Paragraph::new("j/k Select · Enter Details · :reload-layers Retry").style(muted()),
            parts[3],
        );
        return;
    }
    let mut lines = vec![
        Line::styled(
            "Layer details",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from(format!("Resolved image: {}", image.id)),
    ];
    if app.layers.expanded {
        lines.push(Line::styled(
            "Enter/Esc Back · j/k/PgUp/PgDn Scroll",
            muted(),
        ));
        if let Some(id) = image.filesystem.get(app.layers.selected) {
            lines.push(Line::from(format!("Filesystem diff ID: {id}")));
            lines.push(Line::from(
                "Uncompressed diff ID; size and build-step association are unavailable.",
            ));
        } else if let Some(step) = image
            .history
            .get(app.layers.selected.saturating_sub(image.filesystem.len()))
        {
            lines.push(Line::styled(
                "Build history may contain secrets. Instructions are never executed.",
                accent(app, Color::Yellow),
            ));
            lines.push(Line::from(match step.empty_layer {
                Some(true) => "Explicit metadata-only step",
                Some(false) => "Explicit filesystem-producing step",
                None => "Filesystem association unavailable (not inferred from size)",
            }));
            lines.push(Line::from(format!(
                "Reported history size: {}",
                step.size
                    .map(metrics::bytes)
                    .unwrap_or_else(|| "unavailable".into())
            )));
            lines.push(Line::from("Instruction:"));
            lines.push(Line::from(step.instruction.clone()));
            if !step.comment.is_empty() {
                lines.push(Line::from("Comment:"));
                lines.push(Line::from(step.comment.clone()));
            }
        }
    }
    for warning in &image.warnings {
        lines.push(Line::styled(warning.clone(), accent(app, Color::Yellow)));
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    app.detail_scroll = app
        .detail_scroll
        .min(
            paragraph
                .line_count(area.width)
                .saturating_sub(area.height as usize),
        )
        .min(u16::MAX as usize);
    frame.render_widget(paragraph.scroll((app.detail_scroll as u16, 0)), area);
}

fn draw_exec(frame: &mut Frame, app: &App, area: Rect) {
    let Some(session) = app.sessions.active() else {
        frame.render_widget(Paragraph::new("Embedded container shell\n\nSelect a running container or service and press e.\nCtrl-] returns keyboard focus to Tha without closing the shell.\nThe session target stays fixed when you browse other containers.").wrap(Wrap { trim:false }), area);
        return;
    };
    let screen = session.screen();
    frame.render_widget(
        tui_term::widget::PseudoTerminal::new(screen)
            .cursor(tui_term::widget::Cursor::default().visibility(false)),
        area,
    );
    if app.monochrome {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                frame.buffer_mut()[(x, y)]
                    .set_fg(Color::Reset)
                    .set_bg(Color::Reset);
            }
        }
    }
    if app.exec_input
        && app.detail_focus
        && session.running()
        && !screen.hide_cursor()
        && screen.scrollback() == 0
    {
        let (row, col) = screen.cursor_position();
        if row < area.height && col < area.width {
            frame.set_cursor_position((area.x + col, area.y + row));
        }
    }
}

fn draw_performance_history(frame: &mut Frame, app: &App, index: usize, area: Rect) {
    let engine = &app.engines[index];
    let Some(snapshot) = &engine.snapshot else {
        return;
    };
    let now = std::time::Instant::now();
    let container = if app.performance_scope == PerformanceScope::Container {
        app.performance_container().map(|(_, container)| container)
    } else {
        None
    };
    let containers = container
        .as_ref()
        .map(std::slice::from_ref)
        .unwrap_or(&snapshot.containers);
    let summary = if container.is_some() {
        engine.metrics.summary(
            containers,
            now,
            app.metrics_stale_after(),
            engine.error.is_none() && engine.alias.is_none(),
        )
    } else {
        app.engine_metrics(index)
    };
    let title = if let Some(container) = &container {
        format!(
            "Container · {} · {}",
            safe_text(&container.name),
            safe_text(&engine.connection.name)
        )
    } else {
        safe_text(&engine.connection.name)
    };
    let capacity = if container.is_some() {
        summary.memory_limit_bytes
    } else {
        snapshot.memory_capacity_bytes
    }
    .filter(|&limit| limit > 0);
    let points = if container.is_some() {
        engine.metrics.timed_history(containers)
    } else {
        engine.metrics.timed_history(
            snapshot
                .containers
                .iter()
                .filter(|container| container.running()),
        )
    };
    let end = points.last().map(|point| point.sampled_at).unwrap_or(now);
    let span = points
        .first()
        .map(|point| end.saturating_duration_since(point.sampled_at))
        .unwrap_or_default()
        .max(std::time::Duration::from_secs(
            app.metrics_refresh_seconds
                .max(1)
                .saturating_mul((metrics::HISTORY_LENGTH - 1) as u64),
        ));
    let (cpu, memory) = history_columns(&points, area.width as usize, span);
    if area.height < 8 {
        let lines = vec![
            Line::from(title),
            Line::from(format!(
                "CPU {} · RAM {}",
                summary.cpu_label(),
                summary.memory_label()
            )),
            Line::styled("z Zoom · more room for history", muted()),
        ];
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(format!("{} · CPU {}", title, summary.cpu_label())).style(
            if app.performance_focus {
                selected()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            },
        ),
        parts[0],
    );
    let cpu_max = cpu.iter().flatten().copied().max().unwrap_or(0).max(10_000);
    frame.render_widget(
        Paragraph::new(format!(
            "CPU {} · scale {:.0}%",
            summary.cpu_label(),
            cpu_max as f64 / 100.0
        ))
        .style(accent(app, Color::Cyan)),
        parts[1],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "Memory {}{}",
            summary.memory_label(),
            capacity
                .map(|limit| format!(" / {}", metrics::bytes(limit)))
                .unwrap_or_default()
        ))
        .style(accent(app, Color::Green)),
        parts[3],
    );
    if let Some((used, limit)) = summary.memory_bytes.zip(capacity) {
        frame.render_widget(
            Gauge::default()
                .ratio((used as f64 / limit as f64).clamp(0.0, 1.0))
                .label(if container.is_some() {
                    "Container usage / memory limit"
                } else {
                    "Container usage / runtime capacity"
                })
                .gauge_style(accent(app, Color::Green)),
            parts[4],
        );
    } else {
        frame.render_widget(
            Paragraph::new("Capacity or live usage unavailable").style(muted()),
            parts[4],
        );
    }
    for (values, maximum, color, graph) in [
        (&cpu, cpu_max, Color::Cyan, parts[2]),
        (
            &memory,
            capacity
                .unwrap_or(0)
                .max(memory.iter().flatten().copied().max().unwrap_or(1))
                .max(1),
            Color::Green,
            parts[5],
        ),
    ] {
        frame.render_widget(
            Sparkline::default()
                .data(values)
                .max(maximum)
                .style(accent(app, color))
                .absent_value_style(muted())
                .absent_value_symbol("·"),
            graph,
        );
    }
    frame.render_widget(
        Paragraph::new(history_axis(area.width as usize, span)).style(muted()),
        parts[6],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "{} · {}s target · latest {}s ago · · gap{}",
            if container.is_some() {
                "Container"
            } else {
                "Container totals"
            },
            app.metrics_refresh_seconds,
            now.saturating_duration_since(end).as_secs(),
            if engine.error.is_some() {
                " · offline"
            } else {
                ""
            }
        ))
        .style(muted()),
        parts[7],
    );
}

/// Plot by elapsed time, not by frame index. Empty buckets stay gaps.
fn history_columns(
    points: &[metrics::TimedUsage],
    width: usize,
    span: std::time::Duration,
) -> (Vec<Option<u64>>, Vec<Option<u64>>) {
    let mut cpu = vec![None; width];
    let mut memory = vec![None; width];
    let Some(end) = points
        .last()
        .map(|point| point.sampled_at)
        .filter(|_| width > 0)
    else {
        return (cpu, memory);
    };
    for point in points {
        let age = end
            .saturating_duration_since(point.sampled_at)
            .as_secs_f64();
        let position = ((1.0 - age / span.as_secs_f64().max(1.0)).clamp(0.0, 1.0)
            * width.saturating_sub(1) as f64)
            .round() as usize;
        cpu[position] = point.cpu;
        memory[position] = point.memory;
    }
    (cpu, memory)
}

fn history_axis(width: usize, span: std::time::Duration) -> String {
    let mut cells = vec!['─'; width];
    let seconds = span.as_secs();
    let ticks = if width >= 48 { 4 } else { 2 };
    for tick in 0..=ticks {
        let label = if tick == ticks {
            "latest".into()
        } else {
            format!("−{}s", seconds * (ticks - tick) / ticks)
        };
        let length = label.chars().count();
        if length > width {
            continue;
        }
        let start = (width.saturating_sub(1) * tick as usize / ticks as usize)
            .saturating_sub(if tick == ticks {
                length - 1
            } else if tick == 0 {
                0
            } else {
                length / 2
            })
            .min(width - length);
        for (offset, ch) in label.chars().enumerate() {
            cells[start + offset] = ch;
        }
    }
    cells.into_iter().collect()
}

fn draw_usage(frame: &mut Frame, app: &App, index: usize, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let engine = &app.engines[index];
    let line = |offset| Rect::new(area.x, area.y + offset, area.width, 1);
    let condition = if engine.error.is_some() {
        Condition::new(Tone::Warning, "unavailable")
    } else if engine.alias.is_some() {
        Condition::new(Tone::Neutral, "alias")
    } else if let Some(snapshot) = &engine.snapshot {
        crate::model::runtime_condition(&snapshot.containers)
    } else {
        Condition::new(Tone::Warning, "connecting")
    };
    let summary = app.engine_metrics(index);
    let header = Line::from(vec![
        Span::styled(
            format!("{} ", condition.symbol()),
            condition_style(app, condition),
        ),
        Span::raw(format!(
            "{} · CPU {}{}",
            safe_text(&engine.connection.name),
            summary.cpu_label(),
            if area.height < 4 {
                format!(" · Mem {}", summary.memory_label())
            } else {
                String::new()
            }
        )),
    ]);
    frame.render_widget(
        Paragraph::new(header).style(
            if app.performance_focus && app.performance_engine() == Some(index) {
                selected()
            } else {
                Style::default()
            },
        ),
        line(0),
    );
    if area.height < 4 {
        return;
    }
    if let Some(alias) = &engine.alias {
        frame.render_widget(
            Paragraph::new(format!("Alias of {} · not counted twice", safe_text(alias)))
                .style(muted()),
            line(1),
        );
        return;
    }
    if engine.error.is_some() {
        frame.render_widget(
            Paragraph::new("Unavailable · no live measurements")
                .style(condition_style(app, condition)),
            line(1),
        );
        return;
    }
    let Some(snapshot) = &engine.snapshot else {
        frame.render_widget(Paragraph::new("Connecting…").style(muted()), line(1));
        return;
    };
    let history = engine.metrics.cpu_history(&snapshot.containers);
    let history = &history[history.len().saturating_sub(area.width as usize)..];
    let maximum = history
        .iter()
        .flatten()
        .copied()
        .max()
        .unwrap_or(0)
        .max(10_000);
    frame.render_widget(
        Sparkline::default()
            .data(history)
            .max(maximum)
            .style(accent(app, Color::Cyan))
            .absent_value_style(muted())
            .absent_value_symbol("·"),
        line(1),
    );
    let capacity = snapshot.memory_capacity_bytes;
    let memory = format!(
        "Memory {}{}",
        summary.memory_label(),
        capacity
            .map(|bytes| format!(" / {}", metrics::bytes(bytes)))
            .unwrap_or_default()
    );
    if let Some((used, limit)) = summary
        .memory_bytes
        .zip(capacity)
        .filter(|(_, limit)| *limit > 0)
    {
        let ratio = (used as f64 / limit as f64).clamp(0.0, 1.0);
        let color = if ratio >= 0.9 {
            Color::Red
        } else if ratio >= 0.75 {
            Color::Yellow
        } else {
            Color::Green
        };
        frame.render_widget(
            LineGauge::default()
                .ratio(ratio)
                .label(memory)
                .filled_style(accent(app, color))
                .unfilled_style(muted()),
            line(2),
        );
    } else {
        frame.render_widget(
            Paragraph::new(format!("{memory} · capacity unavailable")).style(muted()),
            line(2),
        );
    }
    frame.render_widget(
        Paragraph::new(format!(
            "{}s target · CPU {}/{} Mem {}/{} · scale {:.0}%",
            app.metrics_refresh_seconds,
            summary.cpu_samples,
            summary.running,
            summary.memory_samples,
            summary.running,
            maximum as f64 / 100.0
        ))
        .style(muted()),
        line(3),
    );
}

fn overview_lines(app: &App) -> Vec<Line<'static>> {
    let Some(row) = app.current() else {
        return vec![Line::from("Select a runtime or workload in the explorer.")];
    };
    let engine = &app.engines[row.engine];
    let mut lines = vec![
        Line::styled(
            safe_text(&row.title),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::styled(safe_text(&engine.connection.name), muted()),
        Line::from(""),
    ];
    if let Some(alias) = &engine.alias {
        lines.push(Line::from(format!(
            "Same engine as {}. Browse that connection to avoid duplicates.",
            safe_text(alias)
        )));
        return lines;
    }
    if let Some(error) = &engine.error {
        lines.push(Line::styled(
            "Connection unavailable",
            accent(app, Color::Yellow),
        ));
        lines.push(Line::from(safe_text(error)));
        if engine.snapshot.is_some() {
            lines.push(Line::styled(
                "Showing stale, last-known data below.",
                accent(app, Color::Yellow),
            ));
        }
        lines.push(Line::from(""));
    }
    if row.scope == Scope::Engine {
        if let Some(target) = &engine.runtime {
            lines.push(Line::from(format!(
                "Runtime control: {}",
                safe_text(&target.label())
            )));
            lines.push(Line::styled(
                "S Start runtime · s Stop runtime · r Restart runtime (ALL workloads)",
                muted(),
            ));
        } else {
            lines.push(Line::styled(
                "Runtime control unsupported; select a workload for container actions.",
                muted(),
            ));
        }
        lines.push(Line::from(""));
    }
    let Some(snapshot) = &engine.snapshot else {
        if engine.error.is_none() {
            lines.push(Line::from("Connecting… Other engines remain usable."));
        }
        return lines;
    };
    if row.scope == Scope::Engine {
        lines.push(Line::from(format!(
            "Version      {}",
            safe_text(&snapshot.version)
        )));
        lines.push(Line::from(format!(
            "Endpoint     {}",
            safe_text(&engine.connection.endpoint)
        )));
        lines.push(Line::from(format!(
            "Capacity     {} CPUs · {} memory (not current host usage)",
            snapshot
                .cpu_count
                .map(|value| value.to_string())
                .unwrap_or_else(|| "--".into()),
            snapshot
                .memory_capacity_bytes
                .map(metrics::bytes)
                .unwrap_or_else(|| "--".into())
        )));
    }
    if let Scope::Resource { name, kind } = &row.scope {
        let resources = match kind {
            ResourceKind::Network => &snapshot.networks,
            ResourceKind::Volume => &snapshot.volumes,
        };
        if let Some(resource) = resources.iter().find(|r| &r.name == name) {
            lines.push(Line::from(format!(
                "Ownership    {}",
                resource
                    .project
                    .as_deref()
                    .map(safe_text)
                    .unwrap_or_else(|| "No project label".into())
            )));
            if resource.external
                || app
                    .project
                    .as_ref()
                    .is_some_and(|project| resource.project.as_ref() != Some(&project.name))
            {
                lines.push(Line::styled(
                    "Shared/external resource; not exclusive to this project.",
                    accent(app, Color::Yellow),
                ));
            }
            lines.push(Line::from(safe_text(&resource.details)));
            lines.push(Line::from(""));
            lines.push(Line::styled(
                "Attached containers",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            let members: Vec<_> = resource
                .containers
                .iter()
                .filter(|name| {
                    app.project.as_ref().is_none_or(|project| {
                        snapshot
                            .containers
                            .iter()
                            .any(|c| &c.name == *name && project.contains(c))
                    })
                })
                .collect();
            if resource.containers.is_empty() {
                lines.push(Line::from("No attachments reported."));
            }
            for container in &members {
                lines.push(Line::from(format!("  {}", safe_text(container))));
            }
            if members.len() < resource.containers.len() {
                lines.push(Line::styled(
                    format!(
                        "{} attachments outside this project (hidden)",
                        resource.containers.len() - members.len()
                    ),
                    muted(),
                ));
            }
        }
    } else {
        let containers = app.selected_containers();
        lines.push(Line::from(format!(
            "{} containers · {} running",
            containers.len(),
            containers.iter().filter(|c| c.running()).count()
        )));
        lines.push(Line::from(""));
        let summary = app.metrics_for(row);
        if containers.len() != 1 {
            lines.push(Line::from(format!(
                "Workload CPU     {}",
                summary.cpu_label()
            )));
            lines.push(Line::from(format!(
                "Workload memory  {}",
                summary.memory_label()
            )));
            lines.push(Line::styled(summary.detail(), muted()));
            lines.push(Line::from(""));
        }
        for container in &containers {
            let condition = container.condition();
            lines.push(Line::styled(
                format!(
                    "{}  {} · {}",
                    condition.symbol(),
                    safe_text(&container.name),
                    condition.label,
                ),
                condition_style(app, condition),
            ));
            lines.push(Line::from(format!("  {}", safe_text(&container.status))));
            let usage = engine.metrics.summary(
                std::iter::once(container),
                std::time::Instant::now(),
                app.metrics_stale_after(),
                engine.error.is_none(),
            );
            lines.push(Line::from(format!("  CPU        {}", usage.cpu_label())));
            lines.push(Line::from(format!(
                "  Memory     {}{}",
                usage.memory_label(),
                usage
                    .memory_limit_bytes
                    .map(|limit| format!(" / {} reported limit", metrics::bytes(limit)))
                    .unwrap_or_default()
            )));
            if let Some(error) = engine.metrics.errors.get(&container.key()) {
                lines.push(Line::styled(
                    format!("  Stats      {}", safe_text(error)),
                    accent(app, Color::Yellow),
                ));
            }
            if usage.stale > 0
                && let Some(sample) = engine.metrics.samples.get(&container.key())
            {
                lines.push(Line::styled(
                    format!(
                        "  Last sample {}s ago · CPU {} · memory {} (stale, excluded from totals)",
                        sample.sampled_at.elapsed().as_secs(),
                        sample
                            .usage
                            .cpu_percent
                            .map(|cpu| format!("{cpu:.1}%"))
                            .unwrap_or_else(|| "--".into()),
                        sample
                            .usage
                            .memory_bytes
                            .map(metrics::bytes)
                            .unwrap_or_else(|| "--".into())
                    ),
                    muted(),
                ));
            }
            lines.push(Line::from(format!(
                "  Image      {}",
                safe_text(&container.image)
            )));
            if containers.len() == 1 {
                let key =
                    crate::platform::Key::new(row.engine, snapshot.engine_id.clone(), container);
                let platform = if let Some(platform) = &container.image_platform {
                    platform.label()
                } else {
                    match app.platforms.get(&key) {
                        Some(crate::platform::State::Ready(platform)) => platform.label(),
                        Some(crate::platform::State::Unavailable(error)) => {
                            format!("unavailable · {error}")
                        }
                        Some(crate::platform::State::Loading) | None => {
                            if engine.error.is_some() {
                                "unavailable (disconnected)".into()
                            } else {
                                "loading image metadata…".into()
                            }
                        }
                    }
                };
                lines.push(Line::from(format!("  Platform   {}", safe_text(&platform))));
                lines.push(Line::from(format!(
                    "  ID         {}",
                    safe_text(&container.id)
                )));
                if let Some(project) = &container.project {
                    lines.push(Line::from(format!("  Project    {}", safe_text(project))));
                }
                if let Some(service) = &container.service {
                    lines.push(Line::from(format!("  Service    {}", safe_text(service))));
                }
            }
            if !container.port_bindings.is_empty() {
                for port in &container.port_bindings {
                    lines.push(Line::from(format!(
                        "  Port       {}",
                        safe_text(&port.overview())
                    )));
                }
                lines.push(Line::styled(
                    if engine.error.is_some() {
                        "  Last-known bindings (offline); localhost reachability not verified."
                    } else if !container.running() {
                        "  Container is stopped; published ports are not active."
                    } else {
                        "  Reported bindings; localhost/VM forwarding reachability not verified."
                    },
                    muted(),
                ));
            } else if container.ports.is_empty() {
                lines.push(Line::styled(
                    "  Ports      No port mappings reported",
                    muted(),
                ));
            } else {
                for port in &container.ports {
                    lines.push(Line::from(format!(
                        "  Port       {} (reported)",
                        safe_text(port)
                    )));
                }
            }
            for network in &container.networks {
                lines.push(Line::from(format!("  Network    {}", safe_text(network))));
            }
            for volume in &container.volumes {
                lines.push(Line::from(format!("  Volume     {}", safe_text(volume))));
            }
            if container.managed {
                lines.push(Line::styled(
                    "  Kubernetes-managed · inspection only",
                    accent(app, Color::Yellow),
                ));
            }
            lines.push(Line::from(""));
        }
        if containers.is_empty() {
            lines.push(Line::from("No containers reported in this scope."));
        }
    }
    for warning in &snapshot.warnings {
        lines.push(Line::styled(
            format!("! {}", safe_text(warning)),
            accent(app, Color::Yellow),
        ));
    }
    if row.scope == Scope::Engine
        && let Some(images) = &snapshot.images
    {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            format!("Images ({})", images.len()),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        for image in images {
            let tags = if image.tags.is_empty() {
                "<untagged>".into()
            } else {
                image.tags.join(", ")
            };
            lines.push(Line::from(format!(
                "  {} · {}",
                safe_text(&tags),
                image
                    .size
                    .map(metrics::bytes)
                    .unwrap_or_else(|| "size unavailable".into())
            )));
            lines.push(Line::styled(
                format!("    {}", safe_text(&image.id)),
                muted(),
            ));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        "S Start · s Stop · r Restart · d Delete (Explorer; confirmation required)",
        muted(),
    ));
    lines
}

fn draw_networks(frame: &mut Frame, app: &mut App, area: Rect) {
    let (mut lines, targets) = network_view(app);
    let owner = app.current().map(|row| row.key.clone());
    if app.network_cursor_owner != owner {
        app.network_cursor_owner = owner;
        app.network_cursor = 0;
    }
    app.network_cursor = app.network_cursor.min(targets.len().saturating_sub(1));
    let cursor = app
        .detail_focus
        .then(|| targets.get(app.network_cursor))
        .flatten();
    // Wrapped rows take several terminal lines, so measure before placing the cursor and clicks.
    let tops: Vec<usize> = lines
        .iter()
        .scan(0, |top, line| {
            let current = *top;
            *top += Paragraph::new(line.clone())
                .wrap(Wrap { trim: false })
                .line_count(area.width);
            Some(current)
        })
        .collect();
    let total_rows = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(area.width);
    let bounds = |line: usize| {
        (
            tops[line],
            tops.get(line + 1).copied().unwrap_or(total_rows),
        )
    };
    if let Some(target) = cursor {
        let (top, bottom) = bounds(target.line);
        if app.network_cursor == 0 {
            app.detail_scroll = 0;
        } else if top < app.detail_scroll {
            app.detail_scroll = top;
        } else if bottom > app.detail_scroll + area.height as usize {
            app.detail_scroll = bottom - area.height as usize;
        }
        lines[target.line] = lines[target.line].clone().patch_style(selected());
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let maximum = total_rows.saturating_sub(area.height as usize);
    app.detail_scroll = app.detail_scroll.min(maximum).min(u16::MAX as usize);
    for (index, target) in targets.iter().enumerate() {
        let (top, bottom) = bounds(target.line);
        for row in top.max(app.detail_scroll)..bottom {
            let y = row - app.detail_scroll;
            if y < area.height as usize {
                app.mouse_regions.push((
                    Rect::new(area.x, area.y + y as u16, area.width, 1),
                    MouseTarget::NetworkRow(index),
                ));
            }
        }
    }
    app.network_targets = targets;
    frame.render_widget(paragraph.scroll((app.detail_scroll as u16, 0)), area);
}

#[cfg(test)]
fn network_lines(app: &App) -> Vec<Line<'static>> {
    network_view(app).0
}

fn network_view(app: &App) -> (Vec<Line<'static>>, Vec<NetworkTarget>) {
    let Some(row) = app.current() else {
        return (vec![], vec![]);
    };
    let engine = &app.engines[row.engine];
    let mut lines = vec![
        Line::styled(
            "Network membership",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
    ];
    if let Some(alias) = &engine.alias {
        lines.push(Line::from(format!(
            "Browse {} for this engine's resources.",
            safe_text(alias)
        )));
        return (lines, vec![]);
    }
    if engine.error.is_some() {
        lines.push(Line::styled(
            "Connection unavailable / stale · see Overview for details",
            accent(app, Color::Yellow),
        ));
    }
    if engine.connection.kind == BackendKind::Containerd {
        lines.push(Line::from(
            "Native containerd network inventory is not supported by this MVP adapter.",
        ));
        lines.push(Line::from(
            "Networks depend on the launcher and its CNI configuration.",
        ));
        return (lines, vec![]);
    }
    let Some(snapshot) = &engine.snapshot else {
        lines.push(Line::from("Engine has not returned an inventory."));
        return (lines, vec![]);
    };
    let containers = app.selected_containers();
    let engine_scope = matches!(row.scope, Scope::Engine);
    let mut count = 0;
    let mut targets = Vec::new();
    for network in &snapshot.networks {
        let explicit = matches!(&row.scope, Scope::Resource { name, kind: ResourceKind::Network } if name == &network.name);
        let owned = match &row.scope {
            Scope::Project { project, .. }
            | Scope::Resources {
                project: Some(project),
                ..
            } => network.project.as_ref() == Some(project),
            Scope::Resources {
                project: None,
                kind: ResourceKind::Network,
            } => network.project.is_none() || network.external,
            _ => false,
        };
        if !(engine_scope
            || explicit
            || owned
            || containers
                .iter()
                .any(|c| c.networks.contains(&network.name)))
        {
            continue;
        }
        count += 1;
        targets.push(NetworkTarget {
            line: lines.len(),
            container: None,
        });
        lines.push(Line::styled(
            format!(
                "{}{}",
                safe_text(&network.name),
                if network.external
                    || app
                        .project
                        .as_ref()
                        .is_some_and(|project| network.project.as_ref() != Some(&project.name))
                {
                    " [shared/external]"
                } else {
                    ""
                }
            ),
            accent(app, Color::Cyan),
        ));
        lines.push(Line::from(format!("  {}", safe_text(&network.details))));
        // Engines report attachments in no fixed order, which would move the cursor's row.
        let mut members: Vec<_> = network.containers.iter().collect();
        members.sort();
        for name in members {
            if app.project.as_ref().is_some_and(|project| {
                !snapshot
                    .containers
                    .iter()
                    .any(|c| &c.name == name && project.contains(c))
            }) {
                continue;
            }
            let container = snapshot.containers.iter().find(|c| &c.name == name);
            let project = container
                .and_then(|c| c.project.as_deref())
                .unwrap_or("Standalone");
            if let Some(container) = container {
                targets.push(NetworkTarget {
                    line: lines.len(),
                    container: Some(container.key()),
                });
            }
            lines.push(Line::from(format!(
                "  └─ {} / {}",
                safe_text(project),
                safe_text(name)
            )));
        }
        if network.containers.is_empty() {
            lines.push(Line::styled("  No attachments reported", muted()));
        }
        if let Some(project) = &app.project {
            let hidden = network
                .containers
                .iter()
                .filter(|name| {
                    !snapshot
                        .containers
                        .iter()
                        .any(|c| &c.name == *name && project.contains(c))
                })
                .count();
            if hidden > 0 {
                lines.push(Line::styled(
                    format!("  {hidden} attachments outside this project (hidden)"),
                    muted(),
                ));
            }
        }
        lines.push(Line::from(""));
    }
    if count == 0 {
        lines.push(Line::from("No networks reported for this selection."));
    }
    for warning in &snapshot.warnings {
        lines.push(Line::styled(safe_text(warning), accent(app, Color::Yellow)));
    }
    (lines, targets)
}

fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.height < 3 || area.width < 8 {
        return;
    }
    let areas = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(area);
    let content = Rect::new(
        areas[1].x,
        areas[1].y,
        areas[1].width.saturating_sub(2),
        areas[1].height,
    );
    let source_width = (content.width / 4).clamp(2, 20);
    let payload_width = content.width.saturating_sub(source_width + 1).max(1);
    app.log_view
        .sync(&app.logs, payload_width as usize, areas[1].height as usize);
    let (start, total) = app.log_view.scroll_position(&app.logs);
    let end = (start + areas[1].height as usize).min(total);
    let position = format!(
        "Rows {}–{end} / {total}",
        if total == 0 { 0 } else { start + 1 }
    );
    let since = app
        .log_since
        .and_then(|at| time::OffsetDateTime::from_unix_timestamp(at).ok())
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .map(|at| format!(" · Since {at}"))
        .unwrap_or_default();
    let status = format!(
        "Wrap: {} · JSON: {} · {} · {position}{since}",
        if app.log_view.wrap { "on" } else { "off" },
        if app.logs.pretty_json {
            "pretty"
        } else {
            "raw"
        },
        if app.logs.follow {
            "Following"
        } else {
            "Paused"
        },
    );
    let search = if app.log_view.query().is_empty() {
        format!(
            "{}/{} entries retained · Filter: {} · {} evicted",
            app.logs
                .entries
                .iter()
                .filter(|entry| app.log_view.filter.matches(entry))
                .count(),
            app.logs.entries.len(),
            if app.log_view.filter.active() {
                "on"
            } else {
                "off"
            },
            app.logs.evicted
        )
    } else {
        format!(
            "{}/{} matches {}/{} · Filter: {}",
            app.log_view.match_index.unwrap_or(0),
            app.log_view.match_count,
            if app.log_view.query_is_regex() {
                "regex "
            } else {
                ""
            },
            safe_text(app.log_view.query()),
            if app.log_view.filter.active() {
                "on"
            } else {
                "off"
            },
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(status, muted()),
            Line::styled(search, muted()),
        ]),
        areas[0],
    );
    app.mouse_regions.push((
        Rect::new(
            areas[1].right().saturating_sub(1),
            areas[1].y,
            1,
            areas[1].height,
        ),
        MouseTarget::LogScrollbar,
    ));
    let mut scrollbar = ScrollbarState::new(total)
        .position(start)
        .viewport_content_length(areas[1].height as usize);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"))
            .thumb_style(accent(app, Color::Cyan))
            .track_style(muted()),
        areas[1],
        &mut scrollbar,
    );
    let visible = app.log_view.visible(&app.logs);
    if visible.is_empty() {
        frame.render_widget(
            Paragraph::new(if app.log_view.filter.active() {
                "No matching retained logs. F3 edits; :clear-log-filter resets."
            } else {
                "Waiting for logs. Select a container, service, or project."
            }),
            content,
        );
        return;
    }
    for (offset, row) in visible.into_iter().enumerate() {
        let Some(entry) = log_view::entry(&app.logs, row.sequence) else {
            continue;
        };
        let y = areas[1].y + offset as u16;
        let colors = [Color::Cyan, Color::Magenta, Color::Green, Color::Blue];
        let hash = entry.source.bytes().fold(0usize, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(byte as usize)
        });
        let color = accent(
            app,
            if entry.error {
                Color::Yellow
            } else {
                colors[hash % colors.len()]
            },
        );
        let source = if row.continuation {
            Line::styled("↳", muted())
        } else {
            highlighted(
                &entry.source,
                0..entry.source.len(),
                app.log_view.regex(),
                app.log_view
                    .active
                    .as_ref()
                    .filter(|found| {
                        found.sequence == entry.sequence && found.field == Field::Source
                    })
                    .map(|found| &found.range),
                color,
            )
        };
        frame.render_widget(
            Paragraph::new(source),
            Rect::new(content.x, y, source_width, 1),
        );
        let range = if app.log_view.wrap {
            row.range
        } else {
            let line = &entry.display()[row.range.clone()];
            row.range.start + log_view::byte_at_column(line, app.log_view.horizontal)..row.range.end
        };
        let text = highlighted(
            entry.display(),
            range,
            app.log_view.regex(),
            app.log_view
                .active
                .as_ref()
                .filter(|found| found.sequence == entry.sequence && found.field == Field::Payload)
                .map(|found| &found.range),
            Style::default(),
        );
        frame.render_widget(
            Paragraph::new(text),
            Rect::new(content.x + source_width + 1, y, payload_width, 1),
        );
    }
}

fn highlighted<'a>(
    text: &'a str,
    visible: std::ops::Range<usize>,
    regex: Option<&regex::Regex>,
    active: Option<&std::ops::Range<usize>>,
    base: Style,
) -> Line<'a> {
    let mut spans = Vec::new();
    let mut cursor = visible.start;
    if let Some(regex) = regex {
        for found in regex.find_iter(text) {
            let start = found.start().max(visible.start);
            let end = found.end().min(visible.end);
            if start >= end {
                continue;
            }
            if start > cursor {
                spans.push(Span::styled(&text[cursor..start], base));
            }
            let mut style = base.add_modifier(Modifier::REVERSED);
            if active.is_some_and(|range| *range == found.range()) {
                style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            spans.push(Span::styled(&text[start..end], style));
            cursor = end;
        }
    }
    if cursor < visible.end {
        spans.push(Span::styled(&text[cursor..visible.end], base));
    }
    Line::from(spans)
}

pub fn footer_height(app: &App) -> u16 {
    if app.action_jobs.is_empty() { 2 } else { 3 }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let notice = &app.notice;
    let mut lines = if matches!(app.input_mode, InputMode::LogFilter | InputMode::ExportLogs) {
        vec![
            Line::styled("Log dialog · Enter Apply/Export · Esc Cancel", muted()),
            Line::from(safe_text(notice)),
        ]
    } else if app.input_mode == InputMode::InventorySearch {
        vec![
            Line::from(format!("/{}▏", safe_text(&app.input))),
            Line::styled(
                "Search names, tags, IDs, projects and container references · Enter Apply · Esc Cancel · Ctrl-U Clear",
                muted(),
            ),
        ]
    } else if app.input_mode == InputMode::Search && app.detail_focus && app.tab == Tab::Logs {
        let regex = app.log_view.regex_search;
        vec![
            Line::from(format!(
                "{}/{}▏",
                if regex { "regex " } else { "" },
                safe_text(&app.input)
            )),
            if notice.starts_with("Invalid regex") {
                Line::styled(safe_text(notice), accent(app, Color::Red))
            } else {
                Line::styled(
                    format!(
                        "Enter Apply · Esc Cancel · Ctrl-R {} · Case-insensitive",
                        if regex { "Literal" } else { "Regex" }
                    ),
                    muted(),
                )
            },
        ]
    } else if app.input_mode == InputMode::Search {
        vec![
            Line::from(format!("/{}▏", safe_text(&app.input))),
            Line::styled(
                "Enter Apply · Esc Cancel · Searches the focused view",
                muted(),
            ),
        ]
    } else {
        let hints = if app.overview_mode == OverviewMode::Security {
            "Security · s Scan · S Scan checked · x Cancel · Space/m Check · Tab Images/Findings · Enter Details · z Zoom"
        } else if app.overview_mode != OverviewMode::Containers {
            "Space Mark · u Unused · c Clear · d Delete · / Search · f Filter · s Sort · Enter Details · Esc Back"
        } else if app.exec_input && app.tab == Tab::Exec {
            "Ctrl-] Navigate · SHELL INPUT · Keys go to the selected container session"
        } else if app.performance_focus {
            "Performance · P Scope  z Zoom  j/k Select  Enter Inspect  p Close  Tab Pane  ? Keys"
        } else if app.detail_focus && app.tab == Tab::Security {
            "Security · j/k Select  Enter Details  z Zoom  Tab Explorer · Scans run in the Security view"
        } else if app.detail_focus && app.tab == Tab::History {
            "Metrics · [/ ] Range  ←/→ Pan  c Cursor  h/l Inspect  f Live  z Enlarge  Esc Restore"
        } else if app.detail_focus && app.tab == Tab::Exec {
            "F6 Sessions · Enter Input · :close-exec Close slot · j/k Scroll · Tab Explorer · ? Keys"
        } else if app.detail_focus && app.tab == Tab::Logs {
            "? Keys  F3 Filter  :logs-since  J JSON  w Wrap  / Search  n/N Match  j/k Scroll  f Follow  Tab Explorer"
        } else {
            "Ctrl+←/→ Overview  ? Keys  a Actions  p Performance  Enter Open  e Exec  Tab Pane  z Zoom  1..8 Details  : Commands"
        };
        vec![Line::styled(hints, muted()), Line::from(safe_text(notice))]
    };
    if let Some(status) = app.action_status() {
        let style = if app
            .action_jobs
            .values()
            .flat_map(|job| job.results.iter().flatten())
            .any(|status| matches!(status, crate::actions::Status::Unknown(_)))
        {
            accent(app, Color::Yellow)
        } else if app
            .action_jobs
            .values()
            .flat_map(|job| job.results.iter().flatten())
            .any(|status| matches!(status, crate::actions::Status::Failed(_)))
        {
            accent(app, Color::Red)
        } else if app.has_active_actions() {
            accent(app, Color::Cyan)
        } else {
            muted()
        };
        lines.insert(
            0,
            Line::styled(safe_text(&format!("Activity · {status}")), style),
        );
    }
    const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
    let version_width = VERSION.width() as u16;
    let version_area =
        (area.height > 0 && area.width >= version_width.saturating_add(2)).then(|| {
            if let Some(last) = lines.get_mut(usize::from(area.height - 1)) {
                *last = ellipsize_line(
                    std::mem::take(last),
                    usize::from(area.width - version_width - 1),
                );
            }
            Rect::new(
                area.right() - version_width,
                area.bottom() - 1,
                version_width,
                1,
            )
        });
    frame.render_widget(Paragraph::new(lines), area);
    if let Some(version_area) = version_area {
        frame.render_widget(Paragraph::new(VERSION).style(muted()), version_area);
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_help(frame: &mut Frame, app: &mut App, area: Rect) {
    let area = centered(area, 80, 36);
    frame.render_widget(Clear, area);
    let defaults = concat!(
        "Navigation\n",
        "j / k, arrows     Move in the focused pane\n",
        "h / l             Collapse/parent, expand/child (tree)\n",
        "Space             Toggle the selected explorer branch\n",
        "Enter             Enter a Compose workspace; otherwise open details\n",
        "Backspace         Return to the previous workspace\n",
        "Esc               Clear search, return to explorer, then go back\n",
        "q                 Back through panes/workspaces; quit from home\n",
        "Tab / Shift-Tab   Cycle Explorer / Performance (open) / Details\n",
        "p                 Open/close Performance (compact by default)\n",
        "P (Shift-p)       Switch Runtime / Selected container; open Performance\n",
        "z                 Expand / restore the focused detail tab or Performance\n",
        "Esc               Restore compact Performance before other navigation\n",
        "o                 Sort contexts (pins first; usage is a snapshot)\n",
        "S / s / r / d     Start / Stop / Restart / Delete in Explorer\n",
        "                  Container or Compose rows only; confirmation required\n",
        "                  y/Enter confirms; n/Esc cancels; Delete: type name + Enter\n",
        "                  Delete stopped containers only; keep networks/volumes/images\n",
        "1..8              Info Logs Net Exec Events Metrics Layers Security\n",
        "Ctrl+←/→, F7/F8    Cycle overviews · Alt+1/2/3 direct\n",
        "a                 Contextual actions with disabled reasons (Explorer)\n",
        "Enter in Layers   Expand details; Esc returns; no image pulls\n",
        "F6 / :exec-sessions Switch retained shells (navigation mode only)\n",
        "F3 in Logs        JSON/source/severity/time filters on retained entries\n",
        ":export-logs      Explicit new private JSONL file; may contain secrets\n",
        "gg / G            First / last; G resumes log following\n",
        "Ctrl-u / Ctrl-d   Half-page up / down\n",
        "\nLogs (focus the log pane with Tab)\n",
        "w                 Toggle wrapping on/off immediately\n",
        "h / l             Horizontal scroll when wrapping is off\n",
        "/                 Search; Enter applies, Esc cancels input\n",
        "n / N             Next / previous highlighted occurrence\n",
        "f                 Toggle follow; scrolling pauses it\n",
        "Mouse             Click selects, click again opens; right-click back; wheel scrolls; drag log scrollbar\n",
        "                  Scrollbar + Rows readout track wrapped display lines\n",
        "Esc               Clear highlights without hiding log context\n",
        "\nSecurity (optional module)\n",
        "7 / :security     Open the selected image's Security view\n",
        "Images m/M        Check image/all for :scan-images; 7 opens Security\n",
        "Security Enter    Open advisory details; z enlarge; :cancel-scan stops queue\n",
        "\nActions\n",
        "e / :exec         Embedded shell in the right pane\n",
        "                  Services with replicas open a container picker\n",
        "4 then Enter      Focus Exec input; start if no session exists\n",
        "Ctrl-]            Return from shell input to Tha navigation\n",
        "                  Esc, Ctrl-C, arrows, numbers go to the shell\n",
        "exit              End shell; output stays in the Exec view\n",
        ":close-exec       Close session, not the container\n",
        ":                 Command palette; Tab completes a command\n",
        ":all / :back      All runtimes / previous workspace\n",
        ":wrap             Toggle wrapping through the command palette\n",
        "? / F1 / :help    Open this cheatsheet\n",
        "Ctrl-c            Quit Tha in navigation; inside Exec send interrupt\n",
        "\nPicker: j/k select, Enter chooses, Esc cancels.\n",
        "Colors inherit your terminal's ANSI palette.\n",
        "Green circle: running/healthy; yellow: starting/paused/restarting.\n",
        "Red circle: unhealthy/failed; neutral: completed/stopped/unknown.\n",
        "Exec and lifecycle actions can change workloads. Confirmations remain required."
    );
    let custom = app
        .custom_bindings
        .iter()
        .map(|(key, action)| format!("{} → {}\n", safe_text(key), safe_text(action)))
        .collect::<String>();
    let text = format!(
        "{defaults}\n\nSet a key to 'none' to disable it. Restart after editing config.toml.\n\nConfigurable action names (built-in defaults; custom keys below take precedence)\n{}\nCustom navigation bindings (override defaults only in navigation)\n{custom}\nExec and lifecycle actions can change workloads. Confirmations remain required.",
        crate::shortcuts::ACTIONS
            .iter()
            .map(|info| {
                let keys = info
                    .bindings
                    .iter()
                    .map(|binding| binding.key)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{} — {} [{}]\n",
                    info.id.config_name(),
                    info.label,
                    if keys.is_empty() { "unbound" } else { &keys }
                )
            })
            .collect::<String>()
    );
    let block = Block::bordered()
        .title(" Tha · Shortcuts / Cheatsheet ")
        .title_bottom(" j/k Scroll · ? / Esc Close ");
    let inner = padded(block.inner(area));
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    app.help_scroll = app.help_scroll.min(
        paragraph
            .line_count(inner.width)
            .saturating_sub(inner.height as usize),
    );
    frame.render_widget(block, area);
    frame.render_widget(paragraph.scroll((app.help_scroll as u16, 0)), inner);
}

fn draw_palette(frame: &mut Frame, app: &App, area: Rect) {
    let area = centered(area, 72, COMMANDS.len() as u16 + 5);
    frame.render_widget(Clear, area);
    let mut lines = vec![
        Line::styled(
            format!(":{}▏", safe_text(&app.input)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
    ];
    for &(command, description) in COMMANDS
        .iter()
        .filter(|(command, _)| command.starts_with(&app.input))
    {
        lines.push(Line::from(format!("{command:<18} {description}")));
    }
    lines.push(Line::styled(
        "Enter Execute · Tab Complete · Esc Cancel",
        muted(),
    ));
    let block = Block::bordered().title(" Commands ");
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_picker(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(picker) = &app.picker else {
        return;
    };
    let actions = picker
        .choices
        .iter()
        .any(|choice| matches!(choice, Choice::Action { .. }));
    let area = centered(
        area,
        86,
        if actions {
            18
        } else {
            (picker.choices.len() as u16).saturating_add(3).min(24)
        },
    );
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(format!(" {} ", safe_text(&picker.title)))
        .title_bottom(" j/k Select · Enter Choose · Esc Cancel ");
    let mut inner = padded(block.inner(area));
    frame.render_widget(block, area);
    if actions {
        let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(4)]).split(inner);
        inner = parts[0];
        if let Some(Choice::Compose(action)) = picker.choices.get(picker.selected) {
            frame.render_widget(
                Paragraph::new(action.effect()).wrap(Wrap { trim: false }),
                parts[1],
            );
        }
        if let Some(Choice::Action { action, row }) = picker.choices.get(picker.selected) {
            let explanation = app.action_availability(*action, row).map(|count| format!("{count} existing container(s). Enter opens confirmation; nothing is sent now.")).unwrap_or_else(|reason| format!("Disabled: {reason}"));
            frame.render_widget(
                Paragraph::new(safe_text(&explanation)).wrap(Wrap { trim: false }),
                parts[1],
            );
        }
    }
    let start = picker
        .selected
        .saturating_sub(inner.height.saturating_sub(1) as usize);
    for offset in 0..inner.height {
        let index = start + offset as usize;
        if index < picker.choices.len() {
            app.mouse_regions.push((
                Rect::new(inner.x, inner.y + offset, inner.width, 1),
                MouseTarget::PickerRow(index),
            ));
        }
    }
    let lines: Vec<_> = picker
        .choices
        .iter()
        .enumerate()
        .skip(start)
        .take(inner.height as usize)
        .map(|(index, choice)| {
            let label = match choice {
                Choice::ActionResult(operation) => app
                    .action_jobs
                    .get(operation)
                    .map(|job| {
                        format!(
                            "{} · {} · {}",
                            job.request.connection.name,
                            job.request.action.label(),
                            job.summary()
                        )
                    })
                    .unwrap_or_else(|| "Result no longer available".into()),
                Choice::Session(id) => app
                    .sessions
                    .iter()
                    .find(|(candidate, _)| candidate == id)
                    .map(|(_, session)| {
                        format!(
                            "{} #{} · {} / {} · {}",
                            if app.sessions.active_id() == Some(*id) {
                                "▸"
                            } else {
                                " "
                            },
                            id,
                            session.request.connection.name,
                            session.request.container.name,
                            session.status()
                        )
                    })
                    .unwrap_or_else(|| format!("#{id} · closed")),
                Choice::Action { action, row } => format!(
                    "{} · {}",
                    action.label(),
                    app.action_availability(*action, row)
                        .map(|count| format!("{count} target(s)"))
                        .unwrap_or_else(|_| "disabled".into())
                ),
                Choice::Compose(action) => action.label().into(),
                Choice::ContextSort(mode) => mode.label().into(),
                Choice::Project(project) => format!(
                    "{} / {}{}",
                    app.engines[project.engine].connection.name,
                    project
                        .namespace
                        .as_ref()
                        .map(|ns| format!("{ns} / "))
                        .unwrap_or_default(),
                    project.name
                ),
                Choice::Exec { engine, container } => format!(
                    "{} / {}{}",
                    app.engines[*engine].connection.name,
                    container
                        .namespace
                        .as_ref()
                        .map(|ns| format!("{ns} / "))
                        .unwrap_or_default(),
                    container.name
                ),
            };
            Line::styled(
                safe_text(&label),
                if index == picker.selected {
                    selected()
                } else {
                    Style::default()
                },
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_axis_uses_elapsed_time_and_keeps_missing_samples_absent() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let points = vec![
            metrics::TimedUsage {
                sampled_at: start,
                cpu: Some(10),
                memory: Some(100),
            },
            metrics::TimedUsage {
                sampled_at: start + Duration::from_secs(2),
                cpu: None,
                memory: None,
            },
            metrics::TimedUsage {
                sampled_at: start + Duration::from_secs(10),
                cpu: Some(20),
                memory: Some(200),
            },
        ];
        let (cpu, memory) = history_columns(&points, 11, Duration::from_secs(10));
        assert_eq!(cpu[0], Some(10));
        assert_eq!(cpu[2], None);
        assert_eq!(cpu[10], Some(20));
        assert_eq!(memory[10], Some(200));
        assert!(cpu[3..10].iter().all(Option::is_none));
        let axis = history_axis(60, Duration::from_secs(120));
        assert!(axis.starts_with("−120s"));
        assert!(axis.contains("−60s"));
        assert!(axis.ends_with("latest"));
        assert_eq!(history_axis(2, Duration::from_secs(60)).chars().count(), 2);
    }

    #[test]
    fn inner_tabs_zoom_to_full_width_and_survive_resize() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Container(_)))
            .unwrap();
        app.detail_focus = true;
        app.performance_open = true;
        app.performance_expanded = true;
        for tab in [
            Tab::Overview,
            Tab::Logs,
            Tab::Networks,
            Tab::Exec,
            Tab::Layers,
        ] {
            app.tab = tab;
            app.detail_zoom = Some(tab);
            for (width, height) in [(140, 40), (60, 20), (32, 10), (140, 40)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| draw(frame, &mut app)).unwrap();
                let buffer = terminal.backend().buffer();
                let row: String = (0..width).map(|x| buffer[(x, 2)].symbol()).collect();
                let label = match tab {
                    Tab::Overview => "1 Overview",
                    Tab::Logs => "2 Logs",
                    Tab::Networks => "3 Networks",
                    Tab::Exec => "4 Exec",
                    Tab::Layers => "7 Layers",
                    _ => unreachable!(),
                };
                assert!(row.contains(label), "{tab:?}: {row}");
                assert_eq!(buffer[(width - 1, 2)].symbol(), "┓");
            }
        }
    }

    #[test]
    fn full_width_performance_and_container_empty_state_survive_resize() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.switch_performance_scope();
        app.performance_expanded = true;
        for (width, height) in [(140, 40), (60, 20), (32, 10), (140, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            if width == 140 {
                let buffer = terminal.backend().buffer();
                let header = (0..height)
                    .find(|&y| {
                        let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
                        row.contains("Performance · Selected container")
                    })
                    .unwrap();
                assert_eq!(buffer[(width - 1, header)].symbol(), "┓");
                let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                assert!(text.contains("Select a container"));
            }
        }
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Container(_)))
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Container ·"));
        assert!(text.contains("latest"));
        assert!(text.contains("memory limit"));
    }

    #[test]
    fn expanded_performance_has_multirow_graphs_and_idle_frames_are_stable() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(60, 13)).unwrap();
        terminal
            .draw(|frame| draw_performance_history(frame, &app, 1, frame.area()))
            .unwrap();
        let before = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..13)
            .map(|y| (0..60).map(|x| before[(x, y)].symbol()).collect())
            .collect();
        assert!(rows.iter().any(|row| row.contains("Memory")));
        assert!(rows.iter().any(|row| row.contains("runtime capacity")));
        assert!(
            rows.iter()
                .filter(|row| row.chars().any(|ch| "▁▂▃▄▅▆▇█".contains(ch)))
                .count()
                >= 3
        );
        terminal
            .draw(|frame| draw_performance_history(frame, &app, 1, frame.area()))
            .unwrap();
        assert_eq!(
            &before,
            terminal.backend().buffer(),
            "Rendering alone must not animate fake samples"
        );
    }

    #[test]
    fn layers_tab_is_hidden_and_dismissed_for_non_workload_selections() {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(140, 35)).unwrap();
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Container(_)))
            .unwrap();
        assert!(app.layers_available());
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("7 Layers"));
        let unsupported: Vec<_> = app
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| !matches!(row.scope, Scope::Container(_) | Scope::Service { .. }))
            .map(|(index, _)| index)
            .collect();
        assert!(!unsupported.is_empty());
        for index in unsupported {
            app.selected = index;
            app.tab = Tab::Layers;
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            assert_eq!(app.tab, Tab::Overview);
            assert!(app.layers.key.is_none());
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(!text.contains("7 Layers"));
        }
    }

    #[test]
    fn performance_shrinks_to_fit_connected_runtimes() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.performance_open = true;
        assert_eq!(performance_height(&app, 40), 16);
        app.update_engine(0, Err("stopped".into()));
        app.update_engine(2, Err("stopped".into()));
        assert_eq!(performance_height(&app, 40), 14);
        app.update_engine(1, Err("stopped".into()));
        assert_eq!(performance_height(&app, 40), 6);
        app.performance_open = false;
        assert_eq!(performance_height(&app, 40), 1);
    }

    #[test]
    fn activity_and_error_notices_have_separate_footer_rows() {
        let mut app = App::new(
            vec![crate::config::Connection {
                name: "fixture".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///fixture.sock".into(),
            }],
            100,
            true,
            false,
        );
        app.engines[0].runtime = Some(crate::runtime::Target::Colima {
            profile: "fixture".into(),
        });
        app.request_action(crate::actions::Action::Start);
        app.action_review_visible = true;
        app.confirm_action();
        app.notice = "Permission denied: this notice must not compete with activity".into();
        assert_eq!(footer_height(&app), 3);
        let mut terminal = Terminal::new(TestBackend::new(100, 3)).unwrap();
        terminal
            .draw(|frame| draw_footer(frame, &app, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let activity: String = (0..100).map(|x| buffer[(x, 0)].symbol()).collect();
        let notice: String = (0..100).map(|x| buffer[(x, 2)].symbol()).collect();
        assert!(activity.contains("Activity ·"));
        assert!(activity.contains("Starting…"));
        assert!(!activity.contains("Permission denied"));
        assert!(notice.contains(&app.notice));
        assert!(!notice.contains("Starting…"));
        assert!(notice.ends_with(concat!("v", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn footer_version_is_bottom_right_without_overwriting_status_text() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        let version = concat!("v", env!("CARGO_PKG_VERSION"));
        for width in [32, 100] {
            for mode in [
                InputMode::Normal,
                InputMode::Search,
                InputMode::InventorySearch,
            ] {
                app.input_mode = mode;
                app.notice = "Status: ".to_owned() + &"x".repeat(200);
                let mut terminal = Terminal::new(TestBackend::new(width, 2)).unwrap();
                terminal
                    .draw(|frame| draw_footer(frame, &app, frame.area()))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let last: String = (0..width).map(|x| buffer[(x, 1)].symbol()).collect();
                assert!(last.ends_with(version));
                if width == 32 || mode == InputMode::Normal {
                    assert!(last.contains('…'));
                }
                assert_eq!(buffer[(width - version.len() as u16 - 1, 1)].symbol(), " ");
                if mode == InputMode::Normal {
                    assert!(last.starts_with("Status: "));
                }
            }
        }
    }

    #[test]
    fn footer_version_handles_offsets_and_tiny_areas() {
        let app = App::new(crate::demo::connections(), 100, true, true);
        let version = concat!("v", env!("CARGO_PKG_VERSION"));
        let mut terminal = Terminal::new(TestBackend::new(120, 6)).unwrap();
        let area = Rect::new(3, 2, 100, 2);
        terminal
            .draw(|frame| draw_footer(frame, &app, area))
            .unwrap();
        let last: String = (area.x..area.right())
            .map(|x| terminal.backend().buffer()[(x, area.bottom() - 1)].symbol())
            .collect();
        assert!(last.ends_with(version));
        for width in [0, 1, version.len() as u16] {
            terminal
                .draw(|frame| draw_footer(frame, &app, Rect::new(0, 0, width, 2)))
                .unwrap();
        }
        terminal
            .draw(|frame| draw_footer(frame, &app, Rect::new(0, 0, 100, 0)))
            .unwrap();
    }

    #[test]
    fn pending_actions_stay_visible_on_long_explorer_labels_without_displacing_metrics() {
        let mut app = App::new(
            vec![crate::config::Connection {
                name: "a-very-long-runtime-name-".repeat(5),
                kind: BackendKind::Docker,
                endpoint: "unix:///fixture.sock".into(),
            }],
            100,
            true,
            false,
        );
        app.engines[0].runtime = Some(crate::runtime::Target::Colima {
            profile: "fixture".into(),
        });
        app.update_engine(0, Ok(crate::model::Snapshot::default()));
        app.request_action(crate::actions::Action::Stop);
        app.action_review_visible = true;
        app.confirm_action();
        let mut terminal = Terminal::new(TestBackend::new(90, 16)).unwrap();
        terminal
            .draw(|frame| draw_tree(frame, &mut app, frame.area()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Stopping…"));
        assert!(text.chars().any(|ch| "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(ch)));
        assert!(text.contains("0.0%"));
        assert!(text.contains("0 B"));
        assert!(!app.action_results_visible);
        assert!(app.engines[0].error.is_none());
    }

    #[test]
    fn container_overview_shows_host_ports_and_does_not_claim_reachability() {
        let mut app = App::new(
            vec![crate::config::Connection {
                name: "fixture".into(),
                kind: crate::config::BackendKind::Docker,
                endpoint: "unix:///fixture.sock".into(),
            }],
            100,
            false,
            true,
        );
        app.update_engine(
            0,
            Ok(crate::model::Snapshot {
                containers: vec![crate::model::Container {
                    id: "fixture".into(),
                    name: "api".into(),
                    state: "running".into(),
                    port_bindings: vec![
                        crate::model::PortBinding {
                            host_ip: Some("127.0.0.1".into()),
                            host_port: Some(8080),
                            container_port: 80,
                            protocol: "tcp".into(),
                        },
                        crate::model::PortBinding {
                            host_ip: Some("0.0.0.0".into()),
                            host_port: Some(5353),
                            container_port: 53,
                            protocol: "udp".into(),
                        },
                        crate::model::PortBinding {
                            host_ip: None,
                            host_port: None,
                            container_port: 443,
                            protocol: "tcp".into(),
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        let text: String = overview_lines(&app)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(text.contains("localhost:8080 → 80/tcp (loopback only"));
        assert!(text.contains("0.0.0.0:5353 → 53/udp (all interfaces"));
        assert!(text.contains("443/tcp — exposed, no host port"));
        assert!(text.contains("reachability not verified"));
        app.update_engine(0, Err("fixture offline".into()));
        let text: String = overview_lines(&app)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(text.contains("Last-known bindings (offline)"));
    }

    #[test]
    fn runtime_confirmation_warns_about_all_workloads_and_returns_to_navigation() {
        use crate::{actions::Action, runtime::Target};
        let mut app = App::new(
            vec![crate::config::Connection {
                name: "fixture".into(),
                kind: crate::config::BackendKind::Docker,
                endpoint: "unix:///fixture.sock".into(),
            }],
            100,
            false,
            false,
        );
        app.engines[0].runtime = Some(Target::Colima {
            profile: "fixture".into(),
        });
        app.update_engine(0, Ok(crate::model::Snapshot::default()));
        app.request_action(Action::Stop);
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal
            .draw(|frame| draw_action(frame, &mut app, frame.area()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Stop Colima profile fixture?"));
        assert!(text.contains("all workloads"));
        assert!(text.contains("y / Enter Confirm    n / Esc Cancel"));
        assert!(!text.contains("colima --profile fixture stop"));
        assert!(!text.contains("Endpoint:"));
        assert!(!text.contains("Existing containers only"));
        let occupied: Vec<_> = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .enumerate()
            .filter(|(_, cell)| !cell.symbol().trim().is_empty())
            .map(|(index, _)| (index % 120, index / 120))
            .collect();
        assert!(
            occupied.iter().map(|(x, _)| x).max().unwrap()
                - occupied.iter().map(|(x, _)| x).min().unwrap()
                < 56
        );
        assert!(
            occupied.iter().map(|(_, y)| y).max().unwrap()
                - occupied.iter().map(|(_, y)| y).min().unwrap()
                < 10
        );
        app.confirm_action();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(!app.action_results_visible);
        app.action_results_visible = true;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Pending / checking runtime"));
    }
    use ratatui::{Terminal, backend::TestBackend};

    fn render(width: u16, height: u16, detail: bool) -> String {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.detail_focus = detail;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn wide_layout_has_explorer_and_relationship_details() {
        let text = render(120, 35, false);
        assert!(text.contains("Tha"));
        assert!(text.contains("Explorer"));
        assert!(text.contains("OrbStack"));
        assert!(text.contains("shop-api-1"));
    }

    #[test]
    fn narrow_and_tiny_terminals_render_without_panicking() {
        assert!(render(60, 20, false).contains("Explorer"));
        assert!(render(60, 20, true).contains("Overview"));
        assert!(render(20, 5, false).contains("Tha"));
    }

    #[test]
    fn inherited_theme_does_not_paint_backgrounds_or_rgb_colors() {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        for cell in &terminal.backend().buffer().content {
            assert_eq!(cell.bg, Color::Reset);
            assert!(!matches!(cell.fg, Color::Rgb(_, _, _)));
        }
    }

    #[test]
    fn log_search_highlights_occurrences_without_hiding_surrounding_entries() {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        app.detail_focus = true;
        app.tab = Tab::Logs;
        app.logs.push("api", "before context", false);
        app.logs.push("api", "ERROR and error", false);
        app.logs.push("api", "after context", false);
        app.log_view.set_query("error".into(), &mut app.logs);
        app.log_view.first(&mut app.logs);
        let mut terminal = Terminal::new(TestBackend::new(120, 25)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("before context"));
        assert!(text.contains("after context"));
        assert!(text.contains("1/2 matches"));
        assert!(text.contains("Wrap: on"));
        assert!(
            buffer
                .content
                .iter()
                .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
                .count()
                >= 10
        );
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.modifier.contains(Modifier::UNDERLINED))
        );
        assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
    }

    #[test]
    fn wrapped_highlights_intersect_utf8_ranges_without_losing_text() {
        let text = "ไทย ÉRROR next";
        let regex = regex::RegexBuilder::new("érror")
            .case_insensitive(true)
            .build()
            .unwrap();
        let active = regex.find(text).unwrap().range();
        let mut joined = String::new();
        let mut highlighted_bytes = 0;
        for range in log_view::wrap_ranges(text, 4) {
            for span in
                highlighted(text, range, Some(&regex), Some(&active), Style::default()).spans
            {
                joined.push_str(&span.content);
                if span.style.add_modifier.contains(Modifier::REVERSED) {
                    highlighted_bytes += span.content.len();
                }
            }
        }
        assert_eq!(joined, text);
        assert_eq!(highlighted_bytes, active.len());
    }

    #[test]
    fn project_network_view_hides_outside_workloads_on_shared_resources() {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        let mut snapshot = app.engines[0].snapshot.as_ref().unwrap().clone();
        snapshot.containers.push(crate::model::Container {
            id: "outside".into(),
            name: "outside-private-container".into(),
            project: Some("another".into()),
            networks: vec!["shop_default".into()],
            ..Default::default()
        });
        snapshot.networks[0]
            .containers
            .push("outside-private-container".into());
        snapshot.networks[0].external = true;
        app.update_engine(0, Ok(snapshot));
        app.open_selected();
        app.tab = Tab::Networks;
        let text: String = network_lines(&app)
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
            .collect();
        assert!(!text.contains("outside-private-container"));
        assert!(text.contains("shop-api-1"));
        assert!(text.contains("outside this project (hidden)"));
    }

    #[test]
    fn cheatsheet_is_scrollable_and_shows_runtime_wrap_shortcut() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.help = true;
        app.help_scroll = 18;
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Cheatsheet"));
        assert!(text.contains("Toggle wrapping on/off immediately"));
        assert!(text.contains("Scroll"));
    }

    #[test]
    fn performance_hides_offline_and_alias_contexts_and_recovers_from_stale_scroll() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.update_engine(0, Err("stopped".into()));
        app.engines[2].alias = Some("Colima / development".into());
        app.performance_open = true;
        app.performance_focus = true;
        app.performance_scroll = usize::MAX;
        app.performance_offset = usize::MAX;
        let mut terminal = Terminal::new(TestBackend::new(90, 35)).unwrap();
        terminal
            .draw(|frame| draw_sidebar(frame, &mut app, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let performance_start = 35 - performance_height(&app, 35);
        let text: String = (performance_start..35)
            .flat_map(|y| (0..90).map(move |x| buffer[(x, y)].symbol()))
            .collect();
        assert!(text.contains("Performance · Connected runtimes"));
        assert!(text.contains("Colima / development · CPU 59.0%"));
        assert!(!text.contains("OrbStack"));
        assert!(!text.contains("containerd"));
        assert!(!text.contains("Unavailable"));
        assert_eq!(app.performance_scroll, 0);
        assert_eq!(app.performance_offset, 0);
        let header_y = (performance_start..35)
            .find(|&y| {
                let line: String = (0..90).map(|x| buffer[(x, y)].symbol()).collect();
                line.contains("Colima / development · CPU")
            })
            .unwrap();
        assert!((0..90).any(|x| buffer[(x, header_y)].modifier.contains(Modifier::REVERSED)));

        app.update_engine(1, Err("stopped".into()));
        app.update_engine(2, Err("stopped".into()));
        terminal
            .draw(|frame| draw_sidebar(frame, &mut app, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let performance_start = 35 - performance_height(&app, 35);
        let text: String = (performance_start..35)
            .flat_map(|y| (0..90).map(move |x| buffer[(x, y)].symbol()))
            .collect();
        assert!(text.contains("No connected runtimes"));
        assert!(app.rows.iter().any(|row| row.title.contains("(offline)")));
    }

    #[test]
    fn performance_viewport_keeps_the_last_selection_visible_without_blank_tail_space() {
        let mut app = App::new(
            (0..5)
                .map(|index| crate::config::Connection {
                    name: format!("Runtime{index}"),
                    kind: BackendKind::Docker,
                    endpoint: format!("unix:///runtime{index}.sock"),
                })
                .collect(),
            100,
            true,
            true,
        );
        for index in 0..5 {
            app.update_engine(
                index,
                Ok(crate::model::Snapshot {
                    engine_id: Some(format!("engine{index}")),
                    ..Default::default()
                }),
            );
        }
        app.performance_open = true;
        app.performance_scroll = 4;
        let mut terminal = Terminal::new(TestBackend::new(90, 35)).unwrap();
        terminal
            .draw(|frame| draw_sidebar(frame, &mut app, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let performance_start = 35 - performance_height(&app, 35);
        let text: String = (performance_start..35)
            .flat_map(|y| (0..90).map(move |x| buffer[(x, y)].symbol()))
            .collect();
        for name in ["Runtime2", "Runtime3", "Runtime4"] {
            assert!(text.contains(name));
        }
        assert!(!text.contains("Runtime0"));
        assert!(!text.contains("Runtime1"));
        assert_eq!(app.performance_offset, 2);
    }

    #[test]
    fn performance_graphs_live_in_the_left_dock_not_the_detail_tab() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.performance_open = true;
        let mut terminal = Terminal::new(TestBackend::new(140, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("Workload"));
        assert!(text.contains("CPU"));
        assert!(text.contains("Mem"));
        assert!(text.contains("Performance"));
        assert!(text.contains("scale"));
        assert!(text.contains("1s target"));
        assert!(text.contains("MiB"));
        assert!(text.chars().any(|ch| "▁▂▃▄▅▆▇█".contains(ch)));
        let right: String = (0..35)
            .flat_map(|y| (70..140).map(move |x| buffer[(x, y)].symbol()))
            .collect();
        assert!(!right.contains("scale"));
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| cell.bg == Color::Reset && !matches!(cell.fg, Color::Rgb(_, _, _)))
        );
    }

    #[test]
    fn performance_is_collapsed_by_default_and_global_inside_a_compose_workspace() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.open_selected();
        let mut terminal = Terminal::new(TestBackend::new(140, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("▸ Performance"));
        assert!(!text.contains("Colima / development"));
        app.performance_open = true;
        app.performance_scroll = app
            .performance_engines()
            .iter()
            .position(|&index| index == 1)
            .unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Colima / development"));
        assert!(text.contains("CPU 59.0%"));
    }

    #[test]
    fn padding_adapts_to_terminal_cells_and_single_container_platform_is_visible() {
        assert_eq!(padded(Rect::new(0, 0, 60, 20)), Rect::new(2, 1, 56, 18));
        assert_eq!(padded(Rect::new(0, 0, 20, 5)), Rect::new(1, 0, 18, 5));
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(&row.scope, Scope::Container(key) if key == "/api-1"))
            .unwrap();
        let text: String = overview_lines(&app)
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
            .collect();
        assert!(text.contains("Platform"));
        assert!(text.contains("linux/arm64 (ARM64)"));
        app.selected = app
            .rows
            .iter()
            .position(
                |row| matches!(&row.scope, Scope::Service { service, .. } if service == "postgres"),
            )
            .unwrap();
        let text: String = overview_lines(&app)
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
            .collect();
        assert!(text.contains("linux/amd64 (AMD64 / x86_64)"));
    }

    #[test]
    fn colored_circles_and_text_labels_render_running_starting_and_error_states() {
        let mut app = App::new(
            vec![crate::config::Connection {
                name: "test".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///test".into(),
            }],
            100,
            false,
            true,
        );
        let containers = [
            ("case-green", "healthy"),
            ("case-yellow", "starting"),
            ("case-red", "unhealthy"),
        ]
        .into_iter()
        .map(|(name, health)| crate::model::Container {
            id: name.into(),
            name: name.into(),
            state: "running".into(),
            health: Some(health.into()),
            project: Some("project".into()),
            service: Some("service".into()),
            ..Default::default()
        })
        .collect();
        app.update_engine(
            0,
            Ok(crate::model::Snapshot {
                containers,
                ..Default::default()
            }),
        );
        app.tree_query = "case-".into();
        app.refresh_tree();
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        for color in [Color::Green, Color::Yellow, Color::Red] {
            assert!(
                buffer
                    .content
                    .iter()
                    .any(|cell| cell.fg == color && ["●", "◐"].contains(&cell.symbol()))
            );
        }
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("[starting]"));
        assert!(text.contains("[unhealthy]"));
        assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
    }

    #[test]
    fn disconnected_context_is_a_warning_with_neutral_names_and_overview_diagnostics() {
        let mut app = App::new(
            vec![crate::config::Connection {
                name: "Colima".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///colima.sock".into(),
            }],
            100,
            false,
            true,
        );
        let diagnostic = "unique socket diagnostic: connection refused";
        app.update_engine(0, Err(diagnostic.into()));
        assert_eq!(app.condition_for(&app.rows[0]).unwrap().tone, Tone::Warning);
        app.performance_open = true;
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("Colima (offline)"));
        assert!(text.contains("Connection unavailable"));
        assert!(text.contains(diagnostic));
        assert!(buffer.content.iter().any(|cell| cell.fg == Color::Yellow));
        assert!(buffer.content.iter().all(|cell| cell.fg != Color::Red));
        for y in 0..40 {
            let line: String = (0..160).map(|x| buffer[(x, y)].symbol()).collect();
            if let Some(x) = line.find("Colima") {
                // Names are ASCII; fixture rows contain only single-column symbols.
                let x = line[..x].chars().count() as u16;
                for offset in 0..6 {
                    assert_eq!(buffer[(x + offset, y)].fg, Color::Reset);
                }
            }
        }
        app.tab = Tab::Networks;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("see Overview for details"));
        assert!(!text.contains(diagnostic));
        app.monochrome = true;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.fg == Color::Reset)
        );
    }

    #[test]
    fn stale_inventory_warns_and_recovery_restores_unhealthy_red_indicator() {
        let mut app = App::new(
            vec![crate::config::Connection {
                name: "Colima".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///colima.sock".into(),
            }],
            100,
            false,
            true,
        );
        let snapshot = crate::model::Snapshot {
            containers: vec![crate::model::Container {
                id: "api".into(),
                name: "api".into(),
                state: "running".into(),
                health: Some("unhealthy".into()),
                project: Some("shop".into()),
                service: Some("api".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        app.update_engine(0, Ok(snapshot.clone()));
        assert_eq!(
            app.condition_for(&app.rows[0]).unwrap(),
            Condition::new(Tone::Warning, "workload issues")
        );
        app.update_engine(0, Err("socket unavailable".into()));
        assert_eq!(app.condition_for(&app.rows[0]).unwrap().tone, Tone::Warning);
        let lines = overview_lines(&app);
        assert!(
            lines
                .iter()
                .any(|line| line.to_string().contains("Showing stale, last-known data"))
        );
        app.update_engine(0, Ok(snapshot));
        assert_eq!(
            app.condition_for(&app.rows[0]).unwrap(),
            Condition::new(Tone::Warning, "workload issues")
        );
        app.performance_open = true;
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(!text.contains("[workload issues]"));
        assert!(!text.contains("[errors]"));
        assert!(text.contains("shop [1/1 running]"));
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.fg == Color::Yellow && cell.symbol() == "◐")
        );
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.fg == Color::Red && cell.symbol() == "●")
        );
    }

    #[test]
    fn logs_have_a_scrollbar_and_display_line_position_in_both_wrap_modes() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.detail_focus = true;
        app.tab = Tab::Logs;
        app.log_view.wrap = false;
        for n in 0..100 {
            app.logs
                .push("api", &format!("{n}: {}", "long payload ".repeat(8)), false);
        }
        let mut terminal = Terminal::new(TestBackend::new(140, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let (start, total) = app.log_view.scroll_position(&app.logs);
        assert!(start > 0);
        assert_eq!(total, 100);
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Rows"));
        assert!(text.contains("↑"));
        assert!(text.contains("↓"));
        app.log_view.first(&mut app.logs);
        app.log_view.toggle_wrap(&app.logs);
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let (start, total) = app.log_view.scroll_position(&app.logs);
        assert_eq!(start, 0);
        assert!(total > 100);
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Rows 1–"));
        assert!(text.contains("Paused"));
    }

    #[test]
    fn lifecycle_confirmation_lists_targets_and_small_terminals_cannot_confirm() {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        app.demo = false;
        app.request_action(crate::actions::Action::Stop);
        assert!(app.action_confirmation.is_some());
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(app.action_review_visible);
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for expected in [
            "Stop shop?",
            "Runtime: OrbStack",
            "3 target(s)",
            "shop-api-1",
            "y / Enter Confirm",
            "n / Esc Cancel",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        let mut tiny = Terminal::new(TestBackend::new(35, 8)).unwrap();
        tiny.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(!app.action_review_visible);
        app.confirm_action();
        assert!(app.action_request.is_none());
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.confirm_action();
        assert!(app.action_request.is_some());
        app.action_progress_mut().unwrap().finished = true;
        app.action_progress_mut().unwrap().results[0] =
            Some(crate::actions::Status::Unknown("lost response".into()));
        app.action_results_visible = true;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Outcome unknown"));
        assert!(text.contains("inspect state before retrying"));
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.bg == Color::Reset)
        );
    }

    #[test]
    fn contextual_actions_show_disabled_reasons_and_layers_show_separate_history() {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        app.open_actions();
        let mut terminal = Terminal::new(TestBackend::new(140, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Actions"));
        assert!(text.contains("Start · disabled"));
        assert!(text.contains("Demo mode"));
        app.picker = None;
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Container(_)))
            .unwrap();
        app.tab = Tab::Layers;
        app.detail_focus = true;
        app.layers.state = crate::layers::State::Ready(crate::layers::Image {
            id: "sha256:resolved".into(),
            filesystem: vec!["sha256:base-diff".into()],
            history: vec![crate::layers::History {
                instruction: "ENV MODE=production".into(),
                comment: "explicit OCI metadata".into(),
                size: Some(0),
                empty_layer: Some(true),
            }],
            warnings: vec![],
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("7 Layers"));
        assert!(text.contains("Filesystem layers (1)"));
        assert!(text.contains("Build history (1)"));
        app.layers.selected = 1;
        app.layers.expanded = true;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("may contain secrets"));
        assert!(text.contains("Explicit metadata-only step"));
        assert!(text.contains("ENV MODE=production"));
    }

    #[test]
    fn log_dialogs_explain_filtering_and_require_visible_private_export_review() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.tab = Tab::Logs;
        app.detail_focus = true;
        app.input_mode = InputMode::LogFilter;
        let mut terminal = Terminal::new(TestBackend::new(140, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Filter retained logs"));
        assert!(text.contains("JSON"));
        assert!(text.contains("RFC3339"));
        app.input_mode = InputMode::ExportLogs;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(app.log_export_review_visible);
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("SECRETS"));
        assert!(text.contains("0600"));
        assert!(text.contains("existing files/symlinks refused"));
        let mut tiny = Terminal::new(TestBackend::new(50, 15)).unwrap();
        tiny.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(!app.log_export_review_visible);
    }
    #[test]
    fn ellipsis_uses_display_cells_and_preserves_styles_and_whole_graphemes() {
        assert_eq!(ellipsize_line(Line::from("exact"), 5).to_string(), "exact");
        assert_eq!(ellipsize_line(Line::from("overflow"), 0).to_string(), "");
        assert_eq!(ellipsize_line(Line::from("overflow"), 1).to_string(), "…");
        assert_eq!(ellipsize_line(Line::from("abcdef"), 5).to_string(), "abcd…");
        assert_eq!(ellipsize_line(Line::from("界界界"), 4).to_string(), "界…");
        assert_eq!(
            ellipsize_line(Line::from("e\u{301}e\u{301}e\u{301}"), 2).to_string(),
            "e\u{301}…"
        );
        assert_eq!(
            ellipsize_line(Line::from("👩‍💻abcdef"), 4).to_string(),
            "👩‍💻a…"
        );
        let line = Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Red)),
            Span::raw("a long workload"),
        ]);
        let clipped = ellipsize_line(line, 8);
        assert_eq!(clipped.to_string(), "● a lon…");
        assert_eq!(clipped.spans[0].style.fg, Some(Color::Red));
        assert_eq!(clipped.spans[1].style.fg, None);
        assert!(clipped.width() <= 8);
    }

    #[test]
    fn explorer_marks_truncated_labels_without_changing_metrics_or_full_overview_names() {
        let name = "a-very-long-runtime-name-".repeat(5);
        let mut app = App::new(
            vec![crate::config::Connection {
                name: name.clone(),
                kind: BackendKind::Docker,
                endpoint: "unix:///fixture.sock".into(),
            }],
            100,
            false,
            true,
        );
        app.update_engine(0, Ok(crate::model::Snapshot::default()));
        for width in [35, 52, 78] {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            terminal
                .draw(|frame| draw_tree(frame, &mut app, frame.area()))
                .unwrap();
            let inner = padded(Block::bordered().inner(Rect::new(0, 0, width, 12)));
            let metrics = inner.width >= 44;
            let label_width = if metrics {
                inner.width - 22
            } else {
                inner.width
            };
            let y = inner.y + u16::from(metrics);
            let buffer = terminal.backend().buffer();
            let label: String = (inner.x..inner.x + label_width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            assert!(label.ends_with('…'), "width {width}: {label:?}");
            if metrics {
                let values: String = (inner.x + label_width..inner.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                assert!(values.contains("0.0%"), "{values:?}");
                assert!(values.contains("0 B"), "{values:?}");
                assert!(!values.contains('…'));
            }
        }
        let overview: String = overview_lines(&app)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(overview.contains(&name));
        assert_eq!(app.engines[0].connection.name, name);
    }
}
