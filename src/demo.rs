use crate::{
    app::{App, Scope},
    config::{BackendKind, Connection},
    diagnostics::{Attempt, Endpoint, HealthCheck, Inspection, Probe, ProbeSource},
    model::{Container, PortBinding, Resource, Snapshot},
};

pub fn connections() -> Vec<Connection> {
    [
        ("OrbStack", BackendKind::Docker),
        ("Colima / development", BackendKind::Docker),
        ("containerd / local", BackendKind::Containerd),
        ("Podman / local", BackendKind::Docker),
    ]
    .into_iter()
    .map(|(name, kind)| Connection {
        name: name.into(),
        kind,
        endpoint: "demo://fixture".into(),
    })
    .collect()
}

/// What a real inspect would report for the seeded containers.
pub fn inspection(container: &Container) -> Inspection {
    let stamp = |ago: i64| {
        (time::OffsetDateTime::now_utc() - time::Duration::seconds(ago))
            .format(&time::format_description::well_known::Rfc3339)
            .ok()
    };
    let failing = container.id == "api-2";
    let health = match container.id.as_str() {
        "api-2" => HealthCheck::Configured {
            status: Some("unhealthy".into()),
            failing_streak: Some(3),
            attempts: [30, 60, 90]
                .into_iter()
                .map(|ago| Attempt {
                    started_at: stamp(ago + 1),
                    ended_at: stamp(ago),
                    exit_code: Some(1),
                    output:
                        "curl: (7) Failed to connect to localhost port 3000\nConnection refused"
                            .into(),
                    truncated: false,
                })
                .collect(),
            omitted: 0,
        },
        "db-1" => HealthCheck::Configured {
            status: Some("healthy".into()),
            failing_streak: Some(0),
            attempts: vec![Attempt {
                started_at: stamp(31),
                ended_at: stamp(30),
                exit_code: Some(0),
                output: "accepting connections".into(),
                truncated: false,
            }],
            omitted: 0,
        },
        _ => HealthCheck::NotConfigured,
    };
    let network_id = |name: &str| Some(format!("demo-network-{name}"));
    Inspection {
        id: container.id.clone(),
        state: Some(container.state.clone()),
        restart_count: Some(i64::from(failing)),
        exit_code: Some(0),
        oom_killed: Some(false),
        error: None,
        started_at: stamp(7200),
        finished_at: failing.then(|| stamp(7205)).flatten(),
        health,
        probe: match container.id.as_str() {
            "api-2" => Some(Probe {
                test: r#"["CMD-SHELL", "curl -f http://localhost:3000/health"]"#.into(),
                interval_ns: Some(30_000_000_000),
                timeout_ns: Some(5_000_000_000),
                start_period_ns: Some(10_000_000_000),
                retries: Some(3),
            }),
            "db-1" => Some(Probe {
                test: r#"["CMD", "pg_isready", "-U", "postgres"]"#.into(),
                interval_ns: Some(10_000_000_000),
                retries: Some(5),
                ..Default::default()
            }),
            _ => None,
        },
        probe_source: match container.id.as_str() {
            "api-2" => ProbeSource::Container,
            "db-1" => ProbeSource::Image,
            _ => ProbeSource::Unknown,
        },
        command: matches!(container.id.as_str(), "api-1" | "api-2")
            .then(|| "node server.js --port 3000".into()),
        network_mode: Some(
            container
                .networks
                .first()
                .cloned()
                .unwrap_or_else(|| "none".into()),
        ),
        endpoints: Some(
            container
                .networks
                .iter()
                .enumerate()
                .map(|(index, name)| Endpoint {
                    network: name.clone(),
                    id: network_id(name),
                    ipv4: Some(format!("172.18.{index}.{}", 10 + container.name.len())),
                    ipv6: None,
                    aliases: vec![
                        container.service.clone().unwrap_or_default(),
                        container.name.clone(),
                    ],
                    omitted_aliases: 0,
                })
                .collect(),
        ),
        omitted_endpoints: 0,
        ..Default::default()
    }
}

pub fn populate(app: &mut App) {
    let container = |id: &str, name: &str, project: &str, service: &str| Container {
        id: id.into(),
        name: name.into(),
        image: format!("{service}:latest"),
        state: "running".into(),
        status: "Up 2 hours (healthy)".into(),
        project: Some(project.into()),
        service: Some(service.into()),
        networks: vec![format!("{project}_default")],
        ..Default::default()
    };
    let mut api = container("api-1", "shop-api-1", "shop", "api");
    api.ports = vec!["127.0.0.1:8080 → 3000/tcp".into()];
    api.bind_mounts = vec!["/home/demo/shop → /app".into()];
    api.port_bindings = vec![PortBinding {
        host_ip: Some("127.0.0.1".into()),
        host_port: Some(8080),
        container_port: 3000,
        protocol: "tcp".into(),
    }];
    let mut api_failing = container("api-2", "shop-api-2", "shop", "api");
    api_failing.status = "Up 2 hours (unhealthy)".into();
    api_failing.health = Some("unhealthy".into());
    api_failing.port_bindings = vec![
        PortBinding {
            host_ip: Some("0.0.0.0".into()),
            host_port: Some(8080),
            container_port: 3000,
            protocol: "tcp".into(),
        },
        PortBinding {
            host_ip: None,
            host_port: None,
            container_port: 9090,
            protocol: "tcp".into(),
        },
    ];
    let mut db = container("db-1", "shop-postgres-1", "shop", "postgres");
    db.volumes = vec!["shop_data".into()];
    app.update_engine(
        0,
        Ok(Snapshot {
            engine_id: Some("orb".into()),
            version: "Docker API · demo".into(),
            memory_capacity_bytes: Some(4 * 1024 * 1024 * 1024),
            cpu_count: Some(4),
            containers: vec![
                api,
                api_failing,
                db,
                Container {
                    id: "scratch".into(),
                    name: "scratch-redis".into(),
                    image: "redis:7".into(),
                    state: "exited".into(),
                    status: "Exited (0)".into(),
                    ..Default::default()
                },
            ],
            networks: vec![
                Resource {
                    name: "shop_default".into(),
                    project: Some("shop".into()),
                    details: "driver: bridge · scope: local".into(),
                    containers: vec![
                        "shop-api-1".into(),
                        "shop-api-2".into(),
                        "shop-postgres-1".into(),
                    ],
                    ..Default::default()
                },
                Resource {
                    name: "shared_proxy".into(),
                    external: true,
                    details: "driver: bridge · external".into(),
                    ..Default::default()
                },
            ],
            volumes_available: true,
            images: Some(vec![
                crate::model::Image {
                    id: "sha256:demo-api".into(),
                    tags: vec!["api:latest".into()],
                    digests: vec![],
                    size: Some(80 * 1024 * 1024),
                },
                crate::model::Image {
                    id: "sha256:demo-redis".into(),
                    tags: vec!["redis:7".into()],
                    digests: vec![],
                    size: Some(32 * 1024 * 1024),
                },
                crate::model::Image {
                    id: "sha256:unused".into(),
                    tags: vec![],
                    digests: vec![],
                    size: Some(16 * 1024 * 1024),
                },
            ]),
            volumes: vec![
                Resource {
                    name: "shop_data".into(),
                    project: Some("shop".into()),
                    containers: vec!["shop-postgres-1".into()],
                    details: "driver: local".into(),
                    ..Default::default()
                },
                Resource {
                    name: "old_cache".into(),
                    details: "driver: local · mount: /var/lib/docker/volumes/old_cache/_data"
                        .into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
    );
    app.update_engine(
        1,
        Ok(Snapshot {
            engine_id: Some("colima".into()),
            version: "Docker API · demo".into(),
            memory_capacity_bytes: Some(2 * 1024 * 1024 * 1024),
            cpu_count: Some(2),
            containers: vec![container(
                "worker",
                "payments-worker-1",
                "payments",
                "worker",
            )],
            ..Default::default()
        }),
    );
    let mut native = container("native", "analytics-worker", "analytics", "worker");
    native.namespace = Some("default".into());
    native.networks.clear();
    let managed = Container {
        id: "kube".into(),
        name: "coredns".into(),
        namespace: Some("k8s.io".into()),
        image: "coredns:latest".into(),
        state: "running".into(),
        managed: true,
        ..Default::default()
    };
    app.update_engine(
        2,
        Ok(Snapshot {
            version: "containerd via nerdctl · demo".into(),
            memory_capacity_bytes: Some(4 * 1024 * 1024 * 1024),
            cpu_count: Some(4),
            namespaces: vec!["default".into(), "k8s.io".into()],
            containers: vec![native, managed],
            warnings: vec![
                "Network/volume inventory is unavailable for the native containerd MVP adapter."
                    .into(),
            ],
            ..Default::default()
        }),
    );
    let now = crate::history::now_ms();
    for (ago, key, name, service, action, detail) in [
        (2_400_000, "/api-1", "shop-api-1", "api", "start", ""),
        (540_000, "/api-1", "shop-api-1", "api", "oom", ""),
        (539_000, "/api-1", "shop-api-1", "api", "die", "exit 137"),
        (537_000, "/api-1", "shop-api-1", "api", "start", ""),
        (7_205_000, "/api-2", "shop-api-2", "api", "die", "exit 137"),
        (7_200_000, "/api-2", "shop-api-2", "api", "start", ""),
        (
            90_000,
            "/api-2",
            "shop-api-2",
            "api",
            "health_status",
            "unhealthy",
        ),
        (
            120_000,
            "/db-1",
            "shop-postgres-1",
            "postgres",
            "health_status",
            "healthy",
        ),
    ] {
        app.engines[0].events.add(crate::events::Event {
            at_ms: now - ago,
            key: key.into(),
            name: name.into(),
            project: Some("shop".into()),
            service: Some(service.into()),
            action: action.into(),
            detail: detail.into(),
        });
    }
    app.update_engine(
        3,
        Err(
            "Demo: Podman socket is unavailable. Start the engine separately, then use :refresh."
                .into(),
        ),
    );
    for scope in [
        Scope::Project {
            namespace: None,
            project: "shop".into(),
        },
        Scope::Services {
            namespace: None,
            project: "shop".into(),
        },
        Scope::Service {
            namespace: None,
            project: "shop".into(),
            service: "api".into(),
        },
    ] {
        app.expanded.insert(format!("0/{scope:?}"));
    }
    app.refresh_tree();
    app.selected = app
        .rows
        .iter()
        .position(|row| {
            row.engine == 0
                && matches!(&row.scope, Scope::Project { project, .. } if project == "shop")
        })
        .unwrap_or(0);
    for (engine_index, engine) in app.engines.iter().enumerate() {
        if let Some(snapshot) = &engine.snapshot {
            for container in &snapshot.containers {
                let key =
                    crate::platform::Key::new(engine_index, snapshot.engine_id.clone(), container);
                let architecture = if container.service.as_deref() == Some("postgres") {
                    "amd64"
                } else {
                    "arm64"
                };
                app.platforms.insert(
                    key,
                    crate::platform::State::Ready(crate::platform::ImagePlatform {
                        os: "linux".into(),
                        architecture: architecture.into(),
                        variant: None,
                    }),
                );
            }
        }
    }
    for engine in &mut app.engines {
        if engine.error.is_some() {
            continue;
        }
        let Some(snapshot) = &engine.snapshot else {
            continue;
        };
        for tick in 0..crate::metrics::HISTORY_LENGTH {
            let batch = snapshot
                .containers
                .iter()
                .filter(|c| c.running())
                .enumerate()
                .map(|(index, container)| {
                    let phase = ((tick + index * 7) % 20) as u64;
                    (
                        container.key(),
                        Ok(crate::metrics::Reading {
                            usage: crate::metrics::Usage {
                                cpu_percent: Some(phase as f64 * 3.0 + 2.0),
                                memory_bytes: Some((128 + phase * 2) * 1024 * 1024),
                                memory_limit_bytes: Some(1024 * 1024 * 1024),
                                memory_working_set: true,
                                ..Default::default()
                            },
                            counters: None,
                            sampled_at: std::time::Instant::now(),
                        }),
                    )
                })
                .collect();
            engine.metrics.apply(&snapshot.containers, batch);
        }
    }
}
