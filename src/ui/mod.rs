use crate::{
    app::{App, Choice, InputMode, ResourceKind, Scope, Tab},
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
        Block, Borders, Cell, Clear, LineGauge, Paragraph, Row as TableRow, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Sparkline, Table, Tabs, Wrap,
    },
};

pub const COMMANDS: &[(&str, &str)] = &[
    ("overview", "Show workload overview"),
    ("logs", "Open logs for the selected workload"),
    ("networks", "Show network membership"),
    ("refresh", "Refresh all engine snapshots"),
    ("reconnect-logs", "Reconnect the current log streams"),
    ("wrap", "Toggle log wrapping (w in logs)"),
    ("exec", "Open a shell in the selected container"),
    (
        "layers",
        "Inspect resolved image filesystem layers and build history (5)",
    ),
    ("reload-layers", "Retry image layers lookup (read-only)"),
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

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.action_review_visible = false;
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
    let layout = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(area);
    let connected = app
        .engines
        .iter()
        .filter(|engine| {
            engine.snapshot.is_some() && engine.error.is_none() && engine.alias.is_none()
        })
        .count();
    let header = Line::from(vec![
        Span::styled(" ⊤ Tha ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if app.demo { " Demo · " } else { " " }),
        Span::styled(
            format!("{connected} connected / {} configured", app.engines.len()),
            muted(),
        ),
    ]);
    let breadcrumb = if let Some(project) = &app.project {
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
    if area.width >= 110 {
        let tree_width = (area.width / 2).clamp(52, 72);
        let panes = Layout::horizontal([Constraint::Length(tree_width), Constraint::Min(1)])
            .split(layout[1]);
        draw_sidebar(frame, app, panes[0]);
        draw_details(frame, app, panes[1]);
    } else if app.detail_focus {
        draw_details(frame, app, layout[1]);
    } else {
        draw_sidebar(frame, app, layout[1]);
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
    if app.action_confirmation.is_some() || app.action_progress.is_some() {
        draw_action(frame, app, area);
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
    use crate::actions::{Action, Status};
    let dialog = centered(area, 100, area.height.saturating_sub(2).min(28));
    frame.render_widget(Clear, dialog);
    if area.width < 60 || area.height < 18 {
        frame.render_widget(Paragraph::new("Resize to at least 60 × 18 to review lifecycle targets.\nEsc cancels confirmation; running actions continue.").wrap(Wrap { trim: false }).block(Block::bordered().title(" Lifecycle ")), dialog);
        return;
    }
    let (request, scroll, finished) = if let Some(confirmation) = &app.action_confirmation {
        (&confirmation.request, confirmation.scroll, None)
    } else if let Some(progress) = &app.action_progress {
        (&progress.request, progress.scroll, Some(progress.finished))
    } else {
        return;
    };
    let title = match finished {
        None => format!(" Confirm {} ", request.action.label()),
        Some(false) => format!(" {} · Running ", request.action.label()),
        Some(true) => format!(" {} · Results ", request.action.label()),
    };
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
            Line::from(format!(
                "{}: {} · {} target(s), {} skipped",
                request.action.label(),
                safe_text(&request.name),
                request.containers.len(),
                request.skipped
            )),
        ]),
        parts[0],
    );
    let mut lines = vec![
        Line::from(format!(
            "Endpoint: {}",
            safe_text(&request.connection.endpoint)
        )),
        Line::from(format!("Runtime ID: {}", safe_text(&request.identity))),
        Line::styled(
            "Existing containers only; no Compose dependency orchestration.",
            muted(),
        ),
    ];
    if request.action == Action::Delete {
        lines.push(Line::styled(
            "Container writable data is lost. Networks, ALL volumes and images stay.",
            accent(app, Color::Yellow),
        ));
    } else if matches!(request.action, Action::Stop | Action::Restart) {
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
        if let Some(progress) = &app.action_progress {
            let (text, style) = match &progress.results[index] {
                Some(Status::Succeeded) => ("Succeeded".into(), accent(app, Color::Green)),
                Some(status @ Status::Failed(_)) => (status.text(), accent(app, Color::Red)),
                Some(status) => (status.text(), accent(app, Color::Yellow)),
                None => ("Pending / checking safety".into(), muted()),
            };
            lines.push(Line::styled(safe_text(&text), style));
        }
        lines.push(Line::from(""));
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(parts[1].width)
        .saturating_sub(parts[1].height as usize);
    let scroll = scroll.min(max_scroll).min(u16::MAX as usize);
    frame.render_widget(paragraph.scroll((scroll as u16, 0)), parts[1]);
    let deleting = request.action == Action::Delete;
    let confirmation_name = safe_text(&request.name);
    let prompt = if let Some(confirmation) = &mut app.action_confirmation {
        confirmation.scroll = scroll;
        app.action_review_visible = true;
        let hint = if deleting {
            format!(
                "Type {} then Enter to DELETE\n> {} · Esc Cancel",
                confirmation_name,
                safe_text(&confirmation.typed)
            )
        } else {
            "y Confirm · Enter/Esc Cancel (default)".into()
        };
        format!(
            "{hint}\n{}",
            confirmation
                .error
                .as_deref()
                .map(safe_text)
                .unwrap_or_default()
        )
    } else {
        let progress = app.action_progress.as_mut().unwrap();
        progress.scroll = scroll;
        if progress.finished {
            "Enter/Esc Close · Inventory refresh requested\nNo automatic retry or rollback; inspect unknown outcomes.".into()
        } else {
            "Checking / applying confirmed targets. Please wait.\nQuitting cannot undo a request already sent to the runtime.".into()
        }
    };
    frame.render_widget(Paragraph::new(prompt).wrap(Wrap { trim: false }), parts[2]);
}

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let height = if app.performance_open && area.height >= 12 {
        (area.height / 2).clamp(6, 18)
    } else {
        1
    };
    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(height)]).split(area);
    draw_tree(frame, app, parts[0]);
    if height == 1 {
        frame.render_widget(
            Paragraph::new(" ▸ Performance · All runtimes (p)").style(if app.performance_focus {
                selected()
            } else {
                muted()
            }),
            parts[1],
        );
        return;
    }
    let block = pane(
        " ▾ Performance · All runtimes · p ".into(),
        app.performance_focus,
    )
    .title_bottom(" j/k Scroll · Tab Pane ");
    let inner = padded(block.inner(parts[1]));
    frame.render_widget(block, parts[1]);
    app.performance_scroll = app
        .performance_scroll
        .min(app.engines.len().saturating_sub(1));
    let mut y = inner.y;
    for &index in app.context_order.iter().skip(app.performance_scroll) {
        if y + 3 >= inner.bottom() {
            break;
        }
        draw_usage(frame, app, index, Rect::new(inner.x, y, inner.width, 4));
        y += 4;
    }
}

fn pane(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(if focused {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        })
}

fn draw_tree(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = pane(
        if app.tree_query.is_empty() {
            " Explorer ".into()
        } else {
            format!(" Explorer / {} ", safe_text(&app.tree_query))
        },
        !app.detail_focus && !app.performance_focus,
    );
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    let show_metrics = inner.width >= 44;
    let height = inner.height.saturating_sub(u16::from(show_metrics)) as usize;
    if app.selected < app.tree_offset {
        app.tree_offset = app.selected;
    }
    if app.selected >= app.tree_offset + height {
        app.tree_offset = app.selected.saturating_sub(height.saturating_sub(1));
    }
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
            let condition = app.condition_for(row);
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
            if let Some(condition) = condition {
                name.push(Span::styled(
                    format!("{} ", condition.symbol()),
                    condition_style(app, condition).remove_modifier(Modifier::REVERSED),
                ));
            }
            name.push(Span::raw(safe_text(&row.title)));
            if let Some(condition) = condition.filter(|condition| {
                matches!(condition.tone, Tone::Error | Tone::Warning)
                    && (!matches!(row.scope, Scope::Engine)
                        || app.engines[row.engine].error.is_none()
                            && app.engines[row.engine].snapshot.is_some())
            }) {
                name.push(Span::styled(
                    format!(" [{}]", condition.label),
                    condition_style(app, condition),
                ));
            }
            let mut cells = vec![Cell::from(Line::from(name))];
            if show_metrics {
                let (cpu, memory) =
                    if matches!(row.scope, Scope::Resource { .. } | Scope::Resources { .. }) {
                        (String::new(), String::new())
                    } else if app.engines[row.engine].alias.is_some() {
                        ("↪".into(), "alias".into())
                    } else {
                        let usage = app.metrics_for(row);
                        (usage.cpu_label(), usage.memory_label())
                    };
                cells.push(Cell::from(Line::from(cpu).alignment(Alignment::Right)));
                cells.push(Cell::from(Line::from(memory).alignment(Alignment::Right)));
            }
            TableRow::new(cells).style(style)
        })
        .collect();
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
        Tab::Layers => "5 Layers",
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
    let block = pane(format!(" {label}{exec_context} "), app.detail_focus);
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 2 || app.tab == Tab::Exec && inner.height < 4 {
        return;
    }
    let sections = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if app.tab == Tab::Exec { 2 } else { 1 }),
        Constraint::Min(1),
    ])
    .split(inner);
    let names = if inner.width >= 62 {
        vec!["1 Overview", "2 Logs", "3 Networks", "4 Exec", "5 Layers"]
    } else if inner.width >= 44 {
        vec!["1 Info", "2 Logs", "3 Net", "4 Exec", "5 Layers"]
    } else {
        vec!["1", "2", "3", "4", "5"]
    };
    let active = match app.tab {
        Tab::Overview => 0,
        Tab::Logs => 1,
        Tab::Networks => 2,
        Tab::Exec => 3,
        Tab::Layers => 4,
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
        Tab::Overview | Tab::Networks => {
            let lines = if app.tab == Tab::Networks {
                network_lines(app)
            } else {
                overview_lines(app)
            };
            let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
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
    let mut lines = vec![Line::from(format!("Resolved image: {}", image.id))];
    let mut selected_line = 0;
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
            lines.push(Line::from(format!("Instruction: {}", step.instruction)));
            if !step.comment.is_empty() {
                lines.push(Line::from(format!("Comment: {}", step.comment)));
            }
        }
    } else {
        lines.push(Line::styled(
            "j/k Select · Enter Details · :reload-layers Retry",
            muted(),
        ));
        lines.push(Line::from(format!(
            "Filesystem layers ({}) · base → top · sizes unavailable",
            image.filesystem.len()
        )));
        for (index, id) in image.filesystem.iter().enumerate() {
            if index == app.layers.selected {
                selected_line = lines.len();
            }
            lines.push(Line::styled(
                format!(
                    "{} {}. {id}",
                    if index == app.layers.selected {
                        "▸"
                    } else {
                        " "
                    },
                    index + 1
                ),
                if index == app.layers.selected {
                    selected()
                } else {
                    Style::default()
                },
            ));
        }
        lines.push(Line::from(format!(
            "Build history ({}) · base → top · separate from diff IDs",
            image.history.len()
        )));
        for (index, step) in image.history.iter().enumerate() {
            let active = index + image.filesystem.len() == app.layers.selected;
            if active {
                selected_line = lines.len();
            }
            lines.push(Line::styled(
                format!(
                    "{} {}. {} · {}{}",
                    if active { "▸" } else { " " },
                    index + 1,
                    step.size
                        .map(metrics::bytes)
                        .unwrap_or_else(|| "size unavailable".into()),
                    "instruction hidden (Enter details)",
                    if step.empty_layer == Some(true) {
                        " [metadata-only]"
                    } else {
                        ""
                    }
                ),
                if active { selected() } else { Style::default() },
            ));
        }
        for warning in &image.warnings {
            lines.push(Line::styled(warning.clone(), accent(app, Color::Yellow)));
        }
        lines.push(Line::styled(
            "Build history may contain secrets; Enter reveals the selected step.",
            muted(),
        ));
    }
    if !app.layers.expanded && image.rows() > 0 {
        let position = Paragraph::new(lines[..selected_line].to_vec())
            .wrap(Wrap { trim: false })
            .line_count(area.width);
        if position < app.detail_scroll {
            app.detail_scroll = position;
        }
        if position >= app.detail_scroll + area.height as usize {
            app.detail_scroll = position.saturating_sub(area.height.saturating_sub(1) as usize);
        }
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

fn draw_usage(frame: &mut Frame, app: &App, index: usize, area: Rect) {
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
            "{} · CPU {}",
            safe_text(&engine.connection.name),
            summary.cpu_label()
        )),
    ]);
    frame.render_widget(
        Paragraph::new(header).style(
            if app.performance_focus && app.performance_scroll == index {
                selected()
            } else {
                Style::default()
            },
        ),
        line(0),
    );
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
            for port in &container.ports {
                lines.push(Line::from(format!("  Port       {}", safe_text(port))));
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

fn network_lines(app: &App) -> Vec<Line<'static>> {
    let Some(row) = app.current() else {
        return vec![];
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
        return lines;
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
        return lines;
    }
    let Some(snapshot) = &engine.snapshot else {
        lines.push(Line::from("Engine has not returned an inventory."));
        return lines;
    };
    let containers = app.selected_containers();
    let engine_scope = matches!(row.scope, Scope::Engine);
    let mut count = 0;
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
        for name in &network.containers {
            if app.project.as_ref().is_some_and(|project| {
                !snapshot
                    .containers
                    .iter()
                    .any(|c| &c.name == name && project.contains(c))
            }) {
                continue;
            }
            let project = snapshot
                .containers
                .iter()
                .find(|c| &c.name == name)
                .and_then(|c| c.project.as_deref())
                .unwrap_or("Standalone");
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
    lines
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
    let status = format!(
        "Wrap: {} · {} · {position}",
        if app.log_view.wrap { "on" } else { "off" },
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
            "{}/{} matches /{} · Filter: {}",
            app.log_view.match_index.unwrap_or(0),
            app.log_view.match_count,
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
            log_view::byte_at_column(&entry.text, app.log_view.horizontal)..entry.text.len()
        };
        let text = highlighted(
            &entry.text,
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

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let lines = if matches!(app.input_mode, InputMode::LogFilter | InputMode::ExportLogs) {
        vec![
            Line::styled("Log dialog · Enter Apply/Export · Esc Cancel", muted()),
            Line::from(safe_text(&app.notice)),
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
        let hints = if app.exec_input && app.tab == Tab::Exec {
            "Ctrl-] Navigate · SHELL INPUT · Keys go to the selected container session"
        } else if app.performance_focus {
            "Performance · ALL RUNTIMES · j/k Scroll  Enter Inspect  p Close  Tab Pane  ? Keys"
        } else if app.detail_focus && app.tab == Tab::Exec {
            "F6 Sessions · Enter Input · :close-exec Close slot · j/k Scroll · Tab Explorer · ? Keys"
        } else if app.detail_focus && app.tab == Tab::Logs {
            "? Keys  F3 Filter  :export-logs  w Wrap  / Search  n/N Match  j/k Scroll  f Follow  Tab Explorer"
        } else {
            "? Keys  a Actions  p Performance  Space Toggle  Enter Open  e Exec  Tab Pane  1..5 Views  : Commands"
        };
        vec![
            Line::styled(hints, muted()),
            Line::from(safe_text(&app.notice)),
        ]
    };
    frame.render_widget(Paragraph::new(lines), area);
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
        "p                 Expand/collapse bottom-left Performance\n",
        "o                 Sort contexts (pins first; usage is a snapshot)\n",
        "S / s / r / d     Start / Stop / Restart / Delete in Explorer\n",
        "                  Container or Compose rows only; confirmation required\n",
        "                  y confirms; Enter/Esc cancels; Delete: type name + Enter\n",
        "                  Delete stopped containers only; keep networks/volumes/images\n",
        "1 / 2 / 3 / 4 / 5 Overview / Logs / Networks / Exec / Layers\n",
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
        "                  Scrollbar + Rows readout track wrapped display lines\n",
        "Esc               Clear highlights without hiding log context\n",
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
        "{defaults}\n\nCustom navigation bindings (override defaults only in navigation)\n{custom}"
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

fn draw_picker(frame: &mut Frame, app: &App, area: Rect) {
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
    let lines: Vec<_> = picker
        .choices
        .iter()
        .enumerate()
        .skip(start)
        .take(inner.height as usize)
        .map(|(index, choice)| {
            let label = match choice {
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
        app.help_scroll = 14;
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
            .context_order
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
        assert!(text.contains("Colima [unavailable]"));
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
        assert!(text.contains("[workload issues]"));
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
            "Confirm Stop",
            "Runtime: OrbStack",
            "3 target(s)",
            "shop-api-1",
            "ID: api-1",
            "y Confirm",
            "Enter/Esc Cancel",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        let mut tiny = Terminal::new(TestBackend::new(50, 15)).unwrap();
        tiny.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(!app.action_review_visible);
        app.confirm_action();
        assert!(app.action_request.is_none());
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.confirm_action();
        assert!(app.action_request.is_some());
        app.action_progress.as_mut().unwrap().finished = true;
        app.action_progress.as_mut().unwrap().results[0] =
            Some(crate::actions::Status::Unknown("lost response".into()));
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
        assert!(text.contains("5 Layers"));
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
}
