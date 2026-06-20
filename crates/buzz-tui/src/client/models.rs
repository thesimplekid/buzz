use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub channel_type: String,
    pub visibility: String,
    pub archived: bool,
    pub topic: String,
    pub purpose: String,
    pub owner_pubkey: String,
    pub created_at: u64,
    pub kind: ConversationKind,
}

impl From<&Value> for Channel {
    fn from(value: &Value) -> Self {
        Self {
            id: string_field(value, "channel_id")
                .or_else(|| string_field(value, "id"))
                .unwrap_or_default(),
            name: string_field(value, "name").unwrap_or_else(|| "untitled".to_string()),
            description: string_field(value, "description").unwrap_or_default(),
            channel_type: string_field(value, "channel_type").unwrap_or_default(),
            visibility: string_field(value, "visibility").unwrap_or_default(),
            archived: value
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
            topic: string_field(value, "topic").unwrap_or_default(),
            purpose: string_field(value, "purpose").unwrap_or_default(),
            owner_pubkey: string_field(value, "pubkey").unwrap_or_default(),
            created_at: value
                .get("created_at")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            kind: ConversationKind::Channel,
        }
    }
}

impl Channel {
    pub fn from_dm(value: &Value) -> Self {
        let participants: Vec<String> = value
            .get("participants")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(short_id)
            .collect();
        let name = if participants.is_empty() {
            "dm".to_string()
        } else {
            participants.join(",")
        };
        Self {
            id: string_field(value, "dm_id").unwrap_or_default(),
            name,
            description: "Direct message".to_string(),
            channel_type: "dm".to_string(),
            visibility: String::new(),
            archived: false,
            topic: String::new(),
            purpose: String::new(),
            owner_pubkey: String::new(),
            created_at: value
                .get("created_at")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            kind: ConversationKind::DirectMessage,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ConversationKind {
    #[default]
    Channel,
    DirectMessage,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub pubkey: String,
    pub kind: u64,
    pub content: String,
    pub created_at: u64,
    pub channel_id: String,
    #[serde(default)]
    pub thread_root_id: Option<String>,
}

impl From<&Value> for Message {
    fn from(value: &Value) -> Self {
        Self {
            id: string_field(value, "id").unwrap_or_default(),
            pubkey: string_field(value, "pubkey").unwrap_or_default(),
            kind: value
                .get("kind")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            content: string_field(value, "content").unwrap_or_default(),
            created_at: value
                .get("created_at")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            channel_id: string_field(value, "channel_id").unwrap_or_default(),
            thread_root_id: string_field(value, "thread_root_id")
                .or_else(|| thread_root_tag(value)),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub emoji: String,
    pub count: usize,
    #[serde(default)]
    pub pubkeys: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelMember {
    pub pubkey: String,
    pub role: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelayMember {
    pub pubkey: String,
    pub role: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanvasDocument {
    pub channel_id: String,
    pub content: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    pub workflow_id: String,
    pub content: String,
    pub created_at: u64,
    pub pubkey: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub event_id: String,
    pub kind: u64,
    pub content: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowDetail {
    pub workflow_id: String,
    pub content: String,
    pub created_at: u64,
    pub pubkey: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub pubkey: String,
    pub naddr: String,
    pub coordinate: String,
    pub slug: String,
    pub title: String,
    pub summary: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub published_at: Option<u64>,
    pub updated_at: u64,
    pub content: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoProject {
    pub id: String,
    pub dtag: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub clone_urls: Vec<String>,
    pub web_url: Option<String>,
    pub owner: String,
    #[serde(default)]
    pub relays: Vec<String>,
    pub created_at: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitIssue {
    pub id: String,
    pub repo_owner: String,
    pub repo_id: String,
    pub author: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub labels: Vec<String>,
    pub created_at: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitPatch {
    pub id: String,
    pub repo_owner: String,
    pub repo_id: String,
    pub author: String,
    pub content: String,
    pub commit: Option<String>,
    pub parent_commit: Option<String>,
    pub root: bool,
    pub root_revision: bool,
    pub created_at: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub slug: String,
    pub event_id: String,
    pub created_at: u64,
    #[serde(default)]
    pub value: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CustomEmojiEntry {
    pub shortcode: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadState {
    #[serde(default)]
    pub contexts: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelSection {
    pub id: String,
    pub name: String,
    pub order: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelSections {
    #[serde(default = "default_section_store_version")]
    pub version: u8,
    #[serde(default)]
    pub sections: Vec<ChannelSection>,
    #[serde(default)]
    pub assignments: BTreeMap<String, String>,
}

fn default_section_store_version() -> u8 {
    1
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelPreferenceKind {
    Stars,
    Mutes,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateRepoOptions {
    pub id: String,
    pub name: String,
    pub description: String,
    pub clone_urls: Vec<String>,
    pub web_url: String,
    pub relays: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateIssueOptions {
    pub repo_owner: String,
    pub repo_id: String,
    pub title: String,
    pub content: String,
    pub labels: Vec<String>,
    pub recipients: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreatePatchOptions {
    pub repo_owner: String,
    pub repo_id: String,
    pub content: String,
    pub commit: String,
    pub parent_commit: String,
    pub root: bool,
    pub root_revision: bool,
    pub recipients: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListNotesOptions {
    pub author: NoteAuthor,
    pub tag: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum NoteAuthor {
    #[default]
    Me,
    All,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateChannelOptions {
    pub name: String,
    pub channel_type: String,
    pub visibility: String,
    pub description: String,
    pub ttl: Option<i32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SendDiffOptions {
    pub repo_url: String,
    pub commit_sha: String,
    pub file_path: String,
    pub description: String,
    pub diff: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserProfile {
    pub pubkey: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub picture: String,
    #[serde(default)]
    pub about: String,
    #[serde(default)]
    pub nip05: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UploadedFile {
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default, rename = "type")]
    pub mime_type: String,
    #[serde(default)]
    pub uploaded: i64,
    #[serde(default)]
    pub dim: Option<String>,
    #[serde(default)]
    pub blurhash: Option<String>,
    #[serde(default)]
    pub thumb: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PresenceInfo {
    pub pubkey: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Contact {
    pub pubkey: String,
    pub relay_url: String,
    pub petname: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileField {
    DisplayName,
    About,
    Picture,
    Nip05,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresenceStatus {
    Online,
    Away,
    Offline,
}

impl PresenceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Away => "away",
            Self::Offline => "offline",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Online => Self::Away,
            Self::Away => Self::Offline,
            Self::Offline => Self::Online,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManagedAgentInfo {
    pub pubkey: String,
    pub name: String,
    pub private_key_nsec: Option<String>,
    pub auth_tag: Option<String>,
    pub relay_url: String,
    pub acp_command: String,
    pub agent_command: String,
    pub agent_args: Vec<String>,
    pub mcp_command: String,
    pub turn_timeout_seconds: Option<u64>,
    pub system_prompt: Option<String>,
    pub model: Option<String>,
    pub respond_to: String,
    #[serde(default)]
    pub respond_to_allowlist: Vec<String>,
    #[serde(default = "default_reply_placement")]
    pub reply_placement: String,
    #[serde(default)]
    pub start_on_launch: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub log_path: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManagedAgentLogInfo {
    pub pubkey: String,
    pub log_path: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateManagedAgentOptions {
    pub name: String,
    pub runtime: String,
    pub model: String,
    pub system_prompt: String,
    pub respond_to: String,
    pub respond_to_allowlist: Vec<String>,
    pub reply_placement: String,
    pub start_on_launch: bool,
}

pub fn default_reply_placement() -> String {
    "thread-direct-mentions".to_string()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LongFormNoteOptions {
    pub name: String,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub content: String,
}

impl From<TuiMessageView> for Message {
    fn from(value: TuiMessageView) -> Self {
        Self {
            id: value.id,
            pubkey: value.pubkey,
            kind: value.kind,
            content: value.content,
            created_at: value.created_at,
            channel_id: value.channel_id,
            thread_root_id: value.thread_root_id,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TuiMessageView {
    pub id: String,
    pub pubkey: String,
    pub kind: u64,
    pub content: String,
    pub created_at: u64,
    pub channel_id: String,
    #[serde(default)]
    pub thread_root_id: Option<String>,
}

fn thread_root_tag(value: &Value) -> Option<String> {
    let mut root = None;
    let mut reply = None;
    for tag in value.get("tags")?.as_array()? {
        let parts = tag.as_array()?;
        if parts.first().and_then(Value::as_str) != Some("e") {
            continue;
        }
        let id = parts.get(1).and_then(Value::as_str)?;
        match parts.get(3).and_then(Value::as_str) {
            Some("root") => root = Some(id.to_string()),
            Some("reply") => reply = Some(id.to_string()),
            _ => {}
        }
    }
    root.or(reply)
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(ToString::to_string)
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}
