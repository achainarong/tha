use std::collections::BTreeMap;

pub type Labels = BTreeMap<String, String>;

#[derive(Debug, Clone)]
pub struct PortBinding {
    pub host_ip: Option<String>,
    pub host_port: Option<u16>,
    pub container_port: u16,
    pub protocol: String,
}

impl PortBinding {
    pub fn overview(&self) -> String {
        let destination = format!("{}/{}", self.container_port, self.protocol);
        let Some(port) = self.host_port else {
            return format!("{destination} — exposed, no host port");
        };
        let Some(ip) = self.host_ip.as_deref().filter(|ip| !ip.is_empty()) else {
            return format!("host port {port} → {destination} (host address not reported)");
        };
        match ip.parse::<std::net::IpAddr>() {
            Ok(address) if address.is_loopback() => {
                let local = if address.is_ipv6() {
                    format!("[::1]:{port}")
                } else if ip == "127.0.0.1" {
                    format!("localhost:{port}")
                } else {
                    format!("{ip}:{port}")
                };
                format!("{local} → {destination} (loopback only · {ip})")
            }
            Ok(address) if address.is_unspecified() => {
                let host = if address.is_ipv6() {
                    format!("[{ip}]:{port}")
                } else {
                    format!("{ip}:{port}")
                };
                format!("{host} → {destination} (all interfaces; not localhost-only)")
            }
            _ => {
                let host = if ip.contains(':') {
                    format!("[{ip}]:{port}")
                } else {
                    format!("{ip}:{port}")
                };
                format!("{host} → {destination} (specific host address)")
            }
        }
    }
}

#[cfg(test)]
mod port_tests {
    use super::*;
    #[test]
    fn ports_distinguish_loopback_wildcards_specific_addresses_and_unpublished_ports() {
        let mut binding = PortBinding {
            host_ip: Some("127.0.0.1".into()),
            host_port: Some(8080),
            container_port: 80,
            protocol: "tcp".into(),
        };
        assert_eq!(
            binding.overview(),
            "localhost:8080 → 80/tcp (loopback only · 127.0.0.1)"
        );
        binding.host_ip = Some("::1".into());
        assert!(binding.overview().contains("[::1]:8080"));
        for ip in ["0.0.0.0", "::"] {
            binding.host_ip = Some(ip.into());
            assert!(
                binding
                    .overview()
                    .contains("all interfaces; not localhost-only")
            );
        }
        binding.host_ip = Some("192.0.2.1".into());
        binding.protocol = "udp".into();
        assert!(
            binding
                .overview()
                .contains("192.0.2.1:8080 → 80/udp (specific host address)")
        );
        binding.host_ip = None;
        assert!(binding.overview().contains("host address not reported"));
        binding.host_port = None;
        assert_eq!(binding.overview(), "80/udp — exposed, no host port");
    }
}

#[derive(Debug, Clone, Default)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub namespace: Option<String>,
    pub image: String,
    pub image_id: Option<String>,
    pub image_platform: Option<crate::platform::ImagePlatform>,
    pub state: String,
    pub status: String,
    pub health: Option<String>,
    pub exit_code: Option<i64>,
    pub oom_killed: bool,
    pub error: Option<String>,
    pub project: Option<String>,
    pub service: Option<String>,
    pub compose_working_dir: Option<std::path::PathBuf>,
    pub ports: Vec<String>,
    pub port_bindings: Vec<PortBinding>,
    pub networks: Vec<String>,
    pub volumes: Vec<String>,
    pub bind_mounts: Vec<String>,
    pub managed: bool,
}

impl Container {
    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace.as_deref().unwrap_or(""), self.id)
    }

    pub fn running(&self) -> bool {
        self.state.eq_ignore_ascii_case("running") || self.state.starts_with("Up")
    }

    pub fn condition(&self) -> Condition {
        let state = self.state.to_ascii_lowercase();
        let status = self.status.to_ascii_lowercase();
        let health = self.health.as_deref().unwrap_or("").to_ascii_lowercase();
        if self.oom_killed {
            return Condition::new(Tone::Error, "OOM killed");
        }
        if self.error.as_ref().is_some_and(|error| !error.is_empty())
            || matches!(state.as_str(), "dead" | "failed" | "error")
        {
            return Condition::new(Tone::Error, "failed");
        }
        if state == "restarting" {
            return Condition::new(Tone::Warning, "restarting");
        }
        if state == "paused" {
            return Condition::new(Tone::Warning, "paused");
        }
        if matches!(
            state.as_str(),
            "created" | "starting" | "stopping" | "removing"
        ) {
            return Condition::new(
                Tone::Warning,
                match state.as_str() {
                    "stopping" => "stopping",
                    "removing" => "removing",
                    "created" => "created",
                    _ => "starting",
                },
            );
        }
        if state == "exited" || status.starts_with("exited") || status.starts_with("exit ") {
            let exit = self
                .exit_code
                .or_else(|| {
                    status
                        .strip_prefix("exited (")
                        .and_then(|text| text.split(')').next())
                        .and_then(|code| code.parse::<i64>().ok())
                })
                .or_else(|| {
                    status
                        .strip_prefix("exit ")
                        .and_then(|text| text.split_whitespace().next())
                        .and_then(|code| code.parse::<i64>().ok())
                });
            return Condition::new(
                if exit.is_some_and(|code| code != 0) {
                    Tone::Error
                } else {
                    Tone::Neutral
                },
                if exit == Some(0) {
                    "completed"
                } else if exit.is_some() {
                    "failed exit"
                } else {
                    "exited"
                },
            );
        }
        if self.running() {
            if health == "unhealthy" || status.contains("(unhealthy)") {
                return Condition::new(Tone::Error, "unhealthy");
            }
            if health == "starting" || status.contains("health: starting") {
                return Condition::new(Tone::Warning, "starting");
            }
            return Condition::new(
                Tone::Good,
                if health == "healthy" || status.contains("(healthy)") {
                    "healthy"
                } else {
                    "running"
                },
            );
        }
        Condition::new(Tone::Neutral, "unknown")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Good,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Condition {
    pub tone: Tone,
    pub label: &'static str,
}

impl Condition {
    pub const fn new(tone: Tone, label: &'static str) -> Self {
        Self { tone, label }
    }
    pub fn symbol(self) -> &'static str {
        match self.tone {
            Tone::Good | Tone::Error => "●",
            Tone::Warning => "◐",
            Tone::Neutral => "○",
        }
    }
}

pub fn aggregate_condition<'a>(containers: impl IntoIterator<Item = &'a Container>) -> Condition {
    let mut good = 0;
    let mut warnings = 0;
    let mut errors = 0;
    let mut stopped = 0;
    for container in containers {
        match container.condition().tone {
            Tone::Good => good += 1,
            Tone::Warning => warnings += 1,
            Tone::Error => errors += 1,
            Tone::Neutral => stopped += 1,
        }
    }
    if errors > 0 {
        Condition::new(Tone::Error, "errors")
    } else if warnings > 0 {
        Condition::new(Tone::Warning, "warning")
    } else if good > 0 && stopped > 0 {
        Condition::new(Tone::Warning, "some stopped")
    } else if good > 0 {
        Condition::new(Tone::Good, "running")
    } else {
        Condition::new(Tone::Neutral, "stopped/empty")
    }
}

/// A runtime can be reachable while some of its workloads have failed.
/// Keep those failures red at workload scope, not at the context level.
pub fn runtime_condition<'a>(containers: impl IntoIterator<Item = &'a Container>) -> Condition {
    let condition = aggregate_condition(containers);
    if condition.tone == Tone::Error {
        Condition::new(Tone::Warning, "workload issues")
    } else {
        condition
    }
}

#[derive(Debug, Clone, Default)]
pub struct Resource {
    pub name: String,
    pub project: Option<String>,
    pub external: bool,
    pub details: String,
    pub containers: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub engine_id: Option<String>,
    pub version: String,
    pub memory_capacity_bytes: Option<u64>,
    pub cpu_count: Option<u32>,
    pub containers: Vec<Container>,
    pub networks: Vec<Resource>,
    pub volumes: Vec<Resource>,
    pub volumes_available: bool,
    pub images: Option<Vec<Image>>,
    pub namespaces: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Image {
    pub id: String,
    pub tags: Vec<String>,
    pub size: Option<u64>,
}

pub fn project(labels: &Labels) -> Option<String> {
    label(labels, "com.docker.compose.project")
        .or_else(|| label(labels, "io.podman.compose.project"))
        .or_else(|| label(labels, "nerdctl/compose/project"))
}

pub fn service(labels: &Labels) -> Option<String> {
    label(labels, "com.docker.compose.service")
        .or_else(|| label(labels, "io.podman.compose.service"))
        .or_else(|| label(labels, "nerdctl/compose/service"))
}

pub fn compose_working_dir(labels: &Labels) -> Option<std::path::PathBuf> {
    label(labels, "com.docker.compose.project.working_dir")
        .or_else(|| label(labels, "io.podman.compose.project.working_dir"))
        .map(std::path::PathBuf::from)
}

fn label(labels: &Labels, key: &str) -> Option<String> {
    labels.get(key).filter(|v| !v.is_empty()).cloned()
}

/// Never interpret terminal control sequences supplied by an engine or container.
pub fn safe_text(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            if c == '\u{1b}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if ch == '\t' {
            output.push_str("    ");
        } else if !ch.is_control() {
            output.push(ch);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping_requires_real_labels() {
        assert_eq!(project(&Labels::new()), None);
        let labels = Labels::from([("nerdctl/compose/project".into(), "shop".into())]);
        assert_eq!(project(&labels).as_deref(), Some("shop"));
        let labels = Labels::from([
            ("io.podman.compose.project".into(), "hello-podman".into()),
            ("io.podman.compose.service".into(), "hello".into()),
        ]);
        assert_eq!(project(&labels).as_deref(), Some("hello-podman"));
        assert_eq!(service(&labels).as_deref(), Some("hello"));
    }

    #[test]
    fn strips_terminal_commands_and_control_characters() {
        assert_eq!(safe_text("\x1b[31mred\x1b[0m\r\x07"), "red");
        assert_eq!(safe_text("a\x1b]52;c;secret\x07b"), "ab");
        assert_eq!(safe_text("a\x1b]8;;url\x1b\\b"), "ab");
    }

    #[test]
    fn namespaced_ids_do_not_collide() {
        let a = Container {
            id: "same".into(),
            namespace: Some("a".into()),
            ..Default::default()
        };
        let b = Container {
            namespace: Some("b".into()),
            ..a.clone()
        };
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn state_colors_distinguish_health_startup_failures_and_normal_completion() {
        let mut container = Container {
            state: "running".into(),
            status: "Up 2 hours (healthy)".into(),
            ..Default::default()
        };
        assert_eq!(container.condition().tone, Tone::Good);
        container.status = "Up 2 hours (unhealthy)".into();
        assert_eq!(container.condition().tone, Tone::Error);
        container.status = "Up 2 seconds (health: starting)".into();
        assert_eq!(container.condition().tone, Tone::Warning);
        container.health = Some("unhealthy".into());
        assert_eq!(container.condition().tone, Tone::Error);
        for state in ["restarting", "created", "paused", "stopping"] {
            container.state = state.into();
            assert_eq!(container.condition().tone, Tone::Warning);
        }
        container.health = None;
        container.state = "exited".into();
        container.status = "Exited (0) 3 hours ago".into();
        assert_eq!(container.condition().tone, Tone::Neutral);
        assert_eq!(container.condition().label, "completed");
        container.status = "Exited (137) 1 minute ago".into();
        assert_eq!(container.condition().tone, Tone::Error);
        container.exit_code = Some(0);
        assert_eq!(container.condition().tone, Tone::Neutral);
        container.oom_killed = true;
        assert_eq!(container.condition().tone, Tone::Error);
    }

    #[test]
    fn group_indicators_propagate_errors_and_do_not_hide_stopped_members() {
        let good = Container {
            state: "running".into(),
            ..Default::default()
        };
        let stopped = Container {
            state: "exited".into(),
            exit_code: Some(0),
            ..Default::default()
        };
        let starting = Container {
            state: "created".into(),
            ..Default::default()
        };
        let failed = Container {
            state: "dead".into(),
            ..Default::default()
        };
        assert_eq!(aggregate_condition([&good]).tone, Tone::Good);
        assert_eq!(aggregate_condition([&good, &stopped]).tone, Tone::Warning);
        assert_eq!(aggregate_condition([&good, &starting]).tone, Tone::Warning);
        assert_eq!(
            aggregate_condition([&good, &starting, &failed]).tone,
            Tone::Error
        );
        assert_eq!(aggregate_condition([&stopped]).tone, Tone::Neutral);
        assert_eq!(
            runtime_condition([&good, &failed]),
            Condition::new(Tone::Warning, "workload issues")
        );
        assert_eq!(failed.condition().tone, Tone::Error);
        assert_eq!(runtime_condition([&good]).tone, Tone::Good);
    }
}
