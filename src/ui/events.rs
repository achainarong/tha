//! Newest-first container event list for the selected workload.
use super::*;
use crate::events::Tone;

pub fn tone_style(app: &App, tone: Tone) -> Style {
    match tone {
        Tone::Failure => accent(app, Color::Red),
        Tone::Warning => accent(app, Color::Yellow),
        Tone::Healthy => accent(app, Color::Green),
        Tone::Neutral => muted(),
    }
}

fn stamp(at_ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(at_ms.div_euclid(1000))
        .map(|at| {
            format!(
                "{:02}-{:02} {:02}:{:02}:{:02}",
                at.month() as u8,
                at.day(),
                at.hour(),
                at.minute(),
                at.second()
            )
        })
        .unwrap_or_else(|_| "--".into())
}

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.height < 2 || area.width < 10 {
        return;
    }
    let native = app.current().is_some_and(|row| {
        app.engines[row.engine].connection.kind == crate::config::BackendKind::Containerd
    });
    let persisted = app.metrics_database.is_some();
    let lines: Vec<Line> = app
        .visible_events()
        .into_iter()
        .map(|event| {
            Line::from(vec![
                Span::styled(format!("{}  ", stamp(event.at_ms)), muted()),
                Span::styled(event.label(), tone_style(app, event.tone())),
                Span::raw(format!("  {}", event.name)),
            ])
        })
        .collect();
    let parts = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let total = lines.len();
    let height = parts[1].height as usize;
    let scroll = app.detail_scroll.min(total.saturating_sub(height));
    app.detail_scroll = scroll;
    frame.render_widget(
        Paragraph::new(format!(
            "{total} events · newest first · UTC · {}",
            if persisted {
                "last 24 hours, saved by the service"
            } else {
                "from the engine while Tha is open"
            }
        ))
        .style(muted()),
        parts[0],
    );
    if lines.is_empty() {
        frame.render_widget(
            Paragraph::new(if native {
                "Events are not available for native containerd connections."
            } else {
                "No events for this selection yet. Starts, exits, OOM kills and health changes appear here as they happen."
            })
            .wrap(Wrap { trim: false }),
            parts[1],
        );
        return;
    }
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(scroll)
                .take(height)
                .collect::<Vec<_>>(),
        ),
        parts[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use ratatui::{Terminal, backend::TestBackend};

    fn event(at_ms: i64, key: &str, project: Option<&str>, action: &str, detail: &str) -> Event {
        Event {
            at_ms,
            key: key.into(),
            name: key.trim_start_matches('/').into(),
            project: project.map(Into::into),
            service: None,
            action: action.into(),
            detail: detail.into(),
        }
    }

    fn render(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal
            .draw(|frame| draw(frame, app, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn selection_scopes_the_timeline_and_removed_containers_stay_with_their_project() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        let (project, container) = {
            let snapshot = app.engines[0].snapshot.as_ref().unwrap();
            let member = snapshot
                .containers
                .iter()
                .find(|container| container.project.is_some())
                .unwrap();
            (member.project.clone().unwrap(), member.key())
        };
        app.engines[0].events = Default::default();
        for item in [
            event(1000, &container, Some(&project), "start", ""),
            event(2000, "/recreated-away", Some(&project), "die", "exit 137"),
            event(3000, "/unrelated", None, "oom", ""),
        ] {
            app.engines[0].events.add(item);
        }
        let select = |app: &mut App, wanted: &dyn Fn(&Scope) -> bool| {
            app.selected = app
                .rows
                .iter()
                .position(|row| row.engine == 0 && wanted(&row.scope))
                .unwrap();
        };
        select(&mut app, &|scope| *scope == Scope::Engine);
        assert_eq!(app.visible_events().len(), 3);
        assert_eq!(app.visible_events()[0].action, "oom", "Newest first");
        select(&mut app, &|scope| matches!(scope, Scope::Project { .. }));
        let names: Vec<_> = app.visible_events().iter().map(|e| e.at_ms).collect();
        assert_eq!(names, [2000, 1000]);
        select(
            &mut app,
            &|scope| matches!(scope, Scope::Container(id) if container.ends_with(id.as_str())),
        );
        assert_eq!(app.visible_events().len(), 1);
    }

    #[test]
    fn list_shows_meaningful_labels_clamps_scrolling_and_explains_an_empty_state() {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.engines[0].events = Default::default();
        app.selected = app
            .rows
            .iter()
            .position(|row| row.engine == 0 && row.scope == Scope::Engine)
            .unwrap();
        assert!(render(&mut app).contains("No events for this selection"));
        app.engines[0]
            .events
            .add(event(60_000, "/api", None, "die", "exit 137"));
        app.detail_scroll = 99;
        let text = render(&mut app);
        assert!(text.contains("exited · exit 137"), "{text}");
        assert!(text.contains("01-01 00:01:00"), "{text}");
        assert!(text.contains("1 events"), "{text}");
        assert_eq!(app.detail_scroll, 0);
    }
}
