use super::*;

#[test]
fn timestamp_axis_uses_elapsed_time_and_keeps_missing_samples_absent() {
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let points = vec![
        metrics::TimedUsage {
            sampled_at: start,
            cpu: Some(10),
            memory: Some(100),
        },
        metrics::TimedUsage {
            sampled_at: start + Duration::from_secs(2),
            cpu: None,
            memory: None,
        },
        metrics::TimedUsage {
            sampled_at: start + Duration::from_secs(10),
            cpu: Some(20),
            memory: Some(200),
        },
    ];
    let (cpu, memory) = history_columns(&points, 11, Duration::from_secs(10));
    assert_eq!(cpu[0], Some(10));
    assert_eq!(cpu[2], None);
    assert_eq!(cpu[10], Some(20));
    assert_eq!(memory[10], Some(200));
    assert!(cpu[3..10].iter().all(Option::is_none));
    let axis = history_axis(60, Duration::from_secs(120));
    assert!(axis.starts_with("−120s"));
    assert!(axis.contains("−60s"));
    assert!(axis.ends_with("latest"));
    assert_eq!(history_axis(2, Duration::from_secs(60)).chars().count(), 2);
}

#[test]
fn inner_tabs_zoom_to_full_width_and_survive_resize() {
    let mut app = App::new(crate::demo::connections(), 100, true, true);
    crate::demo::populate(&mut app);
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(row.scope, Scope::Container(_)))
        .unwrap();
    app.detail_focus = true;
    app.performance_open = true;
    app.performance_expanded = true;
    for tab in [
        Tab::Overview,
        Tab::Logs,
        Tab::Networks,
        Tab::Exec,
        Tab::Layers,
    ] {
        app.tab = tab;
        app.detail_zoom = Some(tab);
        for (width, height) in [(140, 40), (60, 20), (32, 10), (140, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let row: String = (0..width).map(|x| buffer[(x, 2)].symbol()).collect();
            let label = match tab {
                Tab::Overview => "1 Overview",
                Tab::Logs => "2 Logs",
                Tab::Networks => "3 Networks",
                Tab::Exec => "4 Exec",
                Tab::Layers => "7 Layers",
                _ => unreachable!(),
            };
            assert!(row.contains(label), "{tab:?}: {row}");
            assert_eq!(buffer[(width - 1, 2)].symbol(), "┓");
        }
    }
}

#[test]
fn full_width_performance_and_container_empty_state_survive_resize() {
    let mut app = App::new(crate::demo::connections(), 100, true, true);
    crate::demo::populate(&mut app);
    app.switch_performance_scope();
    app.performance_expanded = true;
    for (width, height) in [(140, 40), (60, 20), (32, 10), (140, 40)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        if width == 140 {
            let buffer = terminal.backend().buffer();
            let header = (0..height)
                .find(|&y| {
                    let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
                    row.contains("Performance · Selected container")
                })
                .unwrap();
            assert_eq!(buffer[(width - 1, header)].symbol(), "┓");
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("Select a container"));
        }
    }
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(row.scope, Scope::Container(_)))
        .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Container ·"));
    assert!(text.contains("latest"));
    assert!(text.contains("memory limit"));
}

#[test]
fn expanded_performance_has_multirow_graphs_and_idle_frames_are_stable() {
    let mut app = App::new(crate::demo::connections(), 100, true, true);
    crate::demo::populate(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(60, 13)).unwrap();
    terminal
        .draw(|frame| draw_performance_history(frame, &app, 1, frame.area()))
        .unwrap();
    let before = terminal.backend().buffer().clone();
    let rows: Vec<String> = (0..13)
        .map(|y| (0..60).map(|x| before[(x, y)].symbol()).collect())
        .collect();
    assert!(rows.iter().any(|row| row.contains("Memory")));
    assert!(rows.iter().any(|row| row.contains("runtime capacity")));
    assert!(
        rows.iter()
            .filter(|row| row.chars().any(|ch| "▁▂▃▄▅▆▇█".contains(ch)))
            .count()
            >= 3
    );
    terminal
        .draw(|frame| draw_performance_history(frame, &app, 1, frame.area()))
        .unwrap();
    assert_eq!(
        &before,
        terminal.backend().buffer(),
        "Rendering alone must not animate fake samples"
    );
}

#[test]
fn layers_tab_is_hidden_and_dismissed_for_non_workload_selections() {
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(140, 35)).unwrap();
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(row.scope, Scope::Container(_)))
        .unwrap();
    assert!(app.layers_available());
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("7 Layers"));
    let unsupported: Vec<_> = app
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| !matches!(row.scope, Scope::Container(_) | Scope::Service { .. }))
        .map(|(index, _)| index)
        .collect();
    assert!(!unsupported.is_empty());
    for index in unsupported {
        app.selected = index;
        app.tab = Tab::Layers;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.tab, Tab::Overview);
        assert!(app.layers.key.is_none());
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.contains("7 Layers"));
    }
}

#[test]
fn performance_shrinks_to_fit_connected_runtimes() {
    let mut app = App::new(crate::demo::connections(), 100, true, true);
    crate::demo::populate(&mut app);
    app.performance_open = true;
    assert_eq!(performance_height(&app, 40), 16);
    app.update_engine(0, Err("stopped".into()));
    app.update_engine(2, Err("stopped".into()));
    assert_eq!(performance_height(&app, 40), 14);
    app.update_engine(1, Err("stopped".into()));
    assert_eq!(performance_height(&app, 40), 6);
    app.performance_open = false;
    assert_eq!(performance_height(&app, 40), 1);
}

#[test]
fn activity_and_error_notices_have_separate_footer_rows() {
    let mut app = App::new(
        vec![crate::config::Connection {
            name: "fixture".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///fixture.sock".into(),
        }],
        100,
        true,
        false,
    );
    app.engines[0].runtime = Some(crate::runtime::Target::Colima {
        profile: "fixture".into(),
    });
    app.request_action(crate::actions::Action::Start);
    app.action_review_visible = true;
    app.confirm_action();
    app.notice = "Permission denied: this notice must not compete with activity".into();
    assert_eq!(footer_height(&app), 3);
    let mut terminal = Terminal::new(TestBackend::new(100, 3)).unwrap();
    terminal
        .draw(|frame| draw_footer(frame, &app, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let activity: String = (0..100).map(|x| buffer[(x, 0)].symbol()).collect();
    let notice: String = (0..100).map(|x| buffer[(x, 2)].symbol()).collect();
    assert!(activity.contains("Activity ·"));
    assert!(activity.contains("Starting…"));
    assert!(!activity.contains("Permission denied"));
    assert!(notice.contains(&app.notice));
    assert!(!notice.contains("Starting…"));
    assert!(notice.ends_with(concat!("v", env!("CARGO_PKG_VERSION"))));
}

#[test]
fn footer_version_is_bottom_right_without_overwriting_status_text() {
    let mut app = App::new(crate::demo::connections(), 100, true, true);
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    for width in [32, 100] {
        for mode in [
            InputMode::Normal,
            InputMode::Search,
            InputMode::InventorySearch,
        ] {
            app.input_mode = mode;
            app.notice = "Status: ".to_owned() + &"x".repeat(200);
            let mut terminal = Terminal::new(TestBackend::new(width, 2)).unwrap();
            terminal
                .draw(|frame| draw_footer(frame, &app, frame.area()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let last: String = (0..width).map(|x| buffer[(x, 1)].symbol()).collect();
            assert!(last.ends_with(version));
            if width == 32 || mode == InputMode::Normal {
                assert!(last.contains('…'));
            }
            assert_eq!(buffer[(width - version.len() as u16 - 1, 1)].symbol(), " ");
            if mode == InputMode::Normal {
                assert!(last.starts_with("Status: "));
            }
        }
    }
}

#[test]
fn footer_version_handles_offsets_and_tiny_areas() {
    let app = App::new(crate::demo::connections(), 100, true, true);
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    let mut terminal = Terminal::new(TestBackend::new(120, 6)).unwrap();
    let area = Rect::new(3, 2, 100, 2);
    terminal
        .draw(|frame| draw_footer(frame, &app, area))
        .unwrap();
    let last: String = (area.x..area.right())
        .map(|x| terminal.backend().buffer()[(x, area.bottom() - 1)].symbol())
        .collect();
    assert!(last.ends_with(version));
    for width in [0, 1, version.len() as u16] {
        terminal
            .draw(|frame| draw_footer(frame, &app, Rect::new(0, 0, width, 2)))
            .unwrap();
    }
    terminal
        .draw(|frame| draw_footer(frame, &app, Rect::new(0, 0, 100, 0)))
        .unwrap();
}

#[test]
fn pending_actions_stay_visible_on_long_explorer_labels_without_displacing_metrics() {
    let mut app = App::new(
        vec![crate::config::Connection {
            name: "a-very-long-runtime-name-".repeat(5),
            kind: BackendKind::Docker,
            endpoint: "unix:///fixture.sock".into(),
        }],
        100,
        true,
        false,
    );
    app.engines[0].runtime = Some(crate::runtime::Target::Colima {
        profile: "fixture".into(),
    });
    app.update_engine(0, Ok(crate::model::Snapshot::default()));
    app.request_action(crate::actions::Action::Stop);
    app.action_review_visible = true;
    app.confirm_action();
    let mut terminal = Terminal::new(TestBackend::new(90, 16)).unwrap();
    terminal
        .draw(|frame| draw_tree(frame, &mut app, frame.area()))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Stopping…"));
    assert!(text.chars().any(|ch| "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(ch)));
    assert!(text.contains("0.0%"));
    assert!(text.contains("0 B"));
    assert!(!app.action_results_visible);
    assert!(app.engines[0].error.is_none());
}

#[test]
fn container_overview_shows_host_ports_and_does_not_claim_reachability() {
    let mut app = App::new(
        vec![crate::config::Connection {
            name: "fixture".into(),
            kind: crate::config::BackendKind::Docker,
            endpoint: "unix:///fixture.sock".into(),
        }],
        100,
        false,
        true,
    );
    app.update_engine(
        0,
        Ok(crate::model::Snapshot {
            containers: vec![crate::model::Container {
                id: "fixture".into(),
                name: "api".into(),
                state: "running".into(),
                port_bindings: vec![
                    crate::model::PortBinding {
                        host_ip: Some("127.0.0.1".into()),
                        host_port: Some(8080),
                        container_port: 80,
                        protocol: "tcp".into(),
                    },
                    crate::model::PortBinding {
                        host_ip: Some("0.0.0.0".into()),
                        host_port: Some(5353),
                        container_port: 53,
                        protocol: "udp".into(),
                    },
                    crate::model::PortBinding {
                        host_ip: None,
                        host_port: None,
                        container_port: 443,
                        protocol: "tcp".into(),
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }),
    );
    let text: String = overview_lines(&app)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(text.contains("localhost:8080 → 80/tcp (loopback only"));
    assert!(text.contains("0.0.0.0:5353 → 53/udp (all interfaces"));
    assert!(text.contains("443/tcp — exposed, no host port"));
    assert!(!text.contains("reachability not verified"));
    app.engines[0].snapshot.as_mut().unwrap().containers[0].state = "exited".into();
    let text: String = overview_lines(&app)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(text.contains("Container is stopped; published ports are not active."));
    app.update_engine(0, Err("fixture offline".into()));
    let text: String = overview_lines(&app)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(text.contains("Last-known bindings (offline)"));
}

#[test]
fn runtime_confirmation_warns_about_all_workloads_and_returns_to_navigation() {
    use crate::{actions::Action, runtime::Target};
    let mut app = App::new(
        vec![crate::config::Connection {
            name: "fixture".into(),
            kind: crate::config::BackendKind::Docker,
            endpoint: "unix:///fixture.sock".into(),
        }],
        100,
        false,
        false,
    );
    app.engines[0].runtime = Some(Target::Colima {
        profile: "fixture".into(),
    });
    app.update_engine(0, Ok(crate::model::Snapshot::default()));
    app.request_action(Action::Stop);
    let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
    terminal
        .draw(|frame| draw_action(frame, &mut app, frame.area()))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Stop Colima profile fixture?"));
    assert!(text.contains("all workloads"));
    assert!(text.contains("y / Enter Confirm    n / Esc Cancel"));
    assert!(!text.contains("colima --profile fixture stop"));
    assert!(!text.contains("Endpoint:"));
    assert!(!text.contains("Existing containers only"));
    let occupied: Vec<_> = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .enumerate()
        .filter(|(_, cell)| !cell.symbol().trim().is_empty())
        .map(|(index, _)| (index % 120, index / 120))
        .collect();
    assert!(
        occupied.iter().map(|(x, _)| x).max().unwrap()
            - occupied.iter().map(|(x, _)| x).min().unwrap()
            < 56
    );
    assert!(
        occupied.iter().map(|(_, y)| y).max().unwrap()
            - occupied.iter().map(|(_, y)| y).min().unwrap()
            < 10
    );
    app.confirm_action();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(!app.action_results_visible);
    app.action_results_visible = true;
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Pending / checking runtime"));
}
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
        for span in highlighted(text, range, Some(&regex), Some(&active), Style::default()).spans {
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
    app.help_scroll = 18;
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
fn performance_hides_offline_and_alias_contexts_and_recovers_from_stale_scroll() {
    let mut app = App::new(crate::demo::connections(), 100, true, true);
    crate::demo::populate(&mut app);
    app.update_engine(0, Err("stopped".into()));
    app.engines[2].alias = Some("Colima / development".into());
    app.performance_open = true;
    app.performance_focus = true;
    app.performance_scroll = usize::MAX;
    app.performance_offset = usize::MAX;
    let mut terminal = Terminal::new(TestBackend::new(90, 35)).unwrap();
    terminal
        .draw(|frame| draw_sidebar(frame, &mut app, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let performance_start = 35 - performance_height(&app, 35);
    let text: String = (performance_start..35)
        .flat_map(|y| (0..90).map(move |x| buffer[(x, y)].symbol()))
        .collect();
    assert!(text.contains("Performance · Connected runtimes"));
    assert!(text.contains("Colima / development · CPU 59.0%"));
    assert!(!text.contains("OrbStack"));
    assert!(!text.contains("containerd"));
    assert!(!text.contains("Unavailable"));
    assert_eq!(app.performance_scroll, 0);
    assert_eq!(app.performance_offset, 0);
    let header_y = (performance_start..35)
        .find(|&y| {
            let line: String = (0..90).map(|x| buffer[(x, y)].symbol()).collect();
            line.contains("Colima / development · CPU")
        })
        .unwrap();
    assert!((0..90).any(|x| buffer[(x, header_y)].modifier.contains(Modifier::REVERSED)));

    app.update_engine(1, Err("stopped".into()));
    app.update_engine(2, Err("stopped".into()));
    terminal
        .draw(|frame| draw_sidebar(frame, &mut app, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let performance_start = 35 - performance_height(&app, 35);
    let text: String = (performance_start..35)
        .flat_map(|y| (0..90).map(move |x| buffer[(x, y)].symbol()))
        .collect();
    assert!(text.contains("No connected runtimes"));
    assert!(app.rows.iter().any(|row| row.title.contains("(offline)")));
}

#[test]
fn performance_viewport_keeps_the_last_selection_visible_without_blank_tail_space() {
    let mut app = App::new(
        (0..5)
            .map(|index| crate::config::Connection {
                name: format!("Runtime{index}"),
                kind: BackendKind::Docker,
                endpoint: format!("unix:///runtime{index}.sock"),
            })
            .collect(),
        100,
        true,
        true,
    );
    for index in 0..5 {
        app.update_engine(
            index,
            Ok(crate::model::Snapshot {
                engine_id: Some(format!("engine{index}")),
                ..Default::default()
            }),
        );
    }
    app.performance_open = true;
    app.performance_scroll = 4;
    let mut terminal = Terminal::new(TestBackend::new(90, 35)).unwrap();
    terminal
        .draw(|frame| draw_sidebar(frame, &mut app, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let performance_start = 35 - performance_height(&app, 35);
    let text: String = (performance_start..35)
        .flat_map(|y| (0..90).map(move |x| buffer[(x, y)].symbol()))
        .collect();
    for name in ["Runtime2", "Runtime3", "Runtime4"] {
        assert!(text.contains(name));
    }
    assert!(!text.contains("Runtime0"));
    assert!(!text.contains("Runtime1"));
    assert_eq!(app.performance_offset, 2);
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
        .performance_engines()
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
    assert!(text.contains("Colima (offline)"));
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
            project: Some("shop".into()),
            service: Some("api".into()),
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
    assert!(!text.contains("[workload issues]"));
    assert!(!text.contains("[errors]"));
    assert!(text.contains("shop [1/1 running]"));
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
        "Stop shop?",
        "Runtime: OrbStack",
        "3 target(s)",
        "shop-api-1",
        "y / Enter Confirm",
        "n / Esc Cancel",
    ] {
        assert!(text.contains(expected), "{expected}");
    }
    let mut tiny = Terminal::new(TestBackend::new(35, 8)).unwrap();
    tiny.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(!app.action_review_visible);
    app.confirm_action();
    assert!(app.action_request.is_none());
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    app.confirm_action();
    assert!(app.action_request.is_some());
    app.action_progress_mut().unwrap().finished = true;
    app.action_progress_mut().unwrap().results[0] =
        Some(crate::actions::Status::Unknown("lost response".into()));
    app.action_results_visible = true;
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
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(row.scope, Scope::Container(_)))
        .unwrap();
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
    assert!(text.contains("7 Layers"));
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

fn wrap(text: &str, width: usize) -> Vec<String> {
    hanging_wrap(vec![Line::from(text.to_owned())], width)
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn wrapped_label_rows_continue_under_their_value() {
    let rows = wrap("  Command    one two three four five six seven", 28);
    assert_eq!(rows[0], "  Command    one two three ");
    assert!(rows[1..].iter().all(|row| row.starts_with(&" ".repeat(13))));
    assert!(
        rows.iter()
            .all(|row| UnicodeWidthStr::width(row.as_str()) <= 28)
    );
    let joined: String = rows.iter().map(|row| row.trim_start()).collect();
    assert_eq!(joined, "Command    one two three four five six seven");
}

#[test]
fn json_wraps_after_commas_and_a_single_long_token_is_still_kept_whole() {
    let rows = wrap(r#"  Command    {"level":"info","duration_ms":12}"#, 34);
    assert!(rows[0].ends_with(r#""info","#), "{rows:?}");
    assert!(
        rows[1].trim_start().starts_with(r#""duration_ms""#),
        "{rows:?}"
    );
    let rows = wrap(&format!("  Command    {}", "x".repeat(50)), 30);
    let joined: String = rows.iter().map(|row| row.trim_start()).collect();
    assert!(joined.ends_with(&"x".repeat(50)), "{rows:?}");
}

#[test]
fn hanging_wrap_leaves_short_unindented_and_too_narrow_lines_alone_and_keeps_style() {
    assert_eq!(wrap("  Image      nginx", 80), ["  Image      nginx"]);
    let plain = "no indent ".repeat(10);
    assert_eq!(wrap(&plain, 20), std::slice::from_ref(&plain));
    let narrow = "  Command    one two three four";
    assert_eq!(wrap(narrow, 20), [narrow]);
    assert_eq!(
        wrap("    test: aaaa bbbb cccc dddd", 20)[1],
        "      cccc dddd"
    );
    let styled = Line::styled(
        "  Output     red words that are long enough to wrap",
        Style::default().fg(Color::Red),
    );
    let rows = hanging_wrap(vec![styled], 30);
    assert!(rows.len() > 1);
    assert!(rows.iter().all(|row| {
        row.style.fg == Some(Color::Red)
            || row
                .spans
                .iter()
                .all(|span| span.style.fg == Some(Color::Red))
    }));
}

#[test]
fn variable_length_container_details_come_after_the_fixed_ones() {
    let mut app = App::new(crate::demo::connections(), 100, true, true);
    crate::demo::populate(&mut app);
    app.demo = true;
    loop {
        let before = app.expanded.len();
        let branches: Vec<_> = app
            .rows
            .iter()
            .filter(|row| row.branch)
            .map(|row| row.key.clone())
            .collect();
        app.expanded.extend(branches);
        app.refresh_tree();
        if app.expanded.len() == before {
            break;
        }
    }
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(&row.scope, Scope::Container(found) if found == "/api-2"))
        .unwrap();
    app.tab = Tab::Overview;
    app.update_overview_details();
    let mut terminal = Terminal::new(TestBackend::new(170, 50)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle}"));
    for fixed in ["Health     unhealthy", "Image  ", "Port  ", "Network  "] {
        assert!(at(fixed) < at("Command  "), "{fixed}");
    }
    assert!(at("Command  ") < at("Healthcheck ("));
    assert!(at("Healthcheck (") < at("Last check"));
}

#[test]
fn container_overview_shows_health_only_when_a_status_is_reported() {
    let mut app = App::new(crate::demo::connections(), 100, true, true);
    crate::demo::populate(&mut app);
    loop {
        let before = app.expanded.len();
        let branches: Vec<_> = app
            .rows
            .iter()
            .filter(|row| row.branch)
            .map(|row| row.key.clone())
            .collect();
        app.expanded.extend(branches);
        app.refresh_tree();
        if app.expanded.len() == before {
            break;
        }
    }
    app.tab = Tab::Overview;
    for (key, expected) in [
        ("/api-2", Some("Health     unhealthy")),
        ("/db-1", Some("Health     healthy")),
        ("/scratch", None),
    ] {
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(&row.scope, Scope::Container(found) if found == key))
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        match expected {
            Some(expected) => assert!(text.contains(expected), "{key}: {expected}"),
            None => assert!(!text.contains("Health  "), "{key} has no health status"),
        }
    }
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
    app.input_mode = InputMode::LogTextFilter;
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("every word"));
    assert!(text.contains("Tab Advanced JSON"));
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
    assert!(text.contains("Filter: off"));
    assert!(text.contains("0600"));
    assert!(text.contains("existing files/symlinks refused"));
    let mut tiny = Terminal::new(TestBackend::new(50, 15)).unwrap();
    tiny.draw(|frame| draw(frame, &mut app)).unwrap();
    assert!(!app.log_export_review_visible);
}
#[test]
fn ellipsis_uses_display_cells_and_preserves_styles_and_whole_graphemes() {
    assert_eq!(ellipsize_line(Line::from("exact"), 5).to_string(), "exact");
    assert_eq!(ellipsize_line(Line::from("overflow"), 0).to_string(), "");
    assert_eq!(ellipsize_line(Line::from("overflow"), 1).to_string(), "…");
    assert_eq!(ellipsize_line(Line::from("abcdef"), 5).to_string(), "abcd…");
    assert_eq!(ellipsize_line(Line::from("界界界"), 4).to_string(), "界…");
    assert_eq!(
        ellipsize_line(Line::from("e\u{301}e\u{301}e\u{301}"), 2).to_string(),
        "e\u{301}…"
    );
    assert_eq!(
        ellipsize_line(Line::from("👩‍💻abcdef"), 4).to_string(),
        "👩‍💻a…"
    );
    let line = Line::from(vec![
        Span::styled("● ", Style::default().fg(Color::Red)),
        Span::raw("a long workload"),
    ]);
    let clipped = ellipsize_line(line, 8);
    assert_eq!(clipped.to_string(), "● a lon…");
    assert_eq!(clipped.spans[0].style.fg, Some(Color::Red));
    assert_eq!(clipped.spans[1].style.fg, None);
    assert!(clipped.width() <= 8);
}

#[test]
fn explorer_marks_truncated_labels_without_changing_metrics_or_full_overview_names() {
    let name = "a-very-long-runtime-name-".repeat(5);
    let mut app = App::new(
        vec![crate::config::Connection {
            name: name.clone(),
            kind: BackendKind::Docker,
            endpoint: "unix:///fixture.sock".into(),
        }],
        100,
        false,
        true,
    );
    app.update_engine(0, Ok(crate::model::Snapshot::default()));
    for width in [35, 52, 78] {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal
            .draw(|frame| draw_tree(frame, &mut app, frame.area()))
            .unwrap();
        let inner = padded(Block::bordered().inner(Rect::new(0, 0, width, 12)));
        let metrics = inner.width >= 44;
        let label_width = if metrics {
            inner.width - 22
        } else {
            inner.width
        };
        let y = inner.y + u16::from(metrics);
        let buffer = terminal.backend().buffer();
        let label: String = (inner.x..inner.x + label_width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        assert!(label.ends_with('…'), "width {width}: {label:?}");
        if metrics {
            let values: String = (inner.x + label_width..inner.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            assert!(values.contains("0.0%"), "{values:?}");
            assert!(values.contains("0 B"), "{values:?}");
            assert!(!values.contains('…'));
        }
    }
    let overview: String = overview_lines(&app)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(overview.contains(&name));
    assert_eq!(app.engines[0].connection.name, name);
}
