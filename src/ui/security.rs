use super::*;
use crate::security::State;

/// Fourth top-level overview: image selection and findings share one workspace.
pub fn draw_overview(frame: &mut Frame, app: &mut App, area: Rect) {
    if !app.security.enabled {
        frame.render_widget(
            Paragraph::new("Install the security module: tha module install security"),
            area,
        );
        return;
    }
    if !app.security.busy && !app.detail_focus {
        let target = inventory::security_targets(app, false).into_iter().next();
        app.security.select(target);
    }
    let (images, findings) = if app.security.zoom {
        if app.detail_focus {
            (None, Some(area))
        } else {
            (Some(area), None)
        }
    } else if area.width >= 110 {
        let parts = Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(area);
        (Some(parts[0]), Some(parts[1]))
    } else if app.detail_focus {
        (None, Some(area))
    } else {
        (Some(area), None)
    };
    if let Some(images) = images {
        app.overview_mode = OverviewMode::Images;
        inventory::draw_security_images(frame, app, images);
        app.overview_mode = OverviewMode::Security;
    }
    if let Some(findings) = findings {
        draw(frame, app, findings);
    }
}

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = pane(
        if app.security.expanded {
            " Advisory details · Esc Back ".into()
        } else {
            " Vulnerabilities · z Expand/restore ".into()
        },
        app.detail_focus,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let mut lines = Vec::new();
    let compact = inner.height < 14;
    if let Some(target) = &app.security.target {
        lines.push(Line::from(format!(
            "{} · {}",
            safe_text(&target.connection.name),
            safe_text(&target.label)
        )));
        if !compact {
            lines.push(Line::styled(safe_text(&target.image), muted()));
        }
    }
    if !compact {
        lines.push(Line::styled(
            "Image only; excludes live container changes and mounted files.",
            muted(),
        ));
    }
    if !compact && !app.security.batch.is_empty() {
        lines.push(Line::from(app.security.batch.clone()));
    }
    if !compact && !app.security.queue.is_empty() {
        lines.push(Line::from(format!(
            "{} images selected · :scan-images scans the checked queue",
            app.security.queue.len()
        )));
    }
    match &app.security.state {
        State::Idle => lines.push(Line::from(if app.security.enabled {
            "Not scanned · s Scan this image · S Scan checked images"
        } else {
            "Module not installed. Run: tha module install security"
        })),
        State::Running(message) => {
            lines.push(Line::from(message.clone()));
            lines.push(Line::from(
                "Service-owned queue · safe to close the UI · x Cancel",
            ));
        }
        State::Cancelled => lines.push(Line::from(
            "Queue cancelled. Previously completed reports remain in SQLite.",
        )),
        State::Failed(error) => {
            lines.push(Line::styled(
                "Queue stopped on error; this is NOT a clean scan.",
                accent(app, Color::Red),
            ));
            lines.push(Line::from(safe_text(error)));
        }
        State::Ready(report) => {
            let mut counts = [0_usize; 6];
            for finding in &report.findings {
                counts[crate::security::severity_rank(&finding.severity)] += 1;
            }
            let names = if inner.width >= 90 {
                ["Critical", "High", "Medium", "Low", "Negligible", "Unknown"]
            } else {
                ["C", "H", "M", "L", "N", "U"]
            };
            lines.push(Line::from(
                names
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| {
                        Span::styled(
                            format!("{name} {}  ", counts[index]),
                            severity_style(app, index),
                        )
                    })
                    .collect::<Vec<_>>(),
            ));
            if !compact {
                let scanned = time::OffsetDateTime::from_unix_timestamp(report.scanned as i64)
                    .map(|date| date.to_string())
                    .unwrap_or_else(|_| "unknown time".into());
                lines.push(Line::styled(format!("Grype {} · scanned {scanned} · saved report, not a live security guarantee", safe_text(&report.scanner)), muted()));
            }
            if app.security.expanded {
                if let Some(finding) = report.findings.get(app.security.selected) {
                    lines.push(Line::from(format!(
                        "{} · {} · CVSS {} (highest reported base score)",
                        finding.id,
                        finding.severity,
                        score(finding.score)
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::styled("PACKAGE", muted()));
                    lines.push(Line::from(finding.package.clone()));
                    lines.push(Line::from(format!("Installed  {}", finding.installed)));
                    lines.push(Line::from(format!(
                        "Fixed in   {}",
                        if finding.fixed.is_empty() {
                            "No fix reported"
                        } else {
                            &finding.fixed
                        }
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::styled("DESCRIPTION", muted()));
                    lines.push(Line::from(finding.description.clone()));
                    lines.push(Line::from(""));
                    lines.push(Line::styled("ADVISORY", muted()));
                    lines.push(Line::from(finding.url.clone()));
                    lines.push(Line::from(""));
                    let database: serde_json::Value =
                        serde_json::from_str(&report.database).unwrap_or_default();
                    lines.push(Line::styled("SCANNER", muted()));
                    lines.push(Line::from(format!(
                        "Grype {} · database built {}",
                        report.scanner,
                        database
                            .get("built")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("not reported")
                    )));
                }
                lines.push(Line::styled(
                    "Esc/Enter Back to findings · j/k Scroll · z Expand/restore",
                    muted(),
                ));
            } else {
                if report.findings.is_empty() {
                    lines.push(Line::from(
                        "No known matching vulnerabilities found within scanner coverage.",
                    ));
                } else {
                    let header_height = (lines.len() as u16).min(inner.height.saturating_sub(3));
                    let parts = Layout::vertical([
                        Constraint::Length(header_height),
                        Constraint::Min(1),
                        Constraint::Length(1),
                    ])
                    .split(inner);
                    let header: Vec<_> = lines
                        .into_iter()
                        .map(|line| ellipsize_line(line, inner.width as usize))
                        .collect();
                    let footer = format!(
                        "{} / {} · j/k Select · Enter Details · z Zoom",
                        app.security.selected + 1,
                        report.findings.len()
                    );
                    frame.render_widget(Paragraph::new(header), parts[0]);
                    draw_findings(frame, app, parts[1]);
                    frame.render_widget(Paragraph::new(footer).style(muted()), parts[2]);
                    return;
                }
            }
        }
    }
    let list = matches!(app.security.state, State::Ready(_)) && !app.security.expanded;
    if list {
        lines = lines
            .into_iter()
            .map(|line| ellipsize_line(line, inner.width as usize))
            .collect();
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let maximum = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize);
    app.detail_scroll = app.detail_scroll.min(maximum).min(u16::MAX as usize);
    frame.render_widget(paragraph.scroll((app.detail_scroll as u16, 0)), inner);
}

fn severity_style(app: &App, rank: usize) -> Style {
    match rank {
        0 => accent(app, Color::Red).add_modifier(Modifier::BOLD),
        1 => accent(app, Color::Red),
        2 => accent(app, Color::Yellow),
        3 => accent(app, Color::Blue),
        _ => muted(),
    }
}

fn draw_findings(frame: &mut Frame, app: &mut App, area: Rect) {
    use ratatui::widgets::TableState;
    let State::Ready(report) = &app.security.state else {
        return;
    };
    let margin = u16::from(area.height >= 4);
    let capacity = area.height.saturating_sub(1 + margin).max(1) as usize;
    app.security.selected = app
        .security
        .selected
        .min(report.findings.len().saturating_sub(1));
    let mut start = app.security.offset.min(app.security.selected);
    if app.security.selected >= start.saturating_add(capacity) {
        start = app.security.selected + 1 - capacity;
    }
    start = start.min(report.findings.len().saturating_sub(capacity));
    let (headers, widths) = if area.width >= 100 {
        (
            vec![
                "Severity",
                "CVSS",
                "Advisory",
                "Package",
                "Installed",
                "Fixed in",
            ],
            vec![
                Constraint::Length(10),
                Constraint::Length(5),
                Constraint::Length(22),
                Constraint::Min(12),
                Constraint::Length(14),
                Constraint::Length(14),
            ],
        )
    } else if area.width >= 72 {
        (
            vec!["Severity", "CVSS", "Advisory", "Package", "Fixed in"],
            vec![
                Constraint::Length(10),
                Constraint::Length(5),
                Constraint::Length(21),
                Constraint::Min(12),
                Constraint::Length(12),
            ],
        )
    } else if area.width >= 48 {
        (
            vec!["Severity", "CVSS", "Advisory", "Package"],
            vec![
                Constraint::Length(10),
                Constraint::Length(5),
                Constraint::Length(21),
                Constraint::Min(8),
            ],
        )
    } else {
        (
            vec!["Sev", "CVSS", "Advisory"],
            vec![
                Constraint::Length(4),
                Constraint::Length(5),
                Constraint::Min(10),
            ],
        )
    };
    let rows = report
        .findings
        .iter()
        .skip(start)
        .take(capacity)
        .map(|finding| {
            let rank = crate::security::severity_rank(&finding.severity);
            let severity = if area.width < 48 {
                ["Crit", "High", "Med", "Low", "Neg", "Unk"][rank]
            } else {
                finding.severity.as_str()
            };
            let mut cells = vec![
                Cell::from(severity).style(severity_style(app, rank)),
                Cell::from(Line::from(score(finding.score)).alignment(Alignment::Right)),
                Cell::from(finding.id.as_str()),
            ];
            if area.width >= 48 {
                cells.push(Cell::from(finding.package.as_str()));
            }
            if area.width >= 100 {
                cells.push(Cell::from(finding.installed.as_str()));
            }
            if area.width >= 72 {
                cells.push(Cell::from(if finding.fixed.is_empty() {
                    "—"
                } else {
                    finding.fixed.as_str()
                }));
            }
            TableRow::new(cells)
        });
    let mut state =
        TableState::default().with_selected(Some(app.security.selected.saturating_sub(start)));
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .header(TableRow::new(headers).style(muted()).bottom_margin(margin))
            .column_spacing(1)
            .row_highlight_style(selected())
            .highlight_symbol("› "),
        area,
        &mut state,
    );
    app.security.offset = start;
}

fn score(score: Option<f64>) -> String {
    score
        .map(|score| format!("{score:.1}"))
        .unwrap_or_else(|| "—".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn top_level_security_shows_images_and_findings_without_cleanup_actions() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.security.enabled = true;
        app.overview_mode = OverviewMode::Security;
        let mut terminal = Terminal::new(TestBackend::new(180, 40)).unwrap();
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
        assert!(text.contains("[Security]"));
        assert!(text.contains("Security images"));
        assert!(!text.contains("d Delete"));
        assert!(!text.contains("cleanup"));
        // Used images get a scan checkbox too, not only unreferenced ones.
        let snapshot = app.engines[app.inventory_engine].snapshot.as_ref().unwrap();
        let checkboxes = terminal
            .backend()
            .buffer()
            .content
            .chunks(180)
            .map(|row| {
                row[..90]
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .filter(|row| row.contains("[ ] "))
            .count();
        assert_eq!(checkboxes, snapshot.images.as_ref().unwrap().len());
        assert_eq!(app.overview_mode, OverviewMode::Security);
    }

    #[test]
    fn findings_scores_details_and_zoom_survive_resize() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.tab = Tab::Security;
        app.overview_mode = OverviewMode::Security;
        app.security.enabled = true;
        app.detail_focus = true;
        app.security.zoom = true;
        app.security.state = State::Ready(crate::security::Report {
            findings: vec![crate::security::Finding {
                id: "CVE-fixture".into(),
                severity: "Critical".into(),
                score: Some(9.8),
                package: "example".into(),
                installed: "1.0".into(),
                fixed: "1.1".into(),
                description: "Fixture advisory description".into(),
                url: "https://example.invalid/advisory".into(),
            }],
            scanner: "fixture".into(),
            database: "fixture-db".into(),
            scanned: 123,
        });
        for (width, height) in [(140, 40), (60, 20), (32, 10), (140, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| super::super::draw(frame, &mut app))
                .unwrap();
            if width == 140 {
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains("CVE-fixture"));
                assert!(text.contains("9.8"));
                assert!(
                    !text.contains("6 Layers"),
                    "Zoom must replace the explorer/details split"
                );
            }
        }
        app.security.expanded = true;
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
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
        assert!(text.contains("Fixture advisory description"));
        assert!(text.contains("https://example.invalid/advisory"));
        app.security.expanded = false;
        if let State::Ready(report) = &mut app.security.state {
            let finding = report.findings[0].clone();
            report.findings = (0..100)
                .map(|index| crate::security::Finding {
                    id: format!("CVE-{index:03}"),
                    ..finding.clone()
                })
                .collect();
        }
        app.security.move_by(isize::MAX);
        for (width, height) in [(140, 40), (60, 20), (32, 10), (140, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
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
                text.contains("CVE-099"),
                "Selected finding must stay visible at {width}x{height}"
            );
            assert!(text.contains("100 / 100"));
        }
    }
}
