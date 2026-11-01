//! Groups what happened to a workload into incidents, from evidence that is already kept
//! elsewhere. Things that happen close together are shown together, which does not prove that
//! one caused the other.
use crate::events::{Event, Tone};

/// Quiet for this long ends an incident, the same window restart loops are judged by.
pub const GAP_MS: i64 = crate::diagnostics::RESTART_WINDOW_MS;
pub const CPU_SPIKE_PERCENT: f64 = 90.0;
pub const MEMORY_SPIKE_PERCENT: u64 = 90;
pub const MAX_INCIDENTS: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub at_ms: i64,
    pub key: String,
    pub name: String,
    pub label: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    pub start_ms: i64,
    pub end_ms: i64,
    pub entries: Vec<Entry>,
    /// A container was started again after the last failure.
    pub recovered: bool,
}

/// One container's readings, oldest first, as `(time, cpu percent, memory percent of its limit)`.
pub struct Readings<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub points: &'a [(i64, Option<f64>, Option<u64>)],
}

/// The moments a reading crossed its limit. Staying high is one entry, not one per sample.
fn spikes(readings: &Readings) -> Vec<Entry> {
    let mut found = Vec::new();
    let (mut cpu_high, mut memory_high) = (false, false);
    for (at_ms, cpu, memory) in readings.points {
        let entry = |label: String| Entry {
            at_ms: *at_ms,
            key: readings.key.to_owned(),
            name: readings.name.to_owned(),
            label,
            tone: Tone::Warning,
        };
        if let Some(cpu) = cpu {
            let high = *cpu >= CPU_SPIKE_PERCENT;
            if high && !cpu_high {
                found.push(entry(crate::tf!(
                    "CPU reached {percent}%",
                    percent = *cpu as u64
                )));
            }
            cpu_high = high;
        }
        if let Some(memory) = memory {
            let high = *memory >= MEMORY_SPIKE_PERCENT;
            if high && !memory_high {
                found.push(entry(crate::tf!(
                    "Memory reached {percent}% of its limit",
                    percent = memory
                )));
            }
            memory_high = high;
        }
    }
    found
}

/// Newest incident first, with entries oldest first inside each. Only groups containing a
/// failure count: stopping something on purpose, or a brief spike alone, is not an incident.
pub fn build(events: &[&Event], readings: &[Readings]) -> Vec<Incident> {
    let mut entries: Vec<Entry> = events
        .iter()
        .filter(|event| matches!(event.tone(), Tone::Failure | Tone::Warning | Tone::Healthy))
        .map(|event| Entry {
            at_ms: event.at_ms,
            key: event.key.clone(),
            name: event.name.clone(),
            label: event.label(),
            tone: event.tone(),
        })
        .chain(readings.iter().flat_map(spikes))
        .collect();
    entries.sort_by(|a, b| (a.at_ms, &a.key, &a.label).cmp(&(b.at_ms, &b.key, &b.label)));
    let mut groups: Vec<Vec<Entry>> = Vec::new();
    for entry in entries {
        match groups.last_mut() {
            Some(group)
                if group
                    .last()
                    .is_some_and(|last| entry.at_ms - last.at_ms <= GAP_MS) =>
            {
                group.push(entry);
            }
            _ => groups.push(vec![entry]),
        }
    }
    let mut incidents: Vec<Incident> = groups
        .into_iter()
        .filter(|group| group.iter().any(|entry| entry.tone == Tone::Failure))
        .map(|group| {
            let last_failure = group.iter().rposition(|entry| entry.tone == Tone::Failure);
            let recovered = last_failure.is_some_and(|index| {
                group[index + 1..]
                    .iter()
                    .any(|entry| entry.tone == Tone::Healthy)
            });
            Incident {
                start_ms: group.first().map_or(0, |entry| entry.at_ms),
                end_ms: group.last().map_or(0, |entry| entry.at_ms),
                entries: group,
                recovered,
            }
        })
        .collect();
    incidents.reverse();
    incidents.truncate(MAX_INCIDENTS);
    incidents
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(at_ms: i64, action: &str, detail: &str) -> Event {
        Event {
            at_ms,
            key: "/abc".into(),
            name: "api".into(),
            project: None,
            service: None,
            action: action.into(),
            detail: detail.into(),
        }
    }

    fn labels(incident: &Incident) -> Vec<&str> {
        incident
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect()
    }

    #[test]
    fn close_events_become_one_incident_and_later_ones_start_another() {
        let events = [
            event(1_000, "health_status", "unhealthy"),
            event(2_000, "die", "exit 1"),
            event(3_000, "start", ""),
            event(3_000 + GAP_MS + 1, "oom", ""),
        ];
        let refs: Vec<_> = events.iter().collect();
        let incidents = build(&refs, &[]);
        assert_eq!(incidents.len(), 2);
        assert_eq!(labels(&incidents[0]), ["out of memory"]);
        assert!(!incidents[0].recovered);
        assert_eq!(
            labels(&incidents[1]),
            ["health · unhealthy", "exited · exit 1", "started"]
        );
        assert!(incidents[1].recovered);
    }

    #[test]
    fn deliberate_stops_and_lone_spikes_are_not_incidents() {
        let events = [
            event(1_000, "stop", ""),
            event(2_000, "die", "exit 0"),
            event(3_000, "start", ""),
        ];
        let refs: Vec<_> = events.iter().collect();
        let points = [(1_500, Some(99.0), None)];
        let readings = [Readings {
            key: "/abc",
            name: "api",
            points: &points,
        }];
        assert!(build(&refs, &readings).is_empty());
    }

    #[test]
    fn a_spike_near_a_failure_joins_it_and_only_crossings_count() {
        let events = [event(10_000, "oom", "")];
        let refs: Vec<_> = events.iter().collect();
        let points = [
            (1_000, Some(10.0), Some(50)),
            (2_000, Some(95.0), Some(95)),
            (3_000, Some(96.0), Some(96)),
            (4_000, Some(10.0), Some(50)),
            (5_000, None, None),
            (6_000, Some(97.0), None),
        ];
        let readings = [Readings {
            key: "/abc",
            name: "api",
            points: &points,
        }];
        let incidents = build(&refs, &readings);
        assert_eq!(
            labels(&incidents[0]),
            [
                "CPU reached 95%",
                "Memory reached 95% of its limit",
                "CPU reached 97%",
                "out of memory"
            ]
        );
    }

    #[test]
    fn missing_readings_never_count_as_a_spike_or_a_recovery() {
        let points = [(1_000, None, None), (2_000, None, None)];
        let readings = Readings {
            key: "/abc",
            name: "api",
            points: &points,
        };
        assert!(spikes(&readings).is_empty());
    }

    #[test]
    fn incidents_are_bounded() {
        let events: Vec<_> = (0..MAX_INCIDENTS as i64 + 5)
            .map(|index| event(index * (GAP_MS + 1), "oom", ""))
            .collect();
        let refs: Vec<_> = events.iter().collect();
        let incidents = build(&refs, &[]);
        assert_eq!(incidents.len(), MAX_INCIDENTS);
        assert!(
            incidents[0].start_ms > incidents[1].start_ms,
            "Newest first"
        );
    }
}
