//! Shared-time-axis analysis, using terminal-native filled graphs and a linked cursor.
use super::*;
use crate::history::{Point, RANGE_NAMES};

fn clock(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(ms / 1000)
        .map(|time| {
            format!(
                "{:02}:{:02}:{:02}",
                time.hour(),
                time.minute(),
                time.second()
            )
        })
        .unwrap_or_else(|_| "--:--:--".into())
}
fn stamp(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(ms / 1000)
        .map(|time| {
            format!(
                "{:04}-{:02}-{:02} {}",
                time.year(),
                time.month() as u8,
                time.day(),
                clock(ms)
            )
        })
        .unwrap_or_else(|_| clock(ms))
}
fn value(metric: usize, number: Option<f64>) -> String {
    match number {
        Some(number) if metric == 0 => format!("{number:.2}%"),
        Some(number) => format!(
            "{}{}",
            metrics::bytes(number.max(0.0) as u64),
            if metric > 1 { "/s" } else { "" }
        ),
        None => "unavailable".into(),
    }
}
fn time_axis(width: usize, start: i64, end: i64) -> String {
    let mut cells = vec![' '; width];
    let divisions = if width >= 80 {
        4
    } else if width >= 40 {
        2
    } else {
        1
    };
    for tick in 0..=divisions {
        if width < 24 && tick != divisions {
            continue;
        }
        let label = clock(start + (end - start) * tick / divisions);
        let length = label.chars().count();
        if length > width {
            continue;
        }
        let x = width.saturating_sub(1) * tick as usize / divisions as usize;
        let offset = if tick == 0 {
            0
        } else if tick == divisions {
            width - length
        } else {
            x.saturating_sub(length / 2).min(width - length)
        };
        for (index, ch) in label.chars().enumerate() {
            cells[offset + index] = ch;
        }
    }
    cells.into_iter().collect()
}
pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some((engine_index, container)) = app.performance_container() else {
        frame.render_widget(
            Paragraph::new("Select a container to analyze its history"),
            area,
        );
        return;
    };
    let view = &app.history_view;
    let block = pane(
        format!(" History · {} ", safe_text(&container.name)),
        app.detail_focus,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height < 5 {
        return;
    }
    let parts = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(inner);
    let (start, end) = if !view.data.points.is_empty() && view.data.end_ms > view.data.start_ms {
        (view.data.start_ms, view.data.end_ms)
    } else {
        view.bounds()
    };
    let source = view.error.as_deref().unwrap_or(&view.data.source);
    let counters = app.engines[engine_index]
        .metrics
        .samples
        .get(&container.key())
        .map(|sample| {
            let bytes =
                |number: Option<u64>| number.map(metrics::bytes).unwrap_or_else(|| "--".into());
            let u = &sample.usage;
            format!(
                "Latest counters ({}s old): Disk R/W {} / {} · Net Rx/Tx {} / {}",
                sample.sampled_at.elapsed().as_secs(),
                bytes(u.disk_read_bytes),
                bytes(u.disk_write_bytes),
                bytes(u.network_rx_bytes),
                bytes(u.network_tx_bytes)
            )
        })
        .unwrap_or_else(|| "Latest I/O counters unavailable".into());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!(
                "{} · {} → {} UTC · {}{}",
                RANGE_NAMES[view.range],
                if view.range >= 3 {
                    stamp(start)
                } else {
                    clock(start)
                },
                if view.range >= 3 {
                    stamp(end)
                } else {
                    clock(end)
                },
                if view.end_ms.is_some() {
                    "Paused"
                } else {
                    "Live"
                },
                if view.loading { " · Loading…" } else { "" }
            )),
            Line::styled(safe_text(source), muted()),
            Line::styled(counters, muted()),
        ]),
        parts[0],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "[/] Range · ←/→ Pan · c Cursor · h/l Inspect · f Live · z Enlarge",
                muted(),
            ),
            Line::styled(
                "j/k Metric (small terminal) · Esc Restore · · missing samples",
                muted(),
            ),
        ]),
        parts[2],
    );
    if view.data.points.is_empty() {
        frame.render_widget(Paragraph::new(view.error.as_deref().unwrap_or("No readings in this time range. Collection begins when Tha/daemon observes this container. [ selects a shorter range; f returns to live.")).wrap(Wrap{trim:false}),parts[1]);
        return;
    }
    let all = parts[1].height >= 24;
    let groups: &[usize] = if all {
        &[0, 1, 2, 3]
    } else {
        std::slice::from_ref(&view.metric)
    };
    let charts = Layout::vertical(vec![
        Constraint::Ratio(1, groups.len() as u32);
        groups.len()
    ])
    .split(parts[1]);
    let cursor = view
        .cursor
        .and_then(|index| view.data.points.get(index.min(view.data.points.len() - 1)));
    for (&group, &chart) in groups.iter().zip(charts.iter()) {
        draw_chart(
            frame,
            app,
            &view.data.points,
            cursor,
            group,
            chart,
            (start, end),
        );
    }
}
fn draw_chart(
    frame: &mut Frame,
    app: &App,
    points: &[Point],
    cursor: Option<&Point>,
    group: usize,
    area: Rect,
    bounds: (i64, i64),
) {
    let (start, end) = bounds;
    let (name, metrics, color): (&str, &[usize], Color) = match group {
        0 => ("CPU", &[0], Color::Cyan),
        1 => ("Memory", &[1], Color::Green),
        2 => ("Disk read / write", &[2, 3], Color::Magenta),
        _ => ("Network receive / transmit", &[4, 5], Color::Yellow),
    };
    let selected = cursor.or_else(|| points.last());
    let labels = metrics
        .iter()
        .map(|&metric| value(metric, selected.and_then(|point| point.values[metric])))
        .collect::<Vec<_>>()
        .join(" / ");
    let peak = points
        .iter()
        .filter_map(|point| point.peaks[metrics[0]])
        .reduce(f64::max);
    let block = Block::bordered()
        .title(format!(
            " {name}: {labels} · bucket avg{} ",
            cursor
                .map(|point| format!(
                    " @ {}",
                    if end - start >= crate::history::RETENTION_MS {
                        stamp(point.at_ms)
                    } else {
                        clock(point.at_ms)
                    }
                ))
                .unwrap_or_default()
        ))
        .title_bottom(format!(" peak {} ", value(metrics[0], peak)))
        .border_style(muted());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height < 2 {
        return;
    }
    let graph_height = inner.height - 1;
    let graphs = Layout::vertical(vec![
        Constraint::Ratio(1, metrics.len() as u32);
        metrics.len()
    ])
    .split(Rect::new(inner.x, inner.y, inner.width, graph_height));
    for (&metric, &graph) in metrics.iter().zip(graphs.iter()) {
        let maximum = points
            .iter()
            .filter_map(|point| point.peaks[metric])
            .fold(0f64, f64::max)
            .max(1.0);
        let mut columns = vec![None; graph.width as usize];
        let mut previous: Option<(i64, usize, u64)> = None;
        let seconds = if app.history_database.is_some() {
            1
        } else {
            app.metrics_refresh_seconds.max(1)
        };
        let interval = (seconds as i64 * 1000).max((end - start) / 600);
        for point in points {
            if point.at_ms < start || point.at_ms > end {
                continue;
            }
            let x = (((point.at_ms - start) as f64 / (end - start).max(1) as f64)
                * (graph.width - 1) as f64)
                .round() as usize;
            if let Some(number) = point.peaks[metric] {
                let level = (number / maximum * 10_000.0).round() as u64;
                // Connect adjacent valid observations visually, not by inventing
                // stored readings. Explicit failures and skipped bins break the fill.
                if let Some((at, old_x, old_level)) = previous
                    && point.at_ms - at <= interval * 3 / 2
                {
                    for column in &mut columns[old_x..x] {
                        *column = Some(column.unwrap_or(0).max(old_level));
                    }
                }
                columns[x] = Some(columns[x].unwrap_or(0).max(level));
                previous = Some((point.at_ms, x, level));
            } else {
                previous = None;
            }
        }
        frame.render_widget(
            Sparkline::default()
                .data(&columns)
                .max(10_000)
                .style(accent(app, color))
                .absent_value_symbol("·")
                .absent_value_style(muted()),
            graph,
        );
        if let Some(point) = cursor {
            let x = (((point.at_ms - start).clamp(0, (end - start).max(0)) as f64
                / (end - start).max(1) as f64)
                * (graph.width - 1) as f64)
                .round() as u16;
            for y in graph.y..graph.bottom() {
                frame.buffer_mut()[(graph.x + x, y)]
                    .set_symbol("│")
                    .set_style(Style::default().add_modifier(Modifier::BOLD));
            }
        }
    }
    let axis = time_axis(inner.width as usize, start, end);
    frame.render_widget(
        Paragraph::new(axis).style(muted()),
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn axis_timestamps_align_with_actual_graph_positions() {
        let start = 1_700_000_000_000;
        let end = start + 60_000;
        let axis = time_axis(100, start, end);
        assert!(axis.starts_with(&clock(start)));
        assert!(axis.ends_with(&clock(end)));
        assert!(axis[45..56].contains(&clock(start + 30_000)));
        assert_eq!(time_axis(12, start, end).len(), 12);
    }
    #[test]
    fn async_query_window_does_not_drop_the_first_bucket_as_live_time_advances() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Container(_)))
            .unwrap();
        app.tab = Tab::History;
        app.detail_focus = true;
        app.history_view.expanded = true;
        let end = crate::history::now_ms() - 2000;
        let start = end - 60_000;
        app.history_view.data = crate::history::Data {
            points: vec![Point {
                at_ms: start,
                values: [Some(42.0), None, None, None, None, None],
                peaks: [Some(42.0), None, None, None, None, None],
                count: 1,
                valid: [1, 0, 0, 0, 0, 0],
            }],
            source: "queried window".into(),
            start_ms: start,
            end_ms: end,
            bucket_ms: 100,
        };
        let mut terminal = Terminal::new(TestBackend::new(160, 55)).unwrap();
        terminal
            .draw(|frame| super::super::draw(frame, &mut app))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            text.contains('█'),
            "The first retained bucket must not disappear while a new query is loading"
        );
    }
    #[test]
    fn history_shows_four_linked_charts_and_degrades_cleanly_on_small_terminals() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Container(_)))
            .unwrap();
        app.tab = Tab::History;
        app.detail_focus = true;
        app.history_view.expanded = true;
        let now = crate::history::now_ms();
        app.history_view.data = crate::history::Data {
            source: "Daemon fixture".into(),
            points: (0..60)
                .map(|i| Point {
                    at_ms: now - (59 - i) * 1000,
                    values: [Some(10.0); 6],
                    peaks: [Some(20.0); 6],
                    count: 1,
                    valid: [1; 6],
                })
                .collect(),
            ..Default::default()
        };
        app.history_view.cursor = Some(30);
        for (width, height) in [(160, 55), (60, 20), (32, 10), (160, 55)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| super::super::draw(frame, &mut app))
                .unwrap();
            if width == 160 {
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                for name in [
                    "CPU:",
                    "Memory:",
                    "Disk read / write:",
                    "Network receive / transmit:",
                    "UTC",
                    " @ ",
                ] {
                    assert!(text.contains(name), "{name}");
                }
            }
        }
    }
}
