//! The download manager panel and its review dialog.
use super::*;
use crate::{
    downloads::{Folder, JobStatus, Pane, Preview, human_size},
    files::Kind,
};

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    if app.downloads.open.is_some() {
        draw_panel(frame, app, area);
    }
    if app.downloads.dialog.is_some() {
        draw_dialog(frame, app, area);
    }
}

fn muted() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn draw_panel(frame: &mut Frame, app: &App, area: Rect) {
    let manager = &app.downloads;
    let dialog = centered(area, 110, area.height.saturating_sub(2));
    frame.render_widget(Clear, dialog);
    let label = manager
        .tree()
        .map_or_else(String::new, |tree| safe_text(tree.source.label()));
    let block = Block::bordered()
        .title(crate::tf!(" Download manager · {label} ", label = label))
        .title_bottom(match manager.pane {
            Pane::Browse => crate::i18n::t(
                " j/k Move · Enter Open or download · d Download · Tab Downloads · Esc Close ",
            ),
            Pane::Downloads => crate::i18n::t(
                " j/k Move · x Cancel · Tab Browse · Esc Close; downloads keep running ",
            ),
        });
    let inner = padded(block.inner(dialog));
    frame.render_widget(block, dialog);
    let height = inner.height as usize;
    let lines = match manager.pane {
        Pane::Browse => browse_lines(app, height),
        Pane::Downloads => job_lines(app, height),
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn browse_lines(app: &App, height: usize) -> Vec<Line<'static>> {
    let manager = &app.downloads;
    let Some(tree) = manager.tree() else {
        return Vec::new();
    };
    let mut lines = vec![Line::styled(
        if let Some(reason) = &tree.stopped {
            crate::tf!(
                "Browsing is unavailable: {reason} · :download PATH still works",
                reason = safe_text(reason)
            )
        } else if tree.building() {
            crate::tf!(
                "Building the tree in the background: {folders} folders read…",
                folders = tree.listed_folders()
            )
        } else if tree.limited {
            crate::i18n::t(
                "The tree is incomplete: it reached its limit. Opening a folder reads it.",
            )
            .into()
        } else {
            crate::i18n::t("Complete as of when each folder was read.").into()
        },
        muted(),
    )];
    lines.push(Line::from(""));
    let rows = tree.rows();
    if rows.is_empty() && tree.stopped.is_none() {
        lines.push(Line::from(crate::i18n::t("Nothing here yet.")));
    }
    let visible = height.saturating_sub(lines.len()).max(1);
    let offset = manager.selected.saturating_sub(visible - 1);
    for (index, row) in rows.iter().enumerate().skip(offset).take(visible) {
        let marker = match row.entry.kind {
            Kind::Folder if row.open => "▾ ",
            Kind::Folder => "▸ ",
            Kind::Link => "→ ",
            _ => "  ",
        };
        let detail = match (&row.state, row.entry.size) {
            (_, Some(size)) => human_size(size),
            (Some(Folder::Failed(error)), _) => safe_text(error),
            (Some(Folder::Loading), _) => crate::i18n::t("reading…").into(),
            _ => String::new(),
        };
        let name = format!(
            "{}{marker}{}",
            "  ".repeat(row.level),
            safe_text(&row.entry.name)
        );
        let text = format!("{name:<60} {detail}");
        lines.push(if index == manager.selected {
            Line::styled(text, Style::default().add_modifier(Modifier::REVERSED))
        } else {
            Line::from(text)
        });
    }
    lines
}

fn job_lines(app: &App, height: usize) -> Vec<Line<'static>> {
    let manager = &app.downloads;
    if manager.jobs.is_empty() {
        return vec![Line::from(crate::i18n::t(
            "No downloads in this session yet.",
        ))];
    }
    let mut lines = Vec::new();
    let offset = manager.job_selected.saturating_sub(height / 2 / 2);
    for (index, job) in manager.jobs.iter().enumerate().skip(offset) {
        let state = match &job.status {
            JobStatus::Running => {
                let done = human_size(job.bytes);
                let rate = human_size(job.speed());
                match (job.percent(), job.total) {
                    (Some(percent), Some(total)) => crate::tf!(
                        "{percent}% · {done} of about {total} · {rate}/s",
                        percent = percent,
                        done = done,
                        total = human_size(total),
                        rate = rate
                    ),
                    _ => crate::tf!("{done} · {rate}/s", done = done, rate = rate),
                }
            }
            JobStatus::Done => crate::tf!("Finished · {size}", size = human_size(job.bytes)),
            JobStatus::Failed(error) => crate::tf!("Failed: {error}", error = safe_text(error)),
            JobStatus::Cancelled => {
                crate::i18n::t("Cancelled; the partial file was removed").into()
            }
        };
        let title = format!("{} · {}", safe_text(&job.label), safe_text(&job.path));
        lines.push(if index == manager.job_selected {
            Line::styled(title, Style::default().add_modifier(Modifier::REVERSED))
        } else {
            Line::from(title)
        });
        lines.push(Line::styled(
            format!(
                "  {state} → {}",
                safe_text(&job.destination.to_string_lossy())
            ),
            match &job.status {
                JobStatus::Failed(_) => accent(app, Color::Red),
                JobStatus::Done => accent(app, Color::Green),
                _ => muted(),
            },
        ));
    }
    lines
}

fn draw_dialog(frame: &mut Frame, app: &App, area: Rect) {
    let Some(dialog) = &app.downloads.dialog else {
        return;
    };
    let mut lines = vec![
        Line::from(crate::tf!(
            "From: {label}",
            label = safe_text(dialog.source.label())
        )),
        Line::from(crate::tf!("Path: {path}", path = safe_text(&dialog.path))),
    ];
    match &dialog.preview {
        Preview::Loading => lines.push(Line::from(crate::i18n::t("Size: checking…"))),
        Preview::Failed(error) => {
            lines.push(Line::styled(safe_text(error), accent(app, Color::Yellow)))
        }
        Preview::Ready(header) => {
            let size = match (header.kind, dialog.totals, &dialog.count_failed) {
                (Kind::Folder, _, Some(_)) => crate::i18n::t("Size: could not be counted").into(),
                (Kind::Folder, Some(totals), _) if totals.done => crate::tf!(
                    "Size: {size}, {files} files",
                    size = human_size(totals.bytes),
                    files = totals.files
                ),
                (Kind::Folder, Some(totals), _) => crate::tf!(
                    "Size: at least {size}, {files} files…",
                    size = human_size(totals.bytes),
                    files = totals.files
                ),
                (Kind::Folder, None, _) => crate::i18n::t("Size: checking…").into(),
                _ => crate::tf!("Size: {size}", size = human_size(header.size)),
            };
            lines.push(Line::from(size));
            if let Ok(modified) = time::OffsetDateTime::from_unix_timestamp(header.modified)
                && header.modified > 0
                && let Ok(text) = modified.format(&time::format_description::well_known::Rfc3339)
            {
                lines.push(Line::from(crate::tf!("Modified: {time}", time = text)));
            }
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(crate::tf!(
        "Save to: {path}▏",
        path = safe_text(&dialog.save_to)
    )));
    let destination = crate::downloads::expand_home(dialog.save_to.trim());
    let needed = match &dialog.preview {
        Preview::Ready(header) if header.kind == Kind::Folder => {
            dialog.totals.map_or(0, |totals| totals.bytes)
        }
        Preview::Ready(header) => header.size,
        _ => 0,
    };
    let free = destination
        .parent()
        .and_then(crate::files::free_space)
        .filter(|free| *free < needed);
    if let Some(free) = free {
        lines.push(Line::styled(
            crate::tf!("This disk has only {free} free.", free = human_size(free)),
            accent(app, Color::Yellow),
        ));
    }
    if let Some(error) = &dialog.error {
        lines.push(Line::styled(safe_text(error), accent(app, Color::Red)));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        crate::i18n::t("This may contain sensitive data. A folder is saved as one .tar file."),
        muted(),
    ));
    let area = centered(area, 84, (lines.len() as u16 + 4).min(area.height));
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(crate::i18n::t(" Download "))
        .title_bottom(crate::i18n::t(
            " Enter Download · Esc Cancel · Ctrl-u Clear ",
        ));
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 34)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(120)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>() + "\n")
            .collect()
    }

    fn demo_on_container() -> App {
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        crate::demo::populate(&mut app);
        for _ in 0..6 {
            let keys: Vec<_> = app.rows.iter().map(|row| row.key.clone()).collect();
            app.expanded.extend(keys);
            app.refresh_tree();
        }
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, crate::app::Scope::Container(_)))
            .unwrap();
        app
    }

    #[test]
    fn the_panel_browses_a_tree_and_the_dialog_reviews_before_anything_downloads() {
        let mut app = demo_on_container();
        assert!(app.downloads.trees.is_empty(), "selecting starts nothing");
        app.open_download_manager();
        assert!(app.take_download_requests().is_empty());
        let output = screen(&mut app);
        assert!(output.contains("Download manager"), "{output}");
        assert!(output.contains("etc") && output.contains("var"), "{output}");

        // /tmp holds the dump; opening it shows the file with its size.
        let row = app.downloads.tree().unwrap().rows();
        app.downloads.selected = row.iter().position(|row| row.path == "/tmp").unwrap();
        app.downloads.toggle_selected();
        app.take_download_requests();
        let output = screen(&mut app);
        assert!(
            output.contains("mydb.dump") && output.contains("2.4 GB"),
            "{output}"
        );

        let row = app.downloads.tree().unwrap().rows();
        app.downloads.selected = row
            .iter()
            .position(|row| row.path == "/tmp/mydb.dump")
            .unwrap();
        app.downloads.dialog_selected();
        app.take_download_requests();
        let output = screen(&mut app);
        assert!(
            output.contains("Save to:") && output.contains("mydb.dump"),
            "{output}"
        );
        assert!(output.contains("Size: 2.4 GB"), "{output}");
        assert!(app.downloads.jobs.is_empty());
    }

    #[test]
    fn a_folder_shows_its_size_and_file_count_and_saves_as_tar() {
        let mut app = demo_on_container();
        app.download_path("/var/backups/");
        app.take_download_requests();
        let output = screen(&mut app);
        assert!(output.contains("Size: 2.3 GB, 2 files"), "{output}");
        assert!(output.contains("backups.tar"), "{output}");
    }

    #[test]
    fn relative_paths_the_root_and_groups_are_refused_with_a_reason() {
        let mut app = demo_on_container();
        app.download_path("some-folder/");
        assert!(app.notice.contains("full path"), "{}", app.notice);
        app.download_path("/");
        assert!(app.notice.contains("below /"), "{}", app.notice);
        assert!(app.downloads.dialog.is_none());
        app.selected = 0;
        app.open_download_manager();
        assert!(app.downloads.open.is_none());
        assert!(!app.notice.is_empty());
    }

    #[test]
    fn the_status_line_shows_running_downloads_after_the_panel_closes() {
        let mut app = demo_on_container();
        app.downloads.jobs.push(crate::downloads::Job {
            id: 1,
            label: "web".into(),
            path: "/x".into(),
            destination: "/out".into(),
            total: Some(1000),
            bytes: 430,
            started: std::time::Instant::now(),
            finished: None,
            status: JobStatus::Running,
        });
        let output = screen(&mut app);
        assert!(output.contains("↓ 1 · 43%"), "{output}");
        app.downloads.open_source(
            crate::files::Source::Container {
                connection: crate::demo::connections().remove(0),
                id: "abc".into(),
                label: "web".into(),
            },
            std::time::Instant::now(),
        );
        app.take_download_requests();
        app.downloads.pane = Pane::Downloads;
        let output = screen(&mut app);
        assert!(
            output.contains("43%") && output.contains("web · /x"),
            "{output}"
        );
    }

    #[test]
    fn german_renders_the_panel_and_dialog() {
        crate::i18n::isolate_thread();
        crate::i18n::set_language(crate::i18n::Language::German);
        let mut app = demo_on_container();
        app.open_download_manager();
        app.take_download_requests();
        let output = screen(&mut app);
        crate::i18n::set_language(crate::i18n::Language::English);
        assert!(output.contains("Download-Manager"), "{output}");
    }
}
