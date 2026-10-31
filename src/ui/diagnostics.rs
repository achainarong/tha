//! The read-only evidence overlay. Observations and possible explanations are drawn in
//! separate sections, and nothing here may turn missing data into a healthy-looking value.
use super::*;
use crate::diagnostics::{
    self, EVIDENCE_PREVIEW, Freshness, HealthCheck, Incomplete, Inspection, Kind, LogCoverage,
    MAX_ATTACHMENTS, NetworkMode, OverlapKind, ProbeSource, Relationship, Side, Status, View,
};
use time::OffsetDateTime;

fn stamp(value: &str) -> String {
    diagnostics::parse_time(value).map_or_else(|| safe_text(value), stamp_at)
}

fn stamp_at(at: OffsetDateTime) -> String {
    let at = at.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        at.year(),
        at.month() as u8,
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

fn stamp_ms(at_ms: i64) -> String {
    OffsetDateTime::from_unix_timestamp(at_ms.div_euclid(1000))
        .map_or_else(|_| "--".into(), stamp_at)
}

fn heading(text: &str) -> Line<'static> {
    Line::styled(
        text.to_owned(),
        Style::default().add_modifier(Modifier::BOLD),
    )
}

fn short_id(id: &str) -> String {
    safe_text(&id.chars().take(12).collect::<String>())
}

fn unavailable() -> &'static str {
    crate::i18n::t("unavailable")
}

fn evidence(side: &Side) -> Option<&Inspection> {
    side.evidence.as_ref().map(|(inspection, _)| inspection)
}

fn header(app: &App, view: &View) -> Vec<Line<'static>> {
    let side = &view.main;
    let target = &side.target;
    let mut lines = vec![Line::from(crate::tf!(
        "Runtime {runtime} · Container {name} · ID {id}",
        runtime = safe_text(&app.engines[target.engine].connection.name),
        name = safe_text(&target.name),
        id = short_id(&target.id)
    ))];
    lines.push(Line::styled(
        match view.kind {
            Kind::Health => crate::i18n::t(
                "Read-only: no health check is run and nothing is changed. Reported evidence, not a verdict.",
            ),
            Kind::Network => crate::i18n::t(
                "Passive: reported configuration only. Nothing is resolved, pinged or probed, so connectivity is not tested.",
            ),
        },
        muted(),
    ));
    lines.extend(status_lines(app, side));
    if let Some(message) = &view.message {
        lines.push(Line::styled(safe_text(message), accent(app, Color::Yellow)));
    }
    lines.push(Line::from(""));
    lines
}

fn status_lines(app: &App, side: &Side) -> Vec<Line<'static>> {
    let warn = accent(app, Color::Yellow);
    let mut lines = Vec::new();
    match &side.status {
        Status::Loading => lines.push(Line::styled(
            crate::i18n::t("Loading from the runtime…"),
            warn,
        )),
        Status::Ready => {}
        Status::Failed(error) => lines.push(Line::styled(
            crate::tf!(
                "Detailed inspection unavailable: {error}",
                error = safe_text(error)
            ),
            warn,
        )),
        Status::Unsupported => lines.push(Line::styled(
            crate::i18n::t(
                "This backend has no detailed inspection; only inventory facts are shown.",
            ),
            warn,
        )),
    }
    if let Some((_, at)) = &side.evidence {
        lines.push(Line::styled(
            crate::tf!("Observed {at}", at = stamp_ms(*at)),
            muted(),
        ));
    }
    let stale = match app.diagnostics_freshness(&side.target) {
        Freshness::Current => None,
        Freshness::EngineOffline => Some(crate::i18n::t(
            "Stale: the engine is offline, so this may no longer be true.",
        )),
        Freshness::RuntimeChanged => Some(crate::i18n::t(
            "Stale: the endpoint now reports a different runtime. This evidence is about the previous one.",
        )),
        Freshness::Removed => Some(crate::i18n::t(
            "Stale: this container ID is gone. A new container with the same name has none of this evidence.",
        )),
    };
    if let Some(stale) = stale {
        lines.push(Line::styled(stale, warn));
    }
    lines
}

fn duration(ns: i64) -> String {
    let ms = ns / 1_000_000;
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let seconds = ms / 1000;
    match (seconds / 60, seconds % 60) {
        (0, seconds) => format!("{seconds}s"),
        (minutes, 0) => format!("{minutes}m"),
        (minutes, seconds) => format!("{minutes}m{seconds}s"),
    }
}

/// Start overrides and the health check as configured, for the Overview tab. Anything not set
/// is left out. Lines only come back for this exact container, so a recreated one never shows
/// its predecessor's.
pub(super) fn overview_lines(
    app: &App,
    engine: usize,
    container: &crate::model::Container,
) -> Vec<Line<'static>> {
    let Some(side) = app.overview_details.as_ref().filter(|side| {
        side.target.engine == engine && side.target.container_key() == container.key()
    }) else {
        return vec![];
    };
    let Some(inspection) = evidence(side) else {
        return vec![];
    };
    let mut lines = Vec::new();
    // Each line of a script gets its own row, continuing under the first.
    let mut command = |first: String, text: &str| {
        let mut rows = text.lines();
        lines.push(Line::from(
            first.replace("{value}", rows.next().unwrap_or("")),
        ));
        lines.extend(rows.map(|row| Line::from(format!("{}{row}", " ".repeat(13)))));
    };
    if let Some(entrypoint) = &inspection.entrypoint {
        command(crate::tf!("  Entrypoint {a0}", a0 = "{value}"), entrypoint);
    }
    if let Some(value) = &inspection.command {
        command(crate::tf!("  Command    {a0}", a0 = "{value}"), value);
    }
    if let Some(probe) = &inspection.probe {
        lines.push(Line::from(match inspection.probe_source {
            ProbeSource::Image => crate::i18n::t("  Healthcheck (from the image)"),
            ProbeSource::Container => {
                crate::i18n::t("  Healthcheck (set on this container, by Compose or run)")
            }
            ProbeSource::Unknown => crate::i18n::t("  Healthcheck"),
        }));
        // Named as in a Compose file, so these are not translated.
        lines.push(Line::from(format!("    test: {}", probe.test)));
        for (name, value) in [
            ("interval", probe.interval_ns),
            ("timeout", probe.timeout_ns),
            ("start_period", probe.start_period_ns),
        ] {
            if let Some(value) = value {
                lines.push(Line::from(format!("    {name}: {}", duration(value))));
            }
        }
        if let Some(retries) = probe.retries {
            lines.push(Line::from(format!("    retries: {retries}")));
        }
    }
    if let HealthCheck::Configured {
        failing_streak,
        attempts,
        ..
    } = &inspection.health
    {
        if let Some(streak) = failing_streak.filter(|streak| *streak > 0) {
            lines.push(Line::styled(
                crate::tf!("  Failing    {count} checks in a row", count = streak),
                accent(app, Color::Red),
            ));
        }
        if let Some(attempt) = attempts.first() {
            let at = attempt
                .ended_at
                .as_deref()
                .or(attempt.started_at.as_deref());
            let code = attempt.exit_code;
            lines.push(Line::styled(
                crate::tf!(
                    "  Last check {at} · exit {code}",
                    at = at.map_or_else(|| crate::i18n::t("time unavailable").to_owned(), stamp),
                    code = code.map_or_else(|| unavailable().to_owned(), |code| code.to_string())
                ),
                accent(
                    app,
                    match code {
                        Some(0) => Color::Green,
                        Some(_) => Color::Red,
                        None => Color::Yellow,
                    },
                ),
            ));
            if let Some(output) = attempt.output.lines().find(|line| !line.trim().is_empty()) {
                lines.push(Line::from(crate::tf!("  Output     {a0}", a0 = output)));
            }
        }
    }
    lines
}

fn health_lines(app: &App, view: &View) -> Vec<Line<'static>> {
    let side = &view.main;
    let inspection = evidence(side);
    let mut lines = header(app, view);

    lines.push(heading(crate::i18n::t("Current state")));
    let state = inspection
        .and_then(|inspection| inspection.state.clone())
        .unwrap_or_else(|| side.container.state.clone());
    lines.push(Line::from(crate::tf!(
        "  State      {state}",
        state = safe_text(crate::i18n::dynamic(&state))
    )));
    if !side.container.status.is_empty() {
        lines.push(Line::from(crate::tf!(
            "  Inventory  {status}",
            status = safe_text(&side.container.status)
        )));
    }
    let error = inspection
        .and_then(|inspection| inspection.error.clone())
        .or_else(|| {
            side.container
                .error
                .clone()
                .filter(|error| !error.is_empty())
        });
    if let Some(error) = error {
        lines.push(Line::styled(
            crate::tf!("  Error      {error}", error = safe_text(&error)),
            accent(app, Color::Red),
        ));
    }
    let events: Vec<&crate::events::Event> = app.engines[side.target.engine]
        .events
        .newest_first()
        .filter(|event| event.key == side.target.container_key())
        .collect();
    // Between crashes a looping container looks fine, so timed exits overrule the state.
    if inspection.is_some_and(diagnostics::no_current_failure)
        && diagnostics::possible_restart_loop(&events).is_none()
    {
        lines.push(Line::styled(
            crate::i18n::t("  No current failure is reported. History below may still be useful."),
            accent(app, Color::Green),
        ));
    }
    lines.push(Line::from(""));

    lines.extend(health_check_section(app, side));
    lines.extend(exit_section(app, side));
    lines.extend(explanations_section(inspection, &events));
    lines.extend(events_section(app, &events));
    lines.extend(logs_section(app, side, inspection));
    lines.push(Line::styled(
        crate::i18n::t("Evidence stays on this machine and is not saved or exported by this view."),
        muted(),
    ));
    lines
}

fn health_check_section(app: &App, side: &Side) -> Vec<Line<'static>> {
    let mut lines = vec![heading(crate::i18n::t("Health checks"))];
    let unknown = HealthCheck::Unknown;
    let check = evidence(side).map_or(&unknown, |inspection| &inspection.health);
    match check {
        HealthCheck::NotConfigured => lines.push(Line::from(crate::i18n::t(
            "  No health check is configured, so none has passed or failed.",
        ))),
        HealthCheck::Unknown => {
            let status = side
                .container
                .health
                .as_deref()
                .map_or(unavailable(), crate::i18n::dynamic);
            lines.push(Line::from(crate::tf!(
                "  Status {status} (inventory). Check details are unknown.",
                status = safe_text(status)
            )));
        }
        HealthCheck::Configured {
            status,
            failing_streak,
            attempts,
            omitted,
        } => {
            lines.push(Line::from(crate::tf!(
                "  Status {status} · failing streak {streak}",
                status = status
                    .as_deref()
                    .map_or(unavailable(), crate::i18n::dynamic),
                streak = failing_streak
                    .map_or_else(|| unavailable().to_owned(), |streak| streak.to_string())
            )));
            lines.push(Line::styled(
                crate::i18n::t("  The check command and schedule are on the Overview tab (1)."),
                muted(),
            ));
            if attempts.is_empty() {
                lines.push(Line::from(crate::i18n::t("  No attempts reported yet.")));
            }
            for attempt in attempts {
                let at = attempt
                    .ended_at
                    .as_deref()
                    .or(attempt.started_at.as_deref());
                let code = attempt.exit_code;
                lines.push(Line::styled(
                    crate::tf!(
                        "  {at} · exit {code}",
                        at =
                            at.map_or_else(|| crate::i18n::t("time unavailable").to_owned(), stamp),
                        code =
                            code.map_or_else(|| unavailable().to_owned(), |code| code.to_string())
                    ),
                    accent(
                        app,
                        if code == Some(0) {
                            Color::Green
                        } else if code.is_some() {
                            Color::Red
                        } else {
                            Color::Yellow
                        },
                    ),
                ));
                for line in attempt.output.lines() {
                    lines.push(Line::from(format!("    {line}")));
                }
                if attempt.truncated {
                    lines.push(Line::styled(
                        crate::i18n::t("    … output truncated at 4 KiB"),
                        muted(),
                    ));
                }
            }
            if *omitted > 0 {
                lines.push(Line::styled(
                    crate::tf!("  {omitted} older attempts not shown", omitted = omitted),
                    muted(),
                ));
            }
        }
    }
    lines.push(Line::from(""));
    lines
}

fn exit_section(app: &App, side: &Side) -> Vec<Line<'static>> {
    let mut lines = vec![heading(crate::i18n::t("Exit, OOM and restarts"))];
    let inspection = evidence(side);
    let finished = inspection.and_then(|inspection| inspection.finished_at.as_deref());
    let restarts = inspection.and_then(|inspection| inspection.restart_count);
    lines.push(Line::from(match restarts {
        Some(count) => crate::tf!(
            "  Restart count {count} (a total; it does not show a restart loop)",
            count = count
        ),
        None => crate::tf!("  Restart count {count}", count = unavailable()),
    }));
    let running = inspection.is_some_and(Inspection::running);
    let exit = inspection
        .and_then(|inspection| inspection.exit_code)
        .or(side.container.exit_code);
    if running && exit.is_none_or(|code| code == 0) {
        // The runtime clears these on start, so a 0 here says nothing about the last crash.
        lines.push(Line::from(crate::i18n::t(
            "  Exit code: none for the current run. It is cleared on start, so look at the events for earlier exits.",
        )));
        if let Some(at) = finished {
            lines.push(Line::from(crate::tf!(
                "  Previous run ended {at}",
                at = stamp(at)
            )));
        }
    } else {
        lines.push(Line::from(match (exit, finished) {
            (Some(code), Some(at)) => crate::tf!(
                "  Last exit code {code} at {at} (history, not necessarily the current cause)",
                code = code,
                at = stamp(at)
            ),
            (Some(code), None) => {
                crate::tf!(
                    "  Last exit code {code} · no exit time reported",
                    code = code
                )
            }
            (None, _) => crate::tf!("  Last exit code {code}", code = unavailable()),
        }));
    }
    // The inventory flag is false when a backend never reports it, so only inspect can say "no".
    let oom = inspection
        .and_then(|inspection| inspection.oom_killed)
        .or(side.container.oom_killed.then_some(true));
    match oom {
        Some(true) => lines.push(Line::styled(
            match finished {
                Some(at) => crate::tf!(
                    "  OOM flag set for the run that ended {at} (history; not a proven cause)",
                    at = stamp(at)
                ),
                None => {
                    crate::i18n::t("  OOM flag set; time unavailable (not a proven cause)").into()
                }
            },
            accent(app, Color::Red),
        )),
        Some(false) if running => lines.push(Line::from(crate::i18n::t(
            "  OOM flag not set for the current run; it is cleared on start",
        ))),
        Some(false) => lines.push(Line::from(crate::i18n::t("  OOM flag not set"))),
        None => lines.push(Line::from(crate::tf!(
            "  OOM flag {value}",
            value = unavailable()
        ))),
    }
    if let Some(at) = inspection.and_then(|inspection| inspection.started_at.as_deref()) {
        lines.push(Line::from(crate::tf!(
            "  Last started {at}",
            at = stamp(at)
        )));
    }
    lines.push(Line::from(""));
    lines
}

fn explanations_section(
    inspection: Option<&Inspection>,
    events: &[&crate::events::Event],
) -> Vec<Line<'static>> {
    let mut lines = vec![heading(crate::i18n::t(
        "Possible explanations (not proven)",
    ))];
    let mut found = false;
    if let Some(count) = diagnostics::possible_restart_loop(events) {
        found = true;
        lines.push(Line::from(crate::tf!(
            "  Possible restart loop: {count} timed exits within 10 minutes in the available events.",
            count = count
        )));
    }
    let failure = inspection.and_then(diagnostics::failure_time);
    let oom = events
        .iter()
        .find(|event| event.action == "oom")
        .filter(|event| {
            failure.is_some_and(|failure| {
                i128::from(event.at_ms) * 1_000_000 < failure.unix_timestamp_nanos()
            })
        });
    if let (Some(oom), Some(failure)) = (oom, failure) {
        found = true;
        lines.push(Line::from(crate::tf!(
            "  An OOM event at {oom} came before the failure at {failure}. The order alone does not prove a cause.",
            oom = stamp_ms(oom.at_ms),
            failure = stamp_at(failure)
        )));
    }
    if !found {
        lines.push(Line::from(crate::i18n::t(
            "  Nothing in the available evidence suggests one.",
        )));
    }
    lines.push(Line::from(""));
    lines
}

fn events_section(app: &App, events: &[&crate::events::Event]) -> Vec<Line<'static>> {
    let mut lines = vec![heading(
        crate::tf!(
            "Related events ({shown} of {total} · e opens Events)",
            shown = events.len().min(EVIDENCE_PREVIEW),
            total = events.len()
        )
        .as_str(),
    )];
    lines.push(Line::styled(
        if app.metrics_database.is_some() {
            crate::i18n::t("  Coverage: last 24 hours, saved by the service")
        } else {
            crate::i18n::t("  Coverage: only what the engine retained and Tha saw while open")
        },
        muted(),
    ));
    if events.is_empty() {
        lines.push(Line::from(crate::i18n::t(
            "  No events available for this container ID.",
        )));
    }
    for event in events.iter().take(EVIDENCE_PREVIEW) {
        lines.push(Line::from(vec![
            Span::styled(format!("  {}  ", stamp_ms(event.at_ms)), muted()),
            Span::styled(event.label(), super::events::tone_style(app, event.tone())),
        ]));
    }
    lines.push(Line::from(""));
    lines
}

fn logs_section(app: &App, side: &Side, inspection: Option<&Inspection>) -> Vec<Line<'static>> {
    let (coverage, entries) = diagnostics::related_logs(
        &app.logs,
        app.logs_cover(&side.target),
        inspection.and_then(diagnostics::failure_time),
    );
    let mut lines = vec![heading(crate::i18n::t("Related logs (l opens Logs)"))];
    lines.push(Line::styled(
        match coverage {
            LogCoverage::NotRetained => crate::i18n::t(
                "  No logs are retained for exactly this container yet. Opening this view does not load them; l does.",
            )
            .to_owned(),
            LogCoverage::Recent => crate::i18n::t(
                "  Recent available logs. No failure time is known, so they are not tied to a failure.",
            )
            .to_owned(),
            LogCoverage::NearFailure(at) => crate::tf!(
                "  Timestamped within 60 s before or 10 s after the failure at {at}.",
                at = stamp_at(at)
            ),
            LogCoverage::NothingNearFailure(at) => crate::tf!(
                "  No retained log has a timestamp near the failure at {at}.",
                at = stamp_at(at)
            ),
        },
        muted(),
    ));
    let hidden = entries
        .iter()
        .filter(|entry| !app.log_view.filter.matches(entry))
        .count();
    if hidden > 0 {
        lines.push(Line::styled(
            crate::tf!(
                "  {hidden} of these are hidden in Logs by the active filter ({filter}).",
                hidden = hidden,
                filter = app.log_filter_summary()
            ),
            accent(app, Color::Yellow),
        ));
    }
    for entry in entries {
        lines.push(Line::from(format!("  {}", safe_text(&entry.text))));
    }
    lines.push(Line::from(""));
    lines
}

fn network_lines(app: &App, view: &View) -> Vec<Line<'static>> {
    let mut lines = header(app, view);
    lines.extend(mode_section(app, &view.main));
    lines.extend(membership_section(app, &view.main));
    lines.extend(ports_section(app, &view.main));
    lines.extend(peer_section(app, view));
    lines
}

/// Runtime-reported container references can point outside the workspace.
fn sharing_label(app: &App, side: &Side, reference: &str) -> String {
    let containers = app.engines[side.target.engine]
        .snapshot
        .iter()
        .flat_map(|snapshot| &snapshot.containers);
    let visible = containers
        .filter(|container| container.id == reference || container.name == reference)
        .find(|container| {
            app.project
                .as_ref()
                .is_none_or(|project| project.contains(container))
        });
    match visible {
        Some(container) => safe_text(&container.name),
        None if app.project.is_some() => crate::i18n::t("a container outside this project").into(),
        None => short_id(reference),
    }
}

fn mode_section(app: &App, side: &Side) -> Vec<Line<'static>> {
    let mut lines = vec![heading(crate::i18n::t("Network mode"))];
    let mode = evidence(side).and_then(|inspection| inspection.network_mode.as_deref());
    lines.push(Line::from(match mode.map(NetworkMode::parse) {
        None => crate::tf!("  Mode {mode}", mode = unavailable()),
        Some(NetworkMode::Host) => crate::i18n::t(
            "  host: uses the runtime host's network stack, so it has no ordinary network membership or container address.",
        )
        .into(),
        Some(NetworkMode::None) => crate::i18n::t("  none: no network is configured for this container.").into(),
        Some(NetworkMode::Container(reference)) => crate::tf!(
            "  container: shares the network stack of {other}.",
            other = sharing_label(app, side, &reference)
        ),
        Some(NetworkMode::Other(mode)) => crate::tf!("  {mode}", mode = safe_text(&mode)),
    }));
    lines.push(Line::from(""));
    lines
}

fn attachment_lines(app: &App, side: &Side, network: &str) -> Vec<Line<'static>> {
    let Some(snapshot) = &app.engines[side.target.engine].snapshot else {
        return vec![];
    };
    let Some(resource) = snapshot
        .networks
        .iter()
        .find(|resource| resource.name == network)
    else {
        return vec![];
    };
    let mut visible = Vec::new();
    let mut hidden = 0;
    for name in &resource.containers {
        let allowed = app.project.as_ref().is_none_or(|project| {
            snapshot
                .containers
                .iter()
                .any(|container| &container.name == name && project.contains(container))
        });
        if !allowed {
            hidden += 1;
        } else if name != &side.target.name {
            visible.push(safe_text(name));
        }
    }
    visible.sort();
    let more = visible.len().saturating_sub(MAX_ATTACHMENTS);
    visible.truncate(MAX_ATTACHMENTS);
    let mut lines = Vec::new();
    if !visible.is_empty() {
        lines.push(Line::from(crate::tf!(
            "    Also attached: {names}",
            names = visible.join(", ")
        )));
    }
    if more > 0 {
        lines.push(Line::styled(
            crate::tf!("    {more} more attached", more = more),
            muted(),
        ));
    }
    if hidden > 0 {
        lines.push(Line::styled(
            crate::tf!(
                "    {hidden} attachments outside this project (hidden)",
                hidden = hidden
            ),
            muted(),
        ));
    }
    lines
}

fn membership_section(app: &App, side: &Side) -> Vec<Line<'static>> {
    let mut lines = vec![heading(crate::i18n::t("Networks and addresses"))];
    let endpoints = evidence(side).and_then(|inspection| inspection.endpoints.as_ref());
    match endpoints {
        Some(endpoints) => {
            if endpoints.is_empty() {
                lines.push(Line::from(crate::i18n::t(
                    "  No network attachments reported.",
                )));
            }
            for endpoint in endpoints {
                lines.push(Line::styled(
                    crate::tf!(
                        "  {network} · ID {id}",
                        network = endpoint.network,
                        id = endpoint
                            .id
                            .as_deref()
                            .map_or_else(|| crate::i18n::t("unknown").to_owned(), short_id)
                    ),
                    accent(app, Color::Cyan),
                ));
                lines.push(Line::from(crate::tf!(
                    "    IPv4 {ipv4} · IPv6 {ipv6}",
                    ipv4 = endpoint.ipv4.as_deref().unwrap_or(unavailable()),
                    ipv6 = endpoint.ipv6.as_deref().unwrap_or(unavailable())
                )));
                if endpoint.aliases.is_empty() {
                    lines.push(Line::from(crate::tf!(
                        "    Reported aliases: {value}",
                        value = unavailable()
                    )));
                } else {
                    lines.push(Line::from(crate::tf!(
                        "    Reported aliases: {aliases} (resolution not tested)",
                        aliases = endpoint.aliases.join(", ")
                    )));
                }
                if endpoint.omitted_aliases > 0 {
                    lines.push(Line::styled(
                        crate::tf!(
                            "    {omitted} more aliases not shown",
                            omitted = endpoint.omitted_aliases
                        ),
                        muted(),
                    ));
                }
                lines.extend(attachment_lines(app, side, &endpoint.network));
            }
            let omitted = evidence(side).map_or(0, |inspection| inspection.omitted_endpoints);
            if omitted > 0 {
                lines.push(Line::styled(
                    crate::tf!("  {omitted} more networks not shown", omitted = omitted),
                    muted(),
                ));
            }
        }
        None => {
            if side.container.networks.is_empty() {
                lines.push(Line::from(crate::i18n::t("  Membership is unknown.")));
            }
            for network in &side.container.networks {
                lines.push(Line::styled(
                    crate::tf!(
                        "  {network} · ID unknown · addresses unavailable",
                        network = safe_text(network)
                    ),
                    accent(app, Color::Cyan),
                ));
                lines.extend(attachment_lines(app, side, network));
            }
        }
    }
    lines.push(Line::from(""));
    lines
}

fn ports_section(app: &App, side: &Side) -> Vec<Line<'static>> {
    let container = &side.container;
    let mut lines = vec![heading(crate::i18n::t("Exposed and published ports"))];
    if container.port_bindings.is_empty() && container.ports.is_empty() {
        lines.push(Line::from(crate::i18n::t("  No ports reported.")));
    }
    for binding in &container.port_bindings {
        lines.push(Line::from(format!("  {}", safe_text(&binding.overview()))));
    }
    if container.port_bindings.is_empty() {
        for port in &container.ports {
            lines.push(Line::from(crate::tf!(
                "  {port} (reported)",
                port = safe_text(port)
            )));
        }
    }
    if diagnostics::published(&container.port_bindings)
        .next()
        .is_some()
    {
        lines.push(Line::styled(
            crate::i18n::t(
                "  Addresses are on the runtime host. They are not verified from this workstation.",
            ),
            muted(),
        ));
        if !container.running() {
            lines.push(Line::styled(
                crate::i18n::t("  This container is not running, so these bindings are configuration, not active."),
                accent(app, Color::Yellow),
            ));
        }
    }
    lines.extend(overlap_lines(app, side));
    lines.push(Line::from(""));
    lines
}

fn overlap_lines(app: &App, side: &Side) -> Vec<Line<'static>> {
    let Some(snapshot) = &app.engines[side.target.engine].snapshot else {
        return vec![];
    };
    let others: Vec<&crate::model::Container> = snapshot.containers.iter().collect();
    let mut lines = Vec::new();
    let mut hidden = 0;
    for overlap in diagnostics::publication_overlaps(&side.container, &others) {
        let visible = app.project.as_ref().is_none_or(|project| {
            snapshot
                .containers
                .iter()
                .any(|container| container.key() == overlap.other && project.contains(container))
        });
        if !visible {
            hidden += 1;
            continue;
        }
        let text = match overlap.kind {
            OverlapKind::Possible => crate::tf!(
                "  Possible overlap, not confirmed: {other} also reports host port {port}/{protocol}.",
                other = safe_text(&overlap.other_name),
                port = overlap.port,
                protocol = safe_text(&overlap.protocol)
            ),
            OverlapKind::Uncertain => crate::tf!(
                "  Uncertain overlap: {other} also reports host port {port}/{protocol}. It depends on the address family or an unreported address.",
                other = safe_text(&overlap.other_name),
                port = overlap.port,
                protocol = safe_text(&overlap.protocol)
            ),
        };
        lines.push(Line::styled(text, accent(app, Color::Yellow)));
        if !overlap.other_running {
            lines.push(Line::styled(
                crate::i18n::t("    That container is not running, so its binding is configuration, not active."),
                muted(),
            ));
        }
    }
    if hidden > 0 {
        lines.push(Line::styled(
            crate::tf!(
                "  {hidden} possible port overlaps with containers outside this project (hidden)",
                hidden = hidden
            ),
            accent(app, Color::Yellow),
        ));
    }
    if !lines.is_empty() {
        lines.push(Line::styled(
            crate::i18n::t("  Host processes are not checked, and no bind failure is implied."),
            muted(),
        ));
    }
    lines
}

fn peer_section(app: &App, view: &View) -> Vec<Line<'static>> {
    let mut lines = vec![heading(crate::i18n::t(
        "Peer comparison (p chooses a peer)",
    ))];
    let Some(peer) = &view.peer else {
        lines.push(Line::from(crate::i18n::t(
            "  No peer chosen. Choose a container on this runtime to compare network identities.",
        )));
        return lines;
    };
    lines.push(Line::from(crate::tf!(
        "  Peer {name} · ID {id}",
        name = safe_text(&peer.target.name),
        id = short_id(&peer.target.id)
    )));
    lines.extend(status_lines(app, peer).into_iter().map(|mut line| {
        line.spans.insert(0, Span::raw("  "));
        line
    }));
    if peer.status != Status::Loading {
        let relationship = diagnostics::compare(
            &view.main.target,
            evidence(&view.main),
            &peer.target,
            evidence(peer),
        );
        lines.extend(relationship_lines(app, &relationship));
    }
    lines.push(Line::from(""));
    lines.extend(mode_section(app, peer));
    lines.extend(membership_section(app, peer));
    lines
}

fn relationship_lines(app: &App, relationship: &Relationship) -> Vec<Line<'static>> {
    let warn = accent(app, Color::Yellow);
    let same_name = |names: &[String]| {
        names.iter().map(|name| {
            Line::styled(
                crate::tf!(
                    "  Both report a network named {name}, but with different IDs. It is not the same network.",
                    name = name
                ),
                accent(app, Color::Yellow),
            )
        }).collect::<Vec<_>>()
    };
    match relationship {
        Relationship::Shared {
            networks,
            same_name_only,
        } => {
            let mut lines = vec![Line::from(crate::tf!(
                "  Shared network: {networks}. This is configuration; connectivity has not been tested.",
                networks = networks.join(", ")
            ))];
            lines.extend(same_name(same_name_only));
            lines
        }
        Relationship::NoShared { same_name_only } => {
            let mut lines = vec![Line::from(crate::i18n::t(
                "  No shared network is reported. This does not rule out published-port, host-network, routed or external paths.",
            ))];
            lines.extend(same_name(same_name_only));
            lines
        }
        Relationship::Incomplete(reason) => vec![Line::styled(
            match reason {
                Incomplete::MissingEndpoints => crate::i18n::t(
                    "  Comparison incomplete: network details are missing for at least one container.",
                ),
                Incomplete::MissingIds => crate::i18n::t(
                    "  Comparison incomplete: a network ID is missing, and names do not prove identity.",
                ),
                Incomplete::SpecialMode => crate::i18n::t(
                    "  Comparison incomplete: host, none or container-sharing mode is not described by network membership.",
                ),
                Incomplete::DifferentRuntime => crate::i18n::t(
                    "  Comparison incomplete: the two containers are not known to be on the same runtime.",
                ),
            },
            warn,
        )],
    }
}

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(view) = &app.diagnostics else {
        return;
    };
    let kind = view.kind;
    let lines = match kind {
        Kind::Health => health_lines(app, view),
        Kind::Network => network_lines(app, view),
    };
    let dialog = centered(area, 110, area.height.saturating_sub(2));
    frame.render_widget(Clear, dialog);
    let block = Block::bordered()
        .title(match kind {
            Kind::Health => crate::i18n::t(" Why is this unhealthy? "),
            Kind::Network => crate::i18n::t(" Troubleshoot network "),
        })
        .title_bottom(match kind {
            Kind::Health => crate::i18n::t(
                " j/k Scroll · r Refresh · l Logs · e Events · m Metrics · Esc Close ",
            ),
            Kind::Network => {
                crate::i18n::t(" j/k Scroll · r Refresh · p Peer · n Networks · Esc Close ")
            }
        });
    let inner = padded(block.inner(dialog));
    frame.render_widget(block, dialog);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let maximum = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize);
    let Some(view) = &mut app.diagnostics else {
        return;
    };
    view.scroll = view.scroll.min(maximum).min(u16::MAX as usize);
    frame.render_widget(paragraph.scroll((view.scroll as u16, 0)), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        diagnostics::Endpoint,
        model::{Container, PortBinding},
        workspace::Project,
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn demo() -> App {
        let mut app = App::new(crate::demo::connections(), 100, true, true);
        crate::demo::populate(&mut app);
        app.demo = true;
        app
    }

    fn expand_all(app: &mut App) {
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
                return;
            }
        }
    }

    fn select(app: &mut App, key: &str) {
        expand_all(app);
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(&row.scope, Scope::Container(found) if found == key))
            .unwrap_or_else(|| panic!("No row for {key}"));
    }

    fn text(app: &App, kind: Kind) -> String {
        let view = app.diagnostics.as_ref().unwrap();
        assert_eq!(view.kind, kind);
        let lines = match kind {
            Kind::Health => health_lines(app, view),
            Kind::Network => network_lines(app, view),
        };
        lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn open(app: &mut App, key: &str, kind: Kind) -> String {
        select(app, key);
        app.open_diagnostics(kind);
        text(app, kind)
    }

    #[test]
    fn unhealthy_view_separates_observations_from_possible_explanations() {
        let mut app = demo();
        let output = open(&mut app, "/api-2", Kind::Health);
        for expected in [
            "Container shop-api-2",
            "Current state",
            "Health checks",
            "Status unhealthy · failing streak 3",
            "Connection refused",
            "Exit, OOM and restarts",
            "Restart count 1 (a total; it does not show a restart loop)",
            "Exit code: none for the current run",
            "Previous run ended",
            "OOM flag not set for the current run",
            "Possible explanations (not proven)",
            "Related events",
            "Related logs",
            "The check command and schedule are on the Overview tab",
        ] {
            assert!(output.contains(expected), "{expected}\n{output}");
        }
        assert!(!output.contains("No current failure"));
        assert!(!output.contains("Possible restart loop"), "{output}");
    }

    #[test]
    fn a_stopped_containers_exit_is_labeled_as_history_and_never_as_the_cause() {
        let mut app = demo();
        select(&mut app, "/api-2");
        app.open_diagnostics(Kind::Health);
        let view = app.diagnostics.as_mut().unwrap();
        let (inspection, _) = view.main.evidence.as_mut().unwrap();
        inspection.state = Some("exited".into());
        inspection.exit_code = Some(137);
        inspection.oom_killed = Some(true);
        let output = text(&app, Kind::Health);
        assert!(output.contains("Last exit code 137 at"), "{output}");
        assert!(output.contains("history, not necessarily the current cause"));
        assert!(output.contains("not a proven cause"), "{output}");
    }

    #[test]
    fn timed_exits_overrule_a_crash_looping_containers_clean_looking_state() {
        let mut app = demo();
        let now = crate::history::now_ms();
        for ago in [10_000, 70_000, 130_000] {
            app.engines[0].events.add(crate::events::Event {
                at_ms: now - ago,
                key: "/api-1".into(),
                name: "shop-api-1".into(),
                project: Some("shop".into()),
                service: Some("api".into()),
                action: "die".into(),
                detail: "exit 137".into(),
            });
        }
        let output = open(&mut app, "/api-1", Kind::Health);
        assert!(output.contains("Possible restart loop"), "{output}");
        assert!(!output.contains("No current failure"), "{output}");
        assert!(output.contains("timed exits within 10 minutes"));
    }

    fn overview(app: &mut App) -> String {
        app.tab = Tab::Overview;
        app.update_overview_details();
        let mut terminal = Terminal::new(TestBackend::new(170, 45)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn overview_shows_the_health_check_as_configured_with_its_last_result() {
        let mut app = demo();
        select(&mut app, "/api-2");
        let output = overview(&mut app);
        for expected in [
            "Health     unhealthy",
            "Command    node server.js --port 3000",
            "Healthcheck (set on this container, by Compose or run)",
            r#"test: ["CMD-SHELL", "curl -f http://localhost:3000/health"]"#,
            "interval: 30s",
            "timeout: 5s",
            "start_period: 10s",
            "retries: 3",
            "Failing    3 checks in a row",
            "· exit 1",
            "Output     curl: (7) Failed to connect",
        ] {
            assert!(output.contains(expected), "{expected}");
        }
    }

    #[test]
    fn a_long_command_wraps_on_the_overview_rather_than_being_cut_off() {
        let mut app = demo();
        select(&mut app, "/api-2");
        overview(&mut app);
        let side = app.overview_details.as_mut().unwrap();
        let (inspection, _) = side.evidence.as_mut().unwrap();
        inspection.command = Some(format!("{} ENDOFCOMMAND", "word ".repeat(60)));
        assert!(overview(&mut app).contains("ENDOFCOMMAND"));
    }

    #[test]
    fn a_multi_line_command_gets_one_row_per_line_under_its_value() {
        let mut app = demo();
        select(&mut app, "/api-2");
        overview(&mut app);
        let side = app.overview_details.as_mut().unwrap();
        let (inspection, _) = side.evidence.as_mut().unwrap();
        inspection.command = Some("-c 'first line\nsecond line'".into());
        let lines = overview_lines(&app, 0, &side_container(&app));
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert!(
            text.contains(&"  Command    -c 'first line".to_owned()),
            "{text:?}"
        );
        assert!(
            text.contains(&"             second line'".to_owned()),
            "{text:?}"
        );
        assert!(!text.iter().any(|row| row.contains("\\n")), "{text:?}");
    }

    fn side_container(app: &App) -> crate::model::Container {
        app.overview_details.as_ref().unwrap().container.clone()
    }

    #[test]
    fn overview_leaves_out_settings_and_commands_that_were_not_set() {
        let mut app = demo();
        select(&mut app, "/db-1");
        let output = overview(&mut app);
        assert!(output.contains("Healthcheck (from the image)"));
        assert!(output.contains(r#"test: ["CMD", "pg_isready", "-U", "postgres"]"#));
        assert!(output.contains("interval: 10s"));
        for absent in [
            "timeout:",
            "start_period:",
            "Failing  ",
            "Command  ",
            "Entrypoint ",
        ] {
            assert!(!output.contains(absent), "{absent}");
        }
        select(&mut app, "/scratch");
        let output = overview(&mut app);
        for absent in [
            "Healthcheck",
            "test:",
            "Last check",
            "Health  ",
            "Command  ",
        ] {
            assert!(!output.contains(absent), "{absent}");
        }
    }

    #[test]
    fn overview_inspects_only_the_selected_container_and_only_while_it_is_shown() {
        let mut app = demo();
        app.demo = false;
        select(&mut app, "/api-2");
        app.tab = Tab::Logs;
        app.update_overview_details();
        assert!(app.diagnostics_requests.is_empty(), "Overview is not open");

        app.tab = Tab::Overview;
        for _ in 0..5 {
            app.update_overview_details();
        }
        assert_eq!(
            app.diagnostics_requests.len(),
            1,
            "Frames do not re-inspect"
        );
        assert_eq!(app.diagnostics_requests[0].target.id, "api-2");
        assert_eq!(
            app.diagnostics_requests[0].role,
            diagnostics::Role::Overview
        );
        app.diagnostics_requests.clear();

        // A new selection replaces the old details at once.
        select(&mut app, "/db-1");
        app.update_overview_details();
        assert_eq!(app.diagnostics_requests.len(), 1);
        assert_eq!(app.diagnostics_requests[0].target.id, "db-1");
        app.diagnostics_requests.clear();

        // A group has no single container, so nothing is inspected or kept.
        app.selected = app
            .rows
            .iter()
            .position(|row| matches!(row.scope, Scope::Engine))
            .unwrap();
        app.update_overview_details();
        assert!(app.diagnostics_requests.is_empty());
        assert!(app.overview_details.is_none());
    }

    #[test]
    fn overview_never_shows_details_from_a_replaced_container_and_loading_is_not_blocking() {
        let mut app = demo();
        select(&mut app, "/api-2");
        app.tab = Tab::Overview;
        app.update_overview_details();
        for container in &mut app.engines[0].snapshot.as_mut().unwrap().containers {
            if container.id == "api-2" {
                container.id = "api-2-new".into();
            }
        }
        app.refresh_tree();
        select(&mut app, "/api-2-new");
        app.demo = false;
        let output = overview(&mut app);
        assert!(!output.contains("test:"), "Old details must not carry over");
        assert_eq!(app.diagnostics_requests[0].target.id, "api-2-new");
    }

    #[test]
    fn healthy_view_reports_that_rather_than_inventing_a_failure() {
        let mut app = demo();
        let output = open(&mut app, "/db-1", Kind::Health);
        assert!(
            output.contains("No current failure is reported"),
            "{output}"
        );
        assert!(output.contains("Nothing in the available evidence suggests one"));
        let plain = open(&mut app, "/api-1", Kind::Health);
        assert!(plain.contains("No health check is configured"), "{plain}");
    }

    #[test]
    fn events_only_come_from_this_immutable_container_id() {
        let mut app = demo();
        // api-1 has an OOM in the demo timeline; api-2 must not borrow it.
        let other = open(&mut app, "/api-2", Kind::Health);
        assert!(!other.contains("out of memory"), "{other}");
        let own = open(&mut app, "/api-1", Kind::Health);
        assert!(own.contains("out of memory"), "{own}");
    }

    #[test]
    fn unavailable_backends_and_fields_read_as_unknown_never_as_good() {
        let mut app = demo();
        app.demo = false;
        select(&mut app, "default/native");
        app.open_diagnostics(Kind::Health);
        assert!(app.diagnostics_requests.is_empty(), "Nothing to inspect");
        let output = text(&app, Kind::Health);
        assert!(output.contains("no detailed inspection"), "{output}");
        assert!(output.contains("Restart count unavailable"), "{output}");
        assert!(output.contains("OOM flag unavailable"), "{output}");
        assert!(output.contains("Check details are unknown"), "{output}");
        assert!(!output.contains("No current failure"), "{output}");
    }

    #[test]
    fn opening_sends_exactly_one_inspect_and_a_replaced_target_is_reported_not_followed() {
        let mut app = demo();
        app.demo = false;
        select(&mut app, "/api-2");
        app.open_diagnostics(Kind::Health);
        assert_eq!(app.diagnostics_requests.len(), 1);
        assert_eq!(app.diagnostics_requests[0].target.id, "api-2");
        assert_eq!(
            app.diagnostics_requests[0].target.runtime.as_deref(),
            Some("orb")
        );
        assert!(text(&app, Kind::Health).contains("Loading"));
        app.diagnostics_requests.clear();

        let view = app.diagnostics.as_ref().unwrap();
        let (target, generation) = (view.main.target.clone(), view.main.generation);
        // The container is replaced: same name, new ID.
        let snapshot = app.engines[0].snapshot.as_mut().unwrap();
        for container in &mut snapshot.containers {
            if container.id == "api-2" {
                container.id = "api-2-new".into();
            }
        }
        let late = diagnostics::Event {
            role: diagnostics::Role::Main,
            generation,
            target,
            result: Ok(crate::diagnostics::Inspection {
                id: "api-2".into(),
                ..Default::default()
            }),
        };
        app.diagnostics.as_mut().unwrap().complete(late, 1);
        assert!(text(&app, Kind::Health).contains("this container ID is gone"));

        app.refresh_diagnostics();
        assert!(app.diagnostics_requests.is_empty(), "No inspect by name");
        assert!(text(&app, Kind::Health).contains("no longer exists here"));
        assert!(app.diagnostics_link_ready().is_err());
    }

    #[test]
    fn logs_from_a_wider_scope_are_not_presented_as_this_containers() {
        let mut app = demo();
        app.logs.push(
            "shop-api-1",
            "2026-10-29T10:00:00Z from another container",
            false,
        );
        select(&mut app, "/api-2");
        app.log_scope_targets = Some((0, vec!["/api-1".into(), "/api-2".into()]));
        app.open_diagnostics(Kind::Health);
        let output = text(&app, Kind::Health);
        assert!(!output.contains("from another container"), "{output}");
        assert!(output.contains("No logs are retained for exactly this container"));
    }

    #[test]
    fn logs_filtered_out_by_the_user_are_flagged_and_the_filter_is_untouched() {
        let mut app = demo();
        // The demo's latest failed check ended 30 seconds ago.
        let near = time::OffsetDateTime::now_utc() - time::Duration::seconds(35);
        let at = near
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        app.logs
            .push("shop-api-2", &format!("{at} connection refused"), false);
        app.log_view.filter.set_text("absent");
        app.log_filter_terms = "absent".into();
        select(&mut app, "/api-2");
        app.log_scope_targets = Some((0, vec!["/api-2".into()]));
        app.open_diagnostics(Kind::Health);
        let output = text(&app, Kind::Health);
        assert!(output.contains("connection refused"), "{output}");
        assert!(
            output.contains("1 of these are hidden in Logs by the active filter (absent)"),
            "{output}"
        );
        assert!(app.log_view.filter.text_active());
    }

    #[test]
    fn network_view_shows_reported_evidence_with_explicit_not_tested_labels() {
        let mut app = demo();
        let output = open(&mut app, "/api-2", Kind::Network);
        for expected in [
            "Passive",
            "Network mode",
            "shop_default · ID demo-network",
            "172.18.0.",
            "IPv6 unavailable",
            "Reported aliases: api, shop-api-2 (resolution not tested)",
            "Also attached: shop-api-1, shop-postgres-1",
            "0.0.0.0:8080 → 3000/tcp (all interfaces; not localhost-only)",
            "9090/tcp — exposed, no host port",
            "not verified from this workstation",
            "Possible overlap, not confirmed: shop-api-1 also reports host port 8080/tcp",
            "no bind failure is implied",
            "No peer chosen",
        ] {
            assert!(output.contains(expected), "{expected}\n{output}");
        }
    }

    fn with_peer(app: &mut App, key: &str, kind_key: &str) -> String {
        open(app, key, Kind::Network);
        let peer = app
            .diagnostic_peers()
            .into_iter()
            .find(|container| container.key() == kind_key)
            .unwrap();
        app.picker = Some(crate::app::Picker {
            title: String::new(),
            choices: vec![crate::app::Choice::DiagnosticPeer(Box::new(peer))],
            selected: 0,
        });
        app.choose();
        text(app, Kind::Network)
    }

    #[test]
    fn shared_membership_is_never_presented_as_proof_of_connectivity() {
        let mut app = demo();
        let output = with_peer(&mut app, "/api-2", "/db-1");
        assert!(output.contains("Peer shop-postgres-1"), "{output}");
        assert!(
            output.contains("Shared network: shop_default. This is configuration; connectivity has not been tested."),
            "{output}"
        );
        for claim in ["reachable", "connected", "can reach", "✓"] {
            assert!(!output.to_lowercase().contains(claim), "{claim}\n{output}");
        }
    }

    #[test]
    fn no_shared_network_does_not_claim_disconnection() {
        let mut app = demo();
        with_peer(&mut app, "/api-1", "/db-1");
        let peer = app.diagnostics.as_mut().unwrap().peer.as_mut().unwrap();
        let (inspection, _) = peer.evidence.as_mut().unwrap();
        inspection.endpoints = Some(vec![Endpoint {
            network: "elsewhere".into(),
            id: Some("elsewhere-id".into()),
            ..Default::default()
        }]);
        let output = text(&app, Kind::Network);
        assert!(
            output.contains("No shared network is reported. This does not rule out"),
            "{output}"
        );
    }

    #[test]
    fn choosing_no_peer_removes_the_comparison() {
        let mut app = demo();
        with_peer(&mut app, "/api-1", "/db-1");
        app.picker = Some(crate::app::Picker {
            title: String::new(),
            choices: vec![crate::app::Choice::DiagnosticPeerNone],
            selected: 0,
        });
        app.choose();
        assert!(text(&app, Kind::Network).contains("No peer chosen"));
    }

    fn outsider_workspace() -> App {
        let mut app = demo();
        let snapshot = app.engines[0].snapshot.as_mut().unwrap();
        snapshot.containers.push(Container {
            id: "outside".into(),
            name: "outside-secret-name".into(),
            state: "running".into(),
            project: Some("other".into()),
            networks: vec!["shop_default".into()],
            port_bindings: vec![PortBinding {
                host_ip: Some("0.0.0.0".into()),
                host_port: Some(8080),
                container_port: 80,
                protocol: "tcp".into(),
            }],
            ..Default::default()
        });
        snapshot
            .networks
            .iter_mut()
            .find(|network| network.name == "shop_default")
            .unwrap()
            .containers
            .push("outside-secret-name".into());
        app.project = Some(Project {
            engine: 0,
            namespace: None,
            name: "shop".into(),
        });
        app.refresh_tree();
        app
    }

    #[test]
    fn workspace_hides_outside_names_and_details_but_keeps_counts() {
        let mut app = outsider_workspace();
        let output = open(&mut app, "/api-2", Kind::Network);
        assert!(!output.contains("outside-secret-name"), "{output}");
        assert!(
            output.contains("1 attachments outside this project (hidden)"),
            "{output}"
        );
        assert!(
            output
                .contains("1 possible port overlaps with containers outside this project (hidden)"),
            "{output}"
        );
        let peers = app.diagnostic_peers();
        assert!(!peers.is_empty());
        assert!(
            peers
                .iter()
                .all(|peer| peer.project.as_deref() == Some("shop"))
        );
    }

    #[test]
    fn workspace_also_covers_container_sharing_references() {
        let mut app = outsider_workspace();
        select(&mut app, "/api-1");
        app.open_diagnostics(Kind::Network);
        let view = app.diagnostics.as_mut().unwrap();
        let (inspection, _) = view.main.evidence.as_mut().unwrap();
        inspection.network_mode = Some("container:outside".into());
        inspection.endpoints = Some(vec![]);
        let output = text(&app, Kind::Network);
        assert!(
            output.contains("a container outside this project"),
            "{output}"
        );
        assert!(!output.contains("outside-secret-name") && !output.contains("container:outside"));
        // Inside the workspace, the name is fine.
        let view = app.diagnostics.as_mut().unwrap();
        let (inspection, _) = view.main.evidence.as_mut().unwrap();
        inspection.network_mode = Some("container:db-1".into());
        assert!(text(&app, Kind::Network).contains("shares the network stack of shop-postgres-1"));
    }

    #[test]
    fn membership_only_data_keeps_membership_and_marks_addresses_unavailable() {
        let mut app = demo();
        select(&mut app, "/api-1");
        app.open_diagnostics(Kind::Network);
        let view = app.diagnostics.as_mut().unwrap();
        view.main.evidence = None;
        view.main.status = Status::Failed("timed out".into());
        let output = text(&app, Kind::Network);
        assert!(
            output.contains("Detailed inspection unavailable: timed out"),
            "{output}"
        );
        assert!(
            output.contains("shop_default · ID unknown · addresses unavailable"),
            "{output}"
        );
        assert!(output.contains("Mode unavailable"), "{output}");
    }

    #[test]
    fn peer_failure_stays_on_the_peer_and_compares_as_incomplete() {
        let mut app = demo();
        with_peer(&mut app, "/api-2", "/db-1");
        let peer = app.diagnostics.as_mut().unwrap().peer.as_mut().unwrap();
        peer.evidence = None;
        peer.status = Status::Failed("peer timed out".into());
        let output = text(&app, Kind::Network);
        assert!(output.contains("Comparison incomplete"), "{output}");
        assert!(output.contains("peer timed out"));
        assert!(
            output.contains("shop_default · ID demo-network"),
            "Main evidence stays"
        );
    }

    #[test]
    fn runtime_text_is_sanitized_before_it_reaches_the_terminal() {
        let mut app = demo();
        select(&mut app, "/api-1");
        app.open_diagnostics(Kind::Network);
        let view = app.diagnostics.as_mut().unwrap();
        let (inspection, _) = view.main.evidence.as_mut().unwrap();
        inspection.network_mode = Some("bad\x1b]52;c;c2VjcmV0\x07mode".into());
        inspection.endpoints = Some(vec![Endpoint {
            network: "plain".into(),
            ..Default::default()
        }]);
        let output = text(&app, Kind::Network);
        assert!(!output.contains('\x1b'), "{output}");
        assert!(output.contains("badmode"), "{output}");
    }

    #[test]
    fn overlay_renders_and_scrolls_on_narrow_terminals_and_clamps_its_scroll() {
        let mut app = demo();
        select(&mut app, "/api-2");
        app.open_diagnostics(Kind::Health);
        app.diagnostics.as_mut().unwrap().scroll = usize::MAX;
        for (width, height) in [(160, 45), (60, 18), (30, 8), (10, 4)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
            assert!(app.diagnostics.as_ref().unwrap().scroll < 10_000);
        }
    }

    #[test]
    fn german_keeps_health_terms_and_every_view_renders() {
        crate::i18n::isolate_thread();
        crate::i18n::set_language(crate::i18n::Language::German);
        let mut app = demo();
        let output = open(&mut app, "/api-2", Kind::Health);
        assert!(output.contains("Status unhealthy"), "{output}");
        assert!(output.contains("Aktueller Zustand"), "{output}");
        let network = open(&mut app, "/api-2", Kind::Network);
        assert!(network.contains("Auflösung nicht getestet"), "{network}");
        crate::i18n::set_language(crate::i18n::Language::English);
    }
}
