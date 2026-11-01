//! Versioned UI state only: never logs, filters, shell bytes, or credentials.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const LIMIT: u64 = 256 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub connection: String,
    pub scope: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub version: u8,
    pub performance_open: bool,
    pub log_wrap: bool,
    pub context_sort: crate::context_sort::ContextSort,
    pub tab: crate::app::Tab,
    pub expanded: Vec<Row>,
    pub selected: Option<Row>,
    pub project: Option<Row>,
    /// A focused machine, by name. Restoring it never connects a server.
    pub machine: Option<String>,
    pub runtimes: Vec<crate::runtime::Saved>,
    pub settings: crate::settings::SavedSettings,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            performance_open: false,
            log_wrap: true,
            context_sort: Default::default(),
            tab: crate::app::Tab::Overview,
            expanded: Vec::new(),
            selected: None,
            project: None,
            machine: None,
            runtimes: Vec::new(),
            settings: Default::default(),
        }
    }
}
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map(|root| root.join("tha/preferences.toml"))
}
pub fn load(path: &Path) -> Result<Option<Preferences>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            bail!("Preferences must be a regular file, not a symlink or device")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        _ => {}
    }
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() {
        bail!("Preferences must be a regular file");
    }
    let mut text = String::new();
    file.take(LIMIT + 1).read_to_string(&mut text)?;
    if text.len() as u64 > LIMIT {
        bail!("Preferences exceed 256 KiB");
    }
    let preferences: Preferences = toml::from_str(&text).context("Invalid preferences")?;
    if preferences.version != 1 || preferences.expanded.len() > 1024 {
        bail!("Unsupported preferences version or too many expanded branches");
    }
    if preferences.runtimes.len() > 128 {
        bail!("Too many saved runtimes");
    }
    for saved in &preferences.runtimes {
        saved.target.validate()?;
        if saved.name.is_empty()
            || saved.name.len() > 1024
            || crate::model::safe_text(&saved.name) != saved.name
        {
            bail!("Invalid saved runtime name");
        }
    }
    Ok(Some(preferences))
}

pub fn save(path: &Path, preferences: &Preferences) -> Result<()> {
    let text = toml::to_string(preferences)?;
    if text.len() as u64 > LIMIT {
        bail!("Preferences exceed 256 KiB; existing state left intact");
    }
    let parent = path.parent().context("Preferences path has no directory")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("toml.{}.new", std::process::id()));
    // create_new refuses stale temporary files/symlinks. Rename never follows a destination symlink.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok::<_, anyhow::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn private_atomic_roundtrip_and_invalid_state_is_not_silently_accepted() {
        let directory =
            std::env::temp_dir().join(format!("tha-preferences-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("preferences.toml");
        assert!(load(&path).unwrap().is_none());
        let preferences = Preferences {
            performance_open: true,
            log_wrap: false,
            ..Default::default()
        };
        save(&path, &preferences).unwrap();
        assert!(load(&path).unwrap().unwrap().performance_open);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        save(&path, &Preferences::default()).unwrap();
        assert!(!load(&path).unwrap().unwrap().performance_open);
        fs::write(&path, "version = 99").unwrap();
        assert!(load(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "version = 99");
        let unsafe_runtime = "version = 1\n[[runtimes]]\nname = 'fixture'\n[runtimes.target]\nprovider = 'colima'\nprofile = '../unsafe'\n";
        fs::write(&path, unsafe_runtime).unwrap();
        assert!(load(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), unsafe_runtime);
        fs::write(&path, "x".repeat(LIMIT as usize + 1)).unwrap();
        assert!(load(&path).is_err());
        let link = directory.join("link");
        symlink(&path, &link).unwrap();
        assert!(load(&link).is_err());
        fs::remove_file(link).unwrap();
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
