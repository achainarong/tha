//! Named views the user saved on purpose. Unlike preferences they may hold filter text, so they
//! are only written when asked, in their own private file. Applying one never connects a server.
use crate::app::{OverviewMode, Tab};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_VIEWS: usize = 64;
pub const MAX_NAME_BYTES: usize = 64;
const MAX_TEXT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Overview {
    #[default]
    Containers,
    Images,
    Volumes,
    Security,
}

impl From<OverviewMode> for Overview {
    fn from(mode: OverviewMode) -> Self {
        match mode {
            OverviewMode::Containers => Self::Containers,
            OverviewMode::Images => Self::Images,
            OverviewMode::Volumes => Self::Volumes,
            OverviewMode::Security => Self::Security,
        }
    }
}

impl From<Overview> for OverviewMode {
    fn from(overview: Overview) -> Self {
        match overview {
            Overview::Containers => Self::Containers,
            Overview::Images => Self::Images,
            Overview::Volumes => Self::Volumes,
            Overview::Security => Self::Security,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedProject {
    pub connection: String,
    pub namespace: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct SavedInventory {
    pub connection: String,
    pub query: String,
    pub filter: usize,
    pub sort: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedView {
    pub name: String,
    #[serde(default)]
    pub overview: Overview,
    pub tab: Tab,
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub project: Option<SavedProject>,
    #[serde(default)]
    pub selected: Option<crate::preferences::Row>,
    #[serde(default)]
    pub tree_query: String,
    #[serde(default)]
    pub log_terms: String,
    #[serde(default)]
    pub log_advanced: String,
    #[serde(default)]
    pub inventory: Option<SavedInventory>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u8,
    #[serde(default)]
    views: Vec<SavedView>,
}

pub fn default_path() -> Option<PathBuf> {
    crate::preferences::default_path().map(|path| path.with_file_name("views.toml"))
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("{}", crate::i18n::t("A saved view needs a name"));
    }
    if name.len() > MAX_NAME_BYTES || crate::model::safe_text(name) != name {
        bail!(
            "{}",
            crate::i18n::t("View names can use up to 64 bytes of plain text")
        );
    }
    Ok(())
}

pub fn load(path: &Path) -> Result<Vec<SavedView>> {
    let Some(text) = crate::preferences::read_private(path)? else {
        return Ok(Vec::new());
    };
    let file: File = toml::from_str(&text)?;
    if file.version != 1 || file.views.len() > MAX_VIEWS {
        bail!("Unsupported saved views version or too many views");
    }
    for view in &file.views {
        validate_name(&view.name)?;
        let long = [&view.tree_query, &view.log_terms, &view.log_advanced]
            .iter()
            .any(|text| text.len() > MAX_TEXT_BYTES);
        if long {
            bail!("Saved view text is too long");
        }
    }
    Ok(file.views)
}

pub fn save(path: &Path, views: &[SavedView]) -> Result<()> {
    let text = toml::to_string(&File {
        version: 1,
        views: views.to_vec(),
    })?;
    crate::preferences::write_private(path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn view(name: &str) -> SavedView {
        SavedView {
            name: name.into(),
            overview: Overview::Images,
            tab: Tab::Logs,
            machine: Some("prod".into()),
            project: Some(SavedProject {
                connection: "Docker/Local".into(),
                namespace: None,
                name: "shop".into(),
            }),
            selected: None,
            tree_query: "api".into(),
            log_terms: "backend1 500".into(),
            log_advanced: "{}".into(),
            inventory: Some(SavedInventory {
                connection: "Docker/Local".into(),
                query: "nginx".into(),
                filter: 1,
                sort: 2,
            }),
        }
    }

    #[test]
    fn views_round_trip_privately_and_bad_files_are_rejected_and_left_alone() {
        let directory = std::env::temp_dir().join(format!("tha-views-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("views.toml");
        assert!(load(&path).unwrap().is_empty());

        let views = vec![view("Production failures"), view("Backend errors")];
        save(&path, &views).unwrap();
        assert_eq!(load(&path).unwrap(), views);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        for bad in ["version = 2", "version = 1\nunknown = true"] {
            std::fs::write(&path, bad).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
        }
        let bad_name = "version = 1\n[[views]]\nname = \"a\\u001b[2J\"\ntab = \"logs\"\n";
        std::fs::write(&path, bad_name).unwrap();
        assert!(load(&path).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn names_must_be_short_plain_text() {
        assert!(validate_name("Production failures").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name(&"x".repeat(MAX_NAME_BYTES + 1)).is_err());
        assert!(validate_name("bad\u{1b}[2Jname").is_err());
    }
}
