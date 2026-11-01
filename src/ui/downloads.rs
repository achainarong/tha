//! The download manager panel and its review dialog.
use super::*;
use crate::{
    downloads::{FilePreview, JobStatus, Pane, Preview, RowState, human_size},
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
    let dialog = centered(
        area,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, dialog);
    let label = manager
        .tree()
        .map_or_else(String::new, |tree| safe_text(tree.source.label()));
    let block = Block::bordered()
        .title(crate::tf!(" File explorer · {label} ", label = label))
        .title_bottom(match manager.pane {
            Pane::Browse => crate::i18n::t(
                " j/k Move · Enter Folder · / Find in folder · Ctrl-f Find in system · e Edit · d Download · a New · D Delete · r Refresh · Tab Downloads · Esc Close ",
            ),
            Pane::Downloads => {
                crate::i18n::t(" j/k Move · x Cancel · Tab Files · Esc Close; downloads keep running ")
            }
        });
    let inner = padded(block.inner(dialog));
    frame.render_widget(block, dialog);
    let height = inner.height as usize;
    if manager.pane == Pane::Downloads {
        frame.render_widget(Paragraph::new(job_lines(app, height)), inner);
        return;
    }
    let mut footer = Vec::new();
    if let Some(search) = &manager.search {
        let status = if search.running {
            crate::i18n::t("Searching · n/N Matches · x Cancel")
        } else if search.incomplete {
            crate::i18n::t("Incomplete · n/N Matches")
        } else {
            crate::i18n::t("Complete · n/N Matches")
        };
        footer.push(Line::from(crate::tf!(
            "Find in {root}: {query} · {count} matches · {status}",
            root = safe_text(&search.root),
            query = safe_text(&search.query),
            count = search.matches.len(),
            status = status,
        )));
        if let Some(error) = &search.error {
            footer.push(Line::styled(safe_text(error), accent(app, Color::Yellow)));
        } else if search.unreadable > 0 {
            footer.push(Line::from(crate::tf!(
                "{count} folders could not be read",
                count = search.unreadable
            )));
        }
    }
    if let Some(input) = &manager.search_input {
        footer.push(Line::from(crate::tf!(
            "Find in {root}: /{query}▏ · Enter Find · Esc Cancel",
            root = safe_text(&manager.search_root),
            query = safe_text(input),
        )));
    } else if let Some(input) = &manager.new_file_input {
        footer.push(Line::from(crate::tf!(
            "New file in {folder}: {name}▏ · Enter Create · Esc Cancel",
            folder = safe_text(&manager.new_file_folder),
            name = safe_text(input),
        )));
        // A name that was refused is still being typed, so the reason goes below it.
        if let Some(message) = &manager.message {
            footer.push(Line::styled(safe_text(message), accent(app, Color::Yellow)));
        }
    } else if let Some(command) = &manager.command {
        footer.push(Line::from(format!(":{}▏", safe_text(command))));
    } else if let Some(message) = &manager.message {
        footer.push(Line::styled(safe_text(message), accent(app, Color::Yellow)));
    }
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(footer.len() as u16)])
        .split(inner);
    let columns =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(rows[0]);
    frame.render_widget(
        Paragraph::new(browse_lines(
            app,
            columns[0].height as usize,
            columns[0].width as usize,
        )),
        columns[0],
    );
    let preview = Block::new().borders(ratatui::widgets::Borders::LEFT);
    let right = padded(preview.inner(columns[1]));
    frame.render_widget(preview, columns[1]);
    frame.render_widget(
        preview_paragraph(app, right.width, right.height as usize),
        right,
    );
    frame.render_widget(Paragraph::new(footer), rows[1]);
}

/// Read-only, and only ever sanitized text: file contents never reach the terminal raw.
fn preview_paragraph(app: &App, width: u16, height: usize) -> Paragraph<'_> {
    let warn = accent(app, Color::Yellow);
    let Some(preview) = &app.downloads.preview else {
        return Paragraph::new(crate::i18n::t("Select a file to see it here.")).style(muted());
    };
    let lines: Vec<Line<'_>> = match &preview.status {
        FilePreview::Waiting | FilePreview::Loading => {
            vec![Line::styled(crate::i18n::t("Reading…"), muted())]
        }
        FilePreview::Folder => vec![Line::styled(
            crate::i18n::t("A folder. Enter opens it, d downloads it."),
            muted(),
        )],
        FilePreview::Special => vec![Line::styled(
            crate::i18n::t("A link or special file, so it is not shown."),
            muted(),
        )],
        FilePreview::Binary => vec![
            Line::styled(
                crate::i18n::t("This looks like a binary file, so it is not shown."),
                warn,
            ),
            Line::styled(crate::i18n::t("d downloads it."), muted()),
        ],
        FilePreview::TooBig(size) => vec![
            Line::styled(
                crate::tf!(
                    "This file is big ({size}), so it is not shown automatically.",
                    size = human_size(*size)
                ),
                warn,
            ),
            Line::styled(
                crate::tf!(
                    "p shows the first {limit}. d downloads it.",
                    limit = human_size(crate::downloads::PREVIEW_LIMIT as u64)
                ),
                muted(),
            ),
        ],
        FilePreview::Failed(error) => vec![Line::styled(
            crate::tf!(
                "Could not read this file: {error}",
                error = safe_text(error)
            ),
            warn,
        )],
        FilePreview::Text { text, truncated } => {
            // Only what fits, so a frame costs the same whatever the size of the file.
            let total = text.line_count() + usize::from(*truncated);
            let first = preview.scroll.min(total.saturating_sub(1));
            let mut lines: Vec<Line<'_>> = (first..text.line_count())
                .take(height)
                .filter_map(|index| text.line(index))
                .map(Line::from)
                .collect();
            if *truncated && lines.len() < height {
                lines.push(Line::styled(
                    crate::i18n::t("… only the start of this file is shown"),
                    warn,
                ));
            }
            return Paragraph::new(lines);
        }
    };
    let mut paragraph = Paragraph::new(lines);
    if matches!(preview.status, FilePreview::Failed(_)) {
        paragraph = paragraph.wrap(ratatui::widgets::Wrap { trim: false });
    }
    let limit = paragraph.line_count(width).saturating_sub(1);
    let scroll = preview.scroll.min(limit);
    paragraph.scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0))
}

fn browse_lines(app: &App, height: usize, width: usize) -> Vec<Line<'static>> {
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
            (RowState::Failed(error), _) => safe_text(error),
            (RowState::Loading, _) => crate::i18n::t("reading…").into(),
            _ => String::new(),
        };
        let name = format!(
            "{}{marker}{}",
            "  ".repeat(row.level),
            safe_text(&row.entry.name)
        );
        let room = width.saturating_sub(detail.chars().count() + 1);
        let shown: String = name.chars().take(room).collect();
        let text = format!("{shown:<room$} {detail}");
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
        app.open_file_explorer();
        assert!(app.take_download_requests().is_empty());
        let output = screen(&mut app);
        assert!(output.contains("File explorer"), "{output}");
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

    fn settle_preview(app: &mut App) {
        let later = std::time::Instant::now() + std::time::Duration::from_secs(5);
        app.downloads.update_preview(std::time::Instant::now());
        app.downloads.update_preview(later);
        app.take_download_requests();
    }

    #[test]
    fn a_selected_file_shows_on_the_right_and_big_ones_warn_instead() {
        let mut app = demo_on_container();
        app.open_file_explorer();
        app.take_download_requests();
        let rows = app.downloads.tree().unwrap().rows();
        app.downloads.selected = rows.iter().position(|row| row.path == "/etc").unwrap();
        app.downloads.toggle_selected();
        app.take_download_requests();
        let rows = app.downloads.tree().unwrap().rows();
        app.downloads.selected = rows
            .iter()
            .position(|row| row.path == "/etc/hostname")
            .unwrap();
        settle_preview(&mut app);
        let output = screen(&mut app);
        assert!(
            output.contains("Sample contents of /etc/hostname"),
            "{output}"
        );

        let rows = app.downloads.tree().unwrap().rows();
        app.downloads.selected = rows.iter().position(|row| row.path == "/tmp").unwrap();
        app.downloads.toggle_selected();
        app.take_download_requests();
        let rows = app.downloads.tree().unwrap().rows();
        app.downloads.selected = rows
            .iter()
            .position(|row| row.path == "/tmp/mydb.dump")
            .unwrap();
        settle_preview(&mut app);
        let output = screen(&mut app);
        assert!(output.contains("is big (2.4 GB)"), "{output}");
        assert!(!output.contains("Sample contents"), "{output}");
        app.edit_selected_file();
        assert!(app.edit_request.is_none());
    }

    #[test]
    fn file_contents_cannot_send_control_sequences_to_the_terminal() {
        let mut app = demo_on_container();
        app.open_file_explorer();
        app.take_download_requests();
        settle_preview(&mut app);
        app.downloads.preview = None;
        let rows = app.downloads.tree().unwrap().rows();
        app.downloads.selected = rows.iter().position(|row| row.path == "/etc").unwrap();
        app.downloads.update_preview(std::time::Instant::now());
        if let Some(preview) = &mut app.downloads.preview {
            preview.status = FilePreview::text("before\u{1b}[2Jafter", false);
        }
        let output = screen(&mut app);
        assert!(output.contains("before"), "{output}");
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn a_long_preview_scrolls_to_its_end_and_stops_at_the_truncation_notice() {
        let mut app = demo_on_container();
        app.open_file_explorer();
        app.take_download_requests();
        settle_preview(&mut app);
        app.downloads.update_preview(std::time::Instant::now());
        let raw: String = (0..500).map(|index| format!("line-{index:03}\n")).collect();
        let preview = app.downloads.preview.as_mut().unwrap();
        preview.status = FilePreview::text(&raw, true);
        let output = screen(&mut app);
        assert!(output.contains("line-000"), "{output}");
        assert!(!output.contains("line-499"), "{output}");
        assert!(!output.contains("only the start"), "{output}");

        app.downloads.preview.as_mut().unwrap().scroll = 200;
        let output = screen(&mut app);
        assert!(output.contains("line-200"), "{output}");
        assert!(!output.contains("line-199"), "{output}");

        app.downloads.preview.as_mut().unwrap().scroll = usize::MAX;
        let output = screen(&mut app);
        assert!(output.contains("only the start"), "{output}");
        assert!(
            !output.contains("line-499"),
            "only the notice is left: {output}"
        );
    }

    #[test]
    fn long_preview_errors_wrap_so_the_recovery_hint_is_visible() {
        let mut app = demo_on_container();
        app.open_file_explorer();
        app.take_download_requests();
        app.downloads.update_preview(std::time::Instant::now());
        app.downloads.preview.as_mut().unwrap().status = FilePreview::Failed(format!(
            "Invalid agent configuration: {} REPAIR-USING-SSH",
            "unknown field test_param ".repeat(8)
        ));
        let output = screen(&mut app);
        assert!(output.contains("REPAIR-USING-SSH"), "{output}");
    }

    #[test]
    fn file_search_shows_its_scope_and_never_labels_cancelled_results_complete() {
        let mut app = demo_on_container();
        app.open_file_explorer();
        app.take_download_requests();
        app.downloads.begin_search(false);
        app.downloads.append_search("config");
        let output = screen(&mut app);
        assert!(output.contains("Find in /etc: /config"), "{output}");
        app.downloads.confirm_search();
        let output = screen(&mut app);
        assert!(output.contains("Searching"), "{output}");
        app.downloads.stop_search();
        let output = screen(&mut app);
        assert!(output.contains("Incomplete"), "{output}");
        app.downloads.begin_search(true);
        let output = screen(&mut app);
        assert!(output.contains("Find in /: /config"), "{output}");
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
        app.open_file_explorer();
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
    fn the_new_file_prompt_shows_where_it_goes_and_why_a_name_was_refused() {
        let mut app = demo_on_container();
        app.open_file_explorer();
        app.take_download_requests();
        app.downloads.demo = false;
        app.downloads.begin_new_file();
        app.downloads.append_new_file("a/b");
        app.downloads.confirm_new_file();
        let output = screen(&mut app);
        assert!(output.contains("New file in /etc: a/b"), "{output}");
        assert!(output.contains("without / or control"), "{output}");
    }

    #[test]
    fn the_delete_review_shows_what_was_checked_and_a_hostile_name_stays_harmless() {
        let mut app = demo_on_container();
        app.open_file_explorer();
        app.take_download_requests();
        let source = app.downloads.tree().unwrap().source.clone();
        app.downloads.delete_review = Some(crate::downloads::DeleteReview {
            id: 1,
            source,
            path: "/tmp/evil\u{1b}[2J.txt".into(),
            kind: Kind::File,
            check: Preview::Loading,
            error: None,
        });
        let output = screen(&mut app);
        assert!(output.contains("Delete file"), "{output}");
        assert!(output.contains("Checking"), "{output}");
        assert!(!output.contains('\u{1b}'));
        let review = app.downloads.delete_review.as_mut().unwrap();
        review.check = Preview::Ready(crate::files::Header {
            name: "evil".into(),
            kind: Kind::File,
            size: 2048,
            modified: 0,
            mode: 0o644,
            uid: 0,
            gid: 0,
        });
        let output = screen(&mut app);
        assert!(output.contains("2.0 KB"), "{output}");
        assert!(output.contains("cannot be undone"), "{output}");
        let review = app.downloads.delete_review.as_mut().unwrap();
        review.check = Preview::Failed("No such file or folder".into());
        let output = screen(&mut app);
        assert!(output.contains("No such file"), "{output}");
    }

    #[test]
    fn german_renders_the_panel_and_dialog() {
        crate::i18n::isolate_thread();
        crate::i18n::set_language(crate::i18n::Language::German);
        let mut app = demo_on_container();
        app.open_file_explorer();
        app.take_download_requests();
        let output = screen(&mut app);
        crate::i18n::set_language(crate::i18n::Language::English);
        assert!(output.contains("Datei-Explorer"), "{output}");
    }
}
