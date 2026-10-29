//! Bounded, engine-scoped metadata cache. Image architecture is not host architecture.
use crate::{config::Connection, model::Container};
use anyhow::{Result, bail};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePlatform {
    pub os: String,
    pub architecture: String,
    pub variant: Option<String>,
}

impl ImagePlatform {
    pub fn new(
        os: Option<String>,
        architecture: Option<String>,
        variant: Option<String>,
    ) -> Result<Self> {
        let architecture = architecture
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Image architecture was not reported by the runtime"))?;
        Ok(Self {
            os: os
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "unknown-os".into()),
            architecture,
            variant: variant.filter(|value| !value.is_empty()),
        })
    }
    pub fn label(&self) -> String {
        format!(
            "{}/{}{}{}",
            self.os,
            self.architecture,
            self.variant
                .as_ref()
                .map(|variant| format!("/{variant}"))
                .unwrap_or_default(),
            match self.architecture.as_str() {
                "arm64" | "aarch64" => " (ARM64)",
                "amd64" | "x86_64" => " (AMD64 / x86_64)",
                "arm" => " (ARM)",
                _ => "",
            }
        )
    }
}

pub fn immutable_image_id(value: &str) -> bool {
    value.strip_prefix("sha256:").unwrap_or(value).len() == 64
        && value
            .strip_prefix("sha256:")
            .unwrap_or(value)
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub engine: usize,
    pub identity: Option<String>,
    pub namespace: Option<String>,
    pub image: String,
}

impl Key {
    pub fn new(engine: usize, identity: Option<String>, container: &Container) -> Self {
        Self {
            engine,
            identity,
            namespace: container.namespace.clone(),
            image: container
                .image_id
                .as_ref()
                .filter(|id| immutable_image_id(id))
                .cloned()
                .unwrap_or_else(|| format!("container:{}", container.key())),
        }
    }
}

pub enum State {
    Loading,
    Ready(ImagePlatform),
    Unavailable(String),
}
struct Entry {
    key: Key,
    state: State,
    updated: Instant,
}

#[derive(Default)]
pub struct Cache {
    entries: VecDeque<Entry>,
}

impl Cache {
    pub fn get(&self, key: &Key) -> Option<&State> {
        self.entries
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| &entry.state)
    }
    pub fn needs_lookup(&self, key: &Key) -> bool {
        self.entries
            .iter()
            .find(|entry| &entry.key == key)
            .is_none_or(|entry| {
                matches!(entry.state, State::Unavailable(_))
                    && entry.updated.elapsed() >= Duration::from_secs(5)
            })
    }
    pub fn insert(&mut self, key: Key, state: State) {
        self.entries.retain(|entry| entry.key != key);
        if self.entries.len() == 256 {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            key,
            state,
            updated: Instant::now(),
        });
    }
    pub fn cancel_loading(&mut self, key: &Key) {
        self.entries
            .retain(|entry| &entry.key != key || !matches!(entry.state, State::Loading));
    }
}

pub struct Event {
    pub key: Key,
    pub result: Result<ImagePlatform, String>,
}

pub async fn lookup(connection: &Connection, container: &Container) -> Result<ImagePlatform> {
    tokio::time::timeout(Duration::from_secs(4), async {
        match connection.kind {
            crate::config::BackendKind::Docker => {
                crate::backend::docker::platform(connection, container).await
            }
            crate::config::BackendKind::Containerd => {
                crate::backend::containerd::platform(connection, container).await
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Image platform lookup timed out"))?
}

pub fn require_image_id(value: Option<String>) -> Result<String> {
    match value {
        Some(id) if immutable_image_id(&id) => Ok(id),
        _ => bail!(
            "The runtime did not report a resolved image ID; architecture is unavailable rather than guessed from a mutable tag"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_come_from_image_metadata_not_the_host() {
        let amd = ImagePlatform::new(Some("linux".into()), Some("amd64".into()), None).unwrap();
        let arm = ImagePlatform::new(
            Some("linux".into()),
            Some("arm64".into()),
            Some("v8".into()),
        )
        .unwrap();
        assert_eq!(amd.label(), "linux/amd64 (AMD64 / x86_64)");
        assert_eq!(arm.label(), "linux/arm64/v8 (ARM64)");
        assert!(require_image_id(Some("redis:latest".into())).is_err());
        assert!(ImagePlatform::new(None, None, None).is_err());
    }

    #[test]
    fn cache_is_bounded_engine_scoped_and_cancelled_lookups_can_retry() {
        let container = Container {
            id: "same".into(),
            ..Default::default()
        };
        let key = Key::new(0, Some("engine-a".into()), &container);
        let other = Key::new(1, Some("engine-b".into()), &container);
        assert_ne!(key, other);
        let mut cache = Cache::default();
        cache.insert(key.clone(), State::Loading);
        assert!(!cache.needs_lookup(&key));
        cache.cancel_loading(&key);
        assert!(cache.needs_lookup(&key));
        cache.insert(key.clone(), State::Unavailable("denied".into()));
        assert!(!cache.needs_lookup(&key));
        cache.entries[0].updated -= Duration::from_secs(6);
        assert!(cache.needs_lookup(&key));
        for index in 0..257 {
            cache.insert(
                Key {
                    image: format!("image-{index}"),
                    ..key.clone()
                },
                State::Loading,
            );
        }
        assert_eq!(cache.entries.len(), 256);
        assert!(cache.get(&key).is_none());
    }
}
