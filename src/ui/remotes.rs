//! The Servers modal. The logic lives in `crate::machines`.
use super::*;
use crate::machines::{Form, FormRow, Machine, MachineState, ModalView, RemotesModal};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(modal) = app.remotes_modal.clone() else {
        return;
    };
    match &modal.view {
        ModalView::List => draw_list(frame, app, area, &modal),
        ModalView::ConfirmRemove(name) => draw_confirm(frame, area, name),
        ModalView::Form(form) => draw_form(frame, app, area, form),
    }
}

fn state_label(machine: &Machine) -> (&'static str, &'static str) {
    match &machine.state {
        MachineState::NotConnected => ("○", t("Not connected")),
        MachineState::Connecting => ("◐", t("Connecting…")),
        MachineState::Connected => ("●", t("Connected")),
        MachineState::Offline { retrying: true, .. } => ("◐", t("Offline, retrying")),
        MachineState::Offline { .. } => ("⚠", t("Needs attention")),
    }
}

fn mode_label(app: &App, machine: &Machine) -> &'static str {
    machine.remote.connect.unwrap_or(app.remote_connect).label()
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect, modal: &RemotesModal) {
    let area = centered(area, 78, 18);
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(t(" Servers ")).title_bottom(t(
        " a Add · e Edit · d Remove · Enter Connect/Disconnect · Esc Close ",
    ));
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    let mut lines: Vec<Line<'static>> = Vec::new();
    if app.machines.is_empty() {
        lines.push(Line::from(t("No servers yet. Press a to add one.")));
        lines.push(Line::from(""));
        lines.push(Line::styled(
            t("Each server runs tha-agent, which only allows start, stop, restart, logs and inventory."),
            muted(),
        ));
    }
    let selected = modal.selected.min(app.machines.len().saturating_sub(1));
    for (index, machine) in app.machines.iter().enumerate() {
        let (symbol, _) = state_label(machine);
        let text = format!(
            "{symbol} {:<18} {:<28} {}{}",
            safe_text(&machine.remote.name),
            safe_text(&machine.remote.address()),
            mode_label(app, machine),
            if machine.managed {
                ""
            } else {
                " · config.toml"
            },
        );
        lines.push(if index == selected {
            Line::styled(text, selected_style())
        } else {
            Line::from(text)
        });
    }
    let rows = lines.len() as u16;
    if !app.machines.is_empty() {
        for index in 0..rows.min(inner.height) {
            app.mouse_regions.push((
                Rect::new(inner.x, inner.y + index, inner.width, 1),
                MouseTarget::ServerRow(index as usize),
            ));
        }
    }
    if let Some(machine) = app.machines.get(selected) {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            format!(
                "{} · {}",
                state_label(machine).1,
                safe_text(&machine.remote.identity_file)
            ),
            muted(),
        ));
        match &machine.state {
            MachineState::Connected => {
                let count = app.machine_engines(&machine.remote.name).len();
                lines.push(Line::styled(
                    crate::tf!(
                        "{count} engines · delete {delete}",
                        count = count,
                        delete = if machine.allow_delete {
                            t("on")
                        } else {
                            t("off")
                        }
                    ),
                    muted(),
                ));
            }
            MachineState::Offline { reason, .. } => {
                lines.push(Line::styled(safe_text(reason), accent(app, Color::Yellow)));
            }
            _ => {}
        }
    }
    if let Some(message) = &modal.message {
        lines.push(Line::from(""));
        lines.push(Line::styled(safe_text(message), accent(app, Color::Yellow)));
    }
    if app.remotes_path.is_none() && modal.message.is_none() {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            t("Changes last for this session: saving is turned off."),
            muted(),
        ));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_confirm(frame: &mut Frame, area: Rect, name: &str) {
    let area = centered(area, 64, 8);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(t(" Remove server "))
        .title_bottom(t(" y Remove · Esc Cancel "));
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(crate::tf!(
                "Remove {name} from this list and disconnect it?",
                name = safe_text(name)
            )),
            Line::from(""),
            Line::styled(t("The key file and the server are not changed."), muted()),
        ])
        .wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_form(frame: &mut Frame, app: &mut App, area: Rect, form: &Form) {
    let area = centered(area, 86, 26);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(if form.editing.is_some() {
            t(" Edit server ")
        } else {
            t(" Add server ")
        })
        .title_bottom(t(" Tab/↑/↓ Move · Enter Select · Ctrl+S Save · Esc Back "));
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    let connect = match form.connect {
        None => crate::tf!(
            "Use the Settings value ({mode})",
            mode = app.remote_connect.label()
        ),
        Some(mode) => mode.label().to_owned(),
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (index, row) in FormRow::ALL.iter().enumerate() {
        let focused = index == form.focus;
        let text_field = match row {
            FormRow::Name => Some((t("Name"), &form.name, t("<name>"))),
            FormRow::Host => Some((t("Host"), &form.host, t("<name|address>"))),
            FormRow::User => Some((t("User"), &form.user, t("<user>"))),
            FormRow::Port => Some((t("Port"), &form.port, t("<port>"))),
            FormRow::Key => Some((t("SSH key"), &form.key, t("<path>"))),
            _ => None,
        };
        let line = if let Some((label, value, placeholder)) = text_field {
            let mut spans = vec![Span::raw(format!("{label:<12}"))];
            if !value.is_empty() {
                spans.push(Span::raw(safe_text(value)));
            }
            if focused {
                spans.push(Span::raw("▏"));
            }
            // Dimmed and bracketed, so an empty field can never pass for a filled-in one.
            if value.is_empty() {
                spans.push(Span::styled(placeholder.to_owned(), muted()));
            }
            Line::from(spans)
        } else {
            Line::from(match row {
                FormRow::Connect => format!("{:<12}◂ {connect} ▸", t("Connect")),
                FormRow::CreateKey => format!("[ {} ]", t("Create new key at this path")),
                FormRow::CopySetup => format!("[ {} ]", t("Copy server setup command")),
                FormRow::Test => format!("[ {} ]", t("Test connection")),
                _ => format!("[ {} ]", t("Save")),
            })
        };
        lines.push(if focused {
            line.style(selected_style())
        } else {
            line
        });
        if (index as u16) < inner.height {
            app.mouse_regions.push((
                Rect::new(inner.x, inner.y + index as u16, inner.width, 1),
                MouseTarget::ServerRow(index),
            ));
        }
    }
    if crate::remote::unrestricted_login(form.user.trim()) {
        lines.push(Line::styled(
            t("Root key: tha's traffic uses the allowlist, but anyone holding this key has full root access."),
            accent(app, Color::Yellow),
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        t("Step 1: run this on the server to install and set up the agent:"),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    match &form.setup {
        Ok(command) => lines.push(Line::from(safe_text(command))),
        Err(reason) => lines.push(Line::styled(safe_text(reason), muted())),
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        t("Step 2: Test connection confirms the host key. Step 3: Save."),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    if form.confirm_discard {
        lines.push(Line::styled(
            t("Discard your changes? y Discard · any other key keeps editing"),
            accent(app, Color::Yellow),
        ));
    } else if let Some(error) = &form.error {
        lines.push(Line::styled(safe_text(error), accent(app, Color::Red)));
    } else if let Some(note) = &form.note {
        lines.push(Line::styled(safe_text(note), accent(app, Color::Green)));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn selected_style() -> Style {
    super::selected()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{BackendKind, Connection},
        remote::{self, Remote},
    };
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::{Terminal, backend::TestBackend};

    fn app() -> App {
        let mut app = App::new(
            vec![Connection {
                name: "Docker / local".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///local.sock".into(),
            }],
            100,
            false,
            false,
        );
        app.load_machines(
            Vec::new(),
            Ok(vec![Remote {
                name: "prod".into(),
                host: "10.0.0.5".into(),
                user: "tha".into(),
                identity_file: "/keys/k".into(),
                port: None,
                connect: None,
                metrics_refresh_seconds: None,
            }]),
            None,
        );
        app
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_explorer_names_each_machine() {
        let mut app = app();
        let screen = render(&mut app, 160, 30);
        assert!(screen.contains("Local"), "{screen}");
        assert!(screen.contains("prod (remote) [not connected]"), "{screen}");
    }

    #[test]
    fn a_narrow_explorer_keeps_the_name_and_remote_label() {
        let mut app = app();
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("prod (remote)"), "{screen}");
    }

    #[test]
    fn the_list_shows_servers_and_their_actions() {
        let mut app = app();
        app.open_servers();
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains(" Servers "), "{screen}");
        assert!(screen.contains("tha@10.0.0.5"), "{screen}");
        assert!(screen.contains("a Add"), "{screen}");
    }

    #[test]
    fn the_form_renders_every_row_at_a_small_terminal() {
        let mut app = app();
        app.open_servers();
        app.servers_key(KeyEvent::from(KeyCode::Char('a')));
        let screen = render(&mut app, 80, 24);
        for text in [
            "Name",
            "Host",
            "SSH key",
            "Connect",
            "Test connection",
            "Save",
        ] {
            assert!(screen.contains(text), "{text}: {screen}");
        }
    }

    #[test]
    fn empty_fields_show_dimmed_placeholders_that_are_not_values() {
        let mut app = app();
        app.open_servers();
        app.servers_key(KeyEvent::from(KeyCode::Char('a')));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let cells: Vec<_> = buffer.content().iter().collect();
        let text: String = cells.iter().map(|cell| cell.symbol()).collect();
        for placeholder in ["<name>", "<name|address>", "<user>", "<port>", "<path>"] {
            let start = text
                .find(placeholder)
                .unwrap_or_else(|| panic!("{placeholder} missing"));
            let column = text[..start].chars().count();
            assert_eq!(
                cells[column].fg,
                muted().fg.unwrap_or_default(),
                "{placeholder} must be dimmed"
            );
        }
        assert!(!text.contains("such as prod"));
    }

    #[test]
    fn a_typed_value_replaces_the_placeholder() {
        let mut app = app();
        app.open_servers();
        app.servers_key(KeyEvent::from(KeyCode::Char('a')));
        app.servers_key(KeyEvent::from(KeyCode::Char('x')));
        let screen = render(&mut app, 120, 40);
        assert!(!screen.contains("<name>"), "{screen}");
    }

    #[test]
    fn narrow_terminals_do_not_panic() {
        let mut app = app();
        app.open_servers();
        for size in [(40, 10), (20, 5), (10, 3)] {
            render(&mut app, size.0, size.1);
        }
        app.servers_key(KeyEvent::from(KeyCode::Char('a')));
        for size in [(40, 10), (20, 5), (10, 3)] {
            render(&mut app, size.0, size.1);
        }
    }

    #[test]
    fn clicking_selects_a_row() {
        let mut app = app();
        app.open_servers();
        app.servers_key(KeyEvent::from(KeyCode::Char('a')));
        render(&mut app, 120, 30);
        assert!(
            app.mouse_regions
                .iter()
                .any(|(_, target)| matches!(target, MouseTarget::ServerRow(3)))
        );
        app.select_server_row(3);
        let ModalView::Form(form) = &app.remotes_modal.as_ref().unwrap().view else {
            panic!("the form must stay open");
        };
        assert_eq!(form.focus, 3);
    }

    #[test]
    fn a_connected_engine_is_labelled_with_its_endpoint_not_a_temp_socket() {
        let mut app = app();
        app.connect_machine("prod");
        let generation = app.machine("prod").unwrap().generation;
        app.apply_remote_event(remote::Event {
            server: "prod".into(),
            generation,
            update: remote::Update::Connected {
                engines: vec!["docker".into()],
                allow_delete: false,
                capabilities: None,
            },
        });
        let index = app.machine_engines("prod")[0];
        app.update_engine(index, Ok(Default::default()));
        app.selected = app
            .rows
            .iter()
            .position(|row| row.engine == index && row.scope == Scope::Engine)
            .unwrap();
        let screen = render(&mut app, 160, 40);
        assert!(screen.contains("agent+ssh://prod/docker"), "{screen}");
        assert!(!screen.contains(".sock"), "{screen}");
    }

    #[test]
    fn confirming_an_action_names_the_server() {
        use crate::actions::Action;
        let mut app = app();
        app.connect_machine("prod");
        let generation = app.machine("prod").unwrap().generation;
        app.apply_remote_event(remote::Event {
            server: "prod".into(),
            generation,
            update: remote::Update::Connected {
                engines: vec!["docker".into()],
                allow_delete: false,
                capabilities: None,
            },
        });
        let index = app.machine_engines("prod")[0];
        app.update_engine(
            index,
            Ok(crate::model::Snapshot {
                engine_id: Some("id".into()),
                containers: vec![crate::model::Container {
                    id: "abc".into(),
                    name: "api".into(),
                    state: "running".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        app.expanded
            .insert(format!("{index}/{:?}", Scope::Standalone(None)));
        app.refresh_tree();
        let container = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Container(_)))
            .expect("the container row");
        app.selected = container;
        app.request_action(Action::Restart);
        assert!(app.action_confirmation.is_some(), "{:?}", app.notice);
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("On server prod (tha@10.0.0.5)"), "{screen}");
    }

    #[test]
    fn a_server_that_forbids_deleting_refuses_it_before_any_request() {
        use crate::actions::Action;
        let mut app = app();
        app.connect_machine("prod");
        let generation = app.machine("prod").unwrap().generation;
        app.apply_remote_event(remote::Event {
            server: "prod".into(),
            generation,
            update: remote::Update::Connected {
                engines: vec!["docker".into()],
                allow_delete: false,
                capabilities: None,
            },
        });
        let index = app.machine_engines("prod")[0];
        app.update_engine(
            index,
            Ok(crate::model::Snapshot {
                engine_id: Some("id".into()),
                containers: vec![crate::model::Container {
                    id: "abc".into(),
                    name: "api".into(),
                    state: "exited".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        app.expanded
            .insert(format!("{index}/{:?}", Scope::Standalone(None)));
        app.refresh_tree();
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Container(_)))
            .expect("the container row");
        app.request_action(Action::Delete);
        assert!(app.action_confirmation.is_none());
        assert!(app.notice.contains("turned off"), "{}", app.notice);
    }

    /// A container whose only sample is `age` old, as seen between two refreshes.
    fn sampled(app: &mut App, index: usize, age: std::time::Duration) {
        let container = crate::model::Container {
            id: "abc".into(),
            name: "api".into(),
            state: "running".into(),
            ..Default::default()
        };
        app.update_engine(
            index,
            Ok(crate::model::Snapshot {
                engine_id: Some(format!("id-{index}")),
                containers: vec![container.clone()],
                ..Default::default()
            }),
        );
        let batch = vec![(
            container.key(),
            Ok(crate::metrics::Reading {
                usage: crate::metrics::Usage {
                    cpu_percent: Some(12.5),
                    memory_bytes: Some(1024 * 1024),
                    ..Default::default()
                },
                counters: None,
                sampled_at: std::time::Instant::now() - age,
            }),
        )];
        let engine = &mut app.engines[index];
        let containers = engine.snapshot.as_ref().unwrap().containers.clone();
        engine
            .metrics
            .apply(&containers, batch.into_iter().collect());
        app.expanded
            .insert(format!("{index}/{:?}", Scope::Standalone(None)));
        app.refresh_tree();
        app.selected = app
            .rows
            .iter()
            .position(|row| row.engine == index && matches!(row.scope, Scope::Container(_)))
            .expect("the container row");
    }

    fn connected(app: &mut App) -> usize {
        app.connect_machine("prod");
        let generation = app.machine("prod").unwrap().generation;
        app.apply_remote_event(remote::Event {
            server: "prod".into(),
            generation,
            update: remote::Update::Connected {
                engines: vec!["docker".into()],
                allow_delete: false,
                capabilities: None,
            },
        });
        app.machine_engines("prod")[0]
    }

    #[test]
    fn a_server_sample_between_refreshes_never_shows_as_stale() {
        let mut app = app();
        let index = connected(&mut app);
        // Normal for a server's 5 second refresh; never flash stale between samples.
        sampled(&mut app, index, std::time::Duration::from_secs(5));
        let screen = render(&mut app, 160, 40);
        assert!(!screen.contains("stale"), "{screen}");
        assert!(!screen.contains("Last sample"), "{screen}");
        assert!(screen.contains("12.5%"), "{screen}");
    }

    #[test]
    fn server_samples_remain_fresh_for_thirty_seconds() {
        let mut app = app();
        let index = connected(&mut app);
        sampled(&mut app, index, std::time::Duration::from_secs(29));
        let screen = render(&mut app, 160, 40);
        assert!(screen.contains("12.5%"), "{screen}");
        assert!(!screen.contains("stale"), "{screen}");
        assert!(!screen.contains("Last sample"), "{screen}");
    }

    #[test]
    fn local_samples_remain_fresh_for_thirty_seconds() {
        let mut app = app();
        sampled(&mut app, 0, std::time::Duration::from_secs(29));
        let screen = render(&mut app, 160, 40);
        assert!(screen.contains("12.5%"), "{screen}");
        assert!(!screen.contains("stale"), "{screen}");
        assert!(!screen.contains("Last sample"), "{screen}");
    }

    #[test]
    fn a_server_sample_older_than_thirty_seconds_is_stale() {
        let mut app = app();
        let index = connected(&mut app);
        sampled(&mut app, index, std::time::Duration::from_secs(31));
        let screen = render(&mut app, 160, 40);
        assert!(screen.contains("stale"), "{screen}");
        assert!(screen.contains("Last sample"), "{screen}");
    }

    #[test]
    fn a_local_sample_older_than_thirty_seconds_is_stale() {
        let mut app = app();
        sampled(&mut app, 0, std::time::Duration::from_secs(31));
        let screen = render(&mut app, 160, 40);
        assert!(screen.contains("stale"), "{screen}");
        assert!(screen.contains("Last sample"), "{screen}");
    }

    #[test]
    fn the_machine_summary_shows_state_address_and_next_step() {
        let mut app = app();
        app.selected = app
            .rows
            .iter()
            .position(|row| row.scope == Scope::Machine("prod".into()))
            .unwrap();
        let screen = render(&mut app, 160, 40);
        assert!(screen.contains("tha@10.0.0.5"), "{screen}");
        assert!(screen.contains("Enter connects"), "{screen}");
    }
}
