//! One list of what needs attention across every connection, built only from what Tha already
//! knows. Something that could not be checked is reported as such, never as fine.
use crate::{
    app::App,
    machines::{LOCAL_MACHINE, MachineState},
    model::Tone,
    remote,
};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Failure,
    Warning,
    /// Unknown, as opposed to a problem or to being fine.
    NotChecked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    /// The server, or `LOCAL_MACHINE`.
    pub machine: String,
    pub subject: String,
    pub detail: String,
    /// A workload to jump to. Servers and runtimes have none.
    pub target: Option<(usize, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub problems: Vec<Problem>,
    /// Connections that answered, so an empty list can say how much it covers.
    pub checked: usize,
}

/// Saved critical findings per image, as read from disk. See `security::saved_critical`.
pub type Vulnerabilities = HashMap<(String, String), usize>;

pub fn summarize(app: &App, vulnerabilities: &Vulnerabilities) -> Summary {
    let mut summary = Summary::default();
    for machine in &app.machines {
        let name = &machine.remote.name;
        let problem = match &machine.state {
            MachineState::Offline { reason, .. } => Some((
                Severity::Failure,
                crate::tf!(
                    "Unreachable: {reason}",
                    reason = crate::model::safe_text(reason)
                ),
            )),
            MachineState::NotConnected => Some((
                Severity::NotChecked,
                crate::i18n::t("Not connected, so not checked").into(),
            )),
            MachineState::Connecting => Some((
                Severity::NotChecked,
                crate::i18n::t("Connecting, so not checked yet").into(),
            )),
            MachineState::Connected => None,
        };
        if let Some((severity, detail)) = problem {
            summary.problems.push(Problem {
                severity,
                machine: name.clone(),
                subject: name.clone(),
                detail,
                target: None,
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
        let subject = parsed.map_or(engine.connection.name.as_str(), |(_, name)| name);
        // A server that is down already has its own row; its engines would only repeat it.
        if parsed.is_some()
            && app
                .machine(&machine)
                .is_none_or(|machine| !machine.connected())
        {
            continue;
        }
        if let Some(error) = &engine.error {
            summary.problems.push(Problem {
                severity: if engine.snapshot.is_some() {
                    Severity::Failure
                } else {
                    Severity::NotChecked
                },
                machine: machine.clone(),
                subject: crate::model::safe_text(subject),
                detail: if engine.snapshot.is_some() {
                    crate::tf!(
                        "Offline, showing old data: {error}",
                        error = crate::model::safe_text(error)
                    )
                } else {
                    crate::tf!("Offline: {error}", error = crate::model::safe_text(error))
                },
                target: None,
            });
            continue;
        }
        let Some(snapshot) = &engine.snapshot else {
            summary.problems.push(Problem {
                severity: Severity::NotChecked,
                machine: machine.clone(),
                subject: crate::model::safe_text(subject),
                detail: crate::i18n::t("Connecting, so not checked yet").into(),
                target: None,
            });
            continue;
        };
        summary.checked += 1;
        for container in &snapshot.containers {
            let condition = container.condition();
            let restarting = condition.label == "restarting";
            if condition.tone == Tone::Error || restarting {
                summary.problems.push(Problem {
                    severity: if restarting {
                        Severity::Warning
                    } else {
                        Severity::Failure
                    },
                    machine: machine.clone(),
                    subject: crate::model::safe_text(&container.name),
                    detail: crate::i18n::dynamic(condition.label).to_owned(),
                    target: Some((index, container.key())),
                });
            }
            if container.running()
                && let Some(image) = &container.image_id
                && let Some(count) = vulnerabilities
                    .get(&(engine.connection.endpoint.clone(), image.clone()))
                    .filter(|count| **count > 0)
            {
                summary.problems.push(Problem {
                    severity: Severity::Warning,
                    machine: machine.clone(),
                    subject: crate::model::safe_text(&container.name),
                    detail: crate::tf!(
                        "{count} critical vulnerabilities in the last saved scan",
                        count = count
                    ),
                    target: Some((index, container.key())),
                });
            }
        }
    }
    summary.problems.sort_by(|a, b| {
        (a.severity, &a.machine, &a.subject).cmp(&(b.severity, &b.machine, &b.subject))
    });
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{BackendKind, Connection},
        model::{Container, Snapshot},
    };

    fn container(name: &str, state: &str, status: &str) -> Container {
        Container {
            id: format!("id-{name}"),
            name: name.into(),
            image_id: Some(format!("image-{name}")),
            state: state.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    fn app(containers: Vec<Container>) -> App {
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
                containers,
                ..Default::default()
            }),
        );
        app
    }

    #[test]
    fn failures_come_first_and_healthy_or_finished_containers_stay_out() {
        let app = app(vec![
            container("fine", "running", "Up 2 hours"),
            container("done", "exited", "Exited (0) 2 hours ago"),
            container("loop", "restarting", "Restarting (1) 2 seconds ago"),
            container("sick", "running", "Up 2 hours (unhealthy)"),
            container("crashed", "exited", "Exited (1) 2 hours ago"),
        ]);
        let summary = summarize(&app, &HashMap::new());
        let subjects: Vec<_> = summary
            .problems
            .iter()
            .map(|p| p.subject.as_str())
            .collect();
        assert_eq!(subjects, ["crashed", "sick", "loop"]);
        assert_eq!(summary.problems[2].severity, Severity::Warning);
        assert_eq!(summary.checked, 1);
        assert!(
            summary
                .problems
                .iter()
                .all(|problem| problem.target.is_some())
        );
    }

    #[test]
    fn saved_critical_findings_appear_only_for_running_containers() {
        let app = app(vec![
            container("web", "running", "Up 2 hours"),
            container("old", "exited", "Exited (0) 2 hours ago"),
            container("clean", "running", "Up 2 hours"),
        ]);
        let endpoint = "unix:///fixture".to_owned();
        let found = HashMap::from([
            ((endpoint.clone(), "image-web".to_owned()), 3),
            ((endpoint.clone(), "image-old".to_owned()), 9),
            ((endpoint, "image-clean".to_owned()), 0),
        ]);
        let summary = summarize(&app, &found);
        assert_eq!(summary.problems.len(), 1);
        assert_eq!(summary.problems[0].subject, "web");
        assert!(summary.problems[0].detail.contains("3 critical"));
    }

    #[test]
    fn what_could_not_be_checked_is_never_reported_as_fine() {
        let mut offline = app(vec![container("fine", "running", "Up")]);
        offline.update_engine(0, Err("gone".into()));
        let summary = summarize(&offline, &HashMap::new());
        assert_eq!(summary.checked, 0);
        assert_eq!(summary.problems[0].severity, Severity::Failure);
        assert!(summary.problems[0].detail.contains("old data"));

        let mut connecting = app(Vec::new());
        connecting.engines[0].snapshot = None;
        let summary = summarize(&connecting, &HashMap::new());
        assert_eq!(summary.checked, 0);
        assert_eq!(summary.problems[0].severity, Severity::NotChecked);
    }

    #[test]
    fn a_down_server_is_one_row_and_a_connected_one_adds_its_engine() {
        let mut app = app(Vec::new());
        crate::demo::populate_servers(&mut app);
        let summary = summarize(&app, &HashMap::new());
        let offline: Vec<_> = summary
            .problems
            .iter()
            .filter(|problem| problem.target.is_none() && problem.severity == Severity::Failure)
            .collect();
        assert!(!offline.is_empty());
        assert!(
            offline
                .iter()
                .all(|problem| problem.subject == problem.machine)
        );
    }
}
