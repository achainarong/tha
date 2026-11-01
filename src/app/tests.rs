use super::*;
use crate::config::BackendKind;

#[test]
fn borrowed_selection_keeps_membership_order_and_respects_project_focus() {
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    app.selected = app
        .rows
        .iter()
        .position(|row| row.engine == 0 && row.scope == Scope::Engine)
        .unwrap();
    assert_eq!(
        app.selected_container_refs()
            .map(|container| container.id.as_str())
            .collect::<Vec<_>>(),
        vec!["api-1", "api-2", "db-1", "scratch"]
    );
    app.project = Some(Project {
        engine: 0,
        namespace: None,
        name: "shop".into(),
    });
    let selected: Vec<_> = app.selected_container_refs().collect();
    assert_eq!(selected.len(), 3);
    assert!(
        selected
            .iter()
            .all(|container| container.project.as_deref() == Some("shop"))
    );
    assert!(selected.iter().all(|container| {
        app.engines[0]
            .snapshot
            .as_ref()
            .unwrap()
            .containers
            .iter()
            .any(|stored| std::ptr::eq(*container, stored))
    }));
    app.engines[0].snapshot = None;
    assert_eq!(app.selected_container_refs().count(), 0);
    app.rows.clear();
    assert_eq!(app.selected_container_refs().count(), 0);
}

#[test]
fn performance_container_navigation_stays_inside_workspace_and_search_when_branches_are_collapsed()
{
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    app.performance_scope = PerformanceScope::Container;
    app.project = Some(Project {
        engine: 0,
        namespace: None,
        name: "shop".into(),
    });
    app.expanded.clear();
    app.refresh_tree();
    assert!(app.performance_container().is_none());
    for amount in [1, isize::MAX, isize::MIN, -1] {
        app.move_performance_selection(amount);
        let (index, container) = app.performance_container().unwrap();
        assert_eq!(index, 0);
        assert_eq!(container.project.as_deref(), Some("shop"));
    }
    app.tree_query = "shop-api-1".into();
    app.refresh_tree();
    for amount in [isize::MAX, isize::MIN, 1] {
        app.move_performance_selection(amount);
        assert_eq!(app.performance_container().unwrap().1.name, "shop-api-1");
    }
    app.performance_scope = PerformanceScope::Runtime;
    app.performance_scroll = usize::MAX;
    app.move_performance_selection(isize::MIN);
    assert_eq!(app.performance_scroll, 0);
}

#[test]
fn performance_container_navigation_handles_group_positions_empty_lists_and_aliases() {
    let mut app = app();
    app.performance_scope = PerformanceScope::Container;
    assert!(
        !app.rows
            .iter()
            .any(|row| matches!(row.scope, Scope::Container(_)))
    );
    app.move_performance_selection(1);
    assert!(
        app.performance_container().is_some(),
        "Collapsed ancestors are revealed automatically"
    );
    app.tree_query = "no-matching-container".into();
    app.refresh_tree();
    let selected = app.selected;
    app.move_performance_selection(1);
    assert_eq!(app.selected, selected);
    assert!(app.notice.contains("No containers"));
    app.tree_query = "api-1".into();
    app.refresh_tree();
    let container = app
        .rows
        .iter()
        .position(|row| matches!(row.scope, Scope::Container(_)))
        .unwrap();
    app.selected = 0;
    app.move_performance_selection(1);
    assert_eq!(app.selected, container);
    app.move_performance_selection(-1);
    assert_eq!(app.selected, container);
    app.move_performance_selection(isize::MAX);
    app.move_performance_selection(isize::MIN);
    assert_eq!(app.selected, container);
    app.engines[0].alias = Some("primary".into());
    app.selected = 0;
    app.move_performance_selection(1);
    assert_eq!(
        app.selected, 0,
        "An alias cannot become the Performance target"
    );
}

#[test]
fn container_performance_tracks_only_the_current_container_and_keeps_zoom_transient() {
    let mut app = app();
    assert_eq!(app.performance_scope, PerformanceScope::Runtime);
    assert!(!app.performance_expanded);
    app.switch_performance_scope();
    assert!(app.performance_open && app.performance_focus);
    assert!(app.performance_container().is_none());
    app.tree_query = "api-1".into();
    app.refresh_tree();
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(row.scope, Scope::Container(_)))
        .unwrap();
    let (index, container) = app.performance_container().unwrap();
    assert_eq!(container.key(), app.selected_containers()[0].key());
    app.engines[index].error = Some("offline".into());
    assert!(
        app.performance_container().is_some(),
        "Offline history remains inspectable"
    );
    app.selected = 0;
    assert!(
        app.performance_container().is_none(),
        "Never fall back to the previous container"
    );
    app.performance_expanded = true;
    app.toggle_performance();
    assert!(!app.performance_open && !app.performance_expanded);
    app.toggle_performance();
    assert!(!app.performance_expanded);
}

#[test]
fn narrow_row_cache_defers_metrics_until_columns_become_visible() {
    let mut app = app();
    let now = std::time::Instant::now();
    let key = app.rows[0].key.clone();
    app.prepare_row_presentations(0, 1, now, false);
    assert!(app.row_presentation(&key).cpu.is_empty());
    let builds = app.presentations.builds;
    app.prepare_row_presentations(0, 1, now, true);
    assert_eq!(app.row_presentation(&key).cpu, "0.0%");
    assert_eq!(app.presentations.builds, builds + 1);
    app.prepare_row_presentations(0, 1, now, false);
    assert_eq!(app.presentations.builds, builds + 1);
}

#[test]
fn row_presentations_reuse_work_and_expire_without_new_metric_events() {
    use crate::metrics::{Event, Reading, Usage};
    use std::time::{Duration, Instant};
    let mut app = app();
    app.demo = false;
    app.tab = Tab::Logs; // No Overview age clock; test the freshness deadline itself.
    app.engines[0].snapshot.as_mut().unwrap().containers[0].state = "running".into();
    let now = Instant::now();
    let reading = |value, at| Reading {
        usage: Usage {
            cpu_percent: Some(value),
            memory_bytes: Some(1024),
            ..Default::default()
        },
        counters: None,
        sampled_at: at,
    };
    app.apply_metrics(Event {
        engine: 0,
        revision: 0,
        batch: [("/one".into(), Ok(reading(50.0, now)))].into(),
    });
    app.prepare_row_presentations(0, 1, now, true);
    let key = app.rows[0].key.clone();
    let builds = app.presentations.builds;
    assert_eq!(app.row_presentation(&key).cpu, "50.0%");
    for _ in 0..50 {
        app.prepare_row_presentations(0, 1, now + Duration::from_secs(29), true);
    }
    assert_eq!(
        app.presentations.builds, builds,
        "Unchanged frames must not rescan workloads"
    );
    assert_eq!(
        app.next_visual_deadline(now),
        Some(now + Duration::from_secs(30) + Duration::from_nanos(1))
    );
    app.prepare_row_presentations(0, 1, now + Duration::from_secs(30), true);
    assert_eq!(app.row_presentation(&key).cpu, "50.0%");
    app.prepare_row_presentations(0, 1, now + Duration::from_secs(31), true);
    assert_eq!(app.row_presentation(&key).cpu, "stale");
    assert_eq!(app.presentations.builds, builds + 1);
    app.apply_metrics(Event {
        engine: 0,
        revision: 0,
        batch: [(
            "/one".into(),
            Ok(reading(25.0, now + Duration::from_secs(31))),
        )]
        .into(),
    });
    app.prepare_row_presentations(0, 1, now + Duration::from_secs(31), true);
    assert_eq!(app.row_presentation(&key).cpu, "25.0%");
    app.update_engine(0, Err("offline".into()));
    app.prepare_row_presentations(0, 1, now + Duration::from_secs(31), true);
    assert_eq!(
        app.row_presentation(&key).condition.unwrap().label,
        "unavailable"
    );
    assert_ne!(app.row_presentation(&key).cpu, "25.0%");
}

#[test]
fn row_cache_invalidates_for_workspace_changes_and_is_lazy_on_large_inventories() {
    use crate::metrics::{Event, Reading, Usage};
    use std::time::Instant;
    let mut app = app();
    let now = Instant::now();
    let containers: Vec<_> = (0..1000)
        .map(|index| Container {
            id: format!("member-{index}"),
            name: format!("member-{index}"),
            state: "running".into(),
            project: Some(if index == 0 { "alpha" } else { "beta" }.into()),
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
    let reading = |cpu| Reading {
        usage: Usage {
            cpu_percent: Some(cpu),
            ..Default::default()
        },
        counters: None,
        sampled_at: now,
    };
    app.apply_metrics(Event {
        engine: 0,
        revision: 0,
        batch: [
            ("/member-0".into(), Ok(reading(10.0))),
            ("/member-1".into(), Ok(reading(20.0))),
        ]
        .into(),
    });
    app.prepare_row_presentations(0, 1, now, true);
    let key = app.rows[0].key.clone();
    let initial = app.row_presentation(&key).cpu.clone();
    let builds = app.presentations.builds;
    for _ in 0..100 {
        app.prepare_row_presentations(0, 1, now, true);
    }
    assert_eq!(app.presentations.builds, builds);
    assert_eq!(
        app.presentations.rows.len(),
        1,
        "Only visible rows should be calculated"
    );
    app.project = Some(Project {
        engine: 0,
        namespace: None,
        name: "alpha".into(),
    });
    app.prepare_row_presentations(0, 1, now, true);
    assert_eq!(app.row_presentation(&key).cpu, "10.0%");
    assert_ne!(initial, app.row_presentation(&key).cpu);
    assert_eq!(app.presentations.builds, builds + 1);
    app.project = None;
    app.prepare_row_presentations(0, 1, now, true);
    assert_eq!(app.row_presentation(&key).cpu, initial);
}

#[test]
fn security_status_ticks_only_while_scanning_and_visible() {
    use std::time::{Duration, Instant};
    let mut app = app();
    let now = Instant::now();
    app.overview_mode = OverviewMode::Security;
    assert!(app.next_visual_deadline(now).is_none());
    app.security.start(2);
    assert_eq!(
        app.next_visual_deadline(now),
        Some(now + Duration::from_secs(1))
    );
    app.overview_mode = OverviewMode::Images;
    assert!(app.next_visual_deadline(now).is_none());
    app.overview_mode = OverviewMode::Security;
    app.security.busy = false;
    assert!(app.next_visual_deadline(now).is_none());
}

#[test]
fn idle_views_have_no_redraw_deadline_but_actions_and_badge_expiry_do() {
    use crate::{
        actions::{Action, Event, Status, Update},
        runtime::Target,
    };
    use std::time::{Duration, Instant};
    let mut app = app();
    assert!(app.next_visual_deadline(Instant::now()).is_none());
    app.demo = false;
    app.engines[0].runtime = Some(Target::Colima {
        profile: "fixture".into(),
    });
    app.selected = 0;
    app.request_action(Action::Start);
    app.action_review_visible = true;
    app.confirm_action();
    let operation = app.action_request.take().unwrap().operation;
    let now = Instant::now();
    assert_eq!(
        app.next_visual_deadline(now),
        Some(now + Duration::from_millis(100))
    );
    app.apply_action_update(Update {
        operation,
        event: Event::Result {
            index: 0,
            status: Status::Succeeded,
        },
    });
    app.apply_action_update(Update {
        operation,
        event: Event::Finished,
    });
    assert!(app.next_visual_deadline(Instant::now()).is_some());
    assert!(
        app.next_visual_deadline(Instant::now() + Duration::from_secs(6))
            .is_none()
    );
}

#[test]
fn action_feedback_animates_frozen_targets_and_tracks_member_completion() {
    use crate::actions::{Action, Event, Status, Update};
    use std::time::{Duration, Instant};
    let mut app = app();
    app.demo = false;
    app.engines[0].snapshot.as_mut().unwrap().engine_id = Some("fixture".into());
    app.engines[0].snapshot.as_mut().unwrap().containers[0].state = "running".into();
    app.tree_query = "api-1".into();
    app.refresh_tree();
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(row.scope, Scope::Project { .. }))
        .unwrap();
    let project = app.current().unwrap().clone();
    let container = app
        .rows
        .iter()
        .find(|row| matches!(row.scope, Scope::Container(_)))
        .unwrap()
        .clone();
    app.request_action(Action::Restart);
    app.selected = 0; // Changing selection must not retarget the feedback.
    app.action_review_visible = true;
    app.confirm_action();
    let operation = app.action_request.take().unwrap().operation;
    let started = app.action_feedback[&operation].started_at;
    let initial = app.row_action_feedback(&project, started).unwrap();
    assert_eq!(initial.label, "Restarting… 0s");
    assert!(initial.active);
    assert_ne!(
        initial.symbol,
        app.row_action_feedback(&project, started + Duration::from_millis(100))
            .unwrap()
            .symbol
    );
    assert_eq!(
        app.row_action_feedback(&container, started + Duration::from_secs(3))
            .unwrap()
            .label,
        "Restarting… 3s"
    );

    let mut refreshed = app.engines[0].snapshot.clone().unwrap();
    let mut late = refreshed.containers[0].clone();
    late.id = "new-replica".into();
    late.name = "new-replica".into();
    refreshed.containers.push(late);
    app.update_engine(0, Ok(refreshed));
    app.tree_query = "new-replica".into();
    app.refresh_tree();
    let late_row = app
        .rows
        .iter()
        .find(|row| matches!(&row.scope, Scope::Container(key) if key == "/new-replica"))
        .unwrap();
    assert!(app.row_action_feedback(late_row, Instant::now()).is_none());
    assert!(
        app.row_action_feedback(&app.rows[0], Instant::now())
            .is_none()
    );

    app.apply_action_update(Update {
        operation,
        event: Event::Result {
            index: 0,
            status: Status::Succeeded,
        },
    });
    assert!(
        !app.row_action_feedback(&container, Instant::now())
            .unwrap()
            .active
    );
    assert!(
        app.row_action_feedback(&project, Instant::now())
            .unwrap()
            .active
    );
    app.apply_action_update(Update {
        operation,
        event: Event::Finished,
    });
    assert_eq!(
        app.row_action_feedback(&project, Instant::now())
            .unwrap()
            .label,
        "Restart completed"
    );
    assert!(
        app.row_action_feedback(&project, Instant::now() + Duration::from_secs(6))
            .is_none()
    );
}

#[test]
fn runtime_feedback_never_claims_readiness_and_errors_do_not_disappear() {
    use crate::{
        actions::{Action, Event, Status, Update},
        runtime::Target,
    };
    use std::time::{Duration, Instant};
    let mut app = App::new(
        vec![Connection {
            name: "fixture".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///fixture.sock".into(),
        }],
        100,
        false,
        false,
    );
    app.engines[0].runtime = Some(Target::Colima {
        profile: "fixture".into(),
    });
    app.update_engine(0, Err("offline".into()));
    let row = app.current().unwrap().clone();
    for (status, expected) in [
        (Status::Succeeded, "Start command completed"),
        (Status::Failed("denied".into()), "Start failed"),
        (Status::Unknown("timeout".into()), "Outcome unknown"),
        (
            Status::Skipped("previous outcome unknown".into()),
            "Not attempted",
        ),
    ] {
        app.request_action(Action::Start);
        app.action_review_visible = true;
        app.confirm_action();
        let operation = app.action_request.take().unwrap().operation;
        assert_eq!(
            app.action_feedback.len(),
            1,
            "Old animation metadata must be evicted with old results"
        );
        assert_eq!(
            app.row_action_feedback(&row, Instant::now()).unwrap().label,
            "Starting… 0s"
        );
        app.apply_action_update(Update {
            operation,
            event: Event::Result {
                index: 0,
                status: status.clone(),
            },
        });
        app.apply_action_update(Update {
            operation,
            event: Event::Finished,
        });
        let feedback = app.row_action_feedback(&row, Instant::now()).unwrap();
        assert_eq!(feedback.label, expected);
        assert!(!feedback.active);
        if status != Status::Succeeded {
            assert!(
                app.row_action_feedback(&row, Instant::now() + Duration::from_secs(3600))
                    .is_some()
            );
        }
        assert!(app.engines[0].error.is_some());
        assert!(app.engines[0].snapshot.is_none());
        assert!(app.current().unwrap().title.contains("(offline)"));
        assert!(!app.action_results_visible);
    }
}

#[test]
fn performance_lists_only_connected_primary_runtimes_including_idle_ones() {
    let mut app = App::new(
        (0..4)
            .map(|index| Connection {
                name: format!("runtime-{index}"),
                kind: BackendKind::Docker,
                endpoint: format!("unix:///runtime-{index}.sock"),
            })
            .collect(),
        100,
        false,
        true,
    );
    app.update_engine(
        0,
        Ok(Snapshot {
            engine_id: Some("offline".into()),
            ..Default::default()
        }),
    );
    app.update_engine(0, Err("stopped".into()));
    for index in [1, 2] {
        app.update_engine(
            index,
            Ok(Snapshot {
                engine_id: Some("shared".into()),
                ..Default::default()
            }),
        );
    }
    assert!(app.engines[2].alias.is_some());
    assert!(app.engines[3].snapshot.is_none());
    assert_eq!(app.performance_engines(), [1]);
    assert_eq!(app.engine_metrics(1).cpu_percent, Some(0.0));
    assert!(
        app.rows
            .iter()
            .any(|row| row.engine == 0 && row.title.contains("(offline)"))
    );
}

#[test]
fn performance_focus_survives_sorting_new_connections_and_availability_changes() {
    let mut app = App::new(
        ["Zeta", "Beta", "Alpha"]
            .into_iter()
            .map(|name| Connection {
                name: name.into(),
                kind: BackendKind::Docker,
                endpoint: format!("unix:///{name}.sock"),
            })
            .collect(),
        100,
        false,
        true,
    );
    let snapshot = |index| Snapshot {
        engine_id: Some(format!("runtime-{index}")),
        ..Default::default()
    };
    app.update_engine(0, Ok(snapshot(0)));
    app.update_engine(1, Ok(snapshot(1)));
    assert_eq!(app.performance_engine(), Some(0));
    app.performance_scroll = app
        .performance_engines()
        .iter()
        .position(|&index| index == 1)
        .unwrap();
    app.update_engine(2, Ok(snapshot(2)));
    assert_eq!(app.performance_engine(), Some(1));
    app.context_sort = crate::context_sort::ContextSort::Name;
    app.context_pins = vec!["Zeta".into()];
    app.refresh_tree();
    assert_eq!(app.performance_engine(), Some(1));
    app.update_engine(0, Err("stopped".into()));
    assert_eq!(app.performance_engine(), Some(1));
    app.update_engine(1, Err("stopped".into()));
    assert_eq!(app.performance_engine(), Some(2));
    app.update_engine(1, Ok(snapshot(1)));
    assert_eq!(app.performance_engine(), Some(2));
    app.update_engine(1, Err("stopped".into()));
    app.update_engine(2, Err("stopped".into()));
    assert!(app.performance_engines().is_empty());
    assert_eq!(app.performance_scroll, 0);
    assert!(app.performance_engine().is_none());
}

#[test]
fn independent_runtimes_run_concurrently_with_isolated_results_and_alias_locks() {
    use crate::{
        actions::{Action, Event, Status, Update},
        runtime::Target,
    };
    let mut app = App::new(
        vec![
            Connection {
                name: "First".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///one.sock".into(),
            },
            Connection {
                name: "Second".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///two.sock".into(),
            },
        ],
        100,
        false,
        false,
    );
    let first_target = Target::Colima {
        profile: "first".into(),
    };
    let second_target = Target::Colima {
        profile: "second".into(),
    };
    app.engines[0].runtime = Some(first_target.clone());
    app.engines[1].runtime = Some(second_target.clone());
    app.update_engine(
        1,
        Ok(Snapshot {
            engine_id: Some("second".into()),
            ..Default::default()
        }),
    );
    app.selected = app
        .rows
        .iter()
        .position(|row| row.engine == 0 && row.scope == Scope::Engine)
        .unwrap();
    app.request_action(Action::Start);
    app.action_review_visible = true;
    app.confirm_action();
    let first = app.action_request.take().unwrap().operation;
    assert!(
        app.plan_action(Action::Start)
            .unwrap_err()
            .to_string()
            .contains("already active")
    );

    app.selected = app
        .rows
        .iter()
        .position(|row| row.engine == 1 && row.scope == Scope::Engine)
        .unwrap();
    app.engines[1].runtime = Some(first_target);
    assert!(
        app.plan_action(Action::Restart)
            .unwrap_err()
            .to_string()
            .contains("already active")
    );
    app.engines[1].runtime = Some(second_target);
    app.request_action(Action::Restart);
    assert!(app.action_confirmation.is_some());
    app.action_review_visible = true;
    app.confirm_action();
    let second = app.action_request.take().unwrap().operation;
    assert_ne!(first, second);
    assert_eq!(app.action_jobs.len(), 2);
    assert!(app.action_status().unwrap().contains("2 running"));

    assert!(app.apply_action_update(Update {
        operation: second,
        event: Event::Result {
            index: 0,
            status: Status::Failed("fixture denied".into())
        }
    }));
    assert!(app.apply_action_update(Update {
        operation: second,
        event: Event::Finished
    }));
    assert!(!app.action_jobs[&first].finished);
    assert!(app.action_jobs[&first].results[0].is_none());
    assert!(matches!(
        app.action_jobs[&second].results[0],
        Some(Status::Failed(_))
    ));
    app.open_action_results();
    assert_eq!(app.picker.as_ref().unwrap().choices.len(), 2);
    app.picker.as_mut().unwrap().selected = 1;
    app.choose();
    assert_eq!(app.action_selected, Some(first));
    assert!(app.action_results_visible);
    app.action_results_visible = false;

    app.apply_action_update(Update {
        operation: first,
        event: Event::Result {
            index: 0,
            status: Status::Succeeded,
        },
    });
    app.apply_action_update(Update {
        operation: first,
        event: Event::Finished,
    });
    app.selected = app
        .rows
        .iter()
        .position(|row| row.engine == 0 && row.scope == Scope::Engine)
        .unwrap();
    app.request_action(Action::Start);
    app.action_review_visible = true;
    app.confirm_action();
    let next = app.action_request.take().unwrap().operation;
    assert!(!app.action_jobs.contains_key(&first));
    assert!(!app.apply_action_update(Update {
        operation: first,
        event: Event::Result {
            index: 0,
            status: Status::Unknown("late".into())
        }
    }));
    assert!(app.action_jobs[&next].results[0].is_none());
    assert!(app.action_jobs[&second].finished);
}

#[test]
fn runtime_rows_target_the_provider_not_workloads_and_offline_start_is_allowed() {
    use crate::{actions::Action, runtime::Target};
    let mut app = app();
    app.demo = false;
    app.engines[0].runtime = Some(Target::Colima {
        profile: "fixture".into(),
    });
    app.selected = app
        .rows
        .iter()
        .position(|row| row.scope == Scope::Engine)
        .unwrap();
    for action in [Action::Start, Action::Stop, Action::Restart] {
        let request = app.plan_action(action).unwrap();
        assert!(request.runtime.is_some());
        assert!(request.containers.is_empty());
        assert_eq!(request.target_count(), 1);
    }
    assert!(app.plan_action(Action::Delete).is_err());
    app.update_engine(0, Err("Runtime stopped".into()));
    app.selected = app
        .rows
        .iter()
        .position(|row| row.scope == Scope::Engine)
        .unwrap();
    assert!(app.current().unwrap().title.contains("(offline)"));
    assert!(app.plan_action(Action::Stop).is_err());
    assert!(app.plan_action(Action::Restart).is_err());
    app.request_action(Action::Start);
    app.action_review_visible = true;
    app.confirm_action();
    assert!(app.action_confirmation.is_none());
    assert!(app.action_request.as_ref().unwrap().runtime.is_some());
    assert_eq!(app.action_progress().unwrap().results.len(), 1);
    assert!(!app.action_results_visible);
}

#[test]
fn saved_runtime_rows_survive_relaunch_without_sockets_and_rejoin_native_discovery() {
    use crate::runtime::Target;
    let mut original = app();
    original.engines[0].runtime = Some(Target::Podman {
        machine: "fixture".into(),
    });
    let text = toml::to_string(&original.preferences()).unwrap();
    assert!(!text.contains("unix:///test"));
    let preferences = toml::from_str(&text).unwrap();
    let mut restored = App::new(vec![], 100, false, false);
    restored.apply_preferences(preferences, false);
    assert_eq!(restored.engines.len(), 1);
    assert!(restored.rows[0].title.contains("(offline)"));
    assert!(restored.engines[0].connection.endpoint.is_empty());
    restored.reconcile_podman(&crate::discovery::podman::Discovery {
        machines: vec![crate::discovery::podman::Machine {
            name: "fixture".into(),
            endpoint: Ok("unix:///new-forwarded.sock".into()),
        }],
        warning: None,
    });
    assert_eq!(restored.engines.len(), 1);
    assert_eq!(
        restored.engines[0].connection.endpoint,
        "unix:///new-forwarded.sock"
    );
    restored.update_engine(0, Ok(Snapshot::default()));
    assert!(!restored.rows[0].title.contains("(offline)"));
}

#[test]
fn generic_context_never_falls_back_to_bulk_container_actions() {
    let mut app = app();
    app.demo = false;
    app.selected = 0;
    let error = app
        .plan_action(crate::actions::Action::Stop)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unsupported"));
}

#[test]
fn preferences_restore_navigation_without_restoring_shells_or_input_and_cancel_on_interaction() {
    let mut original = app();
    original.open_project(Project {
        engine: 0,
        namespace: None,
        name: "shop".into(),
    });
    original.tab = Tab::Layers;
    original.performance_open = true;
    original.log_view.wrap = false;
    let preferences = original.preferences();
    let serialized = toml::to_string(&preferences).unwrap();
    assert!(!serialized.contains("unix:///test"));
    assert!(!serialized.contains("exec_command"));
    let mut restored = app();
    restored.apply_preferences(preferences.clone(), true);
    restored.restore_navigation();
    assert_eq!(restored.project.as_ref().unwrap().name, "shop");
    assert_eq!(restored.tab, Tab::Layers);
    assert!(restored.performance_open);
    assert!(!restored.log_view.wrap);
    assert!(!restored.exec_input);
    assert!(restored.sessions.is_empty());
    let mut canceled = app();
    canceled.apply_preferences(preferences, true);
    canceled.cancel_startup();
    canceled.restore_navigation();
    assert!(canceled.project.is_none());
    original.tab = Tab::Exec;
    assert_eq!(original.preferences().tab, Tab::Overview);
}

fn app() -> App {
    let mut app = App::new(
        vec![Connection {
            name: "test".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///test".into(),
        }],
        100,
        false,
        true,
    );
    app.update_engine(
        0,
        Ok(Snapshot {
            containers: vec![Container {
                id: "one".into(),
                name: "api-1".into(),
                project: Some("shop".into()),
                service: Some("api".into()),
                ..Default::default()
            }],
            ..Default::default()
        }),
    );
    app
}

#[test]
fn runtime_is_parent_of_projects_and_vim_navigation_works() {
    let mut app = app();
    assert_eq!(app.rows[0].scope, Scope::Engine);
    assert_eq!(app.rows[1].title, "shop [stopped]");
    app.move_selection(1);
    app.expand();
    app.move_selection(1);
    app.expand();
    app.move_selection(1);
    app.expand();
    assert!(app.rows.iter().any(|r| r.title.starts_with("api-1")));
    app.collapse();
    app.collapse();
    assert!(matches!(
        app.current().unwrap().scope,
        Scope::Services { .. }
    ));
}

#[test]
fn refresh_preserves_selection_and_search_finds_descendants() {
    let mut app = app();
    app.move_selection(1);
    let key = app.current().unwrap().key.clone();
    app.refresh_tree();
    assert_eq!(app.current().unwrap().key, key);
    app.tree_query = "api-1".into();
    app.refresh_tree();
    assert!(app.rows.iter().any(|r| r.title.starts_with("api-1")));
}

#[test]
fn grouped_workloads_preserve_sorted_groups_replica_order_and_missing_service_counts() {
    let container = |name: &str,
                     project: Option<&str>,
                     service: Option<&str>,
                     namespace: Option<&str>,
                     state: &str| Container {
        id: name.into(),
        name: name.into(),
        project: project.map(str::to_owned),
        service: service.map(str::to_owned),
        namespace: namespace.map(str::to_owned),
        state: state.into(),
        ..Default::default()
    };
    let snapshot = Snapshot {
        containers: vec![
            container("z-replica-2", Some("z"), Some("api"), None, "running"),
            container("standalone", None, None, None, "exited"),
            container("z-replica-1", Some("z"), Some("api"), None, "exited"),
            container("a-missing", Some("a"), None, None, "running"),
            container(
                "a-explicit",
                Some("a"),
                Some("Unassigned service"),
                None,
                "exited",
            ),
            container("foreign", Some("a"), Some("api"), Some("other"), "running"),
            container("z-worker", Some("z"), Some("worker"), None, "running"),
        ],
        ..Default::default()
    };
    let nodes = workload_tree(0, &snapshot, None);
    assert_eq!(
        nodes
            .iter()
            .map(|node| node.row.title.as_str())
            .collect::<Vec<_>>(),
        ["a [1/2 running]", "z [2/3 running]", "Standalone (1)"]
    );
    let a_services = &nodes[0].children[0];
    assert_eq!(a_services.row.title, "Services (1)");
    assert_eq!(
        a_services.children[0].row.title,
        "Unassigned service [1/2 running]"
    );
    assert_eq!(
        a_services.children[0]
            .children
            .iter()
            .map(|node| node.row.title.as_str())
            .collect::<Vec<_>>(),
        ["a-missing", "a-explicit"]
    );
    let z_services = &nodes[1].children[0];
    assert_eq!(z_services.row.title, "Services (2)");
    assert_eq!(z_services.children[0].row.title, "api [1/2 running]");
    assert_eq!(z_services.children[1].row.title, "worker [1/1 running]");
    assert_eq!(
        z_services.children[0]
            .children
            .iter()
            .map(|node| node.row.title.as_str())
            .collect::<Vec<_>>(),
        ["z-replica-2", "z-replica-1"]
    );
    let other = workload_tree(0, &snapshot, Some(&"other".into()));
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].row.title, "a [1/1 running]");
}

#[test]
fn duplicate_engine_identity_does_not_duplicate_workloads() {
    let mut app = app();
    let mut connection = app.engines[0].connection.clone();
    connection.name = "alias".into();
    app.engines.push(Engine {
        connection,
        runtime: None,
        snapshot: None,
        error: None,
        alias: None,
        metrics: Default::default(),
        metrics_revision: 0,
        events: Default::default(),
    });
    let snapshot = Snapshot {
        engine_id: Some("same".into()),
        ..Default::default()
    };
    app.update_engine(0, Ok(snapshot.clone()));
    app.update_engine(1, Ok(snapshot));
    assert_eq!(app.engines[1].alias.as_deref(), Some("test"));
}

#[test]
fn failure_preserves_last_known_snapshot_but_marks_it_stale() {
    let mut app = app();
    app.update_engine(0, Err("offline".into()));
    assert!(app.engines[0].snapshot.is_some());
    assert!(app.rows[0].title.contains("(offline)"));
}

#[test]
fn next_search_match_skips_ancestors_and_wraps() {
    let mut app = app();
    app.tree_query = "api-1".into();
    app.refresh_tree();
    app.selected = 0;
    app.next_tree_match(false);
    assert!(app.current().unwrap().title.contains("api-1"));
    let selected = app.selected;
    app.next_tree_match(false);
    assert_eq!(app.selected, selected);
    app.next_tree_match(true);
    assert_eq!(app.selected, selected);
}

#[test]
fn counts_distinct_services_and_running_instances() {
    let mut app = app();
    let stopped = app.engines[0].snapshot.as_ref().unwrap().containers[0].clone();
    let running = Container {
        id: "two".into(),
        state: "running".into(),
        ..stopped.clone()
    };
    let replica = Container {
        id: "three".into(),
        ..running.clone()
    };
    let postgres = Container {
        id: "four".into(),
        service: Some("postgres".into()),
        ..running.clone()
    };
    app.update_engine(
        0,
        Ok(Snapshot {
            containers: vec![stopped, running, replica, postgres],
            ..Default::default()
        }),
    );
    let tree = engine_tree(0, &app.engines[0]);
    let project = &tree.children[0];
    assert_eq!(project.row.title, "shop [3/4 running]");
    let services = &project.children[0];
    assert_eq!(services.row.title, "Services (2)");
    assert_eq!(services.children[0].row.title, "api [2/3 running]");
    assert_eq!(services.children[1].row.title, "postgres [1/1 running]");
}

#[test]
fn metric_updates_are_engine_scoped_and_stale_revisions_are_ignored() {
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    let row = app
        .rows
        .iter()
        .find(|row| matches!(row.scope, Scope::Engine) && row.engine == 0)
        .unwrap()
        .clone();
    let other = app
        .rows
        .iter()
        .find(|row| matches!(row.scope, Scope::Engine) && row.engine == 1)
        .unwrap()
        .clone();
    let before_other = app.metrics_for(&other).cpu_percent;
    app.engines[0].metrics_revision = 3;
    let batch = || {
        std::collections::HashMap::from([(
            "/api-1".into(),
            Ok(crate::metrics::Reading {
                usage: crate::metrics::Usage {
                    cpu_percent: Some(123.0),
                    memory_bytes: Some(1024),
                    ..Default::default()
                },
                counters: None,
                sampled_at: std::time::Instant::now(),
            }),
        )])
    };
    let before = app.metrics_for(&row).cpu_percent;
    app.apply_metrics(crate::metrics::Event {
        engine: 0,
        revision: 2,
        batch: batch(),
    });
    assert_eq!(app.metrics_for(&row).cpu_percent, before);
    app.apply_metrics(crate::metrics::Event {
        engine: 0,
        revision: 3,
        batch: batch(),
    });
    assert_ne!(app.metrics_for(&row).cpu_percent, before);
    assert_eq!(app.metrics_for(&other).cpu_percent, before_other);
    app.engines[0].error = Some("offline".into());
    assert_eq!(app.metrics_for(&row).cpu_percent, None);
}

#[test]
fn known_empty_runtime_is_zero_but_disconnected_runtime_is_not() {
    let mut app = app();
    app.update_engine(0, Ok(Snapshot::default()));
    let row = app.rows[0].clone();
    assert_eq!(app.metrics_for(&row).cpu_label(), "0.0%");
    assert_eq!(app.metrics_for(&row).memory_label(), "0 B");
    app.update_engine(0, Err("offline".into()));
    assert_eq!(app.metrics_for(&row).cpu_label(), "--");
}

#[test]
fn native_podman_reconciliation_keeps_machine_indices_across_restarts_and_failures() {
    use crate::discovery::podman::{Discovery, Machine};
    let mut app = app();
    let before = app.engines[0].connection.clone();
    let report = |socket: &str| Discovery {
        machines: vec![
            Machine {
                name: "first".into(),
                endpoint: Ok(socket.into()),
            },
            Machine {
                name: "second".into(),
                endpoint: Err("stopped".into()),
            },
        ],
        warning: None,
    };
    let availability = app.reconcile_podman(&report("unix:///one.sock"));
    let index = app.podman_machines["first"];
    let stopped = app.podman_machines["second"];
    assert!(availability.contains(&(index, true)));
    assert!(availability.contains(&(stopped, false)));
    app.update_engine(
        index,
        Ok(Snapshot {
            engine_id: Some("podman-id".into()),
            ..Default::default()
        }),
    );
    app.selected = app.rows.iter().position(|row| row.engine == index).unwrap();
    let selected = app.current().unwrap().key.clone();
    app.reconcile_podman(&report("unix:///two.sock"));
    assert_eq!(app.podman_machines["first"], index);
    assert_eq!(app.current().unwrap().key, selected);
    assert_eq!(app.engines[index].connection.endpoint, "unix:///two.sock");
    assert!(app.engines[index].error.is_some());
    assert_eq!(app.engines[0].connection, before);
    let failed_scan = Discovery {
        machines: vec![],
        warning: Some("invalid JSON".into()),
    };
    assert!(
        app.reconcile_podman(&failed_scan)
            .iter()
            .all(|(_, available)| !available)
    );
    assert_eq!(app.engines[index].connection.endpoint, "unix:///two.sock");
    app.reconcile_podman(&Discovery::default());
    assert!(
        app.engines[index]
            .error
            .as_ref()
            .unwrap()
            .contains("no longer listed")
    );
    assert_eq!(app.reconcile_podman(&report("unix:///three.sock")).len(), 2);
    assert_eq!(app.podman_machines["first"], index);
}

#[test]
fn native_podman_and_docker_context_aliases_do_not_duplicate_metrics() {
    use crate::discovery::podman::{Discovery, Machine};
    let mut app = app();
    let identity = app.engines[0].snapshot.as_ref().unwrap().engine_id.clone();
    app.reconcile_podman(&Discovery {
        machines: vec![Machine {
            name: "fixture".into(),
            endpoint: Ok("unix:///podman.sock".into()),
        }],
        warning: None,
    });
    let index = app.podman_machines["fixture"];
    app.update_engine(
        index,
        Ok(Snapshot {
            engine_id: identity,
            ..Default::default()
        }),
    );
    // Seed the same nonempty identity explicitly, independent of the app fixture.
    app.update_engine(
        0,
        Ok(Snapshot {
            engine_id: Some("same-podman".into()),
            ..Default::default()
        }),
    );
    app.update_engine(
        index,
        Ok(Snapshot {
            engine_id: Some("same-podman".into()),
            ..Default::default()
        }),
    );
    assert!(app.engines[index].alias.is_some());
    assert_eq!(app.engine_metrics(index).cpu_percent, None);
}

#[test]
fn startup_matching_waits_for_native_discovery_and_uses_machine_project_labels() {
    use crate::discovery::podman::{Discovery, Machine};
    let mut app = app();
    let cwd = std::env::current_dir().unwrap();
    app.configure_startup(cwd.clone());
    app.wait_for_machine_discovery();
    app.update_engine(0, Ok(Snapshot::default()));
    assert!(app.startup_cwd.is_some());
    app.reconcile_podman(&Discovery {
        machines: vec![Machine {
            name: "fixture".into(),
            endpoint: Ok("unix:///podman.sock".into()),
        }],
        warning: None,
    });
    let index = app.podman_machines["fixture"];
    assert!(app.project.is_none());
    app.update_engine(
        index,
        Ok(Snapshot {
            containers: vec![Container {
                id: "native-api".into(),
                project: Some("native-project".into()),
                service: Some("api".into()),
                compose_working_dir: Some(cwd),
                ..Default::default()
            }],
            ..Default::default()
        }),
    );
    assert_eq!(app.project.as_ref().unwrap().engine, index);
    assert_eq!(app.project.as_ref().unwrap().name, "native-project");
}

#[test]
fn matching_podman_socket_is_aliased_even_when_the_api_omits_engine_identity() {
    use crate::discovery::podman::{Discovery, Machine};
    let mut app = app();
    app.reconcile_podman(&Discovery {
        machines: vec![Machine {
            name: "fixture".into(),
            endpoint: Ok("/test".into()),
        }],
        warning: None,
    });
    let index = app.podman_machines["fixture"];
    app.update_engine(index, Ok(Snapshot::default()));
    assert!(app.engines[index].alias.is_some());
}

#[test]
fn late_native_machine_respects_pins_without_resorting_usage_or_moving_focus() {
    use crate::discovery::podman::{Discovery, Machine};
    let mut app = app();
    app.context_sort = crate::context_sort::ContextSort::Cpu;
    app.context_pins = vec!["Podman / preferred".into()];
    let selected = app.current().unwrap().key.clone();
    app.reconcile_podman(&Discovery {
        machines: vec![Machine {
            name: "preferred".into(),
            endpoint: Ok("unix:///podman.sock".into()),
        }],
        warning: None,
    });
    assert_eq!(app.context_order[0], app.podman_machines["preferred"]);
    assert_eq!(app.performance_engine(), Some(0));
    assert_eq!(app.current().unwrap().key, selected);
}

#[test]
fn sorting_preserves_backend_indices_selection_expansion_and_log_targets() {
    use crate::context_sort::ContextSort;
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    let identities: Vec<_> = app
        .engines
        .iter()
        .map(|engine| engine.connection.clone())
        .collect();
    let selection = app.current().unwrap().key.clone();
    let expanded = app.expanded.clone();
    app.log_scope = Some("fixed-log-target".into());
    app.performance_scroll = app
        .performance_engines()
        .iter()
        .position(|&engine| engine == 0)
        .unwrap();
    app.pick_context_sort();
    app.picker.as_mut().unwrap().selected = 2;
    app.choose();
    assert_eq!(app.context_sort, ContextSort::Cpu);
    assert_eq!(app.current().unwrap().key, selection);
    assert_eq!(app.expanded, expanded);
    assert_eq!(app.log_scope.as_deref(), Some("fixed-log-target"));
    assert_eq!(app.performance_engine(), Some(0));
    assert_eq!(
        app.engines
            .iter()
            .map(|engine| engine.connection.clone())
            .collect::<Vec<_>>(),
        identities
    );
    let order = app.context_order.clone();
    app.update_engine(0, Err("disconnected".into()));
    app.refresh_tree();
    assert_eq!(
        app.context_order, order,
        "usage sort must not jump on refresh"
    );
    app.pick_context_sort();
    app.picker.as_mut().unwrap().selected = 0;
    app.choose();
    assert_ne!(app.context_order, order);
}

#[test]
fn project_workspace_excludes_other_engines_and_back_restores_navigation() {
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    let selected = app.current().unwrap().key.clone();
    let rows = app.rows.len();
    let expanded = app.expanded.clone();
    app.open_selected();
    assert_eq!(app.project.as_ref().unwrap().name, "shop");
    assert!(app.rows.iter().all(|row| row.engine == 0));
    assert!(
        !app.rows
            .iter()
            .any(|row| matches!(row.scope, Scope::Engine | Scope::Standalone(_)))
    );
    assert_eq!(app.rows[0].depth, 0);
    assert_eq!(app.selected_containers().len(), 3);
    assert!(app.back());
    assert!(app.project.is_none());
    assert_eq!(app.rows.len(), rows);
    assert_eq!(app.current().unwrap().key, selected);
    assert_eq!(app.expanded, expanded);
}

#[test]
fn workspace_includes_attached_external_network_but_not_other_workloads() {
    let mut app = app();
    let mut snapshot = app.engines[0].snapshot.as_ref().unwrap().clone();
    snapshot.containers[0].networks.push("shared".into());
    snapshot.containers.push(Container {
        id: "other".into(),
        name: "other-api".into(),
        project: Some("other".into()),
        networks: vec!["shared".into()],
        ..Default::default()
    });
    snapshot.networks.push(crate::model::Resource {
        name: "shared".into(),
        external: true,
        containers: vec!["api-1".into(), "other-api".into()],
        ..Default::default()
    });
    app.update_engine(0, Ok(snapshot));
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(&row.scope, Scope::Project { project, .. } if project == "shop"))
        .unwrap();
    app.open_selected();
    app.tree_query = "shared".into();
    app.refresh_tree();
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(&row.scope, Scope::Resource { name, .. } if name == "shared"))
        .unwrap();
    assert!(app.current().unwrap().title.contains("shared/external"));
    assert_eq!(app.selected_containers().len(), 1);
    assert_eq!(app.selected_containers()[0].name, "api-1");
}

#[test]
fn service_exec_requires_replica_choice_and_rejects_offline_engines() {
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    app.demo = false;
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(&row.scope, Scope::Service { service, .. } if service == "api"))
        .unwrap();
    app.request_exec();
    assert!(app.exec_request.is_none());
    assert_eq!(app.picker.as_ref().unwrap().choices.len(), 2);
    app.picker.as_mut().unwrap().selected = 1;
    app.choose();
    let request = app.exec_request.take().unwrap();
    assert_eq!(request.container.name, "shop-api-2");
    assert_eq!(request.connection.name, "OrbStack");
    app.engines[0].error = Some("offline".into());
    app.request_exec();
    assert!(app.exec_request.is_none());
    assert!(app.notice.contains("disconnected"));
}

#[test]
fn resource_only_projects_do_not_create_ghost_workloads_and_resources_remain_visible() {
    let mut app = app();
    app.update_engine(
        0,
        Ok(Snapshot {
            networks: vec![crate::model::Resource {
                name: "empty_default".into(),
                project: Some("empty".into()),
                ..Default::default()
            }],
            ..Default::default()
        }),
    );
    let tree = engine_tree(0, &app.engines[0]);
    assert!(
        !tree
            .children
            .iter()
            .any(|node| matches!(node.row.scope, Scope::Project { .. }))
    );
    assert!(tree.children.iter().any(|node| {
        node.children
            .iter()
            .any(|resource| resource.row.title.contains("empty_default"))
    }));
}

#[test]
fn deleting_last_compose_container_leaves_resources_but_exits_workspace_on_successful_refresh() {
    let mut app = app();
    app.engines[0]
        .snapshot
        .as_mut()
        .unwrap()
        .volumes
        .push(crate::model::Resource {
            name: "shop_data".into(),
            project: Some("shop".into()),
            ..Default::default()
        });
    app.open_project(Project {
        engine: 0,
        namespace: None,
        name: "shop".into(),
    });
    let original = app.engines[0].snapshot.clone().unwrap();
    app.update_engine(0, Err("temporary failure".into()));
    assert!(
        app.project.is_some(),
        "A failed refresh must not claim a stack was removed"
    );
    let mut remaining = original.clone();
    remaining
        .containers
        .retain(|container| container.project.as_deref() != Some("shop"));
    app.update_engine(0, Ok(remaining));
    assert!(app.project.is_none());
    let tree = engine_tree(0, &app.engines[0]);
    assert!(
        !tree
            .children
            .iter()
            .any(|node| matches!(&node.row.scope,Scope::Project {project,..} if project=="shop"))
    );
    assert!(!app.engines[0].snapshot.as_ref().unwrap().volumes.is_empty());
    let mut stopped = original;
    for container in &mut stopped.containers {
        container.state = "exited".into();
    }
    app.update_engine(0, Ok(stopped));
    assert!(
        engine_tree(0, &app.engines[0])
            .children
            .iter()
            .any(|node| matches!(&node.row.scope,Scope::Project {project,..} if project=="shop")),
        "Stopped containers must remain visible"
    );
}

#[test]
fn toggling_a_search_expanded_branch_collapses_it_and_preserves_selection() {
    let mut app = app();
    app.tree_query = "api-1".into();
    app.refresh_tree();
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(row.scope, Scope::Service { .. }))
        .unwrap();
    let key = app.current().unwrap().key.clone();
    app.toggle_branch();
    assert!(app.tree_query.is_empty());
    assert_eq!(app.current().unwrap().key, key);
    assert!(!app.expanded.contains(&key));
    assert!(
        !app.rows
            .iter()
            .any(|row| matches!(row.scope, Scope::Container(_)))
    );
    app.toggle_branch();
    assert!(
        app.rows
            .iter()
            .any(|row| matches!(row.scope, Scope::Container(_)))
    );
}

#[test]
fn counts_do_not_mix_namespaces_or_reset_selection_on_refresh() {
    let mut app = app();
    let mut snapshot = app.engines[0].snapshot.as_ref().unwrap().clone();
    snapshot.containers[0].namespace = Some("first".into());
    let other = Container {
        namespace: Some("second".into()),
        state: "running".into(),
        ..snapshot.containers[0].clone()
    };
    snapshot.containers.push(other);
    snapshot.namespaces = vec!["first".into(), "second".into()];
    app.update_engine(0, Ok(snapshot.clone()));
    let tree = engine_tree(0, &app.engines[0]);
    assert_eq!(tree.children[0].children[0].row.title, "shop [stopped]");
    assert_eq!(tree.children[1].children[0].row.title, "shop [1/1 running]");
    app.selected = 1;
    app.expand();
    app.move_selection(1);
    app.expand();
    let key = app.current().unwrap().key.clone();
    snapshot.containers[0].state = "running".into();
    app.update_engine(0, Ok(snapshot));
    assert_eq!(app.current().unwrap().key, key);
    assert_eq!(app.current().unwrap().title, "shop [1/1 running]");
    assert!(app.expanded.contains(&key));
}

fn saved_view_app() -> App {
    let mut app = App::new(
        vec![Connection {
            name: "fixture".into(),
            kind: BackendKind::Docker,
            endpoint: "demo://fixture".into(),
        }],
        100,
        false,
        true,
    );
    app.update_engine(
        0,
        Ok(Snapshot {
            containers: vec![Container {
                id: "abc".into(),
                name: "web".into(),
                state: "running".into(),
                project: Some("shop".into()),
                service: Some("web".into()),
                ..Default::default()
            }],
            ..Default::default()
        }),
    );
    app.refresh_tree();
    app
}

#[test]
fn saved_views_restore_scope_and_filters_and_back_returns() {
    let mut app = saved_view_app();
    app.open_project(Project {
        engine: 0,
        namespace: None,
        name: "shop".into(),
    });
    app.tab = Tab::Logs;
    app.log_view.filter.set_text("backend1 500");
    app.log_filter_terms = "backend1 500".into();
    app.save_view("Backend errors");
    assert!(app.notice.contains("filter text"), "{}", app.notice);

    while app.back() {}
    app.clear_log_filters();
    app.tab = Tab::Overview;
    assert!(app.project.is_none());

    app.apply_saved_view("Backend errors");
    assert_eq!(app.project.as_ref().unwrap().name, "shop");
    assert_eq!(app.tab, Tab::Logs);
    assert!(app.log_view.filter.text_active());
    assert_eq!(app.log_filter_terms, "backend1 500");
    assert!(app.back());
    assert!(app.project.is_none());
}

#[test]
fn saved_views_name_what_is_gone_never_connect_and_never_restore_a_shell() {
    let mut app = saved_view_app();
    app.tab = Tab::Exec;
    let mut view = app.capture_view("Gone");
    assert_eq!(view.tab, Tab::Overview);
    view.tab = Tab::Exec;
    view.machine = Some("removed-server".into());
    view.log_advanced = "not json".into();
    app.saved_views.push(view);
    app.apply_saved_view("Gone");
    assert_eq!(app.tab, Tab::Overview);
    assert!(app.machine.is_none());
    assert!(app.notice.contains("removed-server"), "{}", app.notice);
    assert!(app.notice.contains("advanced log filter"), "{}", app.notice);
}

#[test]
fn saving_replaces_by_name_and_deleting_needs_an_exact_name() {
    let mut app = saved_view_app();
    app.save_view("one");
    app.tab = Tab::Logs;
    app.save_view("one");
    assert_eq!(app.saved_views.len(), 1);
    assert_eq!(app.saved_views[0].tab, Tab::Logs);
    assert!(app.notice.contains("Replaced"), "{}", app.notice);
    app.save_view("  ");
    assert_eq!(app.saved_views.len(), 1);
    app.delete_view("on");
    assert_eq!(app.saved_views.len(), 1);
    assert!(app.notice.contains("No saved view"), "{}", app.notice);
    app.delete_view("one");
    assert!(app.saved_views.is_empty());
}

#[test]
fn fleet_jumps_to_a_failing_workload_and_back_returns() {
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    app.open_fleet();
    let summary = app.fleet_summary();
    let index = summary
        .problems
        .iter()
        .position(|problem| problem.subject == "shop-api-2" || problem.detail == "unhealthy")
        .expect("the demo has an unhealthy container");
    let subject = summary.problems[index].subject.clone();
    app.fleet_view.as_mut().unwrap().selected = index;
    app.open_fleet_selection();
    assert!(app.fleet_view.is_none());
    assert_eq!(app.selected_containers()[0].name, subject);
    assert!(app.back());
}

#[test]
fn fleet_rows_without_a_workload_stay_open_and_explain() {
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    crate::demo::populate_servers(&mut app);
    app.open_fleet();
    let index = app
        .fleet_summary()
        .problems
        .iter()
        .position(|problem| problem.target.is_none())
        .expect("the demo has a down server");
    app.fleet_view.as_mut().unwrap().selected = index;
    app.open_fleet_selection();
    assert!(app.fleet_view.is_some());
    assert!(app.notice.contains("server"), "{}", app.notice);
}

#[test]
fn the_timeline_shows_failures_and_logs_open_shortly_before_the_moment() {
    let mut app = saved_view_app();
    let key = app.engines[0].snapshot.as_ref().unwrap().containers[0].key();
    app.engines[0].events.add(crate::events::Event {
        at_ms: 1_000_000,
        key: key.clone(),
        name: "web".into(),
        project: Some("shop".into()),
        service: Some("web".into()),
        action: "oom".into(),
        detail: String::new(),
    });
    for _ in 0..6 {
        let keys: Vec<_> = app.rows.iter().map(|row| row.key.clone()).collect();
        app.expanded.extend(keys);
        app.refresh_tree();
    }
    app.selected = app
        .rows
        .iter()
        .position(|row| matches!(row.scope, Scope::Container(_)))
        .unwrap();
    app.open_timeline();
    assert_eq!(app.timeline_incidents().len(), 1);
    assert!(!app.events_saved(0));
    app.open_timeline_logs();
    assert!(app.timeline_view.is_none());
    assert_eq!(app.tab, Tab::Logs);
    assert_eq!(app.log_since, Some(940));
    assert_eq!(app.selected_containers()[0].name, "web");
}

#[test]
fn alerts_show_a_failing_workload_and_turning_them_off_keeps_the_list_empty() {
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    crate::demo::populate(&mut app);
    app.alert_rules.enabled = false;
    app.evaluate_alerts();
    assert!(app.alerts.open.is_empty());

    app.alert_rules.enabled = true;
    app.evaluate_alerts();
    assert!(
        app.alerts.active_count() > 0,
        "the demo has an unhealthy container"
    );
    let name = app.alerts.open[0].1.finding.subject.clone();
    app.alerts.view_open = true;
    app.open_alert_selection();
    assert!(!app.alerts.view_open);
    assert_eq!(app.selected_containers()[0].name, name);
}

#[test]
fn copying_needs_one_container_and_a_long_text_is_refused_rather_than_lost() {
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
        .position(|row| matches!(row.scope, Scope::Container(_)))
        .unwrap();
    let container = app.selected_containers().remove(0);
    app.copy_selected(CopyField::Id);
    assert_eq!(app.clipboard_request.take().unwrap(), container.id);
    app.copy_selected(CopyField::Name);
    assert_eq!(app.clipboard_request.take().unwrap(), container.name);
    app.copy_selected(CopyField::Image);
    assert_eq!(app.clipboard_request.take().unwrap(), container.image);

    app.copy_log_line();
    assert!(app.clipboard_request.is_none(), "Nothing is showing");
    app.logs.push("api", "hello", false);
    app.logs.push("api", "newest", false);
    app.log_view.sync(&app.logs, 80, 10);
    app.copy_log_line();
    assert_eq!(app.clipboard_request.take().unwrap(), "newest");
    app.copy_text("x".repeat(70_000));
    assert!(app.clipboard_request.is_none());
    assert!(app.notice.contains("too long"), "{}", app.notice);
}

#[test]
fn a_changed_configuration_applies_live_and_a_broken_one_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let mut app = App::new(crate::demo::connections(), 100, false, true);
    app.config_path = Some(path.clone());

    std::fs::write(
        &path,
        "[bindings]\n\"x\" = \"zoom\"\n[alerts]\nrestart_count = 9\n",
    )
    .unwrap();
    app.reload_config();
    assert_eq!(app.alert_rules.restart_count, 9);
    assert_eq!(
        app.custom_bindings.get("x").map(String::as_str),
        Some("zoom")
    );
    assert!(app.notice.contains("reloaded"), "{}", app.notice);

    // Nothing changed on disk, so nothing happens and the notice is not repeated.
    app.notice.clear();
    app.reload_config();
    assert!(app.notice.is_empty());

    app.config_modified = None;
    std::fs::write(&path, "[bindings]\n\"x\" = \"no-such-action\"\n").unwrap();
    app.reload_config();
    assert_eq!(app.alert_rules.restart_count, 9);
    assert_eq!(
        app.custom_bindings.get("x").map(String::as_str),
        Some("zoom")
    );
    assert!(app.notice.contains("not reloaded"), "{}", app.notice);
}
