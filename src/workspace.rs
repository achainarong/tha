use crate::{app::Engine, model::Container};
use std::{collections::BTreeSet, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Project {
    pub engine: usize,
    pub namespace: Option<String>,
    pub name: String,
}

impl Project {
    pub fn contains(&self, container: &Container) -> bool {
        container.namespace == self.namespace && container.project.as_ref() == Some(&self.name)
    }
}

/// Use actual, existing working-directory metadata, never a basename guess.
/// Prefer the most specific enclosing project when projects are nested.
pub fn directory_matches(engines: &[Engine], cwd: &Path) -> Vec<Project> {
    let Ok(cwd) = cwd.canonicalize() else {
        return vec![];
    };
    let mut matches = BTreeSet::new();
    let mut deepest = 0;
    for (engine_index, engine) in engines.iter().enumerate() {
        // A server's Compose paths say nothing about this machine's filesystem.
        if engine.error.is_some()
            || engine.alias.is_some()
            || crate::remote::is_remote(&engine.connection)
        {
            continue;
        }
        let Some(snapshot) = &engine.snapshot else {
            continue;
        };
        for container in &snapshot.containers {
            let (Some(name), Some(directory)) =
                (&container.project, &container.compose_working_dir)
            else {
                continue;
            };
            if !directory.is_absolute() {
                continue;
            }
            let Ok(directory) = directory.canonicalize() else {
                continue;
            };
            if !directory.is_dir() || !cwd.starts_with(&directory) {
                continue;
            }
            let depth = directory.components().count();
            if depth < deepest {
                continue;
            }
            if depth > deepest {
                matches.clear();
                deepest = depth;
            }
            matches.insert(Project {
                engine: engine_index,
                namespace: container.namespace.clone(),
                name: name.clone(),
            });
        }
    }
    matches.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::App,
        config::{BackendKind, Connection},
        model::Snapshot,
    };
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tha-project-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn directory(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app() -> App {
        App::new(
            (0..2)
                .map(|i| Connection {
                    name: format!("engine-{i}"),
                    kind: BackendKind::Docker,
                    endpoint: format!("unix:///socket-{i}"),
                })
                .collect(),
            100,
            false,
            false,
        )
    }

    fn container(name: &str, directory: PathBuf) -> Container {
        Container {
            id: name.into(),
            project: Some(name.into()),
            compose_working_dir: Some(directory),
            ..Default::default()
        }
    }

    #[test]
    fn matches_subdirectories_not_similar_prefixes_and_prefers_nested_projects() {
        let fixture = Fixture::new();
        let project = fixture.directory("shop");
        let nested = fixture.directory("shop/nested");
        let cwd = fixture.directory("shop/nested/src");
        let sibling = fixture.directory("shop-other");
        let mut app = app();
        app.update_engine(
            0,
            Ok(Snapshot {
                containers: vec![container("shop", project), container("nested", nested)],
                ..Default::default()
            }),
        );
        let matches = directory_matches(&app.engines, &cwd);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "nested");
        assert!(directory_matches(&app.engines, &sibling).is_empty());
    }

    #[test]
    fn startup_waits_for_connections_and_picks_instead_of_guessing() {
        let fixture = Fixture::new();
        let mut app = app();
        app.configure_startup(fixture.0.clone());
        let snapshot = Snapshot {
            containers: vec![container("shop", fixture.0.clone())],
            ..Default::default()
        };
        app.update_engine(0, Ok(snapshot.clone()));
        assert!(app.project.is_none());
        app.update_engine(1, Ok(snapshot));
        assert!(app.project.is_none());
        assert_eq!(app.picker.as_ref().unwrap().choices.len(), 2);
        app.picker.as_mut().unwrap().selected = 1;
        app.choose();
        assert_eq!(app.project.as_ref().unwrap().engine, 1);
        assert!(app.back());
        assert!(app.project.is_none());
    }

    #[test]
    fn startup_opens_one_match_but_never_after_user_interaction() {
        let fixture = Fixture::new();
        let mut a = app();
        a.configure_startup(fixture.0.clone());
        let snapshot = Snapshot {
            containers: vec![container("shop", fixture.0.clone())],
            ..Default::default()
        };
        a.update_engine(0, Ok(snapshot.clone()));
        a.update_engine(1, Err("offline".into()));
        assert_eq!(a.project.as_ref().unwrap().name, "shop");
        let mut b = app();
        b.configure_startup(fixture.0.clone());
        b.cancel_startup();
        b.update_engine(0, Ok(snapshot));
        b.update_engine(1, Err("offline".into()));
        assert!(b.project.is_none());
        assert!(b.picker.is_none());
    }

    #[test]
    fn aliases_do_not_create_false_ambiguity() {
        let fixture = Fixture::new();
        let mut app = app();
        let snapshot = Snapshot {
            engine_id: Some("same".into()),
            containers: vec![container("shop", fixture.0.clone())],
            ..Default::default()
        };
        app.update_engine(0, Ok(snapshot.clone()));
        app.update_engine(1, Ok(snapshot));
        assert_eq!(directory_matches(&app.engines, &fixture.0).len(), 1);
    }
}
