use crate::{
    app::{App, Scope},
    config::{BackendKind, Connection},
    model::{Container, Resource, Snapshot},
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
                container("api-2", "shop-api-2", "shop", "api"),
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
            volumes: vec![Resource {
                name: "shop_data".into(),
                project: Some("shop".into()),
                containers: vec!["shop-postgres-1".into()],
                details: "driver: local".into(),
                ..Default::default()
            }],
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
