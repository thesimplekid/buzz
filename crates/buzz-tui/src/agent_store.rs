use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nostr::{Keys, ToBech32};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::acp::AgentRuntime;
use crate::client::{
    default_reply_placement, CreateManagedAgentOptions, ManagedAgentInfo, ManagedAgentLogInfo,
};

const DEFAULT_TURN_TIMEOUT_SECONDS: u64 = 600;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManagedAgentStore {
    #[serde(default)]
    pub agents: Vec<TuiManagedAgentRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TuiManagedAgentRecord {
    pub name: String,
    pub pubkey: String,
    pub relay_url: String,
    pub acp_command: String,
    pub agent_command: String,
    #[serde(default)]
    pub agent_args: Vec<String>,
    #[serde(default)]
    pub mcp_command: String,
    #[serde(default = "default_turn_timeout_seconds")]
    pub turn_timeout_seconds: u64,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub respond_to: String,
    #[serde(default)]
    pub respond_to_allowlist: Vec<String>,
    #[serde(default = "default_reply_placement")]
    pub reply_placement: String,
    #[serde(default)]
    pub start_on_launch: bool,
    pub log_path: String,
    pub private_key_nsec: String,
    #[serde(default)]
    pub auth_tag: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Error)]
pub enum ManagedAgentStoreError {
    #[error("failed to read managed agent store: {0}")]
    Read(std::io::Error),
    #[error("failed to parse managed agent store: {0}")]
    Json(serde_json::Error),
    #[error("failed to write managed agent store: {0}")]
    Write(std::io::Error),
    #[error("agent runtime {0:?} is not available")]
    RuntimeUnavailable(String),
    #[error("failed to encode generated agent key: {0}")]
    Key(String),
}

impl ManagedAgentStore {
    pub fn load_or_default(path: &Path) -> Result<Self, ManagedAgentStoreError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path).map_err(ManagedAgentStoreError::Read)?;
        serde_json::from_str(&content).map_err(ManagedAgentStoreError::Json)
    }

    pub fn save(&self, path: &Path) -> Result<(), ManagedAgentStoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ManagedAgentStoreError::Write)?;
        }
        let content = serde_json::to_string_pretty(self).map_err(ManagedAgentStoreError::Json)?;
        std::fs::write(path, content).map_err(ManagedAgentStoreError::Write)
    }

    pub fn infos(&self) -> Vec<ManagedAgentInfo> {
        self.agents
            .iter()
            .map(TuiManagedAgentRecord::info)
            .collect()
    }

    pub fn create_agent(
        &mut self,
        path: &Path,
        options: &CreateManagedAgentOptions,
        runtimes: &[AgentRuntime],
        relay_url: &str,
        default_acp_command: &str,
        default_auth_tag: Option<String>,
    ) -> Result<ManagedAgentInfo, ManagedAgentStoreError> {
        let runtime = runtimes
            .iter()
            .find(|runtime| runtime.id == options.runtime && !runtime.managed)
            .ok_or_else(|| ManagedAgentStoreError::RuntimeUnavailable(options.runtime.clone()))?;
        let keys = Keys::generate();
        let private_key_nsec = keys
            .secret_key()
            .to_bech32()
            .map_err(|error| ManagedAgentStoreError::Key(error.to_string()))?;
        let now = now_seconds();
        let pubkey = keys.public_key().to_hex();
        let record = TuiManagedAgentRecord {
            name: options.name.trim().to_string(),
            pubkey: pubkey.clone(),
            relay_url: relay_url.to_string(),
            acp_command: runtime
                .acp_command
                .clone()
                .unwrap_or_else(|| default_acp_command.to_string()),
            agent_command: runtime.command.clone(),
            agent_args: runtime.args.clone(),
            mcp_command: runtime.mcp_command.clone().unwrap_or_default(),
            turn_timeout_seconds: runtime
                .turn_timeout_seconds
                .unwrap_or(DEFAULT_TURN_TIMEOUT_SECONDS),
            system_prompt: optional_string(&options.system_prompt),
            model: optional_string(&options.model),
            respond_to: if options.respond_to.trim().is_empty() {
                "owner-only".to_string()
            } else {
                options.respond_to.clone()
            },
            respond_to_allowlist: options.respond_to_allowlist.clone(),
            reply_placement: if options.reply_placement.trim().is_empty() {
                default_reply_placement()
            } else {
                options.reply_placement.clone()
            },
            start_on_launch: options.start_on_launch,
            log_path: managed_agent_log_path_for_store(path, &pubkey)
                .display()
                .to_string(),
            private_key_nsec,
            auth_tag: default_auth_tag,
            created_at: now,
            updated_at: now,
        };
        let info = record.info();
        self.agents.push(record);
        self.save(path)?;
        Ok(info)
    }

    pub fn set_start_on_launch(
        &mut self,
        path: &Path,
        pubkey: &str,
        enabled: bool,
    ) -> Result<Option<ManagedAgentInfo>, ManagedAgentStoreError> {
        let Some(record) = self.agents.iter_mut().find(|agent| agent.pubkey == pubkey) else {
            return Ok(None);
        };
        record.start_on_launch = enabled;
        record.updated_at = now_seconds();
        let info = record.info();
        self.save(path)?;
        Ok(Some(info))
    }

    pub fn remove(&mut self, path: &Path, pubkey: &str) -> Result<bool, ManagedAgentStoreError> {
        let before = self.agents.len();
        self.agents.retain(|agent| agent.pubkey != pubkey);
        let deleted = self.agents.len() != before;
        if deleted {
            self.save(path)?;
        }
        Ok(deleted)
    }

    pub fn log(&self, pubkey: &str, lines: usize) -> Option<ManagedAgentLogInfo> {
        let record = self.agents.iter().find(|agent| agent.pubkey == pubkey)?;
        let content = read_log_tail(Path::new(&record.log_path), lines).unwrap_or_default();
        Some(ManagedAgentLogInfo {
            pubkey: record.pubkey.clone(),
            log_path: record.log_path.clone(),
            content,
        })
    }
}

impl TuiManagedAgentRecord {
    pub fn info(&self) -> ManagedAgentInfo {
        ManagedAgentInfo {
            pubkey: self.pubkey.clone(),
            name: self.name.clone(),
            private_key_nsec: Some(self.private_key_nsec.clone()),
            auth_tag: self.auth_tag.clone(),
            relay_url: self.relay_url.clone(),
            acp_command: self.acp_command.clone(),
            agent_command: self.agent_command.clone(),
            agent_args: self.agent_args.clone(),
            mcp_command: self.mcp_command.clone(),
            turn_timeout_seconds: Some(self.turn_timeout_seconds),
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            respond_to: self.respond_to.clone(),
            respond_to_allowlist: self.respond_to_allowlist.clone(),
            reply_placement: if self.reply_placement.trim().is_empty() {
                default_reply_placement()
            } else {
                self.reply_placement.clone()
            },
            start_on_launch: self.start_on_launch,
            status: "stopped".to_string(),
            pid: None,
            log_path: Some(self.log_path.clone()),
            last_error: None,
        }
    }
}

pub fn managed_agent_store_path(override_path: Option<&str>) -> PathBuf {
    if let Some(path) = override_path.filter(|path| !path.trim().is_empty()) {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("BUZZ_TUI_AGENTS") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        if !config_home.trim().is_empty() {
            return PathBuf::from(config_home)
                .join("buzz")
                .join("tui-agents.json");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home)
                .join(".config")
                .join("buzz")
                .join("tui-agents.json");
        }
    }
    PathBuf::from("buzz-tui-agents.json")
}

pub fn managed_agent_log_path_for_store(store_path: &Path, pubkey: &str) -> PathBuf {
    store_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
        .join(format!("{pubkey}.log"))
}

fn read_log_tail(path: &Path, lines: usize) -> std::io::Result<String> {
    let content = std::fs::read_to_string(path)?;
    if lines == 0 {
        return Ok(String::new());
    }
    let mut tail = content.lines().rev().take(lines).collect::<Vec<_>>();
    tail.reverse();
    Ok(tail.join("\n"))
}

fn optional_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn default_turn_timeout_seconds() -> u64 {
    DEFAULT_TURN_TIMEOUT_SECONDS
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::AgentStatus;

    fn runtime(id: &str) -> AgentRuntime {
        AgentRuntime {
            id: id.to_string(),
            label: id.to_string(),
            relay_url: None,
            acp_command: None,
            command: "agent-command".to_string(),
            args: vec!["acp".to_string()],
            model: None,
            mcp_command: Some("mcp-command".to_string()),
            turn_timeout_seconds: Some(42),
            system_prompt: None,
            respond_to: "owner-only".to_string(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: false,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: String::new(),
            last_error: None,
            log_path: None,
        }
    }

    #[test]
    fn created_agent_record_contains_required_runtime_fields() {
        let dir = tempfile_dir();
        let path = dir.join("agents.json");
        let mut store = ManagedAgentStore::default();

        let info = store
            .create_agent(
                &path,
                &CreateManagedAgentOptions {
                    name: "Review Bot".to_string(),
                    runtime: "codex".to_string(),
                    model: "gpt-5".to_string(),
                    system_prompt: "Review code".to_string(),
                    respond_to: "allowlist".to_string(),
                    respond_to_allowlist: vec!["owner".to_string()],
                    reply_placement: "top-level".to_string(),
                    start_on_launch: true,
                },
                &[runtime("codex")],
                "http://localhost:3000",
                "buzz-acp",
                Some("[\"auth\"]".to_string()),
            )
            .unwrap();

        assert_eq!(info.name, "Review Bot");
        assert_eq!(info.relay_url, "http://localhost:3000");
        assert_eq!(info.agent_command, "agent-command");
        assert_eq!(info.agent_args, ["acp"]);
        assert_eq!(info.mcp_command, "mcp-command");
        assert_eq!(info.system_prompt.as_deref(), Some("Review code"));
        assert_eq!(info.model.as_deref(), Some("gpt-5"));
        assert_eq!(info.respond_to, "allowlist");
        assert_eq!(info.respond_to_allowlist, ["owner"]);
        assert_eq!(info.reply_placement, "top-level");
        assert!(info.start_on_launch);
        assert!(info
            .private_key_nsec
            .as_deref()
            .is_some_and(|key| key.starts_with("nsec")));
        assert_eq!(info.auth_tag.as_deref(), Some("[\"auth\"]"));
        assert!(info
            .log_path
            .as_deref()
            .is_some_and(|path| path.contains("logs")));
    }

    #[test]
    fn log_path_is_adjacent_to_store() {
        let path = PathBuf::from("/tmp/buzz-tui-agents/agents.json");
        assert_eq!(
            managed_agent_log_path_for_store(&path, "abc"),
            PathBuf::from("/tmp/buzz-tui-agents/logs/abc.log")
        );
    }

    fn tempfile_dir() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("buzz-tui-agent-store-test-{}", now_seconds()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
