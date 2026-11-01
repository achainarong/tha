//! Offline synthetic workloads. Run `cargo bench --locked --offline --bench performance`.
//! Timings include the named operation, not fixture construction. Frame measurements
//! use TestBackend: they exclude terminal transport, presentation and input queueing.
use ratatui::{Terminal, backend::TestBackend};
use std::{hint::black_box, time::Instant};
use tha::{
    app::{App, Tab},
    config::{BackendKind, Connection},
    logs::{LogBuffer, view::LogView},
    model::{Container, Resource, Snapshot},
};

fn measure(name: &str, iterations: usize, mut action: impl FnMut()) {
    for _ in 0..3 {
        action();
    }
    let mut samples = Vec::new();
    for _ in 0..21 {
        let started = Instant::now();
        for _ in 0..iterations {
            action();
        }
        samples.push(started.elapsed().as_secs_f64() * 1e6 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{name}: median_us={:.3} batch_p95_us={:.3} min_us={:.3} samples=21 iterations={iterations}",
        samples[10], samples[19], samples[0]
    );
}

fn check_search(view: &LogView, buffer: &LogBuffer) {
    let expected: usize = buffer
        .entries
        .iter()
        .filter(|entry| view.filter.matches(entry))
        .map(|entry| {
            view.regex().map_or(0, |regex| {
                regex.find_iter(&entry.source).count() + regex.find_iter(&entry.text).count()
            })
        })
        .sum();
    assert_eq!(view.match_count, expected);
    for row in view.visible(buffer) {
        let entry = tha::logs::view::entry(buffer, row.sequence).unwrap();
        assert!(entry.text.get(row.range).is_some());
    }
}

fn logs() {
    for (capacity, length) in [(500, 100), (5000, 100), (5000, 4000)] {
        let text = "INFO needle abcdefghijklmnopqrstuvwxyz ".repeat(length / 38 + 1);
        let mut buffer = LogBuffer::new(capacity);
        for _ in 0..capacity {
            buffer.push("api", &text, false);
        }
        measure(&format!("logs/{capacity}/{length}/cold_layout"), 1, || {
            let mut view = LogView::new(true);
            view.sync(black_box(&buffer), 60, 30);
            black_box(view);
        });
        let mut view = LogView::new(true);
        view.sync(&buffer, 60, 30);
        measure(
            &format!("logs/{capacity}/{length}/unchanged_sync"),
            100,
            || view.sync(black_box(&buffer), 60, 30),
        );
        measure(
            &format!("logs/{capacity}/{length}/visible_follow"),
            100,
            || {
                black_box(view.visible(black_box(&buffer)));
            },
        );
        measure(
            &format!("logs/{capacity}/{length}/push_and_sync"),
            20,
            || {
                buffer.push("api", &text, false);
                view.sync(&buffer, 60, 30);
            },
        );
        view.set_query("needle".into(), &mut buffer);
        measure(
            &format!("logs/{capacity}/{length}/search_push_and_sync"),
            10,
            || {
                buffer.push("api", &text, false);
                view.sync(&buffer, 60, 30);
            },
        );
        check_search(&view, &buffer);
        // Nothing matches, so nothing has a layout: idle frames must still not rescan.
        let mut filtered = LogView::new(true);
        filtered.filter.set_text("needle absent");
        measure(
            &format!("logs/{capacity}/{length}/text_filter_cold"),
            1,
            || {
                let mut view = LogView::new(true);
                view.filter.set_text("needle absent");
                view.sync(black_box(&buffer), 60, 30);
                black_box(view);
            },
        );
        filtered.sync(&buffer, 60, 30);
        measure(
            &format!("logs/{capacity}/{length}/text_filter_idle_sync"),
            100,
            || filtered.sync(black_box(&buffer), 60, 30),
        );
        measure(
            &format!("logs/{capacity}/{length}/text_filter_push_and_sync"),
            20,
            || {
                buffer.push("api", &text, false);
                filtered.sync(&buffer, 60, 30);
            },
        );
        let mut app = App::new(tha::demo::connections(), capacity, false, true);
        tha::demo::populate(&mut app);
        app.logs = buffer;
        app.log_view = view;
        app.logs.follow = true;
        app.tab = Tab::Logs;
        let mut terminal = Terminal::new(TestBackend::new(160, 50)).unwrap();
        measure(
            &format!("logs/{capacity}/{length}/full_frame_search"),
            10,
            || {
                terminal
                    .draw(|frame| tha::ui::draw(frame, &mut app))
                    .unwrap();
            },
        );
        if capacity == 5000 {
            for batch in [1, 32, 512] {
                measure(
                    &format!("logs/{capacity}/{length}/stream_frame/{batch}_entries"),
                    1,
                    || {
                        for _ in 0..batch {
                            app.logs.push("api", &text, false);
                        }
                        terminal
                            .draw(|frame| tha::ui::draw(frame, &mut app))
                            .unwrap();
                    },
                );
                check_search(&app.log_view, &app.logs);
            }
        }
    }
}

fn tree() {
    for count in [100, 1000, 5000] {
        for projects in [1, count / 5] {
            let mut app = App::new(
                vec![Connection {
                    name: "fixture".into(),
                    kind: BackendKind::Docker,
                    endpoint: "demo://fixture".into(),
                }],
                5000,
                false,
                true,
            );
            let containers = (0..count)
                .map(|i| Container {
                    id: format!("id-{i}"),
                    name: format!("container-{i}"),
                    state: "running".into(),
                    project: Some(format!("project-{}", i % projects)),
                    service: Some(format!("service-{i}")),
                    ..Default::default()
                })
                .collect();
            app.update_engine(
                0,
                Ok(Snapshot {
                    containers,
                    ..Default::default()
                }),
            );
            measure(
                &format!("tree/{count}/{projects}_projects/refresh"),
                3,
                || {
                    app.refresh_tree();
                    black_box(&app.rows);
                },
            );
            app.tree_query = "service-".into();
            measure(
                &format!("tree/{count}/{projects}_projects/search_all"),
                3,
                || {
                    app.refresh_tree();
                    black_box(&app.rows);
                },
            );
            assert_eq!(
                app.rows
                    .iter()
                    .filter(|row| matches!(row.scope, tha::app::Scope::Container(_)))
                    .count(),
                count
            );
            app.tree_query = "no-such-name".into();
            measure(
                &format!("tree/{count}/{projects}_projects/search_none"),
                3,
                || {
                    app.refresh_tree();
                    black_box(&app.rows);
                },
            );
            assert!(app.rows.is_empty());
        }
        // Resource grouping only costs something when each project owns resources.
        let projects = count / 5;
        let mut app = App::new(
            vec![Connection {
                name: "fixture".into(),
                kind: BackendKind::Docker,
                endpoint: "demo://fixture".into(),
            }],
            5000,
            false,
            true,
        );
        let containers = (0..count)
            .map(|i| Container {
                id: format!("id-{i}"),
                name: format!("container-{i}"),
                state: "running".into(),
                project: Some(format!("project-{}", i % projects)),
                service: Some(format!("service-{i}")),
                networks: vec![format!("network-{}", i % projects), "shared".into()],
                volumes: vec![format!("volume-{}", i % projects)],
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let resources = |prefix: &str| {
            (0..projects)
                .map(|project| Resource {
                    name: format!("{prefix}-{project}"),
                    project: Some(format!("project-{project}")),
                    containers: (0..count)
                        .filter(|i| i % projects == project)
                        .map(|i| format!("container-{i}"))
                        .collect(),
                    ..Default::default()
                })
                .collect::<Vec<_>>()
        };
        let mut networks = resources("network");
        networks.push(Resource {
            name: "shared".into(),
            external: true,
            containers: containers.iter().map(|c| c.name.clone()).collect(),
            ..Default::default()
        });
        let volumes = resources("volume");
        app.update_engine(
            0,
            Ok(Snapshot {
                containers,
                networks,
                volumes,
                volumes_available: true,
                ..Default::default()
            }),
        );
        measure(&format!("tree/{count}/resources/refresh"), 3, || {
            app.refresh_tree();
            black_box(&app.rows);
        });
    }
}

fn main() {
    // `cargo test --all-targets` also executes harness-free targets. Benchmarks
    // must be explicit, not a wall-clock test gate on noisy shared runners.
    if std::env::args().any(|argument| argument == "--bench") {
        logs();
        tree();
    }
}
