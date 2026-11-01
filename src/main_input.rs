//! Keyboard, mouse and command input: everything that turns an event into an app change.
use super::*;

pub(super) fn handle_key(
    app: &mut App,
    key: KeyEvent,
    height: usize,
    refresh: &watch::Sender<u64>,
) -> bool {
    app.cancel_startup();
    if app.exec_input && app.detail_focus && app.tab == Tab::Exec {
        if app.keyboard.leaves_exec(key) {
            app.exec_input = false;
            app.detail_focus = false;
            app.performance_focus = false;
            app.notice = tha::i18n::t("Exec is still connected · navigation mode · e refocuses the same container, or select 4 then Enter.").into();
        } else if let Some(session) = app.sessions.active_mut()
            && let Err(error) = session.key(key)
        {
            app.notice = tha::tf!("{error}", error = format!("{error:#}"));
        }
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }
    if app.servers_modal_visible() {
        app.servers_key(key);
        return false;
    }
    if let Some(removal) = app.inventory_removal.as_mut()
        && !removal.dismissed
    {
        if !removal.pending {
            match key.code {
                KeyCode::Esc | KeyCode::Char('n') => app.inventory_removal = None,
                KeyCode::Enter | KeyCode::Char('y')
                    if removal.visible
                        && !removal.targets.is_empty()
                        && key.kind == KeyEventKind::Press
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    let current = removal.targets.iter().all(|target| {
                        app.engines.iter().any(|engine| {
                            engine.error.is_none()
                                && engine.alias.is_none()
                                && engine.connection.endpoint == target.request.connection.endpoint
                                && engine
                                    .snapshot
                                    .as_ref()
                                    .and_then(|snapshot| snapshot.engine_id.as_deref())
                                    == Some(target.request.identity.as_str())
                        })
                    });
                    if current {
                        removal.pending = true;
                        app.notice = tha::tf!(
                            "Deleting {a0} resources · Esc hides progress; no automatic retries",
                            a0 = removal.targets.len()
                        );
                        app.inventory_removal_request = Some(
                            removal
                                .targets
                                .iter()
                                .map(|target| target.request.clone())
                                .collect(),
                        );
                    } else {
                        app.inventory_removal = None;
                        app.notice = tha::i18n::t("Runtime changed; deletion cancelled").into();
                    }
                }
                KeyCode::Up | KeyCode::PageUp => {
                    removal.scroll = removal
                        .scroll
                        .saturating_sub(if key.code == KeyCode::PageUp {
                            height.max(1)
                        } else {
                            1
                        })
                }
                KeyCode::Down | KeyCode::PageDown => {
                    removal.scroll = removal
                        .scroll
                        .saturating_add(if key.code == KeyCode::PageDown {
                            height.max(1)
                        } else {
                            1
                        })
                        .min(u16::MAX as usize)
                }
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Esc => removal.dismissed = true,
                KeyCode::Up | KeyCode::PageUp => {
                    removal.scroll = removal
                        .scroll
                        .saturating_sub(if key.code == KeyCode::PageUp {
                            height.max(1)
                        } else {
                            1
                        })
                }
                KeyCode::Down | KeyCode::PageDown => {
                    removal.scroll = removal
                        .scroll
                        .saturating_add(if key.code == KeyCode::PageDown {
                            height.max(1)
                        } else {
                            1
                        })
                        .min(u16::MAX as usize)
                }
                _ => {}
            }
        }
        return false;
    }
    if app.inventory_removal_results_visible {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                app.inventory_removal_results_visible = false
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => {
                app.inventory_removal_results_scroll = app
                    .inventory_removal_results_scroll
                    .saturating_sub(if key.code == KeyCode::PageUp {
                        height.max(1)
                    } else {
                        1
                    })
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => {
                app.inventory_removal_results_scroll = app
                    .inventory_removal_results_scroll
                    .saturating_add(if key.code == KeyCode::PageDown {
                        height.max(1)
                    } else {
                        1
                    })
                    .min(u16::MAX as usize)
            }
            _ => {}
        }
        return false;
    }
    if app.compose_confirmation.is_some() {
        let typing = app
            .compose_confirmation
            .as_ref()
            .unwrap()
            .request
            .action
            .requires_typing();
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => app.compose_confirmation = None,
            KeyCode::Enter if key.kind == KeyEventKind::Press && plain => app.confirm_compose(),
            KeyCode::Char('n') if !typing => app.compose_confirmation = None,
            KeyCode::Char('y') if !typing && plain => app.confirm_compose(),
            KeyCode::Backspace if typing => {
                app.compose_confirmation.as_mut().unwrap().typed.pop();
            }
            KeyCode::Char(ch) if typing && plain => {
                let confirmation = app.compose_confirmation.as_mut().unwrap();
                if !ch.is_control() && confirmation.typed.len() + ch.len_utf8() <= 1024 {
                    confirmation.typed.push(ch);
                }
            }
            _ => {}
        }
        return false;
    }
    if app.compose_output_visible {
        if let Some(job) = &mut app.compose_job {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    app.compose_output_visible = false;
                }
                KeyCode::Up | KeyCode::Char('k') => job.scroll = job.scroll.saturating_add(1),
                KeyCode::Down | KeyCode::Char('j') => job.scroll = job.scroll.saturating_sub(1),
                KeyCode::PageUp => job.scroll = job.scroll.saturating_add(height.max(1)),
                KeyCode::PageDown => job.scroll = job.scroll.saturating_sub(height.max(1)),
                KeyCode::End | KeyCode::Char('G') => job.scroll = 0,
                _ => {}
            }
        } else {
            app.compose_output_visible = false;
        }
        return false;
    }
    if app.action_confirmation.is_some() {
        use tha::actions::Action;
        let deleting = app.action_confirmation.as_ref().unwrap().request.action == Action::Delete;
        match key.code {
            KeyCode::Esc => app.action_confirmation = None,
            KeyCode::Enter if deleting => app.confirm_action(),
            KeyCode::Enter
                if key.kind == KeyEventKind::Press
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                app.confirm_action()
            }
            KeyCode::Char('n') if !deleting => app.action_confirmation = None,
            KeyCode::Char('y')
                if !deleting
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                app.confirm_action()
            }
            KeyCode::Up | KeyCode::PageUp => {
                let confirmation = app.action_confirmation.as_mut().unwrap();
                confirmation.scroll =
                    confirmation
                        .scroll
                        .saturating_sub(if key.code == KeyCode::PageUp {
                            height.max(1)
                        } else {
                            1
                        });
            }
            KeyCode::Down | KeyCode::PageDown => {
                let confirmation = app.action_confirmation.as_mut().unwrap();
                confirmation.scroll = confirmation
                    .scroll
                    .saturating_add(if key.code == KeyCode::PageDown {
                        height.max(1)
                    } else {
                        1
                    })
                    .min(u16::MAX as usize);
            }
            KeyCode::Backspace if deleting => {
                app.action_confirmation.as_mut().unwrap().typed.pop();
            }
            KeyCode::Char(ch)
                if deleting
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let confirmation = app.action_confirmation.as_mut().unwrap();
                if !ch.is_control() && confirmation.typed.len() + ch.len_utf8() <= 1024 {
                    confirmation.typed.push(ch);
                }
            }
            _ => {}
        }
        return false;
    }
    if app.action_results_visible {
        let Some(progress) = app.action_progress_mut() else {
            app.action_results_visible = false;
            return false;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                app.action_results_visible = false
            }
            KeyCode::Up | KeyCode::PageUp => {
                progress.scroll = progress
                    .scroll
                    .saturating_sub(if key.code == KeyCode::PageUp {
                        height.max(1)
                    } else {
                        1
                    })
            }
            KeyCode::Down | KeyCode::PageDown => {
                progress.scroll = progress
                    .scroll
                    .saturating_add(if key.code == KeyCode::PageDown {
                        height.max(1)
                    } else {
                        1
                    })
                    .min(u16::MAX as usize)
            }
            _ => {}
        }
        return false;
    }
    if app
        .settings
        .is_some_and(|settings| settings.picker.is_some())
    {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.cancel_option(),
            KeyCode::Enter | KeyCode::Char(' ') => app.confirm_option(),
            KeyCode::Char('j') | KeyCode::Down => app.move_option(1),
            KeyCode::Char('k') | KeyCode::Up => app.move_option(-1),
            KeyCode::Char('g') | KeyCode::Home => app.move_option(isize::MIN),
            KeyCode::Char('G') | KeyCode::End => app.move_option(isize::MAX),
            _ => {}
        }
        return false;
    }
    if app.settings.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | ',') => app.settings = None,
            KeyCode::Char('j') | KeyCode::Down => app.move_setting(1),
            KeyCode::Char('k') | KeyCode::Up => app.move_setting(-1),
            KeyCode::Char('h') | KeyCode::Left => app.change_setting(false, false),
            KeyCode::Char('l') | KeyCode::Right => app.change_setting(true, false),
            KeyCode::Enter | KeyCode::Char(' ') => app.change_setting(true, true),
            _ => {}
        }
        return false;
    }
    if app.help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::F(1) => {
                app.help = false
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.help_scroll = app.help_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => app.help_scroll = app.help_scroll.saturating_sub(1),
            KeyCode::PageDown => app.help_scroll = app.help_scroll.saturating_add(height.max(1)),
            KeyCode::PageUp => app.help_scroll = app.help_scroll.saturating_sub(height.max(1)),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.help_scroll = app.help_scroll.saturating_add((height / 2).max(1))
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.help_scroll = app.help_scroll.saturating_sub((height / 2).max(1))
            }
            KeyCode::Char('g') | KeyCode::Home => app.help_scroll = 0,
            KeyCode::Char('G') | KeyCode::End => app.help_scroll = u16::MAX as usize,
            _ => {}
        }
        return false;
    }
    if app.picker.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.picker = None,
            KeyCode::Enter => app.choose(),
            KeyCode::Char('j') | KeyCode::Down => {
                let picker = app.picker.as_mut().unwrap();
                picker.selected = (picker.selected + 1).min(picker.choices.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let picker = app.picker.as_mut().unwrap();
                picker.selected = picker.selected.saturating_sub(1);
            }
            _ => {}
        }
        return false;
    }
    if app.diagnostics.is_some() {
        return handle_diagnostics_key(app, key, height, refresh);
    }
    if app.network_test.is_some() {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter) {
            app.network_test = None;
        }
        return false;
    }
    if app.downloads.upload.is_some() {
        match key.code {
            KeyCode::Esc => app.downloads.upload = None,
            KeyCode::Enter => app.downloads.confirm_upload(std::time::Instant::now()),
            _ => {}
        }
        return false;
    }
    if app.downloads.delete_review.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => app.downloads.close_delete_review(),
            // A held key or a stray chord must not remove a file.
            KeyCode::Enter | KeyCode::Char('y')
                if key.kind == KeyEventKind::Press
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                app.downloads.confirm_delete();
            }
            _ => {}
        }
        return false;
    }
    if app.diagnostic_export.is_some() {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => app.diagnostic_export = None,
            KeyCode::Enter => app.confirm_diagnostic_export(),
            KeyCode::Tab => {
                if let Some(export) = &mut app.diagnostic_export {
                    export.redact = !export.redact;
                }
            }
            KeyCode::Char('u') if control => {
                if let Some(export) = &mut app.diagnostic_export {
                    export.path.clear();
                }
            }
            KeyCode::Backspace => {
                if let Some(export) = &mut app.diagnostic_export {
                    export.path.pop();
                    export.error = None;
                }
            }
            KeyCode::Char(character) if !control => {
                if let Some(export) = &mut app.diagnostic_export {
                    export.path.push(character);
                    export.error = None;
                }
            }
            _ => {}
        }
        return false;
    }
    if app.downloads.dialog.is_some() {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => app.downloads.close_dialog(),
            KeyCode::Enter => app.downloads.confirm_dialog(std::time::Instant::now()),
            KeyCode::Char('u') if control => {
                if let Some(dialog) = &mut app.downloads.dialog {
                    dialog.save_to.clear();
                }
            }
            KeyCode::Backspace => {
                if let Some(dialog) = &mut app.downloads.dialog {
                    dialog.save_to.pop();
                    dialog.error = None;
                }
            }
            KeyCode::Char(character) if !control => {
                if let Some(dialog) = &mut app.downloads.dialog {
                    dialog.save_to.push(character);
                    dialog.error = None;
                }
            }
            _ => {}
        }
        return false;
    }
    if app.downloads.open.is_some() {
        use tha::downloads::Pane;
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if app.downloads.search_input.is_some() {
            match key.code {
                KeyCode::Esc => app.downloads.search_input = None,
                KeyCode::Enter => app.downloads.confirm_search(),
                KeyCode::Char('u') if control => {
                    if let Some(input) = &mut app.downloads.search_input {
                        input.clear();
                    }
                }
                KeyCode::Backspace => {
                    if let Some(input) = &mut app.downloads.search_input {
                        input.pop();
                    }
                }
                KeyCode::Char(character) if !control => {
                    app.downloads.append_search(&character.to_string())
                }
                _ => {}
            }
            return false;
        }
        if app.downloads.new_file_input.is_some() {
            match key.code {
                KeyCode::Esc => {
                    app.downloads.new_file_input = None;
                    app.downloads.message = None;
                }
                KeyCode::Enter => app.downloads.confirm_new_file(),
                KeyCode::Char('u') if control => {
                    if let Some(input) = &mut app.downloads.new_file_input {
                        input.clear();
                    }
                }
                KeyCode::Backspace => {
                    if let Some(input) = &mut app.downloads.new_file_input {
                        input.pop();
                    }
                }
                KeyCode::Char(character) if !control => {
                    app.downloads.append_new_file(&character.to_string())
                }
                _ => {}
            }
            return false;
        }
        if let Some(command) = app.downloads.command.clone() {
            match key.code {
                KeyCode::Esc => app.downloads.command = None,
                KeyCode::Backspace => {
                    app.downloads.command = (!command.is_empty()).then(|| {
                        let mut text = command;
                        text.pop();
                        text
                    });
                }
                KeyCode::Char(character) => {
                    app.downloads.command = Some(command + &character.to_string());
                }
                KeyCode::Enter => {
                    app.downloads.command = None;
                    let command = command.trim();
                    let (name, argument) = command
                        .split_once(char::is_whitespace)
                        .map_or((command, ""), |(name, argument)| (name, argument.trim()));
                    match (name, argument) {
                        ("edit" | "e", "") => app.edit_selected_file(),
                        ("download" | "d", "") => app.downloads.dialog_selected(),
                        ("download" | "d", path) => {
                            app.download_path(path);
                            if app.downloads.dialog.is_none() {
                                app.downloads.message = Some(app.notice.clone());
                            }
                        }
                        ("find" | "find-all", query) => {
                            app.downloads.begin_search(name == "find-all");
                            if !query.is_empty() {
                                app.downloads.search_input = Some(query.to_owned());
                                app.downloads.confirm_search();
                            }
                        }
                        ("new", name) => {
                            app.downloads.begin_new_file();
                            if !name.is_empty() && app.downloads.new_file_input.is_some() {
                                app.downloads.append_new_file(name);
                                app.downloads.confirm_new_file();
                            }
                        }
                        ("delete", "") => app.downloads.begin_delete(),
                        ("q" | "quit", "") => app.downloads.close(),
                        ("", "") => {}
                        _ => {
                            app.downloads.message = Some(tha::tf!(
                                "Unknown command: {command}. Try :edit, :download, :new, :delete, :find, :find-all or :q",
                                command = tha::model::safe_text(command)
                            ));
                        }
                    }
                }
                _ => {}
            }
            return false;
        }
        let pane = app.downloads.pane;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.downloads.close(),
            KeyCode::Tab | KeyCode::BackTab => {
                app.downloads.pane = if pane == Pane::Browse {
                    Pane::Downloads
                } else {
                    Pane::Browse
                };
            }
            KeyCode::Char('j') | KeyCode::Down => app.downloads.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => app.downloads.move_selection(-1),
            KeyCode::PageDown => app.downloads.scroll_preview(height.max(1) as isize),
            KeyCode::PageUp => app.downloads.scroll_preview(-(height.max(1) as isize)),
            KeyCode::Char('x') if pane == Pane::Downloads => app.downloads.cancel_selected(),
            KeyCode::Char('/') if pane == Pane::Browse => app.downloads.begin_search(false),
            KeyCode::Char('f') if pane == Pane::Browse && control => {
                app.downloads.begin_search(true)
            }
            KeyCode::Char('n') if pane == Pane::Browse => app.downloads.next_search_match(false),
            KeyCode::Char('N') if pane == Pane::Browse => app.downloads.next_search_match(true),
            KeyCode::Char('x') if pane == Pane::Browse => app.downloads.stop_search(),
            KeyCode::Char(':') if pane == Pane::Browse => {
                app.downloads.command = Some(String::new());
            }
            KeyCode::Char('e') if pane == Pane::Browse => app.edit_selected_file(),
            KeyCode::Char('d') if pane == Pane::Browse => app.downloads.dialog_selected(),
            KeyCode::Char('a') if pane == Pane::Browse => app.downloads.begin_new_file(),
            KeyCode::Char('D') if pane == Pane::Browse => app.downloads.begin_delete(),
            KeyCode::Char('p') if pane == Pane::Browse => app.downloads.show_big_file(),
            KeyCode::Char('r') if pane == Pane::Browse => app.downloads.refresh_selected(),
            KeyCode::Char(' ') | KeyCode::Enter if pane == Pane::Browse => {
                app.downloads.toggle_selected();
            }
            _ => {}
        }
        return false;
    }
    if app.alerts.view_open {
        let count = app.alerts.open.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.alerts.view_open = false,
            KeyCode::Enter => app.open_alert_selection(),
            KeyCode::Char('j') | KeyCode::Down => {
                app.alerts.selected = (app.alerts.selected + 1).min(count.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.alerts.selected = app.alerts.selected.saturating_sub(1);
            }
            _ => {}
        }
        return false;
    }
    if app.timeline_view.is_some() {
        let count: usize = app
            .timeline_incidents()
            .iter()
            .map(|incident| incident.entries.len())
            .sum();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.timeline_view = None,
            KeyCode::Enter | KeyCode::Char('l') => app.open_timeline_logs(),
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(view) = &mut app.timeline_view {
                    view.selected = (view.selected + 1).min(count.saturating_sub(1));
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(view) = &mut app.timeline_view {
                    view.selected = view.selected.saturating_sub(1);
                }
            }
            _ => {}
        }
        return false;
    }
    if app.fleet_view.is_some() {
        let count = app.fleet_summary().problems.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.fleet_view = None,
            KeyCode::Enter => app.open_fleet_selection(),
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(view) = &mut app.fleet_view {
                    view.selected = (view.selected + 1).min(count.saturating_sub(1));
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(view) = &mut app.fleet_view {
                    view.selected = view.selected.saturating_sub(1);
                }
            }
            _ => {}
        }
        return false;
    }
    if let Some(view) = &mut app.capabilities_view {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => app.capabilities_view = None,
            KeyCode::Char('j') | KeyCode::Down => view.scroll = view.scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => view.scroll = view.scroll.saturating_sub(1),
            _ => {}
        }
        return false;
    }
    if app.input_mode == InputMode::InventorySearch {
        let view = app
            .inventory_views
            .entry((
                app.inventory_engine,
                if app.overview_mode == tha::app::OverviewMode::Security {
                    tha::app::OverviewMode::Images
                } else {
                    app.overview_mode
                },
            ))
            .or_default();
        match key.code {
            KeyCode::Esc => {
                app.input_mode = InputMode::Normal;
                app.input.clear();
            }
            KeyCode::Enter => {
                view.query = std::mem::take(&mut app.input);
                view.marked.clear();
                view.index = 0;
                view.selected = None;
                view.offset = 0;
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.input.clear()
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && app.input.len() + ch.len_utf8() <= 1024 =>
            {
                app.input.push(ch)
            }
            _ => {}
        }
        return false;
    }
    if app.input_mode != InputMode::Normal {
        match key.code {
            KeyCode::Esc => {
                app.input_mode = InputMode::Normal;
                app.input.clear();
            }
            KeyCode::Enter => {
                let mode = app.input_mode;
                let input = if mode == InputMode::ExportLogs {
                    app.input.clone()
                } else {
                    app.input.trim().to_owned()
                };
                app.input_mode = InputMode::Normal;
                app.input.clear();
                if mode == InputMode::Command {
                    return execute(app, &input, refresh);
                }
                if mode == InputMode::LogTextFilter {
                    app.log_view.filter.set_text(&input);
                    app.log_filter_terms = input;
                    app.notice = if app.log_view.filter.text_active() {
                        tha::i18n::t("Showing entries whose message contains every term. Retained logs are kept; F3 edits, :clear-log-filter resets.").into()
                    } else {
                        tha::i18n::t("Text filter cleared; advanced filters are unchanged.").into()
                    };
                } else if mode == InputMode::LogFilter {
                    match logs::filter::Filter::parse(&input) {
                        Ok(filter) => {
                            app.log_view.filter.set_advanced(filter);
                            app.log_filter_text = input;
                            app.notice = tha::i18n::t("Applied retained-log filter; entries without required metadata do not match. :clear-log-filter resets it.").into();
                        }
                        Err(error) => {
                            app.notice = tha::tf!(
                                "Invalid log filter: {error}",
                                error = format!("{error:#}")
                            );
                            app.input_mode = mode;
                            app.input = input;
                        }
                    }
                } else if mode == InputMode::ExportLogs {
                    if let Err(error) = queue_log_export(app, &input) {
                        app.notice = tha::tf!("{error}", error = format!("{error:#}"));
                        app.input_mode = mode;
                        app.input = input;
                    }
                } else if app.detail_focus && app.tab == Tab::Logs {
                    if let Err(error) = app.log_view.check_query(&input) {
                        app.notice = tha::model::safe_text(&tha::tf!(
                            "Invalid regex: {error}",
                            error = error
                        ))
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                        app.input_mode = mode;
                        app.input = input;
                    } else {
                        if app.notice.starts_with(tha::i18n::t("Invalid regex")) {
                            app.notice.clear();
                        }
                        app.log_view.set_query(input, &mut app.logs);
                    }
                } else {
                    app.tree_query = input;
                    app.refresh_tree();
                }
            }
            KeyCode::Char('r')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && app.input_mode == InputMode::Search
                    && app.detail_focus
                    && app.tab == Tab::Logs =>
            {
                app.log_view.regex_search = !app.log_view.regex_search;
            }
            KeyCode::Tab
                if matches!(
                    app.input_mode,
                    InputMode::LogTextFilter | InputMode::LogFilter
                ) =>
            {
                app.toggle_log_filter_field();
            }
            KeyCode::Tab if app.input_mode == InputMode::Command => {
                if let Some((command, _)) = ui::COMMANDS
                    .iter()
                    .find(|(command, _)| command.starts_with(&app.input))
                {
                    app.input = (*command).into();
                }
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(
                        app.input_mode,
                        InputMode::LogTextFilter | InputMode::LogFilter | InputMode::ExportLogs
                    ) =>
            {
                app.input.clear()
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && app.input.len() + ch.len_utf8() <= 1024 =>
            {
                app.input.push(ch);
            }
            _ => {}
        }
        return false;
    }
    // Only navigation reaches this point. Editors, confirmations and PTYs retain raw keys.
    let context = NavigationContext::of(app);
    let Some(action) = shortcuts::resolve(context, key, &app.keymap, app.security.enabled) else {
        app.pending_g = false;
        return false;
    };
    if !action.info().scope.includes(context) {
        app.notice = tha::tf!(
            "{a0} is unavailable in {a1}",
            a0 = action.info().label,
            a1 = context.label()
        );
        return false;
    }
    if app.keymap.action(key).is_some()
        && let Some(reason) = shortcuts::unavailable(app, action)
    {
        app.notice = reason;
        return false;
    }
    dispatch_navigation(app, action, key, height, refresh)
}

/// The overlay is modal, so its keys never compete with global shortcuts.
pub(super) fn handle_diagnostics_key(
    app: &mut App,
    key: KeyEvent,
    height: usize,
    refresh: &watch::Sender<u64>,
) -> bool {
    use tha::diagnostics::Kind;
    let Some(view) = &mut app.diagnostics else {
        return false;
    };
    let kind = view.kind;
    let page = height.max(1);
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.diagnostics_close(),
        KeyCode::Char('j') | KeyCode::Down => view.scroll = view.scroll.saturating_add(1),
        KeyCode::Char('k') | KeyCode::Up => view.scroll = view.scroll.saturating_sub(1),
        KeyCode::PageDown => view.scroll = view.scroll.saturating_add(page),
        KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(page),
        KeyCode::Char('d') if control => view.scroll = view.scroll.saturating_add(page / 2),
        KeyCode::Char('u') if control => view.scroll = view.scroll.saturating_sub(page / 2),
        KeyCode::Char('g') | KeyCode::Home => view.scroll = 0,
        KeyCode::Char('G') | KeyCode::End => view.scroll = usize::MAX,
        KeyCode::Char('r') => app.refresh_diagnostics(),
        KeyCode::Char('p') if kind == Kind::Network => app.open_diagnostic_peers(),
        KeyCode::Char('n') if kind == Kind::Network => {
            return follow_diagnostics_link(app, A::Networks, key, height, refresh);
        }
        KeyCode::Char('L') if kind == Kind::Health => app.load_diagnostic_logs(),
        KeyCode::Char('x') if kind == Kind::Health => {
            let name = tha::files::file_name(&view.main.target.name).to_owned();
            app.diagnostic_export = Some(tha::app::DiagnosticExport {
                path: tha::downloads::default_destination(&format!("{name}-diagnostics.json")),
                redact: true,
                error: None,
            });
        }
        KeyCode::Char('l') if kind == Kind::Health => {
            return follow_diagnostics_link(app, A::Logs, key, height, refresh);
        }
        KeyCode::Char('e') if kind == Kind::Health => {
            return follow_diagnostics_link(app, A::Events, key, height, refresh);
        }
        KeyCode::Char('m') if kind == Kind::Health => {
            return follow_diagnostics_link(app, A::Metrics, key, height, refresh);
        }
        _ => {}
    }
    false
}

/// Opens an existing view for the same container, with its filters left exactly as they are.
pub(super) fn follow_diagnostics_link(
    app: &mut App,
    action: A,
    key: KeyEvent,
    height: usize,
    refresh: &watch::Sender<u64>,
) -> bool {
    let blocked = app
        .diagnostics_link_ready()
        .err()
        .or_else(|| shortcuts::unavailable(app, action));
    if let Some(reason) = blocked {
        if let Some(view) = &mut app.diagnostics {
            view.message = Some(reason);
        }
        return false;
    }
    app.diagnostics_close();
    let quit = dispatch_navigation(app, action, key, height, refresh);
    match action {
        A::Logs if app.log_view.filter.active() => {
            app.notice = tha::tf!(
                "Logs keep the active filter ({filter}), which may hide related entries. :clear-log-filter shows them.",
                filter = app.log_filter_summary()
            );
        }
        A::Metrics => {
            app.notice = tha::i18n::t(
                "Metrics show only samples collected so far; earlier history may not exist.",
            )
            .into();
        }
        _ => {}
    }
    quit
}

pub(super) fn dispatch_navigation(
    app: &mut App,
    action: A,
    key: KeyEvent,
    height: usize,
    refresh: &watch::Sender<u64>,
) -> bool {
    // Command-only actions use the same implementation and safeguards as the palette.
    if matches!(
        action,
        A::AllRuntimes
            | A::AdvancedLogFilter
            | A::ClearLogFilter
            | A::Diagnose
            | A::NetworkDiagnose
            | A::ExportLogs
            | A::ReconnectLogs
            | A::LogsSince
            | A::SaveView
            | A::Upload
            | A::NetworkTest
            | A::Download
            | A::OpenView
            | A::DeleteView
            | A::ComposeUpdate
            | A::ComposePull
            | A::ComposeUp
            | A::ComposeRecreate
            | A::ComposeDown
            | A::ComposeOutput
            | A::PrettyJson
            | A::CheckUpdates
            | A::PullImage
            | A::CloseExec
            | A::UpdateSecurity
            | A::ReloadLayers
            | A::Refresh
            | A::ActionResults
            | A::RemovalResults
            | A::Monochrome
            | A::Quit
    ) {
        return execute(app, action.info().commands[0], refresh);
    }
    if matches!(action, A::PreviousView | A::NextView) {
        use tha::app::OverviewMode::*;
        let forward = action == A::NextView;
        let modes: &[tha::app::OverviewMode] = if app.security.enabled {
            &[Containers, Images, Volumes, Security]
        } else {
            &[Containers, Images, Volumes]
        };
        let index = modes
            .iter()
            .position(|mode| *mode == app.overview_mode)
            .unwrap_or(0);
        let next = modes[if forward {
            (index + 1) % modes.len()
        } else {
            (index + modes.len() - 1) % modes.len()
        }];
        switch_overview(app, next);
        return false;
    }
    match action {
        A::ContainersView => switch_overview(app, tha::app::OverviewMode::Containers),
        A::ImagesView => switch_overview(app, tha::app::OverviewMode::Images),
        A::VolumesView => switch_overview(app, tha::app::OverviewMode::Volumes),
        A::SecurityView => switch_overview(app, tha::app::OverviewMode::Security),
        A::Help => {
            app.help = true;
            app.help_scroll = 0;
        }
        A::Settings => app.settings = Some(Default::default()),
        A::Servers => app.open_servers(),
        A::ToggleConnection => app.toggle_selected_connection(),
        A::Commands => {
            app.input_mode = InputMode::Command;
            app.input.clear();
        }
        A::Views => {
            return execute(app, "views", refresh);
        }
        _ => {}
    }
    if matches!(
        action,
        A::ContainersView
            | A::ImagesView
            | A::VolumesView
            | A::SecurityView
            | A::Help
            | A::Commands
            | A::Views
    ) {
        return false;
    }
    // Detail shortcuts explicitly return from an inventory to workload details.
    if matches!(
        action,
        A::Overview | A::Logs | A::Networks | A::Exec | A::Metrics | A::Layers | A::Events
    ) {
        switch_overview(app, tha::app::OverviewMode::Containers);
    }
    if app.overview_mode == tha::app::OverviewMode::Security {
        let finding_list =
            !app.security.expanded && matches!(app.security.state, tha::security::State::Ready(_));
        match action {
            A::ScanImage => app.request_security_scan(false),
            A::ScanImages => app.request_security_scan(true),
            A::CancelScan => app.security.cancel = true,
            A::CheckSecurityImage | A::CheckImage | A::CheckImages => {
                app.overview_mode = tha::app::OverviewMode::Images;
                ui::inventory::navigate(
                    app,
                    if action == A::CheckImages {
                        KeyCode::Char('M')
                    } else {
                        KeyCode::Char('m')
                    },
                    height,
                );
                app.overview_mode = tha::app::OverviewMode::Security;
            }
            A::ClearScanChecks => {
                app.inventory_views
                    .entry((app.inventory_engine, tha::app::OverviewMode::Images))
                    .or_default()
                    .scan_marked
                    .clear();
            }
            A::SecurityFocusNext => app.detail_focus = !app.detail_focus,
            A::Zoom => app.security.zoom = !app.security.zoom,
            A::Open if !app.detail_focus => app.open_security(),
            A::Open if matches!(&app.security.state, tha::security::State::Ready(report) if !report.findings.is_empty()) =>
            {
                app.security.expanded = !app.security.expanded;
                app.detail_scroll = 0;
            }
            A::Open => {}
            A::Escape if app.security.expanded => {
                app.security.expanded = false;
                app.detail_scroll = 0;
            }
            A::Escape if app.security.zoom => app.security.zoom = false,
            A::Escape if app.detail_focus => app.detail_focus = false,
            A::Down if app.detail_focus && finding_list => app.security.move_by(1),
            A::Up if app.detail_focus && finding_list => app.security.move_by(-1),
            A::First if app.detail_focus && finding_list => app.security.move_by(isize::MIN),
            A::Last if app.detail_focus && finding_list => app.security.move_by(isize::MAX),
            A::PageDown if app.detail_focus && finding_list => {
                app.security.move_by(height as isize)
            }
            A::PageUp if app.detail_focus && finding_list => {
                app.security.move_by(-(height as isize))
            }
            A::Down if app.detail_focus => app.detail_scroll = app.detail_scroll.saturating_add(1),
            A::Up if app.detail_focus => app.detail_scroll = app.detail_scroll.saturating_sub(1),
            A::PageDown if app.detail_focus => {
                app.detail_scroll = app.detail_scroll.saturating_add(height)
            }
            A::PageUp if app.detail_focus => {
                app.detail_scroll = app.detail_scroll.saturating_sub(height)
            }
            A::First if app.detail_focus => app.detail_scroll = 0,
            A::Last if app.detail_focus => app.detail_scroll = u16::MAX as usize,
            _ => {
                app.overview_mode = tha::app::OverviewMode::Images;
                let quit = dispatch_navigation(app, action, key, height, refresh);
                if app.overview_mode == tha::app::OverviewMode::Images {
                    app.overview_mode = tha::app::OverviewMode::Security;
                }
                return quit;
            }
        }
        return false;
    }
    if app.overview_mode != tha::app::OverviewMode::Containers {
        match action {
            A::RemoveResources => ui::inventory::begin_removal(app),
            A::PruneResources => ui::inventory::begin_prune(app),
            A::Escape
                if app
                    .inventory_views
                    .get(&(app.inventory_engine, app.overview_mode))
                    .is_some_and(|view| view.details) =>
            {
                ui::inventory::navigate(app, KeyCode::Esc, height)
            }
            A::Escape => switch_overview(app, tha::app::OverviewMode::Containers),
            A::Back => return true,
            A::PreviousRuntime
            | A::NextRuntime
            | A::SecurityPreviousRuntime
            | A::SecurityNextRuntime => {
                // Steps through what the tabs offer, never onto a hidden or disconnected engine.
                let engines = app.engines_in_view();
                if let Some(position) = engines.iter().position(|&e| e == app.inventory_engine) {
                    let next = if matches!(action, A::PreviousRuntime | A::SecurityPreviousRuntime)
                    {
                        position.saturating_sub(1)
                    } else {
                        (position + 1).min(engines.len() - 1)
                    };
                    app.inventory_engine = engines[next];
                } else if let Some(&first) = engines.first() {
                    app.inventory_engine = first;
                }
            }
            A::Search => {
                let view = app
                    .inventory_views
                    .entry((app.inventory_engine, app.overview_mode))
                    .or_default();
                app.input = view.query.clone();
                view.details = false;
                app.input_mode = InputMode::InventorySearch;
            }
            A::Up
            | A::Down
            | A::PageUp
            | A::PageDown
            | A::First
            | A::Last
            | A::Open
            | A::InventoryFilter
            | A::InventorySort
            | A::Mark
            | A::MarkUnused
            | A::ClearMarks
            | A::Zoom
            | A::CheckImage
            | A::CheckImages => {
                // The existing inventory operation accepts a canonical key, not
                // a user binding. Never feed it the original configured key.
                let code = if action == A::First
                    && (key.code == KeyCode::Home || app.keymap.action(key).is_some())
                {
                    KeyCode::Home
                } else if action == A::Last {
                    KeyCode::End
                } else {
                    let binding = action.info().bindings.first().expect("inventory binding");
                    let event: KeyEvent = binding
                        .key
                        .parse::<crokey::KeyCombination>()
                        .expect("catalog key")
                        .into();
                    event.code
                };
                ui::inventory::navigate(app, code, height)
            }
            A::RefreshInventory => refresh.send_modify(|value| *value += 1),
            A::Security => app.open_security(),
            _ => {}
        }
        return false;
    }
    if app.detail_focus && app.tab == Tab::Security {
        let handled = match action {
            A::Zoom => {
                app.security.zoom = !app.security.zoom;
                true
            }
            A::Open => {
                app.security.expanded = !app.security.expanded;
                app.detail_scroll = 0;
                true
            }
            A::Escape if app.security.expanded => {
                app.security.expanded = false;
                app.detail_scroll = 0;
                true
            }
            A::Escape if app.security.zoom => {
                app.security.zoom = false;
                true
            }
            A::Down if !app.security.expanded => {
                app.security.move_by(1);
                true
            }
            A::Up if !app.security.expanded => {
                app.security.move_by(-1);
                true
            }
            A::First if !app.security.expanded => {
                app.security.move_by(isize::MIN);
                true
            }
            A::Last if !app.security.expanded => {
                app.security.move_by(isize::MAX);
                true
            }
            A::PageDown if !app.security.expanded => {
                app.security.move_by(height as isize);
                true
            }
            A::PageUp if !app.security.expanded => {
                app.security.move_by(-(height as isize));
                true
            }
            _ => false,
        };
        if handled {
            return false;
        }
    }
    if app.detail_focus && app.tab == Tab::History {
        let view = &mut app.history_view;
        let handled = match action {
            A::PreviousRange => {
                view.range_by(-1);
                true
            }
            A::NextRange => {
                view.range_by(1);
                true
            }
            A::MetricsLeft | A::MetricsRight
                if view.cursor.is_none()
                    && (matches!(key.code, KeyCode::Left | KeyCode::Right)
                        || app.keymap.action(key).is_some()) =>
            {
                view.pan(if action == A::MetricsLeft { -1 } else { 1 });
                true
            }
            A::MetricsLeft => {
                view.cursor = Some(
                    view.cursor
                        .unwrap_or(view.data.points.len().saturating_sub(1))
                        .saturating_sub(1),
                );
                view.end_ms.get_or_insert_with(tha::history::now_ms);
                true
            }
            A::MetricsRight => {
                view.cursor = Some(
                    view.cursor
                        .unwrap_or(0)
                        .saturating_add(1)
                        .min(view.data.points.len().saturating_sub(1)),
                );
                view.end_ms.get_or_insert_with(tha::history::now_ms);
                true
            }
            A::MetricsCursor => {
                view.cursor = if view.cursor.is_some() {
                    None
                } else {
                    Some(view.data.points.len().saturating_sub(1))
                };
                view.end_ms.get_or_insert_with(tha::history::now_ms);
                true
            }
            A::MetricsLive => {
                view.end_ms = None;
                view.cursor = None;
                true
            }
            A::Zoom => {
                view.expanded = !view.expanded;
                true
            }
            A::Down => {
                view.metric = (view.metric + 1) % 4;
                true
            }
            A::Up => {
                view.metric = (view.metric + 3) % 4;
                true
            }
            A::Escape if view.expanded => {
                view.expanded = false;
                true
            }
            A::Escape if view.cursor.is_some() => {
                view.cursor = None;
                true
            }
            _ => false,
        };
        if handled {
            return false;
        }
    }
    let pending_g = app.pending_g;
    app.pending_g = false;
    match action {
        A::FilterLogs => app.open_log_filter(InputMode::LogTextFilter),
        A::Start | A::Stop | A::Restart | A::DeleteContainer | A::Pause | A::Unpause | A::Kill => {
            app.request_action(action.lifecycle().expect("lifecycle action"))
        }
        A::Back => {
            if app.explorer_zoomed() {
                app.explorer_zoom = false;
            } else if app.detail_zoom == Some(app.tab) {
                app.detail_zoom = None;
            } else if app.tab == Tab::History && app.history_view.expanded {
                app.history_view.expanded = false;
            } else if app.performance_focus {
                app.performance_focus = false;
            } else if app.detail_focus {
                app.detail_focus = false;
            } else if !app.back() {
                return true;
            }
        }
        A::Escape => {
            if app.explorer_zoomed() {
                app.explorer_zoom = false;
            } else if app.detail_zoom == Some(app.tab) {
                app.detail_zoom = None;
            } else if app.tab == Tab::History && app.history_view.expanded {
                app.history_view.expanded = false;
            } else if app.performance_open && app.performance_expanded {
                app.performance_expanded = false;
            } else if app.tab == Tab::Layers && app.detail_focus && app.layers.expanded {
                app.layers.expanded = false;
                app.detail_scroll = 0;
            } else if app.performance_focus {
                app.performance_focus = false;
            } else if app.detail_focus && app.tab == Tab::Logs && !app.log_view.query().is_empty() {
                app.log_view.set_query(String::new(), &mut app.logs);
            } else if !app.tree_query.is_empty() {
                app.tree_query.clear();
                app.refresh_tree();
            } else if app.detail_focus {
                app.detail_focus = false;
            } else {
                app.back();
            }
        }
        A::WorkspaceBack => {
            app.back();
        }
        A::OpenExec => app.request_exec(),
        A::Performance | A::PerformanceSpace => app.toggle_performance(),
        A::PerformanceScope => app.switch_performance_scope(),
        A::Zoom if app.performance_focus => {
            app.performance_expanded = !app.performance_expanded;
        }
        A::Zoom if app.detail_focus => {
            app.detail_zoom = if app.detail_zoom == Some(app.tab) {
                None
            } else {
                Some(app.tab)
            };
        }
        A::Zoom => app.explorer_zoom = !app.explorer_zoom,
        A::SortContexts => app.pick_context_sort(),
        A::SavedViews => app.pick_saved_view(),
        A::CopyId => app.copy_selected(tha::app::CopyField::Id),
        A::CopyName => app.copy_selected(tha::app::CopyField::Name),
        A::CopyImage => app.copy_selected(tha::app::CopyField::Image),
        A::CopyLogLine => app.copy_log_line(),
        A::ComposeEdit => app.request_compose_edit(),
        A::Alerts => app.alerts.view_open = true,
        A::Timeline => app.open_timeline(),
        A::Fleet => app.open_fleet(),
        A::FileExplorer => app.open_file_explorer(),
        A::Capabilities => app.open_capabilities(),
        A::ExecSessions => app.pick_sessions(),
        A::Actions => app.open_actions(),
        A::Search => {
            app.input_mode = InputMode::Search;
            app.input.clear();
        }
        A::FocusNext | A::FocusPrevious => app.cycle_focus(action == A::FocusPrevious),
        A::Overview => {
            app.tab = Tab::Overview;
            app.detail_scroll = 0;
        }
        A::Events => {
            app.tab = Tab::Events;
            app.detail_focus = true;
            app.performance_focus = false;
            app.detail_scroll = 0;
        }
        A::Logs => {
            app.tab = Tab::Logs;
            app.detail_scroll = 0;
        }
        A::Networks => {
            app.tab = Tab::Networks;
            app.detail_scroll = 0;
        }
        A::Exec => {
            if !app.exec_tab_available() {
                app.notice = tha::i18n::t("Exec requires a running container or service.").into();
                return false;
            }
            app.tab = Tab::Exec;
            app.detail_focus = true;
            app.performance_focus = false;
            app.exec_input = false;
        }
        A::Layers => {
            if !app.layers_available() {
                return false;
            }
            app.tab = Tab::Layers;
            app.detail_focus = true;
            app.performance_focus = false;
            app.layers.expanded = false;
            app.detail_scroll = 0;
        }
        A::Security => app.open_security(),
        A::Metrics if app.performance_container().is_some() => {
            app.tab = Tab::History;
            app.detail_focus = true;
            app.performance_focus = false;
        }
        A::Open => {
            if app.performance_focus
                && app.performance_scope == tha::app::PerformanceScope::Container
            {
                if app.performance_container().is_some() {
                    app.performance_focus = false;
                    app.detail_focus = true;
                    app.tab = Tab::Overview;
                }
            } else if app.performance_focus {
                let engine_index = app.performance_engine();
                let Some(engine_index) = engine_index else {
                    return false;
                };
                while app.back() {}
                app.performance_focus = false;
                app.tree_query.clear();
                app.refresh_tree();
                app.selected = app
                    .rows
                    .iter()
                    .position(|row| {
                        row.engine == engine_index && row.scope == tha::app::Scope::Engine
                    })
                    .unwrap_or(0);
                app.tab = Tab::Overview;
                app.detail_focus = true;
            } else if app.detail_focus && app.tab == Tab::Layers {
                if matches!(&app.layers.state, tha::layers::State::Ready(image) if image.rows() > 0)
                {
                    app.layers.expanded = !app.layers.expanded;
                    app.detail_scroll = 0;
                }
            } else if app.detail_focus && app.network_cursor_active() {
                app.open_network_target();
            } else if app.detail_focus && app.tab == Tab::Exec {
                if let Some(session) = app.sessions.active_mut()
                    && session.running()
                {
                    session.follow();
                    app.exec_input = true;
                } else if app.sessions.active().is_none() {
                    app.request_exec();
                }
            } else if !app.detail_focus {
                app.open_selected();
            }
        }
        A::ToggleBranch => app.toggle_branch(),
        A::Down => move_by(app, 1, height),
        A::Up => move_by(app, -1, height),
        A::HalfPageDown => move_by(app, (height / 2).max(1) as isize, height),
        A::HalfPageUp => move_by(app, -((height / 2).max(1) as isize), height),
        A::PageDown => move_by(app, height.max(1) as isize, height),
        A::PageUp => move_by(app, -(height.max(1) as isize), height),
        A::Left => {
            if !app.detail_focus && !app.performance_focus {
                app.collapse();
            } else if app.tab == Tab::Logs && !app.log_view.wrap && key.code != KeyCode::Left {
                app.log_view.horizontal = app.log_view.horizontal.saturating_sub(8);
            } else if app.detail_focus {
                return cycle_tab(app, false, key, height, refresh);
            }
        }
        A::Right => {
            if !app.detail_focus && !app.performance_focus {
                app.expand();
            } else if app.tab == Tab::Logs && !app.log_view.wrap && key.code != KeyCode::Right {
                app.log_view.horizontal = (app.log_view.horizontal + 8).min(16_384);
            } else if app.detail_focus {
                return cycle_tab(app, true, key, height, refresh);
            }
        }
        A::PreviousTab => return cycle_tab(app, false, key, height, refresh),
        A::NextTab => return cycle_tab(app, true, key, height, refresh),
        A::First => {
            if pending_g || key.code == KeyCode::Home || app.keymap.action(key).is_some() {
                if app.performance_focus {
                    app.move_performance_selection(isize::MIN);
                } else if !app.detail_focus {
                    app.selected = 0;
                } else if app.tab == Tab::Logs {
                    app.log_view.first(&mut app.logs);
                } else if app.network_cursor_active() {
                    app.move_network_cursor(isize::MIN);
                } else if app.tab == Tab::Layers && !app.layers.expanded {
                    app.layers.selected = 0;
                    app.detail_scroll = 0;
                } else if app.tab == Tab::Exec {
                    if let Some(session) = app.sessions.active_mut() {
                        session.scroll(500);
                    }
                } else {
                    app.detail_scroll = 0;
                }
            } else {
                app.pending_g = true;
            }
        }
        A::Last => {
            if app.performance_focus {
                app.move_performance_selection(isize::MAX);
            } else if !app.detail_focus {
                app.selected = app.rows.len().saturating_sub(1);
            } else if app.tab == Tab::Logs {
                app.logs.follow = true;
            } else if app.network_cursor_active() {
                app.move_network_cursor(isize::MAX);
            } else if app.tab == Tab::Layers && !app.layers.expanded {
                app.layers.move_by(isize::MAX);
            } else if app.tab == Tab::Exec {
                if let Some(session) = app.sessions.active_mut() {
                    session.follow();
                }
            } else {
                app.detail_scroll = u16::MAX as usize;
            }
        }
        A::Follow => {
            if app.logs.follow {
                app.log_view.scroll(&mut app.logs, 0);
            } else {
                app.logs.follow = true;
            }
        }
        A::Wrap => app.log_view.toggle_wrap(&app.logs),
        A::NextIssue | A::PreviousIssue => {
            let backwards = action == A::PreviousIssue;
            let (view, logs) = (&mut app.log_view, &mut app.logs);
            if !view.next_issue(logs, backwards) {
                app.notice = tha::i18n::t("No possible issues in the logs that are showing").into();
            }
        }
        A::NextMatch | A::PreviousMatch => {
            let backwards = action == A::PreviousMatch;
            if app.detail_focus && app.tab == Tab::Logs {
                app.log_view.next_match(&mut app.logs, backwards);
            } else {
                app.next_tree_match(backwards);
            }
        }
        _ => {}
    }
    false
}

/// Moves along the tab bar through the same actions as the number keys.
pub(super) fn cycle_tab(
    app: &mut App,
    forward: bool,
    key: KeyEvent,
    height: usize,
    refresh: &watch::Sender<u64>,
) -> bool {
    let tabs = app.available_tabs();
    let index = tabs.iter().position(|tab| *tab == app.tab).unwrap_or(0);
    let next = if forward {
        (index + 1) % tabs.len()
    } else {
        (index + tabs.len() - 1) % tabs.len()
    };
    dispatch_navigation(app, tab_action(tabs[next]), key, height, refresh)
}

pub(super) fn tab_action(tab: Tab) -> A {
    match tab {
        Tab::Overview => A::Overview,
        Tab::Logs => A::Logs,
        Tab::Networks => A::Networks,
        Tab::Exec => A::Exec,
        Tab::Events => A::Events,
        Tab::History => A::Metrics,
        Tab::Layers => A::Layers,
        Tab::Security => A::Security,
    }
}

pub(super) const WHEEL_ROWS: usize = 3;

/// Mouse events only navigate. Anything destructive still needs its keyboard confirmation.
pub(super) fn handle_mouse(
    app: &mut App,
    mouse: MouseEvent,
    height: usize,
    refresh: &watch::Sender<u64>,
) -> bool {
    match mouse.kind {
        MouseEventKind::Up(MouseButton::Left) => {
            app.log_scrollbar_drag = None;
            return false;
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if app.log_scrollbar_drag.is_some() {
                seek_log_scrollbar(app, mouse.row);
            }
            return false;
        }
        MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
        | MouseEventKind::ScrollUp
        | MouseEventKind::ScrollDown => {}
        _ => return false,
    }
    app.cancel_startup();
    if app.action_confirmation.is_some()
        || app.compose_confirmation.is_some()
        || app.input_mode != InputMode::Normal
        || app
            .inventory_removal
            .as_ref()
            .is_some_and(|removal| !removal.dismissed)
    {
        return false;
    }
    let back = mouse.kind == MouseEventKind::Down(MouseButton::Right);
    if back && app.exec_input {
        // Esc belongs to the shell, so this matches Ctrl-] instead.
        app.exec_input = false;
        app.detail_focus = false;
        return false;
    }
    let target = app.mouse_target(mouse.column, mouse.row);
    let wheel = match mouse.kind {
        MouseEventKind::ScrollUp => Some(KeyCode::Up),
        MouseEventKind::ScrollDown => Some(KeyCode::Down),
        _ => None,
    };
    if app.servers_modal_visible() {
        if back {
            return handle_key(app, KeyEvent::from(KeyCode::Esc), height, refresh);
        }
        if let Some(code) = wheel {
            return handle_key(app, KeyEvent::from(code), height, refresh);
        }
        if let Some(MouseTarget::ServerRow(index)) = target {
            app.select_server_row(index);
        }
        return false;
    }
    if app.help
        || app.settings.is_some()
        || app.picker.is_some()
        || app.diagnostics.is_some()
        || app.inventory_removal_results_visible
        || app.action_results_visible
        || app.compose_output_visible
    {
        if back {
            return handle_key(app, KeyEvent::from(KeyCode::Esc), height, refresh);
        }
        if let Some(code) = wheel {
            return handle_key(app, KeyEvent::from(code), height, refresh);
        }
        if let Some(MouseTarget::OptionRow(index)) = target
            && let Some(picker) = app.settings.and_then(|settings| settings.picker)
        {
            if picker.selected == index {
                app.confirm_option();
            } else {
                app.preview_option(index);
            }
        } else if let Some(MouseTarget::SettingsRow(index)) = target
            && let Some(settings) = &mut app.settings
        {
            if settings.selected == index {
                app.change_setting(true, true);
            } else {
                settings.selected = index;
            }
        } else if let Some(MouseTarget::PickerRow(index)) = target
            && let Some(picker) = &mut app.picker
        {
            if picker.selected == index {
                app.choose();
            } else {
                picker.selected = index;
            }
        }
        return false;
    }
    if back {
        let esc = KeyEvent::from(KeyCode::Esc);
        return dispatch_navigation(app, A::Escape, esc, height, refresh);
    }
    let Some(target) = target else {
        return false;
    };
    match wheel {
        Some(code) => wheel_over(app, target, code, height, refresh),
        None => click(app, target, mouse, height, refresh),
    }
}

pub(super) fn focus_explorer(app: &mut App) {
    app.exec_input = false;
    app.detail_focus = false;
    app.performance_focus = false;
}

pub(super) fn focus_details(app: &mut App) {
    app.detail_focus = true;
    app.performance_focus = false;
}

pub(super) fn focus_performance(app: &mut App) {
    app.exec_input = false;
    app.detail_focus = false;
    app.performance_focus = true;
}

pub(super) fn wheel_over(
    app: &mut App,
    target: MouseTarget,
    code: KeyCode,
    height: usize,
    refresh: &watch::Sender<u64>,
) -> bool {
    use MouseTarget as T;
    match target {
        T::Explorer | T::TreeRow(_) | T::TreeArrow(_) => focus_explorer(app),
        T::Performance | T::PerformanceEngine(_) => focus_performance(app),
        T::Details
        | T::DetailTab(_)
        | T::MetricsRange(_)
        | T::LogScrollbar
        | T::NetworkRow(_)
        | T::LayerRow(_)
        | T::FindingsPane
        | T::FindingRow(_) => focus_details(app),
        T::InventoryPane | T::InventoryRow(_) => {
            if app.overview_mode == OverviewMode::Security {
                app.detail_focus = false;
            }
        }
        T::Overview(_)
        | T::CollapsedPerformance
        | T::InventoryRuntime(_)
        | T::PickerRow(_)
        | T::SettingsRow(_)
        | T::ServerRow(_)
        | T::OptionRow(_) => {
            return false;
        }
    }
    let logs =
        app.overview_mode == OverviewMode::Containers && app.detail_focus && app.tab == Tab::Logs;
    // Rolling down at the live end should not stop following.
    if logs && app.logs.follow && code == KeyCode::Down {
        return false;
    }
    let scrolling_text = app.overview_mode == OverviewMode::Containers
        && app.detail_focus
        && matches!(
            app.tab,
            Tab::Overview | Tab::Logs | Tab::Exec | Tab::Events | Tab::History
        );
    let action = if code == KeyCode::Up { A::Up } else { A::Down };
    for _ in 0..if scrolling_text { WHEEL_ROWS } else { 1 } {
        if dispatch_navigation(app, action, KeyEvent::from(code), height, refresh) {
            return true;
        }
    }
    false
}

pub(super) fn click(
    app: &mut App,
    target: MouseTarget,
    mouse: MouseEvent,
    height: usize,
    refresh: &watch::Sender<u64>,
) -> bool {
    use MouseTarget as T;
    let open = KeyEvent::from(KeyCode::Enter);
    let explorer_focused = !app.detail_focus && !app.performance_focus;
    let details_focused = app.detail_focus && !app.performance_focus;
    match target {
        T::Overview(mode) => {
            let action = match mode {
                OverviewMode::Containers => A::ContainersView,
                OverviewMode::Images => A::ImagesView,
                OverviewMode::Volumes => A::VolumesView,
                OverviewMode::Security => A::SecurityView,
            };
            app.exec_input = false;
            return dispatch_navigation(app, action, open, height, refresh);
        }
        T::Explorer => focus_explorer(app),
        T::TreeRow(index) => {
            let again = explorer_focused && app.selected == index;
            focus_explorer(app);
            if again {
                return dispatch_navigation(app, A::Open, open, height, refresh);
            }
            app.selected = index;
            app.detail_scroll = 0;
        }
        T::TreeArrow(index) => {
            focus_explorer(app);
            app.selected = index;
            app.detail_scroll = 0;
            app.toggle_branch();
        }
        T::Details => {
            if details_focused && app.tab == Tab::Exec && app.sessions.active().is_some() {
                return dispatch_navigation(app, A::Open, open, height, refresh);
            }
            focus_details(app);
        }
        T::DetailTab(tab) => {
            app.exec_input = false;
            focus_details(app);
            return dispatch_navigation(app, tab_action(tab), open, height, refresh);
        }
        T::LogScrollbar => {
            focus_details(app);
            let track = app
                .mouse_regions
                .iter()
                .find(|(_, target)| *target == T::LogScrollbar)
                .map(|(area, _)| *area);
            match track {
                Some(area) if mouse.row == area.y => app.log_view.scroll(&mut app.logs, -1),
                Some(area) if mouse.row + 1 == area.bottom() => {
                    app.log_view.scroll(&mut app.logs, 1)
                }
                _ => {
                    seek_log_scrollbar(app, mouse.row);
                    app.log_scrollbar_drag =
                        Some((mouse.row, app.log_view.scroll_position(&app.logs).0));
                }
            }
        }
        T::MetricsRange(index) => {
            focus_details(app);
            let amount = index as isize - app.history_view.range as isize;
            app.history_view.range_by(amount);
        }
        T::NetworkRow(index) => {
            if details_focused && app.network_cursor == index {
                app.open_network_target();
            } else {
                focus_details(app);
                app.network_cursor = index;
            }
        }
        T::LayerRow(index) => {
            if details_focused && app.layers.selected == index {
                return dispatch_navigation(app, A::Open, open, height, refresh);
            }
            focus_details(app);
            app.layers.selected = index;
        }
        T::FindingsPane => focus_details(app),
        T::FindingRow(index) => {
            if details_focused && app.security.selected == index {
                return dispatch_navigation(app, A::Open, open, height, refresh);
            }
            focus_details(app);
            app.security.selected = index;
        }
        T::Performance => focus_performance(app),
        T::CollapsedPerformance => app.toggle_performance(),
        T::PerformanceEngine(index) => {
            if app.performance_focus && app.performance_scroll == index {
                return dispatch_navigation(app, A::Open, open, height, refresh);
            }
            focus_performance(app);
            app.performance_scroll = index;
        }
        T::InventoryPane => {
            if app.overview_mode == OverviewMode::Security {
                app.detail_focus = false;
            }
        }
        T::InventoryRuntime(index) => {
            if let Some(&engine) = app.engines_in_view().get(index) {
                app.inventory_engine = engine;
            }
        }
        T::InventoryRow(index) => {
            let security = app.overview_mode == OverviewMode::Security;
            let mode = if security {
                OverviewMode::Images
            } else {
                app.overview_mode
            };
            let view = app
                .inventory_views
                .entry((app.inventory_engine, mode))
                .or_default();
            if view.index == index && !(security && app.detail_focus) {
                return dispatch_navigation(app, A::Open, open, height, refresh);
            }
            view.index = index;
            // Selection follows the row's identity, so the old key would pull it back.
            view.selected = None;
            if security {
                app.detail_focus = false;
            }
        }
        T::PickerRow(_) | T::SettingsRow(_) | T::ServerRow(_) | T::OptionRow(_) => {}
    }
    false
}

/// The arrows at each end of the scrollbar are not part of its track.
pub(super) fn seek_log_scrollbar(app: &mut App, row: u16) {
    let Some(area) = app
        .mouse_regions
        .iter()
        .find(|(_, target)| *target == MouseTarget::LogScrollbar)
        .map(|(area, _)| *area)
    else {
        return;
    };
    let track = area.height.saturating_sub(2);
    if track == 0 {
        return;
    }
    let (thumb_start, thumb_length) = app.log_view.scrollbar_thumb(&app.logs, track as usize);
    let travel = (track as usize).saturating_sub(thumb_length);
    if travel == 0 {
        return;
    }
    let (position, total) = app.log_view.scroll_position(&app.logs);
    let range = total.saturating_sub(area.height as usize);
    let target = if let Some((origin_row, origin_position)) = app.log_scrollbar_drag {
        let delta = row as isize - origin_row as isize;
        origin_position
            .saturating_add_signed(delta * range as isize / travel as isize)
            .min(range)
    } else {
        let offset = row.saturating_sub(area.y + 1) as usize;
        if (thumb_start..thumb_start + thumb_length).contains(&offset) {
            // Grabbing the thumb pauses following without repositioning it.
            app.log_view.scroll(&mut app.logs, 0);
            return;
        }
        offset.saturating_sub(thumb_length / 2).min(travel) * range / travel
    };
    if target == position {
        app.log_view.scroll(&mut app.logs, 0);
        return;
    }
    app.log_view
        .scroll_to_fraction(&mut app.logs, target, range.max(1));
}

pub(super) fn move_by(app: &mut App, amount: isize, _height: usize) {
    if app.performance_focus {
        app.move_performance_selection(amount);
    } else if !app.detail_focus {
        app.move_selection(amount);
        app.detail_scroll = 0;
    } else if app.tab == Tab::Logs {
        app.log_view.scroll(&mut app.logs, amount);
    } else if app.network_cursor_active() {
        app.move_network_cursor(amount);
    } else if app.tab == Tab::Layers && !app.layers.expanded {
        app.layers.move_by(amount);
    } else if app.tab == Tab::Exec {
        if let Some(session) = app.sessions.active_mut() {
            session.scroll(-amount);
        }
    } else {
        app.detail_scroll = app
            .detail_scroll
            .saturating_add_signed(amount)
            .min(u16::MAX as usize);
    }
}

pub(super) fn switch_overview(app: &mut App, mode: tha::app::OverviewMode) {
    if mode == tha::app::OverviewMode::Security && !app.security.enabled {
        return;
    }
    if app.overview_mode == tha::app::OverviewMode::Containers
        && mode != tha::app::OverviewMode::Containers
    {
        app.inventory_engine = app.current().map(|row| row.engine).unwrap_or(0);
    }
    app.overview_mode = mode;
    if mode == tha::app::OverviewMode::Security {
        app.detail_focus = false;
    }
}

pub(super) fn execute(app: &mut App, command: &str, refresh: &watch::Sender<u64>) -> bool {
    if matches!(
        command,
        "overview" | "logs" | "networks" | "layers" | "history" | "metrics" | "exec" | "events"
    ) {
        switch_overview(app, tha::app::OverviewMode::Containers);
    }
    app.notice.clear();
    if let Some(arguments) = command.strip_prefix("upload ") {
        app.request_upload(arguments);
        return false;
    }
    if let Some(destination) = command.strip_prefix("network-test ") {
        app.request_network_test(destination);
        return false;
    }
    if let Some(path) = command.strip_prefix("download ") {
        app.download_path(path);
        return false;
    }
    if let Some(name) = command.strip_prefix("save-view ") {
        app.save_view(name);
        return false;
    }
    if let Some(name) = command.strip_prefix("delete-view ") {
        app.delete_view(name);
        return false;
    }
    if let Some(name) = command.strip_prefix("open-view ") {
        app.apply_saved_view(name.trim());
        return false;
    }
    if let Some(argument) = command.strip_prefix("logs-since ") {
        match logs::parse_since(argument, time::OffsetDateTime::now_utc()) {
            Ok(since) => {
                app.log_since = since;
                app.tab = Tab::Logs;
                app.detail_focus = true;
                app.performance_focus = false;
                if app.demo {
                    app.notice =
                        tha::i18n::t("Demo mode: fixture logs ignore the start time.").into();
                }
            }
            Err(error) => {
                app.notice = tha::tf!("{error}", error = format!("{error:#}"));
                app.input_mode = InputMode::Command;
                app.input = command.into();
            }
        }
        return false;
    }
    match command {
        "events" => {
            app.tab = Tab::Events;
            app.detail_focus = true;
            app.performance_focus = false;
            app.detail_scroll = 0;
        }
        "compose-update" => app.request_compose(tha::compose::Action::Update),
        "compose-pull" => app.request_compose(tha::compose::Action::Pull),
        "compose-up" => app.request_compose(tha::compose::Action::Up),
        "compose-recreate" => app.request_compose(tha::compose::Action::Recreate),
        "compose-down" => app.request_compose(tha::compose::Action::Down),
        "compose-output" => {
            if app.compose_job.is_some() {
                app.compose_output_visible = true;
            } else {
                app.notice = tha::i18n::t("No Compose operation has run in this session").into();
            }
        }
        "logs-since" => {
            app.input_mode = InputMode::Command;
            app.input = "logs-since ".into();
        }
        "save-view" | "delete-view" | "open-view" | "download" | "upload" | "network-test" => {
            app.input_mode = InputMode::Command;
            app.input = format!("{command} ");
        }
        "json" => {
            app.log_view.toggle_pretty_json(&mut app.logs);
        }
        "check-updates" => ui::inventory::begin_update_check(app),
        "pull-image" => ui::inventory::begin_image_pull(app),
        "security" => app.open_security(),
        "scan-image" => app.request_security_scan(false),
        "scan-images" => app.request_security_scan(true),
        "prune" => ui::inventory::begin_prune(app),
        "cancel-scan" if app.overview_mode == tha::app::OverviewMode::Containers => {
            app.notice = tha::i18n::t("Scans run from the Security view (Ctrl+→ or Alt+4)").into();
        }
        "cancel-scan" => {
            app.security.cancel = true;
            app.notice = tha::i18n::t("Cancelling security queue and removing scanner…").into();
        }
        "update-security-db" => {
            app.request_security_scan(false);
            if app.security.request {
                app.security.update = true;
            }
        }
        "removal-results" => {
            if app.inventory_removal_report.is_some() {
                app.inventory_removal_results_visible = true;
                app.inventory_removal_results_scroll = 0;
            } else {
                app.notice = tha::i18n::t("No image/volume cleanup results yet").into();
            }
        }
        "quit" | "q" => return true,
        "overview" => {
            app.tab = Tab::Overview;
            app.detail_focus = true;
            app.detail_scroll = 0;
        }
        "logs" => {
            app.tab = Tab::Logs;
            app.detail_focus = true;
        }
        "networks" => {
            app.tab = Tab::Networks;
            app.detail_focus = true;
            app.detail_scroll = 0;
        }
        "refresh" => {
            refresh.send_modify(|value| *value += 1);
            if app.demo {
                app.notice = tha::i18n::t("Demo mode: fixture data is unchanged.").into();
            } else {
                app.notice = tha::i18n::t("Refreshing all engines…").into();
            }
        }
        "reconnect-logs" => {
            app.log_scope = None;
            app.tab = Tab::Logs;
            app.detail_focus = true;
        }
        "wrap" => app.log_view.toggle_wrap(&app.logs),
        "exec" => app.request_exec(),
        "layers" => {
            if !app.layers_available() {
                app.notice =
                    tha::i18n::t("Layers requires a container or single-container service").into();
                return false;
            }
            app.tab = Tab::Layers;
            app.detail_focus = true;
            app.performance_focus = false;
            app.detail_scroll = 0;
        }
        "images" | "volumes" => {
            let mode = if command == "images" {
                tha::app::OverviewMode::Images
            } else {
                tha::app::OverviewMode::Volumes
            };
            switch_overview(app, mode);
        }
        "history" | "metrics" => {
            if app.performance_container().is_some() {
                app.tab = Tab::History;
                app.detail_focus = true;
                app.performance_focus = false;
            } else {
                app.notice = tha::i18n::t("Select a container to open Metrics").into();
            }
        }
        "reload-layers" => {
            app.layers.select(None);
            app.detail_scroll = 0;
        }
        "actions" => app.open_actions(),
        "action-results" => app.open_action_results(),
        "filter-logs" => {
            app.tab = Tab::Logs;
            app.detail_focus = true;
            app.performance_focus = false;
            app.open_log_filter(InputMode::LogTextFilter);
        }
        "filter-logs-advanced" => {
            app.tab = Tab::Logs;
            app.detail_focus = true;
            app.performance_focus = false;
            app.open_log_filter(InputMode::LogFilter);
        }
        "clear-log-filter" => app.clear_log_filters(),
        "diagnose" => app.open_diagnostics(tha::diagnostics::Kind::Health),
        "network-diagnose" => app.open_diagnostics(tha::diagnostics::Kind::Network),
        "export-logs" => {
            if app.tab != Tab::Logs || app.log_scope.is_none() {
                app.notice =
                    tha::i18n::t("Open and review Logs for the desired workload before exporting")
                        .into();
            } else if app.log_export_busy {
                app.notice = tha::i18n::t("Another export is already running").into();
            } else {
                app.log_export_scope = app.log_scope.clone();
                app.log_export_review_visible = false;
                app.input_mode = InputMode::ExportLogs;
                app.input.clear();
                app.notice.clear();
            }
        }
        "close-exec" => app.close_exec(),
        "exec-sessions" => app.pick_sessions(),
        "performance" => app.toggle_performance(),
        "sort-contexts" => app.pick_context_sort(),
        "saved-views" => app.pick_saved_view(),
        "next-issue" | "previous-issue" => {
            let (view, logs) = (&mut app.log_view, &mut app.logs);
            if !view.next_issue(logs, command == "previous-issue") {
                app.notice = tha::i18n::t("No possible issues in the logs that are showing").into();
            }
        }
        "pause" => app.request_action(tha::actions::Action::Pause),
        "unpause" => app.request_action(tha::actions::Action::Unpause),
        "kill" => app.request_action(tha::actions::Action::Kill),
        "copy-id" => app.copy_selected(tha::app::CopyField::Id),
        "copy-name" => app.copy_selected(tha::app::CopyField::Name),
        "copy-image" => app.copy_selected(tha::app::CopyField::Image),
        "copy-log-line" => app.copy_log_line(),
        "compose-edit" => app.request_compose_edit(),
        "alerts" => app.alerts.view_open = true,
        "timeline" => app.open_timeline(),
        "fleet" => app.open_fleet(),
        "file-explorer" => app.open_file_explorer(),
        "capabilities" => app.open_capabilities(),
        "back" => {
            app.back();
        }
        "all" => while app.back() {},
        "monochrome" => app.monochrome = !app.monochrome,
        "settings" => app.settings = Some(Default::default()),
        "servers" => app.open_servers(),
        "toggle-connection" => app.toggle_selected_connection(),
        "help" => {
            app.help = true;
            app.help_scroll = 0;
        }
        "" => {}
        _ => {
            app.notice = tha::tf!(
                "Unknown command: {command}. Press : then Tab to discover commands.",
                command = command
            )
        }
    }
    false
}
