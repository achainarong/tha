//! Warnings raised from data Tha already has. Each incident is one alert with a rising count,
//! and it clears when the problem stops. Missing data is never treated as a problem or as a
//! recovery: while a connection is down, its alerts are marked as not current.
use crate::{
    app::App,
    events::Event,
    machines::{LOCAL_MACHINE, MachineState},
    remote,
};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const MAX_KEPT: usize = 200;

/// `[alerts]` in the configuration. Thresholds apply to every connection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rules {
    pub enabled: bool,
    /// Exits within `restart_minutes` that count as a crash loop.
    pub restart_count: usize,
    pub restart_minutes: i64,
    pub memory_percent: u64,
    pub memory_minutes: i64,
    /// How long an out-of-memory kill stays active.
    pub oom_minutes: i64,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            enabled: true,
            restart_count: 5,
            restart_minutes: 10,
            memory_percent: 90,
            memory_minutes: 5,
            oom_minutes: 10,
        }
    }
}

impl Rules {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.restart_count >= 2 && self.restart_minutes >= 1,
            "alerts.restart_count must be at least 2 and restart_minutes at least 1"
        );
        anyhow::ensure!(
            (1..=100).contains(&self.memory_percent) && self.memory_minutes >= 1,
            "alerts.memory_percent must be 1 to 100 and memory_minutes at least 1"
        );
        anyhow::ensure!(
            self.oom_minutes >= 1,
            "alerts.oom_minutes must be at least 1"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    CrashLoop,
    OutOfMemory,
    Unhealthy,
    HighMemory,
    ServerDown,
}

/// What one evaluation found. `count` rises across evaluations of the same incident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: Rule,
    pub machine: String,
    pub subject: String,
    pub detail: String,
    pub target: Option<(usize, String)>,
    /// The identity of this incident, unique per rule and thing it concerns.
    pub key: String,
    pub count: u64,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub finding: Finding,
    pub first_ms: i64,
    pub last_ms: i64,
    pub active: bool,
    /// The connection is down, so this could neither be confirmed nor cleared.
    pub not_current: bool,
}

#[derive(Debug, Default)]
pub struct Alerts {
    pub open: Vec<(String, Alert)>,
    pub selected: usize,
    pub view_open: bool,
    /// Anything that ended before this happened while Tha was closed.
    pub started_ms: i64,
}

/// A rule over one container's events, shared with the agent's own evaluation.
pub fn crash_loop(rules: &Rules, exits: &[i64]) -> Option<(usize, i64)> {
    let newest = *exits.iter().max()?;
    let window = rules.restart_minutes * 60_000;
    let count = exits.iter().filter(|at| newest - **at <= window).count();
    (count >= rules.restart_count).then_some((count, newest))
}

fn exits_by_container<'a>(events: impl Iterator<Item = &'a Event>) -> BTreeMap<&'a str, Vec<i64>> {
    let mut exits: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for event in events {
        // A clean exit is someone stopping it or a job finishing, not a crash.
        if event.action == "die" && event.detail != "exit 0" {
            exits.entry(&event.key).or_default().push(event.at_ms);
        }
    }
    exits
}

/// Whether all readings over the whole period are at or above the limit. Too few readings, or a
/// missing one, means it cannot be said, so it does not count.
fn sustained(
    points: &[(i64, Option<f64>, Option<u64>)],
    rules: &Rules,
    now_ms: i64,
) -> Option<u64> {
    let period = rules.memory_minutes * 60_000;
    let recent: Vec<_> = points
        .iter()
        .filter(|(at, ..)| now_ms - *at <= period)
        .collect();
    let oldest = points.iter().map(|(at, ..)| *at).min()?;
    if now_ms - oldest < period || recent.is_empty() {
        return None;
    }
    let mut highest = 0;
    for (_, _, memory) in &recent {
        let memory = (*memory)?;
        if memory < rules.memory_percent {
            return None;
        }
        highest = highest.max(memory);
    }
    Some(highest)
}

/// What one pass over the current data found.
#[derive(Debug, Default)]
pub struct Evaluation {
    pub findings: Vec<Finding>,
    /// Incidents that already ended, such as overnight, seen in saved events.
    pub past: Vec<Finding>,
    /// Key prefixes of connections whose data cannot be trusted right now.
    pub unknown: Vec<String>,
}

pub fn evaluate(app: &App, rules: &Rules, now_ms: i64) -> Evaluation {
    let mut findings = Vec::new();
    let mut past = Vec::new();
    let mut unknown = Vec::new();
    for machine in &app.machines {
        if let MachineState::Offline { reason, .. } = &machine.state {
            findings.push(Finding {
                rule: Rule::ServerDown,
                machine: machine.remote.name.clone(),
                subject: machine.remote.name.clone(),
                detail: crate::tf!(
                    "Unreachable: {reason}",
                    reason = crate::model::safe_text(reason)
                ),
                target: None,
                key: format!("server/{}", machine.remote.name),
                count: 1,
                at_ms: now_ms,
            });
        }
    }
    for (index, engine) in app.engines.iter().enumerate() {
        if engine.alias.is_some() {
            continue;
        }
        let parsed = remote::parse_endpoint(&engine.connection.endpoint);
        let machine =
            parsed.map_or_else(|| LOCAL_MACHINE.to_owned(), |(server, _)| server.to_owned());
        let endpoint = &engine.connection.endpoint;
        if engine.error.is_some() {
            unknown.push(format!("{endpoint}#"));
            continue;
        }
        let Some(snapshot) = &engine.snapshot else {
            unknown.push(format!("{endpoint}#"));
            continue;
        };
        let name_of = |key: &str| {
            snapshot
                .containers
                .iter()
                .find(|container| container.key() == key)
                .map(|container| container.name.clone())
        };
        let known = |key: &str| name_of(key).map(|_| (index, key.to_owned()));
        let exits = exits_by_container(engine.events.newest_first());
        for (key, times) in &exits {
            let Some((count, newest)) = crash_loop(rules, times) else {
                continue;
            };
            let name = name_of(key).unwrap_or_else(|| (*key).to_owned());
            // Over once the loop has been quiet for its window.
            let ended = now_ms - newest > rules.restart_minutes * 60_000;
            (if ended { &mut past } else { &mut findings }).push(Finding {
                rule: Rule::CrashLoop,
                machine: machine.clone(),
                subject: crate::model::safe_text(&name),
                detail: crate::tf!(
                    "Exited {count} times in {minutes} minutes",
                    count = count,
                    minutes = rules.restart_minutes
                ),
                target: known(key),
                key: format!("{endpoint}#loop/{key}"),
                count: count as u64,
                at_ms: newest,
            });
        }
        let mut killed: BTreeMap<&str, (&Event, u64)> = BTreeMap::new();
        for event in engine.events.newest_first() {
            if event.action == "oom" {
                let found = killed.entry(&event.key).or_insert((event, 0));
                if now_ms - event.at_ms <= rules.oom_minutes * 60_000 {
                    found.1 += 1;
                }
            }
        }
        for (key, (event, count)) in killed {
            (if count > 0 { &mut findings } else { &mut past }).push(Finding {
                rule: Rule::OutOfMemory,
                machine: machine.clone(),
                subject: crate::model::safe_text(&event.name),
                detail: crate::i18n::t("Killed for running out of memory").into(),
                target: known(key),
                key: format!("{endpoint}#oom/{key}"),
                count: count.max(1),
                at_ms: event.at_ms,
            });
        }
        for container in &snapshot.containers {
            let key = container.key();
            if container.condition().label == "unhealthy" {
                findings.push(Finding {
                    rule: Rule::Unhealthy,
                    machine: machine.clone(),
                    subject: crate::model::safe_text(&container.name),
                    detail: crate::i18n::t("Its health check is failing").into(),
                    target: Some((index, key.clone())),
                    key: format!("{endpoint}#health/{key}"),
                    count: 1,
                    at_ms: now_ms,
                });
            }
            if !container.running() {
                continue;
            }
            let points = engine.metrics.session_points(&key);
            if let Some(percent) = sustained(&points, rules, now_ms) {
                findings.push(Finding {
                    rule: Rule::HighMemory,
                    machine: machine.clone(),
                    subject: crate::model::safe_text(&container.name),
                    detail: crate::tf!(
                        "Memory at {percent}% of its limit for {minutes} minutes",
                        percent = percent,
                        minutes = rules.memory_minutes
                    ),
                    target: Some((index, key.clone())),
                    key: format!("{endpoint}#memory/{key}"),
                    count: 1,
                    at_ms: now_ms,
                });
            }
        }
    }
    Evaluation {
        findings,
        past,
        unknown,
    }
}

impl Alerts {
    /// Folds a new evaluation in: same incident, same alert. A key under `unknown` keeps its
    /// state, because nothing can be said about it while its connection is down.
    pub fn update(&mut self, evaluation: Evaluation, now_ms: i64) {
        let Evaluation {
            findings,
            past,
            unknown,
        } = evaluation;
        for (key, alert) in &mut self.open {
            if !alert.active {
                continue;
            }
            let down = unknown.iter().any(|prefix| key.starts_with(prefix));
            alert.not_current = down;
            if !down && !findings.iter().any(|finding| &finding.key == key) {
                alert.active = false;
                alert.last_ms = now_ms;
            }
        }
        for finding in findings {
            let existing = self
                .open
                .iter_mut()
                .find(|(key, alert)| *key == finding.key && alert.active);
            if let Some((_, alert)) = existing {
                alert.finding.count = alert.finding.count.max(finding.count);
                alert.finding.detail = finding.detail;
                alert.finding.target = finding.target;
                alert.finding.at_ms = finding.at_ms;
                alert.last_ms = finding.at_ms.max(alert.last_ms);
                alert.not_current = false;
            } else {
                self.open.push((
                    finding.key.clone(),
                    Alert {
                        first_ms: finding.at_ms,
                        last_ms: finding.at_ms,
                        finding,
                        active: true,
                        not_current: false,
                    },
                ));
            }
        }
        for finding in past {
            let known = self
                .open
                .iter()
                .any(|(key, alert)| *key == finding.key && alert.finding.at_ms == finding.at_ms);
            if !known {
                self.open.push((
                    finding.key.clone(),
                    Alert {
                        first_ms: finding.at_ms,
                        last_ms: finding.at_ms,
                        finding,
                        active: false,
                        not_current: false,
                    },
                ));
            }
        }
        // Active first, then newest. The oldest cleared ones are dropped first.
        self.open.sort_by(|(_, a), (_, b)| {
            (!a.active, std::cmp::Reverse(a.last_ms))
                .cmp(&(!b.active, std::cmp::Reverse(b.last_ms)))
        });
        self.open.truncate(MAX_KEPT);
        self.selected = self.selected.min(self.open.len().saturating_sub(1));
    }

    pub fn active_count(&self) -> usize {
        self.open.iter().filter(|(_, alert)| alert.active).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{BackendKind, Connection},
        model::{Container, Snapshot},
    };

    fn app() -> App {
        let mut app = App::new(
            vec![Connection {
                name: "local".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///fixture".into(),
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
                    name: "api".into(),
                    state: "running".into(),
                    status: "Up 2 hours".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        app
    }

    fn event(key: &str, at_ms: i64, action: &str, detail: &str) -> Event {
        Event {
            at_ms,
            key: key.into(),
            name: "api".into(),
            project: None,
            service: None,
            action: action.into(),
            detail: detail.into(),
        }
    }

    fn run(app: &App, alerts: &mut Alerts, now_ms: i64) {
        alerts.update(evaluate(app, &Rules::default(), now_ms), now_ms);
    }

    const MINUTE: i64 = 60_000;

    #[test]
    fn a_crash_loop_is_one_alert_with_a_rising_count_that_clears_when_it_stops() {
        let mut app = app();
        let key = app.engines[0].snapshot.as_ref().unwrap().containers[0].key();
        let mut alerts = Alerts::default();
        let start = 100 * MINUTE;
        for index in 0..4 {
            app.engines[0]
                .events
                .add(event(&key, start + index, "die", "exit 1"));
        }
        run(&app, &mut alerts, start + 10);
        assert_eq!(alerts.active_count(), 0, "Four exits are below the limit");

        for index in 4..7 {
            app.engines[0]
                .events
                .add(event(&key, start + index, "die", "exit 1"));
            run(&app, &mut alerts, start + 10);
        }
        assert_eq!(alerts.open.len(), 1, "One incident, one alert");
        assert_eq!(alerts.open[0].1.finding.count, 7);
        assert_eq!(alerts.open[0].1.finding.target, Some((0, key.clone())));

        run(&app, &mut alerts, start + 11 * MINUTE);
        assert_eq!(alerts.active_count(), 0);
        assert_eq!(alerts.open.len(), 1, "A cleared alert stays in the list");

        // Coming back is a new incident, not the old one reopened.
        let later = start + 60 * MINUTE;
        for index in 0..5 {
            app.engines[0]
                .events
                .add(event(&key, later + index, "die", "exit 1"));
        }
        run(&app, &mut alerts, later + 10);
        assert_eq!(alerts.open.len(), 2);
        assert_eq!(alerts.active_count(), 1);
    }

    #[test]
    fn clean_exits_are_not_a_crash_loop() {
        let mut app = app();
        let key = app.engines[0].snapshot.as_ref().unwrap().containers[0].key();
        for index in 0..9 {
            app.engines[0]
                .events
                .add(event(&key, index, "die", "exit 0"));
        }
        let mut alerts = Alerts::default();
        run(&app, &mut alerts, 100);
        assert!(alerts.open.is_empty());
    }

    #[test]
    fn out_of_memory_clears_after_its_window_and_unhealthy_follows_the_container() {
        let mut app = app();
        let key = app.engines[0].snapshot.as_ref().unwrap().containers[0].key();
        app.engines[0].events.add(event(&key, 0, "oom", ""));
        let mut alerts = Alerts::default();
        run(&app, &mut alerts, MINUTE);
        assert_eq!(alerts.active_count(), 1);
        run(&app, &mut alerts, 11 * MINUTE);
        assert_eq!(alerts.active_count(), 0);

        let mut sick = app.engines[0].snapshot.clone().unwrap();
        sick.containers[0].health = Some("unhealthy".into());
        sick.containers[0].status = "Up 2 hours (unhealthy)".into();
        app.update_engine(0, Ok(sick.clone()));
        run(&app, &mut alerts, 12 * MINUTE);
        assert_eq!(alerts.active_count(), 1);
        sick.containers[0].health = Some("healthy".into());
        sick.containers[0].status = "Up 2 hours (healthy)".into();
        app.update_engine(0, Ok(sick));
        run(&app, &mut alerts, 13 * MINUTE);
        assert_eq!(alerts.active_count(), 0);
    }

    #[test]
    fn a_down_connection_neither_confirms_nor_clears_an_alert() {
        let mut app = app();
        let mut sick = app.engines[0].snapshot.clone().unwrap();
        sick.containers[0].health = Some("unhealthy".into());
        sick.containers[0].status = "Up 2 hours (unhealthy)".into();
        app.update_engine(0, Ok(sick));
        let mut alerts = Alerts::default();
        run(&app, &mut alerts, 1000);
        app.update_engine(0, Err("gone".into()));
        run(&app, &mut alerts, 2000);
        assert_eq!(alerts.active_count(), 1);
        assert!(alerts.open[0].1.not_current);
    }

    #[test]
    fn high_memory_needs_the_whole_period_covered_and_nothing_missing() {
        let rules = Rules::default();
        let now = 100 * MINUTE;
        let steady = |value: Option<u64>| {
            (0..=6)
                .map(|minutes| (now - minutes * MINUTE, Some(1.0), value))
                .collect::<Vec<_>>()
        };
        assert_eq!(sustained(&steady(Some(95)), &rules, now), Some(95));
        assert_eq!(sustained(&steady(Some(50)), &rules, now), None);
        assert_eq!(sustained(&steady(None), &rules, now), None);
        // High, but not seen for long enough to say it stayed high.
        let short = [(now - MINUTE, Some(1.0), Some(99))];
        assert_eq!(sustained(&short, &rules, now), None);
        // One reading below the limit in the period breaks it.
        let mut dipped = steady(Some(95));
        dipped[2].2 = Some(10);
        assert_eq!(sustained(&dipped, &rules, now), None);
    }

    #[test]
    fn an_incident_that_ended_before_tha_opened_is_listed_once_as_over() {
        let mut app = app();
        let key = app.engines[0].snapshot.as_ref().unwrap().containers[0].key();
        for index in 0..6 {
            app.engines[0]
                .events
                .add(event(&key, index, "die", "exit 1"));
        }
        app.engines[0].events.add(event(&key, 10, "oom", ""));
        let mut alerts = Alerts::default();
        let now = 500 * MINUTE;
        run(&app, &mut alerts, now);
        run(&app, &mut alerts, now + MINUTE);
        assert_eq!(alerts.open.len(), 2, "{:?}", alerts.open);
        assert_eq!(alerts.active_count(), 0);
        assert_eq!(alerts.open[0].1.last_ms, 10);
    }

    #[test]
    fn a_cleared_alert_is_not_listed_a_second_time_from_saved_events() {
        let mut app = app();
        let key = app.engines[0].snapshot.as_ref().unwrap().containers[0].key();
        app.engines[0].events.add(event(&key, 0, "oom", ""));
        let mut alerts = Alerts::default();
        run(&app, &mut alerts, MINUTE);
        run(&app, &mut alerts, 20 * MINUTE);
        run(&app, &mut alerts, 21 * MINUTE);
        assert_eq!(alerts.open.len(), 1);
    }

    #[test]
    fn invalid_thresholds_are_refused() {
        assert!(Rules::default().validate().is_ok());
        for rules in [
            Rules {
                restart_count: 1,
                ..Default::default()
            },
            Rules {
                memory_percent: 0,
                ..Default::default()
            },
            Rules {
                memory_percent: 101,
                ..Default::default()
            },
            Rules {
                oom_minutes: 0,
                ..Default::default()
            },
        ] {
            assert!(rules.validate().is_err(), "{rules:?}");
        }
    }

    #[test]
    fn the_list_is_bounded_and_keeps_active_alerts() {
        let mut alerts = Alerts::default();
        let finding = |index: usize| Finding {
            rule: Rule::Unhealthy,
            machine: "m".into(),
            subject: "s".into(),
            detail: "d".into(),
            target: None,
            key: format!("k{index}"),
            count: 1,
            at_ms: index as i64,
        };
        alerts.update(
            Evaluation {
                findings: (0..MAX_KEPT + 5).map(finding).collect(),
                ..Default::default()
            },
            1,
        );
        assert_eq!(alerts.open.len(), MAX_KEPT);
        assert_eq!(alerts.active_count(), MAX_KEPT);
    }
}
