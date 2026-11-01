//! What one connection can do, and why not when it cannot. It reads what is already known and
//! makes no requests. "Available" means Tha has what the feature needs, not that a run succeeds.
use crate::{app::App, config::BackendKind, machines::Machine, remote};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    /// Possible here, but someone has to turn it on.
    TurnedOff,
    MissingDependency,
    Unsupported,
    /// The connection is down, so nothing can be said about it right now.
    Unreachable,
    /// An older agent does not report this.
    Unknown,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Available => crate::i18n::t("available"),
            Self::TurnedOff => crate::i18n::t("turned off"),
            Self::MissingDependency => crate::i18n::t("missing dependency"),
            Self::Unsupported => crate::i18n::t("not supported"),
            Self::Unreachable => crate::i18n::t("unreachable"),
            Self::Unknown => crate::i18n::t("unknown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub feature: &'static str,
    pub status: Status,
    pub detail: String,
}

fn row(feature: &'static str, status: Status, detail: &str) -> Capability {
    Capability {
        feature,
        status,
        detail: detail.into(),
    }
}

fn available(feature: &'static str, detail: &str) -> Capability {
    row(feature, Status::Available, detail)
}

/// Every feature this view covers, in display order.
fn features() -> [&'static str; 10] {
    [
        crate::i18n::t("Logs"),
        crate::i18n::t("Events"),
        crate::i18n::t("Diagnostics"),
        crate::i18n::t("Shell"),
        crate::i18n::t("Compose actions"),
        crate::i18n::t("Image pull and updates"),
        crate::i18n::t("Deleting resources"),
        crate::i18n::t("Security scans"),
        crate::i18n::t("Metrics history"),
        crate::i18n::t("File browsing and download"),
    ]
}

fn unreachable_rows(detail: &str) -> Vec<Capability> {
    features()
        .into_iter()
        .map(|feature| row(feature, Status::Unreachable, detail))
        .collect()
}

fn docker_rows(app: &App, connection: &crate::config::Connection) -> Vec<Capability> {
    let [
        logs,
        events,
        diagnostics,
        shell,
        compose,
        images,
        delete,
        security,
        history,
        files,
    ] = features();
    let cli = crate::exec::runtime_cli(connection);
    vec![
        available(logs, ""),
        available(events, ""),
        available(diagnostics, ""),
        match &cli {
            Some((name, _)) => available(shell, name),
            None => row(
                shell,
                Status::MissingDependency,
                crate::i18n::t("Install the docker or podman CLI"),
            ),
        },
        if cli.is_some() || crate::exec::find_program("docker-compose").is_some() {
            available(
                compose,
                crate::i18n::t("Needs the Compose plugin or docker-compose; checked when it runs"),
            )
        } else {
            row(
                compose,
                Status::MissingDependency,
                crate::i18n::t("Install Docker Compose"),
            )
        },
        available(images, ""),
        available(delete, ""),
        if app.security.enabled {
            available(security, "")
        } else {
            row(
                security,
                Status::TurnedOff,
                crate::i18n::t("Run: tha module install security"),
            )
        },
        if app.metrics_database.is_some() {
            available(history, crate::i18n::t("Saved by the background service"))
        } else {
            row(
                history,
                Status::TurnedOff,
                crate::i18n::t("Only this session; install service mode to keep it"),
            )
        },
        available(
            files,
            crate::i18n::t("Browsing needs a running container with ls; downloading does not"),
        ),
    ]
}

fn containerd_rows() -> Vec<Capability> {
    let [
        logs,
        events,
        diagnostics,
        shell,
        compose,
        images,
        delete,
        security,
        history,
        files,
    ] = features();
    let nerdctl = crate::exec::find_program("nerdctl").is_some();
    let needs_nerdctl = |feature| {
        if nerdctl {
            available(feature, "")
        } else {
            row(
                feature,
                Status::MissingDependency,
                crate::i18n::t("Install nerdctl"),
            )
        }
    };
    let unsupported = crate::i18n::t("Not available on native containerd");
    vec![
        needs_nerdctl(logs),
        row(events, Status::Unsupported, unsupported),
        row(diagnostics, Status::Unsupported, unsupported),
        needs_nerdctl(shell),
        row(compose, Status::Unsupported, unsupported),
        row(images, Status::Unsupported, unsupported),
        row(delete, Status::Unsupported, unsupported),
        row(security, Status::Unsupported, unsupported),
        row(history, Status::Unsupported, unsupported),
        row(files, Status::Unsupported, unsupported),
    ]
}

fn server_rows(machine: &Machine) -> Vec<Capability> {
    let [
        logs,
        events,
        diagnostics,
        shell,
        compose,
        images,
        delete,
        security,
        history,
        files,
    ] = features();
    let opt_in = |feature, on: Option<bool>, flag: &str| match on {
        Some(true) => available(feature, ""),
        Some(false) => row(
            feature,
            Status::TurnedOff,
            &crate::tf!("Rerun setup on the server with {flag}", flag = flag),
        ),
        None => row(
            feature,
            Status::Unknown,
            crate::i18n::t("This agent does not report it; update tha-agent"),
        ),
    };
    let reported = machine.capabilities.as_ref();
    let agent = match reported.map(|c| c.agent_version.as_str()) {
        Some(version) if crate::update::newer(crate::update::current(), version) => row(
            crate::i18n::t("Agent version"),
            Status::TurnedOff,
            &crate::tf!(
                "{version} is older than this Tha; rerun the setup command on the server",
                version = crate::model::safe_text(version)
            ),
        ),
        Some(version) => available(
            crate::i18n::t("Agent version"),
            &crate::model::safe_text(version),
        ),
        None => row(
            crate::i18n::t("Agent version"),
            Status::Unknown,
            crate::i18n::t("This agent does not report it; update tha-agent"),
        ),
    };
    vec![
        available(logs, ""),
        available(events, ""),
        available(diagnostics, ""),
        opt_in(shell, reported.map(|c| c.allow_exec), "--allow-exec"),
        opt_in(
            compose,
            reported.map(|c| c.allow_compose),
            "--allow-compose",
        ),
        opt_in(images, reported.map(|c| c.allow_images), "--allow-images"),
        opt_in(delete, Some(machine.allow_delete), "--allow-delete"),
        match reported.map(|c| c.security) {
            Some(false) => row(
                security,
                Status::TurnedOff,
                crate::i18n::t("Rerun setup on the server without --no-security"),
            ),
            on => opt_in(security, on, ""),
        },
        match reported.map(|c| c.history) {
            Some(true) => available(history, crate::i18n::t("Saved by the agent for 24 hours")),
            _ => row(
                history,
                Status::Unknown,
                crate::i18n::t("This agent does not report it; update tha-agent"),
            ),
        },
        opt_in(
            files,
            reported.map(|c| c.allow_download),
            "--allow-download",
        ),
        opt_in(
            crate::i18n::t("Uploading into containers"),
            reported.map(|c| c.allow_upload),
            "--allow-upload",
        ),
        opt_in(
            crate::i18n::t("Files on this server"),
            reported.map(|c| c.allow_server_download),
            "--allow-server-download",
        ),
        opt_in(
            crate::i18n::t("Editing Compose files"),
            reported.map(|c| c.allow_compose_edit),
            "--allow-compose-edit",
        ),
        agent,
    ]
}

/// A server row has no engine of its own, so `server` covers it when it is not connected.
pub fn report(app: &App, engine: Option<usize>, server: Option<&str>) -> Vec<Capability> {
    let connection = engine.and_then(|index| app.engines.get(index));
    let name = server.map(str::to_owned).or_else(|| {
        connection
            .and_then(|engine| remote::parse_endpoint(&engine.connection.endpoint))
            .map(|(server, _)| server.to_owned())
    });
    if let Some(machine) = name.as_deref().and_then(|name| app.machine(name)) {
        if !machine.connected() {
            return unreachable_rows(crate::i18n::t("Connect to this server to check"));
        }
        return server_rows(machine);
    }
    let Some(engine) = connection else {
        return unreachable_rows(crate::i18n::t("Select a connection to check"));
    };
    if engine.error.is_some() {
        return unreachable_rows(crate::i18n::t("This connection is offline"));
    }
    match engine.connection.kind {
        BackendKind::Docker => docker_rows(app, &engine.connection),
        BackendKind::Containerd => containerd_rows(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agent::Capabilities, config::Connection, machines::MachineState, remote::Remote};

    fn app(kind: BackendKind) -> App {
        let mut app = App::new(
            vec![Connection {
                name: "fixture".into(),
                kind,
                endpoint: "unix:///fixture".into(),
            }],
            100,
            false,
            false,
        );
        app.engines[0].error = None;
        app
    }

    fn status<'a>(rows: &'a [Capability], feature: &str) -> &'a Capability {
        rows.iter().find(|row| row.feature == feature).unwrap()
    }

    fn server(app: &mut App, capabilities: Option<Capabilities>, allow_delete: bool) {
        app.load_machines(
            vec![Remote {
                name: "prod".into(),
                host: "example.invalid".into(),
                user: "tha".into(),
                identity_file: "/key".into(),
                port: None,
                connect: None,
                metrics_refresh_seconds: None,
            }],
            Ok(Vec::new()),
            None,
        );
        let machine = app.machine_mut("prod").unwrap();
        machine.state = MachineState::Connected;
        machine.allow_delete = allow_delete;
        machine.capabilities = capabilities;
    }

    #[test]
    fn local_docker_explains_what_is_turned_off_and_how_to_enable_it() {
        let app = app(BackendKind::Docker);
        let rows = report(&app, Some(0), None);
        assert_eq!(status(&rows, "Logs").status, Status::Available);
        let security = status(&rows, "Security scans");
        assert_eq!(security.status, Status::TurnedOff);
        assert!(security.detail.contains("module install"));
        assert_eq!(status(&rows, "Metrics history").status, Status::TurnedOff);
    }

    #[test]
    fn native_containerd_reports_unsupported_features_as_unsupported() {
        let rows = report(&app(BackendKind::Containerd), Some(0), None);
        for feature in ["Events", "Compose actions", "Security scans"] {
            assert_eq!(status(&rows, feature).status, Status::Unsupported);
        }
    }

    #[test]
    fn server_opt_ins_come_from_the_agent_and_older_agents_read_as_unknown() {
        let mut app = app(BackendKind::Docker);
        server(&mut app, None, false);
        let rows = report(&app, None, Some("prod"));
        assert_eq!(status(&rows, "Shell").status, Status::Unknown);
        let delete = status(&rows, "Deleting resources");
        assert_eq!(delete.status, Status::TurnedOff);
        assert!(delete.detail.contains("--allow-delete"));

        server(
            &mut app,
            Some(Capabilities {
                allow_exec: true,
                allow_images: false,
                allow_download: false,
                allow_server_download: false,
                allow_compose: false,
                allow_compose_edit: false,
                allow_upload: false,
                security: false,
                history: true,
                agent_version: "1".into(),
            }),
            true,
        );
        let rows = report(&app, None, Some("prod"));
        assert_eq!(status(&rows, "Shell").status, Status::Available);
        assert_eq!(
            status(&rows, "Deleting resources").status,
            Status::Available
        );
        let security = status(&rows, "Security scans");
        assert_eq!(security.status, Status::TurnedOff);
        assert!(security.detail.contains("--no-security"));
    }

    #[test]
    fn a_down_connection_is_unreachable_not_unsupported() {
        let mut offline = app(BackendKind::Docker);
        offline.engines[0].error = Some("gone".into());
        let rows = report(&offline, Some(0), None);
        assert!(rows.iter().all(|row| row.status == Status::Unreachable));

        let mut disconnected = app(BackendKind::Docker);
        server(&mut disconnected, None, true);
        disconnected.machine_mut("prod").unwrap().state = MachineState::NotConnected;
        let rows = report(&disconnected, None, Some("prod"));
        assert!(rows.iter().all(|row| row.status == Status::Unreachable));
    }
}
