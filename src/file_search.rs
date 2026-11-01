//! Explicit, cancellable filesystem searches. Reuses directory listing transport and permissions;
//! no shell, `find` program, symlink traversal, or file-content reads are involved.
use crate::{
    downloads::Event,
    files::{self, Kind, Listing, Source},
};
use std::{collections::VecDeque, future::Future, time::Duration};
use tokio::{sync::mpsc, time::Instant};

pub const MAX_RESULTS: usize = 500;
const MAX_FOLDERS: usize = 2000;
const MAX_ENTRIES: usize = 100_000;
const BUDGET: Duration = Duration::from_secs(30);
const FOLDER_TIMEOUT: Duration = Duration::from_secs(5);
const SKIPPED: [&str; 3] = ["/proc", "/sys", "/dev"];

pub struct Request {
    pub generation: u64,
    pub source: Source,
    pub root: String,
    pub query: String,
    pub demo: bool,
}

pub async fn run(request: Request, sender: mpsc::Sender<Event>) {
    let source = request.source.clone();
    let demo = request.demo;
    walk(request, sender, move |path| {
        let source = source.clone();
        async move {
            if demo {
                Ok(crate::downloads::demo_listing(&path))
            } else {
                files::list(&source, &path)
                    .await
                    .map_err(|error| crate::model::safe_text(&format!("{error:#}")))
            }
        }
    })
    .await;
}

async fn walk<F, Fut>(request: Request, sender: mpsc::Sender<Event>, mut list: F)
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Listing, String>>,
{
    let key = request.source.key();
    let query = request.query.to_lowercase();
    let mut queue = VecDeque::from([request.root.clone()]);
    let deadline = Instant::now() + BUDGET;
    let (mut folders, mut entries, mut matches, mut unreadable) = (0, 0, 0, 0);
    let mut incomplete = false;
    let mut error = None;
    while let Some(path) = queue.pop_front() {
        if folders >= MAX_FOLDERS || entries >= MAX_ENTRIES || Instant::now() >= deadline {
            incomplete = true;
            break;
        }
        folders += 1;
        let result = tokio::time::timeout_at(
            deadline.min(Instant::now() + FOLDER_TIMEOUT),
            list(path.clone()),
        )
        .await
        .unwrap_or_else(|_| Err(crate::i18n::t("Timed out").to_owned()));
        let mut listing = match result {
            Ok(listing) => listing,
            Err(reason) => {
                unreadable += 1;
                incomplete = true;
                if path == request.root {
                    error = Some(reason);
                    break;
                }
                continue;
            }
        };
        if listing.entries.len() > MAX_ENTRIES - entries {
            listing.entries.truncate(MAX_ENTRIES - entries);
            listing.truncated = true;
        }
        incomplete |= listing.truncated;
        entries += listing.entries.len();
        for entry in &listing.entries {
            let child = files::join(&path, &entry.name);
            matches += usize::from(child.to_lowercase().contains(&query));
            if entry.kind == Kind::Folder && !SKIPPED.contains(&child.as_str()) {
                // Bound the queue as well as the work. Unqueued directories mean partial results.
                if folders + queue.len() < MAX_FOLDERS {
                    queue.push_back(child);
                } else {
                    incomplete = true;
                }
            }
        }
        if sender
            .send(Event::SearchListed {
                key: key.clone(),
                generation: request.generation,
                path,
                listing,
            })
            .await
            .is_err()
        {
            return;
        }
        if matches >= MAX_RESULTS {
            incomplete = true;
            break;
        }
    }
    let _ = sender
        .send(Event::SearchFinished {
            key,
            generation: request.generation,
            incomplete,
            unreadable,
            error,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{BackendKind, Connection},
        files::Entry,
    };
    use std::collections::BTreeMap;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry {
            name: name.into(),
            kind,
            size: (kind == Kind::File).then_some(1),
        }
    }

    async fn fixture(root: &str, query: &str, listings: BTreeMap<String, Listing>) -> Vec<Event> {
        let request = Request {
            generation: 1,
            root: root.into(),
            query: query.into(),
            demo: false,
            source: Source::Container {
                connection: Connection {
                    name: "fixture".into(),
                    kind: BackendKind::Docker,
                    endpoint: "unix:///not-a-real-engine".into(),
                },
                id: "fixture".into(),
                label: "fixture".into(),
            },
        };
        let (tx, mut rx) = mpsc::channel(4);
        let work = walk(request, tx, |path| {
            std::future::ready(
                listings
                    .get(&path)
                    .cloned()
                    .ok_or_else(|| "Permission denied".into()),
            )
        });
        let collect = async {
            let mut events = Vec::new();
            while let Some(event) = rx.recv().await {
                events.push(event);
            }
            events
        };
        tokio::join!(work, collect).1
    }

    #[tokio::test]
    async fn a_folder_search_never_leaves_its_scope_and_can_go_beyond_background_depth() {
        let mut listings = BTreeMap::new();
        let mut path = "/scope".to_owned();
        for _ in 0..10 {
            listings.insert(
                path.clone(),
                Listing {
                    entries: vec![entry("nested", Kind::Folder)],
                    truncated: false,
                },
            );
            path.push_str("/nested");
        }
        listings.insert(
            path.clone(),
            Listing {
                entries: vec![entry("target.txt", Kind::File)],
                truncated: false,
            },
        );
        let events = fixture("/scope", "target", listings).await;
        assert!(events.iter().any(
            |event| matches!(event, Event::SearchListed { path: found, .. } if *found == path)
        ));
        assert!(events.iter().all(|event| !matches!(event, Event::SearchListed { path, .. } if !path.starts_with("/scope"))));
        assert!(matches!(
            events.last(),
            Some(Event::SearchFinished {
                incomplete: false,
                unreadable: 0,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_system_search_skips_virtual_trees_and_links_and_reports_unreadable_folders() {
        let root = Listing {
            entries: vec![
                entry("proc", Kind::Folder),
                entry("sys", Kind::Folder),
                entry("dev", Kind::Folder),
                entry("unreadable", Kind::Folder),
                entry("link", Kind::Link),
            ],
            truncated: false,
        };
        let events = fixture("/", "anything", BTreeMap::from([("/".into(), root)])).await;
        assert!(matches!(
            events.last(),
            Some(Event::SearchFinished {
                incomplete: true,
                unreadable: 1,
                error: None,
                ..
            })
        ));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::SearchListed { .. }))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn search_result_limits_are_reported_as_incomplete_not_as_a_complete_search() {
        let listing = Listing {
            entries: (0..MAX_RESULTS + 1)
                .map(|i| entry(&format!("match-{i}"), Kind::File))
                .collect(),
            truncated: false,
        };
        let events = fixture("/", "match", BTreeMap::from([("/".into(), listing)])).await;
        assert!(matches!(
            events.last(),
            Some(Event::SearchFinished {
                incomplete: true,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_root_failure_is_reported_without_claiming_no_files_exist() {
        let events = fixture("/", "anything", BTreeMap::new()).await;
        assert!(
            matches!(events.last(), Some(Event::SearchFinished { incomplete: true, error: Some(error), .. }) if error == "Permission denied")
        );
    }

    #[tokio::test]
    async fn the_directory_budget_bounds_work_and_marks_the_results_incomplete() {
        let mut listings = BTreeMap::new();
        let root = Listing {
            entries: (0..MAX_FOLDERS + 1)
                .map(|i| entry(&format!("folder-{i}"), Kind::Folder))
                .collect(),
            truncated: false,
        };
        for entry in &root.entries {
            listings.insert(format!("/{}", entry.name), Listing::default());
        }
        listings.insert("/".into(), root);
        let events = fixture("/", "no-such-file", listings).await;
        assert!(
            events
                .iter()
                .filter(|event| matches!(event, Event::SearchListed { .. }))
                .count()
                <= MAX_FOLDERS
        );
        assert!(matches!(
            events.last(),
            Some(Event::SearchFinished {
                incomplete: true,
                ..
            })
        ));
    }
}
