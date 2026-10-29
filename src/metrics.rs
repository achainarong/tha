//! Runtime-independent usage, CPU deltas, freshness, and bounded recent history.
use crate::model::Container;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

pub const HISTORY_LENGTH: usize = 60;
pub const MAX_CONCURRENT_REQUESTS: usize = 8;

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub memory_limit_bytes: Option<u64>,
    pub cpu_warming: bool,
    pub memory_working_set: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CpuCounters {
    pub total: u64,
    pub system: u64,
    pub online_cpus: u32,
}

pub fn cpu_percent(previous: Option<CpuCounters>, current: CpuCounters) -> Option<f64> {
    let previous = previous?;
    let cpu_delta = current.total.checked_sub(previous.total)?;
    let system_delta = current.system.checked_sub(previous.system)?;
    if system_delta == 0 || current.online_cpus == 0 {
        return None;
    }
    let percent = cpu_delta as f64 / system_delta as f64 * current.online_cpus as f64 * 100.0;
    percent.is_finite().then_some(percent)
}

#[derive(Debug)]
pub struct Reading {
    pub usage: Usage,
    pub counters: Option<CpuCounters>,
    pub sampled_at: Instant,
}

pub type Batch = HashMap<String, Result<Reading, String>>;

#[derive(Debug, Clone)]
pub struct Target {
    pub revision: u64,
    pub identity: Option<String>,
    pub containers: Vec<Container>,
}

impl Target {
    pub fn same_workload(&self, identity: &Option<String>, containers: &[Container]) -> bool {
        self.identity == *identity
            && self.containers.len() == containers.len()
            && self
                .containers
                .iter()
                .zip(containers)
                .all(|(a, b)| a.key() == b.key() && a.running() == b.running())
    }
}

pub struct Event {
    pub engine: usize,
    pub revision: u64,
    pub batch: Batch,
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub usage: Usage,
    pub sampled_at: Instant,
}

#[derive(Debug)]
struct Frame {
    values: HashMap<String, Usage>,
}

#[derive(Debug, Default)]
pub struct Store {
    pub samples: HashMap<String, Sample>,
    pub errors: HashMap<String, String>,
    history: VecDeque<Frame>,
}

impl Store {
    pub fn reconcile(&mut self, containers: &[Container]) {
        let running: HashSet<_> = containers
            .iter()
            .filter(|c| c.running())
            .map(Container::key)
            .collect();
        self.samples.retain(|key, _| running.contains(key));
        self.errors.retain(|key, _| running.contains(key));
    }

    pub fn apply(&mut self, containers: &[Container], batch: Batch) {
        self.reconcile(containers);
        let running: HashSet<_> = containers
            .iter()
            .filter(|c| c.running())
            .map(Container::key)
            .collect();
        let mut values = HashMap::new();
        for (key, result) in batch {
            if !running.contains(&key) {
                continue;
            }
            match result {
                Ok(reading) => {
                    self.errors.remove(&key);
                    values.insert(key.clone(), reading.usage.clone());
                    self.samples.insert(
                        key,
                        Sample {
                            usage: reading.usage,
                            sampled_at: reading.sampled_at,
                        },
                    );
                }
                Err(error) => {
                    self.errors.insert(key, error);
                }
            }
        }
        if self.history.len() == HISTORY_LENGTH {
            self.history.pop_front();
        }
        self.history.push_back(Frame { values });
    }

    pub fn summary<'a>(
        &self,
        containers: impl IntoIterator<Item = &'a Container>,
        now: Instant,
        stale_after: Duration,
        available: bool,
    ) -> Summary {
        let mut summary = Summary::default();
        let mut cpu = 0.0;
        let mut memory = 0u64;
        for container in containers {
            if !container.running() {
                summary.stopped += 1;
                continue;
            }
            summary.running += 1;
            let key = container.key();
            let sample = self.samples.get(&key);
            let fresh = available
                && !self.errors.contains_key(&key)
                && sample.is_some_and(|sample| {
                    now.saturating_duration_since(sample.sampled_at) <= stale_after
                });
            if !fresh {
                if sample.is_some() {
                    summary.stale += 1;
                }
                continue;
            }
            let sample = sample.expect("fresh sample exists");
            if let Some(value) = sample
                .usage
                .cpu_percent
                .filter(|value| value.is_finite() && *value >= 0.0)
            {
                cpu += value;
                summary.cpu_samples += 1;
            } else if sample.usage.cpu_warming {
                summary.warming += 1;
            }
            if let Some(value) = sample.usage.memory_bytes {
                memory = memory.saturating_add(value);
                summary.memory_samples += 1;
            }
            if summary.running == 1 {
                summary.memory_limit_bytes = sample.usage.memory_limit_bytes;
            }
        }
        summary.cpu_percent = (summary.cpu_samples > 0).then_some(cpu);
        summary.memory_bytes = (summary.memory_samples > 0).then_some(memory);
        if summary.running != 1 {
            summary.memory_limit_bytes = None;
        }
        summary
    }

    /// Gaps remain absent, not zero. Membership is the currently selected workload.
    pub fn cpu_history(&self, containers: &[Container]) -> Vec<Option<u64>> {
        let keys: Vec<_> = containers
            .iter()
            .filter(|c| c.running())
            .map(Container::key)
            .collect();
        self.history
            .iter()
            .map(|frame| {
                if keys.is_empty() {
                    return None;
                }
                let mut total = 0.0;
                for key in &keys {
                    let value = frame.values.get(key)?.cpu_percent?;
                    if !value.is_finite() || value < 0.0 {
                        return None;
                    }
                    total += value;
                }
                Some((total * 100.0).round() as u64)
            })
            .collect()
    }
}

#[derive(Debug, Default)]
pub struct Summary {
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub memory_limit_bytes: Option<u64>,
    pub running: usize,
    pub stopped: usize,
    pub cpu_samples: usize,
    pub memory_samples: usize,
    pub warming: usize,
    pub stale: usize,
}

impl Summary {
    pub fn cpu_label(&self) -> String {
        match self.cpu_percent {
            Some(value) => format!(
                "{value:.1}%{}",
                if self.cpu_samples < self.running {
                    "*"
                } else {
                    ""
                }
            ),
            None if self.running == 0 && self.stopped > 0 => "stopped".into(),
            None if self.stale > 0 => "stale".into(),
            None if self.warming > 0 => "warm".into(),
            _ => "--".into(),
        }
    }

    pub fn memory_label(&self) -> String {
        match self.memory_bytes {
            Some(value) => format!(
                "{}{}",
                bytes(value),
                if self.memory_samples < self.running {
                    "*"
                } else {
                    ""
                }
            ),
            None if self.running == 0 && self.stopped > 0 => "stopped".into(),
            None if self.stale > 0 => "stale".into(),
            _ => "--".into(),
        }
    }

    pub fn detail(&self) -> String {
        format!(
            "CPU {}/{} · memory {}/{} live running samples{}{}",
            self.cpu_samples,
            self.running,
            self.memory_samples,
            self.running,
            if self.warming > 0 {
                format!(" · {} CPU warming up", self.warming)
            } else {
                String::new()
            },
            if self.stale > 0 {
                format!(" · {} stale", self.stale)
            } else {
                String::new()
            }
        )
    }
}

pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut number = value as f64;
    let mut unit = 0;
    while number >= 1024.0 && unit < UNITS.len() - 1 {
        number /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{number:.1} {}", UNITS[unit])
    }
}

pub async fn worker(
    engine: usize,
    connection: crate::config::Connection,
    mut targets: tokio::sync::watch::Receiver<Option<Target>>,
    tx: tokio::sync::mpsc::Sender<Event>,
    refresh_seconds: u64,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(refresh_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut previous = HashMap::new();
    let mut identity = None;
    let mut rotation = 0usize;
    loop {
        tokio::select! {
            _ = interval.tick() => {},
            changed = targets.changed() => { if changed.is_err() { break; } }
        }
        let Some(target) = targets.borrow().clone() else {
            previous.clear();
            identity = None;
            continue;
        };
        if identity.as_ref() != Some(&target.identity) {
            previous.clear();
            identity = Some(target.identity.clone());
        }
        let running: HashSet<_> = target
            .containers
            .iter()
            .filter(|c| c.running())
            .map(Container::key)
            .collect();
        previous.retain(|key, _| running.contains(key));
        let mut containers = target.containers.clone();
        // Rotate the bounded request queue so a very large/slow engine cannot
        // permanently starve containers at the end of its inventory.
        if !containers.is_empty() {
            let offset = rotation % containers.len();
            containers.rotate_left(offset);
        }
        rotation = rotation.wrapping_add(MAX_CONCURRENT_REQUESTS);
        let batch = tokio::select! {
            batch = crate::backend::metrics(&connection, &containers, &previous) => batch,
            changed = targets.changed() => { if changed.is_err() { break; } else { continue; } }
        };
        for (key, result) in &batch {
            if let Ok(reading) = result
                && let Some(counters) = reading.counters
            {
                previous.insert(key.clone(), counters);
            }
        }
        if tx
            .send(Event {
                engine,
                revision: target.revision,
                batch,
            })
            .await
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn container(id: &str) -> Container {
        Container {
            id: id.into(),
            state: "running".into(),
            ..Default::default()
        }
    }
    fn reading(cpu: Option<f64>, memory: Option<u64>, at: Instant) -> Reading {
        Reading {
            usage: Usage {
                cpu_percent: cpu,
                memory_bytes: memory,
                memory_limit_bytes: Some(4096),
                ..Default::default()
            },
            counters: None,
            sampled_at: at,
        }
    }

    #[test]
    fn cpu_uses_core_percent_and_handles_idle_first_sample_and_counter_resets() {
        let previous = CpuCounters {
            total: 100,
            system: 1000,
            online_cpus: 4,
        };
        let current = CpuCounters {
            total: 200,
            system: 1100,
            online_cpus: 4,
        };
        assert_eq!(cpu_percent(Some(previous), current), Some(400.0));
        assert_eq!(cpu_percent(None, current), None);
        assert_eq!(
            cpu_percent(
                Some(previous),
                CpuCounters {
                    total: 100,
                    ..current
                }
            ),
            Some(0.0)
        );
        assert_eq!(
            cpu_percent(
                Some(previous),
                CpuCounters {
                    total: 1,
                    ..current
                }
            ),
            None
        );
        assert_eq!(
            cpu_percent(
                Some(previous),
                CpuCounters {
                    system: 1,
                    ..current
                }
            ),
            None
        );
        assert_eq!(
            cpu_percent(
                Some(previous),
                CpuCounters {
                    system: 1000,
                    ..current
                }
            ),
            None
        );
        assert_eq!(
            cpu_percent(
                Some(previous),
                CpuCounters {
                    online_cpus: 0,
                    ..current
                }
            ),
            None
        );
    }

    #[test]
    fn partial_totals_exclude_missing_and_failed_samples_without_summing_limits() {
        let now = Instant::now();
        let containers = vec![container("a"), container("b")];
        let mut store = Store::default();
        store.apply(
            &containers,
            HashMap::from([(
                containers[0].key(),
                Ok(reading(Some(75.0), Some(1024), now)),
            )]),
        );
        let summary = store.summary(&containers, now, Duration::from_secs(3), true);
        assert_eq!(summary.cpu_label(), "75.0%*");
        assert_eq!(summary.memory_label(), "1.0 KiB*");
        assert_eq!(summary.memory_limit_bytes, None);
        assert_eq!(summary.cpu_samples, 1);
        store.apply(
            &containers,
            HashMap::from([(containers[0].key(), Err("denied".into()))]),
        );
        let summary = store.summary(&containers, now, Duration::from_secs(3), true);
        assert_eq!(summary.cpu_label(), "stale");
        assert_eq!(summary.cpu_percent, None);
        assert!(store.samples.contains_key(&containers[0].key()));
    }

    #[test]
    fn elapsed_and_offline_samples_are_stale_and_stopped_containers_are_not_zero() {
        let now = Instant::now();
        let mut containers = vec![container("a")];
        let mut store = Store::default();
        store.apply(
            &containers,
            HashMap::from([(containers[0].key(), Ok(reading(Some(0.0), Some(0), now)))]),
        );
        assert_eq!(
            store
                .summary(&containers, now, Duration::from_secs(3), true)
                .cpu_label(),
            "0.0%"
        );
        assert_eq!(
            store
                .summary(
                    &containers,
                    now + Duration::from_secs(4),
                    Duration::from_secs(3),
                    true
                )
                .cpu_label(),
            "stale"
        );
        assert_eq!(
            store
                .summary(&containers, now, Duration::from_secs(3), false)
                .memory_label(),
            "stale"
        );
        containers[0].state = "exited".into();
        store.reconcile(&containers);
        let stopped = store.summary(&containers, now, Duration::from_secs(3), true);
        assert_eq!(stopped.cpu_label(), "stopped");
        assert_eq!(stopped.memory_label(), "stopped");
        assert!(store.samples.is_empty());
    }

    #[test]
    fn history_is_bounded_and_partial_samples_remain_gaps() {
        let now = Instant::now();
        let containers = vec![container("a"), container("b")];
        let mut store = Store::default();
        for _ in 0..HISTORY_LENGTH + 5 {
            store.apply(
                &containers,
                HashMap::from([(containers[0].key(), Ok(reading(Some(10.0), Some(100), now)))]),
            );
        }
        let history = store.cpu_history(&containers);
        assert_eq!(history.len(), HISTORY_LENGTH);
        assert!(history.iter().all(Option::is_none));
        assert_eq!(
            store.cpu_history(&containers[..1]).last(),
            Some(&Some(1000))
        );
    }

    #[test]
    fn namespace_keys_do_not_collide_and_warming_is_distinct_from_unavailable() {
        let now = Instant::now();
        let a = Container {
            namespace: Some("a".into()),
            ..container("same")
        };
        let b = Container {
            namespace: Some("b".into()),
            ..container("same")
        };
        let containers = vec![a.clone(), b.clone()];
        let mut store = Store::default();
        let mut warm = reading(None, Some(256), now);
        warm.usage.cpu_warming = true;
        store.apply(
            &containers,
            HashMap::from([
                (a.key(), Ok(warm)),
                (b.key(), Ok(reading(Some(90.0), Some(512), now))),
            ]),
        );
        assert_eq!(
            store
                .summary([&a], now, Duration::from_secs(3), true)
                .cpu_label(),
            "warm"
        );
        assert_eq!(
            store
                .summary([&b], now, Duration::from_secs(3), true)
                .cpu_label(),
            "90.0%"
        );
        assert_eq!(
            store
                .summary(&containers, now, Duration::from_secs(3), true)
                .cpu_label(),
            "90.0%*"
        );
    }
}
