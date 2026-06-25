use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TuiWorkspace {
    pub id: String,
    pub name: String,
    pub relay: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub active_id: String,
    #[serde(default)]
    pub workspaces: Vec<TuiWorkspace>,
    #[serde(default)]
    pub read_frontiers: BTreeMap<String, BTreeMap<String, u64>>,
    #[serde(default)]
    pub manual_unread: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace name cannot be empty")]
    EmptyName,
    #[error("workspace relay cannot be empty")]
    EmptyRelay,
    #[error("workspace relay must start with http://, https://, ws://, or wss://")]
    InvalidRelay,
    #[error("failed to read workspace config: {0}")]
    Read(std::io::Error),
    #[error("failed to parse workspace config: {0}")]
    Json(serde_json::Error),
    #[error("failed to save workspace config: {0}")]
    Write(std::io::Error),
}

impl WorkspaceConfig {
    pub fn with_default(relay: &str) -> Self {
        let workspace = TuiWorkspace {
            id: workspace_id("default", relay, &[]),
            name: default_workspace_name(relay),
            relay: normalize_workspace_relay(relay).unwrap_or_else(|_| relay.to_string()),
        };
        Self {
            active_id: workspace.id.clone(),
            workspaces: vec![workspace],
            read_frontiers: BTreeMap::new(),
            manual_unread: BTreeMap::new(),
        }
    }

    pub fn load_or_default(path: &PathBuf, relay: &str) -> Result<Self, WorkspaceError> {
        if !path.exists() {
            return Ok(Self::with_default(relay));
        }
        let content = std::fs::read_to_string(path).map_err(WorkspaceError::Read)?;
        let mut config: Self = serde_json::from_str(&content).map_err(WorkspaceError::Json)?;
        config.ensure_default(relay);
        Ok(config)
    }

    pub fn save(&self, path: &PathBuf) -> Result<(), WorkspaceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(WorkspaceError::Write)?;
        }
        let content = serde_json::to_string_pretty(self).map_err(WorkspaceError::Json)?;
        std::fs::write(path, content).map_err(WorkspaceError::Write)
    }

    pub fn ensure_default(&mut self, relay: &str) {
        if self.workspaces.is_empty() {
            *self = Self::with_default(relay);
            return;
        }
        if !self.workspaces.iter().any(|w| w.id == self.active_id) {
            self.active_id = self
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone())
                .unwrap_or_default();
        }
    }

    pub fn active_index(&self) -> usize {
        self.workspaces
            .iter()
            .position(|workspace| workspace.id == self.active_id)
            .unwrap_or_default()
    }

    pub fn add_workspace(&mut self, name: &str, relay: &str) -> Result<String, WorkspaceError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(WorkspaceError::EmptyName);
        }
        let relay = normalize_workspace_relay(relay)?;
        let id = workspace_id(name, &relay, &self.workspaces);
        self.workspaces.push(TuiWorkspace {
            id: id.clone(),
            name: name.to_string(),
            relay,
        });
        Ok(id)
    }

    pub fn remove_workspace(&mut self, index: usize) -> Option<TuiWorkspace> {
        if self.workspaces.len() <= 1 || index >= self.workspaces.len() {
            return None;
        }
        let removed = self.workspaces.remove(index);
        self.read_frontiers.remove(&removed.id);
        self.manual_unread.remove(&removed.id);
        if removed.id == self.active_id {
            self.active_id = self
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone())
                .unwrap_or_default();
        }
        Some(removed)
    }

    pub fn set_active(&mut self, id: &str) {
        if self.workspaces.iter().any(|workspace| workspace.id == id) {
            self.active_id = id.to_string();
        }
    }
}

pub fn workspace_store_path(override_path: Option<&str>) -> PathBuf {
    if let Some(path) = override_path.filter(|path| !path.trim().is_empty()) {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("BUZZ_TUI_WORKSPACES") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        if !config_home.trim().is_empty() {
            return PathBuf::from(config_home)
                .join("buzz")
                .join("tui-workspaces.json");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home)
                .join(".config")
                .join("buzz")
                .join("tui-workspaces.json");
        }
    }
    PathBuf::from("buzz-tui-workspaces.json")
}

pub fn parse_workspace_input(input: &str) -> Result<(String, String), WorkspaceError> {
    let mut parts = input.split_whitespace();
    let Some(first) = parts.next() else {
        return Err(WorkspaceError::EmptyRelay);
    };
    let Some(second) = parts.next() else {
        let relay = normalize_workspace_relay(first)?;
        return Ok((default_workspace_name(&relay), relay));
    };
    let name = first.trim();
    if name.is_empty() {
        return Err(WorkspaceError::EmptyName);
    }
    let relay = normalize_workspace_relay(second)?;
    Ok((name.to_string(), relay))
}

pub fn normalize_workspace_relay(relay: &str) -> Result<String, WorkspaceError> {
    let relay = relay.trim();
    if relay.is_empty() {
        return Err(WorkspaceError::EmptyRelay);
    }
    if let Some(rest) = relay.strip_prefix("ws://") {
        return Ok(format!("http://{rest}"));
    }
    if let Some(rest) = relay.strip_prefix("wss://") {
        return Ok(format!("https://{rest}"));
    }
    if relay.starts_with("http://") || relay.starts_with("https://") {
        return Ok(relay.to_string());
    }
    Err(WorkspaceError::InvalidRelay)
}

fn default_workspace_name(relay: &str) -> String {
    relay
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("wss://")
        .trim_start_matches("ws://")
        .trim_end_matches('/')
        .split('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace")
        .to_string()
}

fn workspace_id(name: &str, relay: &str, existing: &[TuiWorkspace]) -> String {
    let base = slugify(&format!("{name}-{relay}"));
    let base = if base.is_empty() {
        "workspace".to_string()
    } else {
        base
    };
    let mut id = base.clone();
    let mut suffix = 2usize;
    while existing.iter().any(|workspace| workspace.id == id) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_workspace_input_accepts_relay_only() {
        assert_eq!(
            parse_workspace_input("wss://relay.example").unwrap(),
            (
                "relay.example".to_string(),
                "https://relay.example".to_string()
            )
        );
    }

    #[test]
    fn parse_workspace_input_accepts_name_and_relay() {
        assert_eq!(
            parse_workspace_input("staging http://localhost:3000").unwrap(),
            ("staging".to_string(), "http://localhost:3000".to_string())
        );
    }

    #[test]
    fn parse_workspace_input_rejects_non_http_relay() {
        assert!(matches!(
            parse_workspace_input("staging relay.example"),
            Err(WorkspaceError::InvalidRelay)
        ));
    }

    #[test]
    fn add_workspace_generates_unique_ids() {
        let mut config = WorkspaceConfig::with_default("http://localhost:3000");
        let first = config
            .add_workspace("local", "http://localhost:3000")
            .unwrap();
        let second = config
            .add_workspace("local", "http://localhost:3000")
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(config.workspaces.len(), 3);
    }

    #[test]
    fn remove_workspace_refuses_to_remove_last_workspace() {
        let mut config = WorkspaceConfig::with_default("http://localhost:3000");

        assert_eq!(config.remove_workspace(0), None);
        assert_eq!(config.workspaces.len(), 1);
    }

    #[test]
    fn workspace_config_accepts_legacy_files_without_read_state() {
        let config: WorkspaceConfig = serde_json::from_str(
            r#"{
                "active_id": "default",
                "workspaces": [
                    {"id": "default", "name": "local", "relay": "http://localhost:3000"}
                ]
            }"#,
        )
        .unwrap();

        assert!(config.read_frontiers.is_empty());
        assert!(config.manual_unread.is_empty());
    }

    #[test]
    fn remove_workspace_removes_local_read_state() {
        let mut config = WorkspaceConfig::with_default("http://localhost:3000");
        let removed_id = config
            .add_workspace("staging", "http://localhost:3001")
            .unwrap();
        config
            .read_frontiers
            .entry(removed_id.clone())
            .or_default()
            .insert("channel-1".to_string(), 42);
        config
            .manual_unread
            .entry(removed_id.clone())
            .or_default()
            .insert("channel-2".to_string());

        let removed = config.remove_workspace(1).unwrap();

        assert_eq!(removed.id, removed_id);
        assert!(!config.read_frontiers.contains_key(&removed_id));
        assert!(!config.manual_unread.contains_key(&removed_id));
    }
}
