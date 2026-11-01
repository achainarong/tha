//! Opt-in checks against a real engine. They run `ls` and read files in an existing container,
//! and change nothing in it:
//!
//!   THA_LIVE_SOCKET=/path/docker.sock THA_LIVE_CONTAINER=name \\
//!     cargo test --test files_live -- --ignored
use tha::{
    config::{BackendKind, Connection},
    files::{self, Kind, Source},
};

fn source() -> Source {
    let socket = std::env::var("THA_LIVE_SOCKET").expect("THA_LIVE_SOCKET");
    let container = std::env::var("THA_LIVE_CONTAINER").expect("THA_LIVE_CONTAINER");
    Source::Container {
        connection: Connection {
            name: "live".into(),
            kind: BackendKind::Docker,
            endpoint: format!("unix://{socket}"),
        },
        id: container.clone(),
        label: container,
    }
}

#[tokio::test]
#[ignore = "needs a running container"]
async fn browsing_sizing_and_downloading_work_against_a_real_engine() {
    let source = source();
    let root = files::list(&source, "/").await.unwrap();
    assert!(
        root.entries
            .iter()
            .any(|entry| entry.name == "etc" && entry.kind == Kind::Folder),
        "{root:?}"
    );
    let etc = files::list(&source, "/etc").await.unwrap();
    assert!(etc.entries.iter().any(|entry| entry.name == "hostname"));

    let file = files::stat(&source, "/etc/hostname").await.unwrap();
    assert_eq!(file.kind, Kind::File);
    assert!(file.size > 0);
    let folder = files::stat(&source, "/etc").await.unwrap();
    assert_eq!(folder.kind, Kind::Folder);
    assert!(files::stat(&source, "/no/such/path").await.is_err());
    assert!(files::list(&source, "/no/such/path").await.is_err());

    let mut totals = files::Totals::default();
    files::count(&source, "/etc", |update| totals = update)
        .await
        .unwrap();
    assert!(
        totals.done && totals.files > 1 && totals.bytes > 0,
        "{totals:?}"
    );

    let directory = std::env::temp_dir().join(format!("tha-live-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let saved = directory.join("hostname");
    let written = files::download(&source, "/etc/hostname", &saved, false, |_| {})
        .await
        .unwrap();
    assert_eq!(written, file.size);
    assert_eq!(std::fs::read(&saved).unwrap().len() as u64, file.size);

    let archive = directory.join("etc.tar");
    files::download(&source, "/etc", &archive, true, |_| {})
        .await
        .unwrap();
    let listing = std::process::Command::new("tar")
        .arg("-tf")
        .arg(&archive)
        .output()
        .unwrap();
    assert!(listing.status.success(), "{listing:?}");
    assert!(String::from_utf8_lossy(&listing.stdout).contains("etc/hostname"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
#[ignore = "needs a running container; writes one small file under /tmp in it"]
async fn uploading_and_network_tests_work_against_a_real_engine() {
    let source = source();
    let directory = std::env::temp_dir().join(format!("tha-live-up-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let local = directory.join("tha-live-upload.txt");
    std::fs::write(&local, b"hello from tha").unwrap();
    files::upload(&source, &local, "/tmp", |_| {})
        .await
        .unwrap();
    let saved = directory.join("back.txt");
    files::download(&source, "/tmp/tha-live-upload.txt", &saved, false, |_| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), b"hello from tha");

    let found = files::network_test(&source, "localhost", 80).await.unwrap();
    assert_eq!(found.resolved, Some(true), "{found:?}");
    std::fs::remove_dir_all(directory).unwrap();
}

/// The server path without SSH: the real agent code in front of the real engine.
#[tokio::test]
#[ignore = "needs a running container; writes one small file under /tmp in it"]
async fn the_agent_routes_work_against_a_real_engine_and_refuse_what_is_off() {
    use tha::agent::{Agent, serve};
    let socket = std::env::var("THA_LIVE_SOCKET").expect("THA_LIVE_SOCKET");
    let container = std::env::var("THA_LIVE_CONTAINER").expect("THA_LIVE_CONTAINER");
    let connection = tha::remote::connection("live-test", "docker");
    let path = tha::remote::local_socket(&connection.endpoint).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let allowed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let open = allowed.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let on = open.load(std::sync::atomic::Ordering::SeqCst);
            let agent = Agent {
                allow_delete: false,
                allow_exec: on,
                allow_images: false,
                allow_download: on,
                allow_compose: false,
                allow_upload: on,
                socket: socket.clone().into(),
                audit: |_| {},
                security: None,
                history: None,
            };
            tokio::spawn(async move {
                let _ = serve(stream, agent).await;
            });
        }
    });
    let source = Source::Container {
        connection,
        id: container.clone(),
        label: container,
    };

    let root = files::list(&source, "/").await.unwrap();
    assert!(
        root.entries.iter().any(|entry| entry.name == "etc"),
        "{root:?}"
    );
    assert_eq!(
        files::stat(&source, "/etc/hostname").await.unwrap().kind,
        Kind::File
    );
    assert!(files::stat(&source, "/no/such/path").await.is_err());

    let mut totals = files::Totals::default();
    files::count(&source, "/etc", |update| totals = update)
        .await
        .unwrap();
    assert!(totals.done && totals.files > 1, "{totals:?}");

    let directory = std::env::temp_dir().join(format!("tha-live-agent-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let archive = directory.join("etc.tar");
    files::download(&source, "/etc", &archive, true, |_| {})
        .await
        .unwrap();
    let local = directory.join("tha-live-agent.txt");
    std::fs::write(&local, vec![b'x'; 300_000]).unwrap();
    files::upload(&source, &local, "/tmp", |_| {})
        .await
        .unwrap();
    let back = directory.join("back.txt");
    files::download(&source, "/tmp/tha-live-agent.txt", &back, false, |_| {})
        .await
        .unwrap();
    assert_eq!(std::fs::metadata(&back).unwrap().len(), 300_000);
    let found = files::network_test(&source, "localhost", 80).await.unwrap();
    assert_eq!(found.resolved, Some(true), "{found:?}");

    allowed.store(false, std::sync::atomic::Ordering::SeqCst);
    let refused = format!("{:#}", files::list(&source, "/").await.unwrap_err());
    assert!(refused.contains("--allow-exec"), "{refused}");
    let refused = format!(
        "{:#}",
        files::download(&source, "/etc", &directory.join("no.tar"), true, |_| {})
            .await
            .unwrap_err()
    );
    assert!(refused.contains("--allow-download"), "{refused}");
    assert!(!directory.join("no.tar").exists());
    let refused = format!(
        "{:#}",
        files::upload(&source, &local, "/tmp", |_| {})
            .await
            .unwrap_err()
    );
    assert!(refused.contains("--allow-upload"), "{refused}");
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
#[ignore = "needs a running container; writes small files under /tmp in it"]
async fn editing_a_container_file_keeps_its_owner_and_mode_and_refuses_a_changed_file() {
    use tha::compose_edit::{Location, fetch, send_back};
    let source = source();
    let directory = std::env::temp_dir().join(format!("tha-live-edit-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = "/tmp/tha-live-edit.conf";
    // A starting point with a real owner and mode, written the same way an edit writes.
    let start = files::Header {
        name: "tha-live-edit.conf".into(),
        kind: Kind::File,
        size: 3,
        modified: 1_700_000_000,
        mode: 0o640,
        uid: 101,
        gid: 101,
    };
    let archive = files::edited_archive("tha-live-edit.conf", &start, b"", b"old");
    files::upload_bytes(&source, "/tmp", archive).await.unwrap();

    let location = Location::Container {
        source: source.clone(),
        path: path.into(),
    };
    let fetched = fetch(&location).await.unwrap();
    assert!(fetched.writable);
    assert!(
        !send_back(&location, &fetched).await.unwrap(),
        "No change, nothing sent"
    );

    std::fs::write(&fetched.path, b"new contents").unwrap();
    assert!(send_back(&location, &fetched).await.unwrap());
    let after = files::read_start(&source, path, 4096).await.unwrap();
    assert_eq!(after.bytes, b"new contents");
    assert_eq!(
        (
            after.header.mode & 0o777,
            after.header.uid,
            after.header.gid
        ),
        (0o640, 101, 101)
    );
    let backup = files::read_start(&source, "/tmp/tha-live-edit.conf.tha-backup", 4096)
        .await
        .unwrap();
    assert_eq!(backup.bytes, b"old");
    assert_eq!(backup.header.mode & 0o777, 0o600);

    // Someone else saved it in the meantime, so this edit must not overwrite theirs.
    let stale = fetched;
    std::fs::write(&stale.path, b"my later edit").unwrap();
    let error = send_back(&location, &stale).await.unwrap_err();
    assert!(error.to_string().contains("changed"), "{error:#}");
    assert_eq!(
        files::read_start(&source, path, 4096).await.unwrap().bytes,
        b"new contents"
    );

    let binary = files::read_start(&source, "/bin/sh", 4096).await;
    if let Ok(start) = binary {
        assert!(files::looks_binary(&start.bytes, start.truncated));
    }
    std::fs::remove_dir_all(directory).unwrap();
}
