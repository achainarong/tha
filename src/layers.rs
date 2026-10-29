//! Read-only resolved-image metadata. Filesystem diff IDs are not history IDs.
use crate::{
    config::{BackendKind, Connection},
    model::Container,
    platform,
};
use anyhow::{Context, Result};

pub const MAX_ROWS: usize = 512;
pub const MAX_TEXT: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub image: platform::Key,
    pub endpoint: String,
}

#[derive(Debug, Clone)]
pub struct History {
    pub instruction: String,
    pub comment: String,
    pub size: Option<u64>,
    /// Only explicit OCI metadata qualifies. Zero bytes does not imply empty_layer.
    pub empty_layer: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub id: String,
    /// Uncompressed filesystem diff IDs, base to top. Sizes are not in this metadata.
    pub filesystem: Vec<String>,
    pub history: Vec<History>,
    pub warnings: Vec<String>,
}

pub fn text(value: &str) -> String {
    let mut value = crate::model::safe_text(value);
    if value.len() > MAX_TEXT {
        let mut end = MAX_TEXT - 20;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        value.push_str(" … [truncated]");
    }
    value
}

impl Image {
    pub fn bound(mut self) -> Self {
        if self.filesystem.len() > MAX_ROWS || self.history.len() > MAX_ROWS {
            self.warnings.push(format!(
                "Only the first {MAX_ROWS} rows in each section are retained"
            ));
        }
        self.filesystem.truncate(MAX_ROWS);
        self.history.truncate(MAX_ROWS);
        self.id = text(&self.id);
        for id in &mut self.filesystem {
            *id = text(id);
        }
        for row in &mut self.history {
            row.instruction = text(&row.instruction);
            row.comment = text(&row.comment);
        }
        for warning in &mut self.warnings {
            *warning = text(warning);
        }
        self
    }
    pub fn rows(&self) -> usize {
        self.filesystem.len() + self.history.len()
    }
}

#[derive(Default)]
pub enum State {
    #[default]
    Idle,
    Loading,
    Ready(Image),
    Failed(String),
}

#[derive(Default)]
pub struct View {
    pub generation: u64,
    pub key: Option<Key>,
    pub state: State,
    pub selected: usize,
    pub expanded: bool,
}

impl View {
    pub fn select(&mut self, key: Option<Key>) {
        self.generation = self.generation.wrapping_add(1);
        self.state = if key.is_some() {
            State::Loading
        } else {
            State::Idle
        };
        self.key = key;
        self.selected = 0;
        self.expanded = false;
    }
    pub fn complete(&mut self, event: Event) {
        if self.key.as_ref() == Some(&event.key) && self.generation == event.generation {
            self.state = match event.result {
                Ok(image) => State::Ready(image),
                Err(error) => State::Failed(error),
            };
        }
    }
    pub fn move_by(&mut self, amount: isize) {
        let count = match &self.state {
            State::Ready(image) => image.rows(),
            _ => 0,
        };
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(count.saturating_sub(1));
    }
}

pub struct Event {
    pub generation: u64,
    pub key: Key,
    pub result: Result<Image, String>,
}

pub async fn lookup(connection: &Connection, container: &Container) -> Result<Image> {
    tokio::time::timeout(std::time::Duration::from_secs(6), async {
        match connection.kind {
            BackendKind::Docker => crate::backend::docker::layers(connection, container).await,
            BackendKind::Containerd => {
                crate::backend::containerd::layers(connection, container).await
            }
        }
    })
    .await
    .context("Image layers lookup timed out after 6s")?
    .map(Image::bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_of_same_target_discards_results_queued_by_a_cancelled_lookup() {
        let container = crate::model::Container {
            id: "fixture".into(),
            ..Default::default()
        };
        let key = Key {
            image: platform::Key::new(0, Some("fixture-runtime".into()), &container),
            endpoint: "fixture".into(),
        };
        let mut view = View::default();
        view.select(Some(key.clone()));
        let old = view.generation;
        view.select(None);
        view.select(Some(key.clone()));
        view.complete(Event {
            key: key.clone(),
            generation: old,
            result: Err("old failure".into()),
        });
        assert!(matches!(view.state, State::Loading));
        view.complete(Event {
            key,
            generation: view.generation,
            result: Err("new failure".into()),
        });
        assert!(matches!(view.state, State::Failed(error) if error == "new failure"));
    }
    #[test]
    fn metadata_is_bounded_sanitized_and_zero_bytes_are_not_treated_as_metadata_steps() {
        let image = Image {
            id: "resolved".into(),
            filesystem: vec!["sha256:diff".into(); MAX_ROWS + 1],
            history: vec![History {
                instruction: format!("\x1b]52;c;secret\x07{}", "x".repeat(10000)),
                comment: String::new(),
                size: Some(0),
                empty_layer: None,
            }],
            warnings: vec![],
        }
        .bound();
        assert_eq!(image.filesystem.len(), MAX_ROWS);
        assert!(!image.warnings.is_empty());
        assert!(image.history[0].instruction.len() <= MAX_TEXT);
        assert!(!image.history[0].instruction.contains('\x1b'));
        assert_eq!(image.history[0].empty_layer, None);
    }
}
