use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TuiWorkspace {
    pub id: String,
    pub name: String,
    pub relay: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repos_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_tag: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TuiLayoutPreferences {
    #[serde(default)]
    pub detail_panel_visible: bool,
    #[serde(default)]
    pub agent_panel_visible: bool,
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u16,
    #[serde(default = "default_detail_width")]
    pub detail_width: u16,
    #[serde(default = "default_agent_panel_height")]
    pub agent_panel_height: u16,
}

impl Default for TuiLayoutPreferences {
    fn default() -> Self {
        Self {
            detail_panel_visible: false,
            agent_panel_visible: false,
            sidebar_width: default_sidebar_width(),
            detail_width: default_detail_width(),
            agent_panel_height: default_agent_panel_height(),
        }
    }
}

const fn default_sidebar_width() -> u16 {
    30
}

const fn default_detail_width() -> u16 {
    80
}

const fn default_agent_panel_height() -> u16 {
    9
}

fn new_read_state_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn is_valid_read_state_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SavedSnippet {
    pub id: String,
    pub name: String,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub active_id: String,
    #[serde(default)]
    pub workspaces: Vec<TuiWorkspace>,
    #[serde(default)]
    pub read_frontiers: BTreeMap<String, BTreeMap<String, u64>>,
    #[serde(default)]
    pub manual_unread: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub snippets: Vec<SavedSnippet>,
    /// Composer drafts keyed `workspace::channel::thread` so unsent text
    /// survives restarts, matching the desktop drafts inbox.
    #[serde(default)]
    pub drafts: BTreeMap<String, String>,
    /// Locally followed thread roots per workspace, with follow timestamps.
    #[serde(default)]
    pub thread_follows: BTreeMap<String, BTreeMap<String, u64>>,
    #[serde(default)]
    pub layout: TuiLayoutPreferences,
    /// Stable, installation-local NIP-RS writer identity.
    #[serde(default = "new_read_state_id")]
    pub read_state_client_id: String,
    /// Stable, installation-local NIP-RS replaceable-event slot.
    #[serde(default = "new_read_state_id")]
    pub read_state_slot_id: String,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            active_id: String::new(),
            workspaces: Vec::new(),
            read_frontiers: BTreeMap::new(),
            manual_unread: BTreeMap::new(),
            snippets: Vec::new(),
            drafts: BTreeMap::new(),
            thread_follows: BTreeMap::new(),
            layout: TuiLayoutPreferences::default(),
            read_state_client_id: new_read_state_id(),
            read_state_slot_id: new_read_state_id(),
        }
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace name cannot be empty")]
    EmptyName,
    #[error("workspace relay cannot be empty")]
    EmptyRelay,
    #[error("workspace relay must start with http://, https://, ws://, or wss://")]
    InvalidRelay,
    #[error("workspace repos_dir must be an absolute existing directory")]
    InvalidReposDir,
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
            repos_dir: None,
            auth_tag: None,
        };
        Self {
            active_id: workspace.id.clone(),
            workspaces: vec![workspace],
            read_frontiers: BTreeMap::new(),
            manual_unread: BTreeMap::new(),
            snippets: Vec::new(),
            drafts: BTreeMap::new(),
            thread_follows: BTreeMap::new(),
            layout: TuiLayoutPreferences::default(),
            read_state_client_id: new_read_state_id(),
            read_state_slot_id: new_read_state_id(),
        }
    }

    pub fn load_or_default(path: &PathBuf, relay: &str) -> Result<Self, WorkspaceError> {
        if !path.exists() {
            return Ok(Self::with_default(relay));
        }
        let content = std::fs::read_to_string(path).map_err(WorkspaceError::Read)?;
        let mut config: Self = serde_json::from_str(&content).map_err(WorkspaceError::Json)?;
        config.ensure_default(relay);
        config.validate_repos_dirs()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<(), WorkspaceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(WorkspaceError::Write)?;
        }
        let content = serde_json::to_string_pretty(self).map_err(WorkspaceError::Json)?;
        write_private_file(path, content.as_bytes()).map_err(WorkspaceError::Write)
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
        if !is_valid_read_state_id(&self.read_state_client_id) {
            self.read_state_client_id = new_read_state_id();
        }
        if !is_valid_read_state_id(&self.read_state_slot_id) {
            self.read_state_slot_id = new_read_state_id();
        }
    }

    pub fn validate_repos_dirs(&self) -> Result<(), WorkspaceError> {
        for workspace in &self.workspaces {
            if let Some(path) = workspace.repos_dir.as_deref() {
                validate_repos_dir(path)?;
            }
        }
        Ok(())
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
            repos_dir: None,
            auth_tag: None,
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
        self.thread_follows.remove(&removed.id);
        let draft_prefix = format!("{}::", removed.id);
        self.drafts.retain(|key, _| !key.starts_with(&draft_prefix));
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

    pub fn next_snippet_id(&self, name: &str) -> String {
        snippet_id(name, &self.snippets)
    }
}

fn write_private_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn validate_repos_dir(path: &str) -> Result<String, WorkspaceError> {
    let path = Path::new(path.trim());
    if path.as_os_str().is_empty() || !path.is_absolute() || !path.is_dir() {
        return Err(WorkspaceError::InvalidReposDir);
    }
    Ok(path.display().to_string())
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

fn snippet_id(name: &str, existing: &[SavedSnippet]) -> String {
    let base = slugify(name);
    let base = if base.is_empty() {
        "snippet".to_string()
    } else {
        base
    };
    let mut id = base.clone();
    let mut suffix = 2usize;
    while existing.iter().any(|snippet| snippet.id == id) {
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
        assert!(config.snippets.is_empty());
        assert!(config.thread_follows.is_empty());
        assert_eq!(config.workspaces[0].repos_dir, None);
        assert_eq!(config.workspaces[0].auth_tag, None);
        assert!(!config.layout.detail_panel_visible);
        assert!(!config.layout.agent_panel_visible);
        assert_eq!(config.layout.sidebar_width, 30);
        assert!(is_valid_read_state_id(&config.read_state_client_id));
        assert!(is_valid_read_state_id(&config.read_state_slot_id));
        assert_ne!(config.read_state_client_id, config.read_state_slot_id);
    }

    #[test]
    fn workspace_repos_dir_must_be_absolute_existing_directory() {
        assert!(validate_repos_dir("/tmp").is_ok());
        assert!(matches!(
            validate_repos_dir("relative"),
            Err(WorkspaceError::InvalidReposDir)
        ));
        assert!(matches!(
            validate_repos_dir("/definitely/not/a/buzz/tui/repo/dir"),
            Err(WorkspaceError::InvalidReposDir)
        ));
    }

    #[test]
    fn save_includes_snippets_without_disturbing_workspace_state() {
        let mut config = WorkspaceConfig::with_default("http://localhost:3000");
        let workspace_id = config.active_id.clone();
        config
            .read_frontiers
            .entry(workspace_id.clone())
            .or_default()
            .insert("channel-1".to_string(), 42);
        config
            .manual_unread
            .entry(workspace_id.clone())
            .or_default()
            .insert("channel-2".to_string());
        config
            .thread_follows
            .entry(workspace_id)
            .or_default()
            .insert("thread-1".to_string(), 100);
        config.snippets.push(SavedSnippet {
            id: "greeting".to_string(),
            name: "Greeting".to_string(),
            content: "hello".to_string(),
        });

        let saved = serde_json::to_string(&config).unwrap();
        let loaded: WorkspaceConfig = serde_json::from_str(&saved).unwrap();

        assert_eq!(loaded.snippets, config.snippets);
        assert_eq!(loaded.read_frontiers, config.read_frontiers);
        assert_eq!(loaded.manual_unread, config.manual_unread);
        assert_eq!(loaded.thread_follows, config.thread_follows);
    }

    #[test]
    fn snippet_ids_are_unique_and_slugged() {
        let mut config = WorkspaceConfig::with_default("http://localhost:3000");
        config.snippets.push(SavedSnippet {
            id: "daily-update".to_string(),
            name: "Daily Update".to_string(),
            content: "first".to_string(),
        });

        assert_eq!(config.next_snippet_id("Daily Update"), "daily-update-2");
        assert_eq!(config.next_snippet_id("!!!"), "snippet");
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
        config
            .thread_follows
            .entry(removed_id.clone())
            .or_default()
            .insert("thread-1".to_string(), 100);

        let removed = config.remove_workspace(1).unwrap();

        assert_eq!(removed.id, removed_id);
        assert!(!config.read_frontiers.contains_key(&removed_id));
        assert!(!config.manual_unread.contains_key(&removed_id));
        assert!(!config.thread_follows.contains_key(&removed_id));
    }

    #[cfg(unix)]
    #[test]
    fn saved_workspace_config_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path =
            std::env::temp_dir().join(format!("buzz-tui-workspaces-{}.json", uuid::Uuid::new_v4()));
        WorkspaceConfig::with_default("http://localhost:3000")
            .save(&path)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_file(path).unwrap();
    }
}
