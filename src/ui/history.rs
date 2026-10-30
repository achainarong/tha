//! Adaptive per-container analysis, with labeled axes and gap-aware Braille lines.
use super::*;
use crate::history::{Point, RANGE_LABELS, RANGE_NAMES, RANGES};
use ratatui::{
    symbols::Marker,
    widgets::canvas::{Canvas, Line as CanvasLine, Points},
};

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
fn axis_value(metric: usize, number: f64, maximum: f64) -> String {
    if metric == 0 {
        return if maximum < 1.0 {
            format!("{number:.2}%")
        } else {
            format!("{number:.1}%")
        };
    }
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let unit = ((maximum.max(1.0).log2() / 10.0).floor() as usize).min(units.len() - 1);
    let value = number / 1024f64.powi(unit as i32);
    let number = if value >= 100.0 {
        format!("{value:.0}")
    } else if value >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    };
    format!(
        "{number} {}{}",
        units[unit],
        if metric > 1 { "/s" } else { "" }
    )
}
fn history_notice(view: &crate::history::View) -> Option<&str> {
    view.error.as_deref().or_else(|| {
        view.data
            .source
            .split_once("Warning: ")
            .map(|(_, warning)| warning.split(" · ").next().unwrap_or(warning))
    })
}

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some((engine_index, container)) = app.performance_container() else {
        frame.render_widget(
            Paragraph::new("Select a container to analyze its metrics"),
            area,
        );
        return;
    };
    let runtime = app
        .history_view
        .target
        .as_ref()
        .map(|target| target.0.clone())
        .unwrap_or_else(|| {
            format!(
                "{}:{}",
                app.engines[engine_index].connection.endpoint,
                app.engines[engine_index]
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.engine_id.as_deref())
                    .unwrap_or("unknown")
            )
        });
    let scale = app.history_view.chart_scale(runtime, container.key());
    let view = &app.history_view;
    let block = pane(
        format!(" Metrics · {} ", safe_text(&container.name)),
        app.detail_focus,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height < 5 {
        return;
    }
    let mut ranges = vec![Span::styled("Range: ", muted())];
    for (index, label) in RANGE_LABELS.iter().enumerate() {
        ranges.push(if index == view.range {
            Span::styled(
                format!("[{label}]"),
                accent(app, Color::Cyan).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(*label, muted())
        });
        ranges.push(Span::raw(" "));
    }
    ranges.push(Span::styled("· [ shorter / ] longer", muted()));
    let legend = Paragraph::new(Line::from(ranges)).wrap(Wrap { trim: true });
    let legend_height = legend.line_count(inner.width) as u16;
    let notice = history_notice(view);
    let parts = Layout::vertical([
        Constraint::Length(legend_height),
        Constraint::Length(2 + u16::from(notice.is_some())),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(inner);
    let (start, end) = if !view.data.points.is_empty() && view.data.end_ms > view.data.start_ms {
        (view.data.start_ms, view.data.end_ms)
    } else {
        view.bounds()
    };
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
    let mut header = vec![
        Line::from(format!(
            "{} · {} → {} UTC · {}{}",
            RANGE_NAMES[view.range],
            if RANGES[view.range] >= 3_600_000 {
                stamp(start)
            } else {
                clock(start)
            },
            if RANGES[view.range] >= 3_600_000 {
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
        Line::styled(counters, muted()),
    ];
    if let Some(notice) = notice {
        header.push(Line::styled(safe_text(notice), accent(app, Color::Yellow)));
    }
    frame.render_widget(Paragraph::new(header), parts[1]);
    frame.render_widget(legend, parts[0]);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "←/→ Pan · c Cursor · h/l Inspect · f Live · z Enlarge",
                muted(),
            ),
            Line::styled(
                "j/k Metric (small terminal) · Esc Restore · gaps = missing · 100% CPU = 1 core · auto scale",
                muted(),
            ),
        ]),
        parts[3],
    );
    if view.data.points.is_empty() {
        frame.render_widget(Paragraph::new(view.error.as_deref().unwrap_or("No readings in this time range. Collection begins when Tha/daemon observes this container. [ selects a shorter range; f returns to live.")).wrap(Wrap{trim:false}),parts[2]);
        return;
    }
    let all = parts[2].height >= 24;
    let groups: &[usize] = if all {
        &[0, 1, 2, 3]
    } else {
        std::slice::from_ref(&view.metric)
    };
    let charts = Layout::vertical(vec![
        Constraint::Ratio(1, groups.len() as u32);
        groups.len()
    ])
    .split(parts[2]);
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
            &scale,
        );
    }
}
fn runs(
    points: &[Point],
    metric: usize,
    bounds: (i64, i64),
    interval: i64,
) -> Vec<Vec<(f64, f64)>> {
    let mut result = Vec::new();
    let mut run = Vec::new();
    let mut previous = None;
    for point in points
        .iter()
        .filter(|point| point.at_ms >= bounds.0 && point.at_ms <= bounds.1)
    {
        let value = point.values[metric].filter(|value| value.is_finite() && *value >= 0.0);
        if (value.is_none() || previous.is_some_and(|at| point.at_ms - at > interval * 3 / 2))
            && !run.is_empty()
        {
            result.push(std::mem::take(&mut run));
        }
        if let Some(value) = value {
            run.push(((point.at_ms - bounds.0) as f64 / 1000.0, value));
            previous = Some(point.at_ms);
        } else {
            previous = None;
        }
    }
    if !run.is_empty() {
        result.push(run);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn draw_chart(
    frame: &mut Frame,
    app: &App,
    points: &[Point],
    cursor: Option<&Point>,
    group: usize,
    area: Rect,
    bounds: (i64, i64),
    scale: &crate::history::ChartScale,
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
    let summary = metrics
        .iter()
        .map(|&metric| {
            format!(
                "avg {} / peak {}",
                value(metric, scale.averages[metric]),
                value(metric, scale.peaks[metric])
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let block = Block::bordered()
        .title(format!(
            " {name}: {labels}{}{} ",
            if group == 0 {
                selected
                    .and_then(|point| point.values[0])
                    .map(|cpu| format!(" · {:.4} cores", cpu / 100.0))
                    .unwrap_or_default()
            } else {
                String::new()
            },
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
        .title_bottom(format!(
            " {} · {summary} · auto ",
            match group {
                2 => "read blue / write magenta",
                3 => "Rx blue / Tx yellow",
                _ => "dotted = average",
            }
        ))
        .border_style(muted());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 18 || inner.height < 3 {
        return;
    }
    let axis_width = 13.min(inner.width / 3);
    let graph = Rect::new(
        inner.x + axis_width,
        inner.y,
        inner.width - axis_width,
        inner.height - 1,
    );
    let maximum = scale.limits[group];
    for (y, fraction) in [
        (graph.y, 1.0),
        (graph.y + graph.height / 2, 0.5),
        (graph.bottom() - 1, 0.0),
    ] {
        frame.render_widget(
            Paragraph::new(axis_value(metrics[0], maximum * fraction, maximum))
                .alignment(Alignment::Right)
                .style(muted()),
            Rect::new(inner.x, y, axis_width.saturating_sub(1), 1),
        );
    }
    let duration = (end - start).max(1) as f64 / 1000.0;
    let seconds = if app.history_database.is_some() {
        1
    } else {
        app.metrics_refresh_seconds.max(1)
    };
    let interval = (seconds as i64 * 1000)
        .max(app.history_view.data.bucket_ms)
        .max((end - start) / 600);
    let series: Vec<_> = metrics
        .iter()
        .map(|&metric| runs(points, metric, bounds, interval))
        .collect();
    let peaks: Vec<Vec<_>> = metrics
        .iter()
        .map(|&metric| {
            points
                .iter()
                .filter(|point| point.at_ms >= start && point.at_ms <= end)
                .filter_map(|point| {
                    point.peaks[metric]
                        .filter(|value| value.is_finite())
                        .map(|value| ((point.at_ms - start) as f64 / 1000.0, value))
                })
                .collect()
        })
        .collect();
    let colors: Vec<_> = metrics
        .iter()
        .enumerate()
        .map(|(index, _)| {
            accent(
                app,
                if metrics.len() == 2 && index == 0 {
                    Color::Blue
                } else {
                    color
                },
            )
            .fg
            .unwrap_or(color)
        })
        .collect();
    let average: Vec<_> = (0..graph.width / 3)
        .map(|x| {
            (
                x as f64 * 3.0 / (graph.width - 1).max(1) as f64 * duration,
                scale.averages[metrics[0]].unwrap_or(0.0),
            )
        })
        .collect();
    frame.render_widget(
        Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([0.0, duration])
            .y_bounds([0.0, maximum])
            .paint(|context| {
                if metrics.len() == 1 && scale.averages[metrics[0]].is_some() {
                    context.draw(&Points {
                        coords: &average,
                        color: Color::DarkGray,
                    });
                }
                for (index, runs) in series.iter().enumerate() {
                    for run in runs {
                        for pair in run.windows(2) {
                            context.draw(&CanvasLine {
                                x1: pair[0].0,
                                y1: pair[0].1,
                                x2: pair[1].0,
                                y2: pair[1].1,
                                color: colors[index],
                            });
                        }
                        context.draw(&Points {
                            coords: run,
                            color: colors[index],
                        });
                    }
                    context.draw(&Points {
                        coords: &peaks[index],
                        color: colors[index],
                    });
                }
            }),
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
    let axis = time_axis(graph.width as usize, start, end);
    frame.render_widget(
        Paragraph::new(axis).style(muted()),
        Rect::new(graph.x, inner.bottom() - 1, graph.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn routine_collector_diagnostics_are_hidden_but_warnings_and_errors_remain() {
        let mut view = crate::history::View::default();
        view.data.source = "Daemon · heartbeat 2s ago · 0 dropped samples · live 1s · SQLite flush 15s · session live overlay".into();
        assert_eq!(history_notice(&view), None);
        view.data.source = "Daemon live unavailable · persisted history · Warning: Disk budget reached · heartbeat 2s ago".into();
        assert_eq!(history_notice(&view), Some("Disk budget reached"));
        view.error = Some("Cannot read history".into());
        assert_eq!(history_notice(&view), Some("Cannot read history"));
    }

    #[test]
    fn range_legend_wraps_and_highlights_the_selection_even_without_readings() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.history_view.data.source =
            "Daemon · heartbeat 2s ago · 0 dropped samples · SQLite flush 15s".into();
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Container(_)))
            .unwrap();
        for (range, selected_label) in RANGE_LABELS.iter().enumerate() {
            app.history_view.range = range;
            for width in [40, 60, 160] {
                let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
                terminal
                    .draw(|frame| {
                        let area = frame.area();
                        draw(frame, &mut app, area);
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let text = buffer
                    .content
                    .chunks(width as usize)
                    .map(|row| {
                        row[1..row.len() - 1]
                            .iter()
                            .map(|cell| cell.symbol())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                for diagnostic in ["heartbeat", "SQLite", "dropped samples"] {
                    assert!(!text.contains(diagnostic), "Unexpected {diagnostic}");
                }
                for label in RANGE_LABELS {
                    assert!(text.contains(label), "Missing {label} at width {width}");
                }
                assert!(text.contains(&format!("[{selected_label}]")));
                assert!(text.contains("[ shorter"));
                assert!(text.contains("] longer"), "width {width}: {text}");
                assert!(buffer.content.iter().any(|cell| {
                    cell.symbol() == "[" && cell.modifier.contains(Modifier::BOLD)
                }));
            }
        }
    }

    #[test]
    fn vertical_axis_uses_one_unit_for_zero_midpoint_and_maximum() {
        let maximum = 1.25 * 1024.0 * 1024.0;
        assert_eq!(axis_value(1, 0.0, maximum), "0.00 MiB");
        assert_eq!(axis_value(1, maximum / 2.0, maximum), "0.62 MiB");
        assert_eq!(axis_value(2, 0.0, 4096.0), "0.00 KiB/s");
        assert_eq!(axis_value(0, 0.05, 0.1), "0.05%");
    }
    #[test]
    fn missing_observations_break_lines_instead_of_becoming_zeros() {
        let point = |at, value| Point {
            at_ms: at,
            values: [value; 6],
            peaks: [value; 6],
            count: 1,
            valid: [1; 6],
        };
        let points = vec![
            point(0, Some(1.0)),
            point(1000, Some(2.0)),
            point(2000, None),
            point(3000, Some(3.0)),
            point(10_000, Some(4.0)),
        ];
        let lines = runs(&points, 0, (0, 10_000), 1000);
        assert_eq!(lines.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1, 1]);
    }
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
            text.contains("UTC")
                && terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|cell| cell.fg == Color::Cyan
                        && cell
                            .symbol()
                            .chars()
                            .any(|ch| ('\u{2801}'..='\u{28ff}').contains(&ch))),
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
