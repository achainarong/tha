//! Compose confirmation and output dialogs.
use super::*;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.compose_confirmation.is_some() {
        draw_confirmation(frame, app, area);
    } else if app.compose_output_visible {
        draw_output(frame, app, area);
    }
}

fn draw_confirmation(frame: &mut Frame, app: &App, area: Rect) {
    let Some(confirmation) = &app.compose_confirmation else {
        return;
    };
    let request = &confirmation.request;
    let target = &request.target;
    let dialog = centered(area, 96, area.height.saturating_sub(2).min(20));
    frame.render_widget(Clear, dialog);
    let path = |path: &std::path::Path| safe_text(&path.to_string_lossy());
    let mut lines = vec![
        Line::styled(
            format!("{} · {}", request.action.label(), target.name()),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from(request.action.effect()),
        Line::from(crate::tf!(
            "Runtime: {a0}",
            a0 = safe_text(&request.connection.name)
        )),
        Line::from(crate::tf!(
            "Directory: {a0}",
            a0 = path(&target.working_dir)
        )),
    ];
    lines.extend(
        target
            .files
            .iter()
            .map(|file| Line::from(crate::tf!("File: {a0}", a0 = path(file)))),
    );
    lines.extend(
        target
            .env_files
            .iter()
            .map(|file| Line::from(crate::tf!("Env file: {a0}", a0 = path(file)))),
    );
    lines.push(Line::styled(
        crate::i18n::t("These paths come from container labels and Compose runs what they contain. Check them."),
        accent(app, Color::Yellow),
    ));
    lines.push(Line::from(""));
    let hint = if request.action.requires_typing() {
        lines.push(Line::from(crate::tf!(
            "Type {a0} to confirm: {a1}▏",
            a0 = target.project,
            a1 = safe_text(&confirmation.typed)
        )));
        crate::i18n::t(" Enter Confirm · Esc Cancel ")
    } else {
        lines.push(Line::from(crate::i18n::t("Run this now?")));
        crate::i18n::t(" y/Enter Run · n/Esc Cancel ")
    };
    if let Some(error) = &confirmation.error {
        lines.push(Line::styled(error.clone(), accent(app, Color::Red)));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .title(crate::i18n::t(" Compose "))
                .title_bottom(hint)
                .border_style(accent(app, Color::Yellow)),
        ),
        dialog,
    );
}

fn draw_output(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(job) = &app.compose_job else {
        return;
    };
    let dialog = centered(area, 110, area.height.saturating_sub(2).min(30));
    frame.render_widget(Clear, dialog);
    let (status, style) = match &job.outcome {
        None => (
            crate::i18n::t("Running…").to_owned(),
            accent(app, Color::Cyan),
        ),
        Some(Ok(())) => (
            crate::i18n::t("Finished").to_owned(),
            accent(app, Color::Green),
        ),
        Some(Err(error)) => (
            crate::tf!("Failed: {error}", error = error),
            accent(app, Color::Red),
        ),
    };
    let block = Block::bordered()
        .title(format!(" {} ", safe_text(&job.title)))
        .title_bottom(crate::i18n::t(
            " j/k Scroll · G Latest · Esc Close; a running operation continues ",
        ));
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);
    if inner.height < 2 {
        return;
    }
    let parts = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(inner);
    frame.render_widget(Paragraph::new(Line::styled(status, style)), parts[0]);
    let height = parts[1].height as usize;
    let total = job.lines.len();
    // scroll counts lines back from the newest, so output follows until scrolled away.
    let back = job.scroll.min(total.saturating_sub(height));
    let first = total.saturating_sub(height + back);
    let visible: Vec<Line> = job
        .lines
        .iter()
        .skip(first)
        .take(height)
        .map(|line| Line::from(line.as_str()))
        .collect();
    frame.render_widget(Paragraph::new(visible), parts[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{Action, Confirmation, Job, Request, Target};
    use ratatui::{Terminal, backend::TestBackend};

    fn render(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
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

    fn app(action: Action) -> App {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.compose_confirmation = Some(Confirmation {
            request: Request {
                connection: app.engines[0].connection.clone(),
                action,
                target: Target {
                    project: "shop".into(),
                    service: None,
                    working_dir: "/srv/shop".into(),
                    files: vec!["/srv/shop/compose.yaml".into()],
                    env_files: vec![],
                },
            },
            typed: String::new(),
            error: None,
        });
        app
    }

    #[test]
    fn confirmation_shows_the_labelled_paths_it_will_run() {
        let mut update = app(Action::Update);
        let text = render(&mut update);
        for expected in [
            "/srv/shop/compose.yaml",
            "Directory: /srv/shop",
            "y/Enter Run",
        ] {
            assert!(text.contains(expected), "{expected}: {text}");
        }
        let mut down = app(Action::Down);
        let text = render(&mut down);
        assert!(text.contains("Volumes and images are kept"), "{text}");
        assert!(text.contains("Type shop to confirm"), "{text}");
    }

    #[test]
    fn down_needs_the_exact_name_and_a_confirmed_request_opens_its_output() {
        let mut down = app(Action::Down);
        down.confirm_compose();
        assert!(down.compose_request.is_none());
        let confirmation = down.compose_confirmation.as_mut().unwrap();
        assert!(confirmation.error.is_some());
        confirmation.typed = "shop".into();
        down.confirm_compose();
        assert!(down.compose_request.is_some());
        assert!(down.compose_busy());
        assert!(down.compose_output_visible);
        let mut second = app(Action::Update);
        second.compose_job = Some(Job::new("running".into()));
        second.confirm_compose();
        assert!(second.compose_request.is_none(), "One operation at a time");
    }

    #[test]
    fn output_follows_the_newest_lines_and_reports_failure() {
        let mut app = app(Action::Up);
        app.compose_confirmation = None;
        app.compose_output_visible = true;
        let mut job = Job::new("Compose up".into());
        for line in 0..100 {
            job.push(format!("line-{line:03}"));
        }
        app.compose_job = Some(job);
        let text = render(&mut app);
        assert!(text.contains("Running"), "{text}");
        assert!(text.contains("line-099"), "{text}");
        assert!(!text.contains("line-000"), "{text}");
        let job = app.compose_job.as_mut().unwrap();
        job.scroll = 1000;
        job.outcome = Some(Err("Compose exited with 1".into()));
        let text = render(&mut app);
        assert!(text.contains("Failed: Compose exited with 1"), "{text}");
        assert!(text.contains("line-000"), "{text}");
    }
}
