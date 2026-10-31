//! Runtime-independent usage, CPU deltas, freshness, and bounded recent history.
use crate::model::Container;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

pub const HISTORY_LENGTH: usize = 60;
pub const MAX_CONCURRENT_REQUESTS: usize = 8;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub memory_limit_bytes: Option<u64>,
    pub cpu_warming: bool,
    pub memory_working_set: bool,
    pub network_rx_bytes: Option<u64>,
    pub network_tx_bytes: Option<u64>,
    pub disk_read_bytes: Option<u64>,
    pub disk_write_bytes: Option<u64>,
    pub network_rx_rate: Option<f64>,
    pub network_tx_rate: Option<f64>,
    pub disk_read_rate: Option<f64>,
    pub disk_write_rate: Option<f64>,
}

/// Rates use actual elapsed time. Missing/reset counters never become fake zeroes.
pub fn io_rates(current: &mut Usage, previous: &Usage, elapsed: Duration) {
    let rate = |value: Option<u64>, old: Option<u64>| {
        let seconds = elapsed.as_secs_f64();
        (seconds > 0.0).then_some(())?;
        Some(value?.checked_sub(old?)? as f64 / seconds)
    };
    current.network_rx_rate = rate(current.network_rx_bytes, previous.network_rx_bytes);
    current.network_tx_rate = rate(current.network_tx_bytes, previous.network_tx_bytes);
    current.disk_read_rate = rate(current.disk_read_bytes, previous.disk_read_bytes);
    current.disk_write_rate = rate(current.disk_write_bytes, previous.disk_write_bytes);
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
    sampled_at: Instant,
    local: bool,
    wall_ms: i64,
}

#[derive(Debug, Default)]
pub struct Store {
    pub samples: HashMap<String, Sample>,
    pub errors: HashMap<String, String>,
    history: VecDeque<Frame>,
}

#[derive(Debug)]
pub struct TimedUsage {
    pub sampled_at: Instant,
    pub cpu: Option<u64>,
    pub memory: Option<u64>,
}

impl Store {
    pub fn local_samples(&self, key: &str, start: i64, end: i64) -> Vec<crate::history::Sample> {
        self.history
            .iter()
            .filter(|frame| frame.local)
            .filter_map(|frame| {
                let at = frame.wall_ms;
                if at < start || at > end {
                    return None;
                }
                Some(crate::history::Sample {
                    key: key.into(),
                    name: String::new(),
                    at_ms: at,
                    usage: frame.values.get(key).cloned(),
                })
            })
            .collect()
    }
    pub fn apply_remote(&mut self, containers: &[Container], batch: Batch) {
        self.apply(containers, batch);
        if let Some(frame) = self.history.back_mut() {
            frame.local = false;
        }
    }
    pub fn local_history(&self, key: &str, start: i64, end: i64) -> crate::history::Data {
        let points = self
            .history
            .iter()
            .filter_map(|frame| {
                let at_ms = frame.wall_ms;
                if at_ms < start || at_ms > end {
                    return None;
                }
                let usage = frame.values.get(key);
                let values = usage
                    .map(|u| {
                        [
                            u.cpu_percent,
                            u.memory_bytes.map(|value| value as f64),
                            u.disk_read_rate,
                            u.disk_write_rate,
                            u.network_rx_rate,
                            u.network_tx_rate,
                        ]
                    })
                    .unwrap_or([None; 6]);
                Some(crate::history::Point {
                    at_ms,
                    values,
                    peaks: values,
                    count: 1,
                    valid: values.map(|value| u64::from(value.is_some())),
                })
            })
            .collect();
        crate::history::Data {
            points,
            source: crate::i18n::t("Standalone session · last 60 sampling batches only").into(),
            start_ms: start,
            bucket_ms: 1,
            end_ms: end,
        }
    }
    pub fn has_history(&self) -> bool {
        !self.history.is_empty()
    }

    /// Snapshot all retained readings for these exact IDs, including stopped
    /// containers. Selection never changes which containers are collected.
    pub fn timed_history<'a>(
        &self,
        containers: impl IntoIterator<Item = &'a Container>,
    ) -> Vec<TimedUsage> {
        let keys: Vec<_> = containers.into_iter().map(Container::key).collect();
        self.history
            .iter()
            .map(|frame| {
                let cpu = keys.iter().try_fold(0.0, |sum, key| {
                    let value = frame.values.get(key)?.cpu_percent?;
                    (value.is_finite() && value >= 0.0).then_some(sum + value)
                });
                let memory = keys.iter().try_fold(0u64, |sum, key| {
                    Some(sum.saturating_add(frame.values.get(key)?.memory_bytes?))
                });
                TimedUsage {
                    sampled_at: frame.sampled_at,
                    cpu: cpu
                        .filter(|_| !keys.is_empty())
                        .map(|value| (value * 100.0).round() as u64),
                    memory: memory.filter(|_| !keys.is_empty()),
                }
            })
            .collect()
    }

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
        let sampled_at = batch
            .values()
            .filter_map(|result| result.as_ref().ok().map(|reading| reading.sampled_at))
            .max()
            .unwrap_or_else(Instant::now);
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
        self.history.push_back(Frame {
            values,
            sampled_at,
            local: true,
            wall_ms: crate::history::now_ms()
                .saturating_sub(sampled_at.elapsed().as_millis() as i64),
        });
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

    /// Complete-workload totals only; missing members are gaps, never invented zeroes.
    /// A stale tail is marked without advancing history on UI redraws.
    pub fn performance_history(
        &self,
        containers: &[Container],
        now: Instant,
        stale_after: Duration,
    ) -> (Vec<Option<u64>>, Vec<Option<u64>>) {
        let keys: Vec<_> = containers
            .iter()
            .filter(|c| c.running())
            .map(Container::key)
            .collect();
        let mut cpu = Vec::new();
        let mut memory = Vec::new();
        let mut previous: Option<Instant> = None;
        for frame in &self.history {
            if previous
                .is_some_and(|at| frame.sampled_at.saturating_duration_since(at) > stale_after)
            {
                cpu.push(None);
                memory.push(None);
            }
            previous = Some(frame.sampled_at);
            let total = keys.iter().try_fold(0u64, |sum, key| {
                Some(sum.saturating_add(frame.values.get(key)?.memory_bytes?))
            });
            memory.push(total.filter(|_| !keys.is_empty()));
            let total = keys.iter().try_fold(0.0, |sum, key| {
                let value = frame.values.get(key)?.cpu_percent?;
                (value.is_finite() && value >= 0.0).then_some(sum + value)
            });
            cpu.push(
                total
                    .filter(|_| !keys.is_empty())
                    .map(|value| (value * 100.0).round() as u64),
            );
        }
        if previous.is_some_and(|at| now.saturating_duration_since(at) > stale_after) {
            cpu.push(None);
            memory.push(None);
        }
        (cpu, memory)
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
            None if self.running == 0 && self.stopped > 0 => crate::i18n::t("stopped").into(),
            None if self.stale > 0 => crate::i18n::t("stale").into(),
            None if self.warming > 0 => crate::i18n::t("warm").into(),
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
            None if self.running == 0 && self.stopped > 0 => crate::i18n::t("stopped").into(),
            None if self.stale > 0 => crate::i18n::t("stale").into(),
            _ => "--".into(),
        }
    }

    pub fn detail(&self) -> String {
        crate::tf!(
            "CPU {a0}/{a1} · memory {a2}/{a3} live running samples{a4}{a5}",
            a0 = self.cpu_samples,
            a1 = self.running,
            a2 = self.memory_samples,
            a3 = self.running,
            a4 = if self.warming > 0 {
                crate::tf!(" · {a0} CPU warming up", a0 = self.warming)
            } else {
                String::new()
            },
            a5 = if self.stale > 0 {
                crate::tf!(" · {a0} stale", a0 = self.stale)
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
    targets: tokio::sync::watch::Receiver<Option<Target>>,
    tx: tokio::sync::mpsc::Sender<Event>,
    refresh_seconds: u64,
) {
    worker_with_limit(engine, connection, targets, tx, refresh_seconds, None).await;
}

pub async fn worker_with_limit(
    engine: usize,
    connection: crate::config::Connection,
    mut targets: tokio::sync::watch::Receiver<Option<Target>>,
    tx: tokio::sync::mpsc::Sender<Event>,
    refresh_seconds: u64,
    limit: Option<std::sync::Arc<tokio::sync::Semaphore>>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(refresh_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut previous = HashMap::new();
    let mut identity = None;
    let mut rotation = 0usize;
    let mut previous_io: HashMap<String, (Usage, Instant)> = HashMap::new();
    loop {
        tokio::select! {
            _ = interval.tick() => {},
            changed = targets.changed() => { if changed.is_err() { break; } }
        }
        let Some(target) = targets.borrow().clone() else {
            previous.clear();
            previous_io.clear();
            identity = None;
            continue;
        };
        if identity.as_ref() != Some(&target.identity) {
            previous.clear();
            previous_io.clear();
            identity = Some(target.identity.clone());
        }
        let running: HashSet<_> = target
            .containers
            .iter()
            .filter(|c| c.running())
            .map(Container::key)
            .collect();
        previous.retain(|key, _| running.contains(key));
        previous_io.retain(|key, _| running.contains(key));
        let mut containers = target.containers.clone();
        // Rotate the bounded request queue so a very large/slow engine cannot
        // permanently starve containers at the end of its inventory.
        if !containers.is_empty() {
            let offset = rotation % containers.len();
            containers.rotate_left(offset);
        }
        rotation = rotation.wrapping_add(MAX_CONCURRENT_REQUESTS);
        // Global daemon budget. Waiting is bounded by the next sampling slot;
        // no per-runtime queue can grow behind a slow engine.
        let _permit = if let Some(limit) = &limit {
            match tokio::time::timeout(Duration::from_millis(500), limit.acquire()).await {
                Ok(Ok(permit)) => Some(permit),
                _ => continue,
            }
        } else {
            None
        };
        let mut batch = tokio::select! {
            batch = crate::backend::metrics(&connection, &containers, &previous) => batch,
            changed = targets.changed() => { if changed.is_err() { break; } else { continue; } }
        };
        for (key, result) in &mut batch {
            if let Ok(reading) = result {
                if let Some((old, at)) = previous_io.get(key) {
                    io_rates(
                        &mut reading.usage,
                        old,
                        reading.sampled_at.saturating_duration_since(*at),
                    );
                }
                previous_io.insert(key.clone(), (reading.usage.clone(), reading.sampled_at));
            } else {
                previous_io.remove(key);
            }
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
    #[test]
    fn session_overlay_does_not_replay_daemon_observations() {
        let mut store = Store::default();
        let containers = vec![container("one")];
        let now = Instant::now();
        store.apply_remote(
            &containers,
            [("/one".into(), Ok(reading(Some(10.0), Some(100), now)))].into(),
        );
        let start = crate::history::now_ms() - 60_000;
        let end = crate::history::now_ms() + 1000;
        assert!(store.local_samples("/one", start, end).is_empty());
        store.apply(
            &containers,
            [("/one".into(), Ok(reading(Some(55.0), Some(200), now)))].into(),
        );
        let samples = store.local_samples("/one", start, end);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].usage.as_ref().unwrap().cpu_percent, Some(55.0));
    }
    #[test]
    fn io_rates_handle_elapsed_time_resets_and_missing_counters() {
        let previous = Usage {
            network_rx_bytes: Some(100),
            disk_read_bytes: Some(20),
            ..Default::default()
        };
        let mut next = Usage {
            network_rx_bytes: Some(300),
            disk_read_bytes: Some(40),
            ..Default::default()
        };
        io_rates(&mut next, &previous, Duration::from_secs(2));
        assert_eq!(next.network_rx_rate, Some(100.0));
        assert_eq!(next.disk_read_rate, Some(10.0));
        assert_eq!(next.network_tx_rate, None);
        next.network_rx_bytes = Some(1);
        io_rates(&mut next, &previous, Duration::from_secs(1));
        assert_eq!(next.network_rx_rate, None);
        io_rates(&mut next, &previous, Duration::ZERO);
        assert_eq!(next.disk_read_rate, None);
    }
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
    fn timestamped_history_is_per_container_bounded_and_retains_stopped_readings() {
        let mut store = Store::default();
        let mut containers = vec![container("one"), container("two")];
        let start = Instant::now();
        for tick in 0..HISTORY_LENGTH + 5 {
            let at = start + Duration::from_secs(tick as u64 * 2);
            store.apply(
                &containers,
                [
                    ("/one".into(), Ok(reading(Some(10.0), Some(100), at))),
                    ("/two".into(), Ok(reading(Some(20.0), Some(200), at))),
                ]
                .into(),
            );
        }
        let first = store.timed_history(&containers[..1]);
        let second = store.timed_history(&containers[1..]);
        assert_eq!(first.len(), HISTORY_LENGTH);
        assert_eq!(first[0].sampled_at, start + Duration::from_secs(10));
        assert!(
            first
                .iter()
                .all(|point| point.cpu == Some(1000) && point.memory == Some(100))
        );
        assert!(
            second
                .iter()
                .all(|point| point.cpu == Some(2000) && point.memory == Some(200))
        );
        containers[0].state = "exited".into();
        store.reconcile(&containers);
        assert_eq!(store.timed_history(&containers[..1])[0].memory, Some(100));
        let missing = store.timed_history(&[container("replacement")]);
        assert!(
            missing
                .iter()
                .all(|point| point.cpu.is_none() && point.memory.is_none())
        );
    }

    #[test]
    fn performance_history_tracks_memory_and_real_samples_with_gaps() {
        let mut store = Store::default();
        let containers = vec![container("one"), container("two")];
        let now = Instant::now();
        let stale = Duration::from_secs(3);
        store.apply(
            &containers,
            [
                ("/one".into(), Ok(reading(Some(120.0), Some(100), now))),
                ("/two".into(), Ok(reading(Some(80.0), Some(200), now))),
            ]
            .into(),
        );
        assert_eq!(
            store.performance_history(&containers, now, stale),
            (vec![Some(20_000)], vec![Some(300)])
        );
        store.apply(
            &containers,
            [
                (
                    "/one".into(),
                    Ok(reading(Some(0.0), Some(0), now + Duration::from_secs(1))),
                ),
                ("/two".into(), Err("missing".into())),
            ]
            .into(),
        );
        assert_eq!(
            store.performance_history(&containers, now + Duration::from_secs(1), stale),
            (vec![Some(20_000), None], vec![Some(300), None])
        );
        let after = now + Duration::from_secs(6);
        let first = store.performance_history(&containers, after, stale);
        assert_eq!(first.0, vec![Some(20_000), None, None]);
        assert_eq!(
            store.performance_history(&containers, after, stale),
            first,
            "Idle rendering must not append frames"
        );
        store.apply(
            &containers,
            [
                ("/one".into(), Ok(reading(Some(0.0), Some(0), after))),
                ("/two".into(), Ok(reading(Some(0.0), Some(0), after))),
            ]
            .into(),
        );
        let (cpu, memory) = store.performance_history(&containers, after, stale);
        assert_eq!(cpu, vec![Some(20_000), None, None, Some(0)]);
        assert_eq!(memory, vec![Some(300), None, None, Some(0)]);
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
