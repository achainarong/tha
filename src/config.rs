use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Docker,
    Containerd,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Connection {
    pub name: String,
    pub kind: BackendKind,
    pub endpoint: String,
}

impl Connection {
    pub fn key(&self) -> String {
        format!("{:?}:{}", self.kind, self.endpoint)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub refresh_seconds: u64,
    pub metrics_refresh_seconds: u64,
    pub log_capacity: usize,
    pub monochrome: bool,
    pub log_wrap: bool,
    pub exec_command: Vec<String>,
    pub connections: Vec<Connection>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_seconds: 5,
            metrics_refresh_seconds: 1,
            log_capacity: 5000,
            monochrome: false,
            log_wrap: true,
            exec_command: vec!["sh".into()],
            connections: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&PathBuf>) -> Result<Self> {
        let explicit = path.is_some();
        let default = default_path();
        let path = path.or(default.as_ref());
        let config: Self = match path {
            Some(p) if p.exists() => toml::from_str(
                &std::fs::read_to_string(p)
                    .with_context(|| format!("Cannot read {}", p.display()))?,
            )
            .with_context(|| format!("Invalid configuration in {}", p.display()))?,
            Some(p) if explicit => {
                bail!("Configuration does not exist: {}", p.display())
            }
            _ => Self::default(),
        };
        if !(1..=300).contains(&config.refresh_seconds) {
            bail!("refresh_seconds must be between 1 and 300");
        }
        if !(1..=60).contains(&config.metrics_refresh_seconds) {
            bail!("metrics_refresh_seconds must be between 1 and 60");
        }
        if !(100..=100_000).contains(&config.log_capacity) {
            bail!("log_capacity must be between 100 and 100000");
        }
        if config.exec_command.is_empty() || config.exec_command[0].trim().is_empty() {
            bail!("exec_command must contain an executable, for example [\"sh\"]");
        }
        for connection in &config.connections {
            if connection.name.trim().is_empty() || connection.endpoint.trim().is_empty() {
                bail!("Connection names and endpoints must not be empty");
            }
        }
        Ok(config)
    }
}

pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|root| root.join("tha/config.toml"))
}

pub fn parse_connection(value: &str, kind: BackendKind) -> Result<Connection> {
    let (name, endpoint) = value
        .split_once('=')
        .context("Expected NAME=ENDPOINT, e.g. local=unix:///var/run/docker.sock")?;
    if name.trim().is_empty() || endpoint.trim().is_empty() {
        bail!("Connection name and endpoint must not be empty");
    }
    Ok(Connection {
        name: name.trim().into(),
        kind,
        endpoint: endpoint.trim().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_default_to_one_second_without_changing_inventory_refresh() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.metrics_refresh_seconds, 1);
        assert_eq!(config.refresh_seconds, 5);
    }

    #[test]
    fn connection_requires_name_and_endpoint() {
        assert!(parse_connection("local", BackendKind::Docker).is_err());
        assert!(parse_connection("=socket", BackendKind::Docker).is_err());
        assert!(parse_connection("local=", BackendKind::Docker).is_err());
        assert_eq!(
            parse_connection("orb=unix:///socket", BackendKind::Docker)
                .unwrap()
                .endpoint,
            "unix:///socket"
        );
    }
}
