use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub channel_type: String,
    /// Participant pubkeys advertised by DM kind-39000 metadata.
    #[serde(default)]
    pub participant_pubkeys: Vec<String>,
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
            participant_pubkeys: value
                .get("participant_pubkeys")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ConversationKind {
    #[default]
    Channel,
    DirectMessage,
}

/// One decrypted NIP-AM agent turn metric (kind 44200), flattened for display.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentTurnMetric {
    pub agent_pubkey: String,
    pub created_at: u64,
    pub harness: String,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

/// NIP-CW thread summary overlay (kind 39005) for one thread root.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThreadSummary {
    #[serde(default)]
    pub reply_count: u64,
    #[serde(default)]
    pub descendant_count: u64,
    #[serde(default)]
    pub last_reply_at: Option<u64>,
}

/// NIP-CW keyset pagination cursor: echo the relay's `next_cursor` verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowCursor {
    pub created_at: u64,
    pub id: String,
}

/// One server-assembled channel window (NIP-CW): top-level rows plus
/// relay-signed overlays partitioned out of the flat response.
#[derive(Clone, Debug, Default)]
pub struct ChannelWindowPage {
    pub messages: Vec<Message>,
    pub summaries: BTreeMap<String, ThreadSummary>,
    pub has_more: bool,
    pub next_cursor: Option<WindowCursor>,
    /// Structural checks passed: exactly one kind-39006 bounds event echoing
    /// the request cursor, with `has_more` consistent with `next_cursor`.
    /// A `false` here is the NIP-CW downgrade signal.
    pub valid: bool,
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
    #[serde(default)]
    pub edited_at: Option<u64>,
    #[serde(default)]
    pub edit_event_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
/// Parsed content for a relay-signed kind-40099 timeline event.
pub struct SystemMessage {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    public_reason: Option<String>,
    #[serde(default)]
    participants: Vec<String>,
}

impl SystemMessage {
    /// Parse a system-message payload, returning `None` for other kinds or invalid JSON.
    pub fn parse(kind: u64, content: &str) -> Option<Self> {
        (kind == u64::from(buzz_core::kind::KIND_SYSTEM_MESSAGE))
            .then(|| serde_json::from_str(content).ok())
            .flatten()
    }

    /// Return all identities referenced by the system event for profile hydration.
    pub fn profile_pubkeys(&self) -> Vec<String> {
        let mut pubkeys = [self.actor.as_deref(), self.target.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|pubkey| !pubkey.is_empty())
            .map(str::to_string)
            .chain(
                self.participants
                    .iter()
                    .map(String::as_str)
                    .map(str::trim)
                    .filter(|pubkey| !pubkey.is_empty())
                    .map(str::to_string),
            )
            .collect::<Vec<_>>();
        pubkeys.sort();
        pubkeys.dedup();
        pubkeys
    }

    /// Format the event as a human-readable timeline sentence.
    pub fn describe(&self, resolve_label: impl Fn(&str) -> String) -> Option<String> {
        let actor = || self.actor.as_deref().map(&resolve_label);
        let target = || self.target.as_deref().map(&resolve_label);

        match self.event_type.as_str() {
            "member_joined" => {
                let actor_pubkey = self.actor.as_deref()?;
                let target_pubkey = self.target.as_deref()?;
                if actor_pubkey.eq_ignore_ascii_case(target_pubkey) {
                    Some(format!("{} joined the channel", target()?))
                } else {
                    Some(format!("{} was added by {}", target()?, actor()?))
                }
            }
            "member_left" => Some(format!("{} left the channel", actor()?)),
            "member_removed" => Some(format!(
                "{} removed {} from the channel",
                actor()?,
                target()?
            )),
            "topic_changed" => Some(format!(
                "{} changed the topic to “{}”",
                actor()?,
                self.topic.as_deref().unwrap_or_default()
            )),
            "purpose_changed" => Some(format!(
                "{} changed the purpose to “{}”",
                actor()?,
                self.purpose.as_deref().unwrap_or_default()
            )),
            "visibility_changed" => Some(format!(
                "{} changed the channel visibility to {}",
                actor()?,
                self.visibility.as_deref().unwrap_or("unknown")
            )),
            "channel_created" => Some(format!("{} created this channel", actor()?)),
            "dm_created" => Some(format!("{} started this direct message", actor()?)),
            "channel_archived" => Some(format!("{} archived this channel", actor()?)),
            "channel_unarchived" => Some(format!("{} unarchived this channel", actor()?)),
            "channel_deleted" => Some(format!("{} deleted this channel", actor()?)),
            "message_deleted" => self
                .public_reason
                .as_deref()
                .filter(|reason| !reason.trim().is_empty())
                .map(|reason| format!("Removed by community moderators: {reason}"))
                .or_else(|| Some(format!("{} removed a message", actor()?))),
            _ => None,
        }
    }
}

/// Describe a valid kind-48100 huddle-start payload without exposing its
/// internal ephemeral channel UUID in the timeline.
pub(crate) fn huddle_started_description(kind: u64, content: &str) -> Option<&'static str> {
    if kind != u64::from(buzz_core::kind::KIND_HUDDLE_STARTED) {
        return None;
    }
    serde_json::from_str::<Value>(content)
        .ok()?
        .get("ephemeral_channel_id")
        .and_then(Value::as_str)
        .filter(|channel_id| !channel_id.trim().is_empty())
        .map(|_| "started a huddle")
}

/// Whether a timeline row represents human-visible conversation for unread
/// accounting. Lifecycle rows remain visible without creating unread badges.
pub(crate) fn is_conversational_unread_kind(kind: u64) -> bool {
    u32::try_from(kind).map_or(true, |kind| {
        !matches!(
            kind,
            buzz_core::kind::KIND_SYSTEM_MESSAGE
                | buzz_core::kind::KIND_JOB_REQUEST
                | buzz_core::kind::KIND_JOB_ACCEPTED
                | buzz_core::kind::KIND_JOB_PROGRESS
                | buzz_core::kind::KIND_JOB_RESULT
                | buzz_core::kind::KIND_JOB_CANCEL
                | buzz_core::kind::KIND_JOB_ERROR
                | buzz_core::kind::KIND_HUDDLE_STARTED
                | buzz_core::kind::KIND_HUDDLE_PARTICIPANT_JOINED
                | buzz_core::kind::KIND_HUDDLE_PARTICIPANT_LEFT
                | buzz_core::kind::KIND_HUDDLE_ENDED
        )
    })
}

/// Return profile identities referenced by a timeline row.
pub fn timeline_profile_pubkeys(kind: u64, author: &str, content: &str) -> Vec<String> {
    if let Some(system_message) = SystemMessage::parse(kind, content) {
        return system_message.profile_pubkeys();
    }
    if author.trim().is_empty() {
        Vec::new()
    } else {
        vec![author.trim().to_string()]
    }
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
            edited_at: value.get("edited_at").and_then(Value::as_u64),
            edit_event_id: string_field(value, "edit_event_id"),
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
    pub run_id: String,
    pub step_id: Option<String>,
    pub status: String,
    pub output: Option<Value>,
    pub error: Option<String>,
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
pub struct GitPullRequest {
    pub id: String,
    pub repo_owner: String,
    pub repo_id: String,
    pub author: String,
    pub title: String,
    pub content: String,
    pub commit: String,
    pub branch_name: Option<String>,
    pub target_branch: Option<String>,
    #[serde(default)]
    pub clone_urls: Vec<String>,
    pub status: String,
    pub status_created_at: Option<u64>,
    pub approval_count: usize,
    pub change_request_count: usize,
    pub review_created_at: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReminderStatus {
    Pending,
    Done,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderTarget {
    pub event_id: String,
    pub channel_id: String,
    pub preview: String,
    pub author_pubkey: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReminderContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<ReminderTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub status: ReminderStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reminder {
    pub id: String,
    pub not_before: Option<u64>,
    pub content: ReminderContent,
    pub created_at: u64,
    pub event_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReminderGroup {
    pub label: &'static str,
    pub reminders: Vec<Reminder>,
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
    Status,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserStatus {
    pub text: String,
    pub emoji: String,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
pub struct ModerationReport {
    pub id: String,
    pub report_event_id: String,
    pub reporter_pubkey: String,
    pub target_kind: String,
    pub target: String,
    pub channel_id: Option<String>,
    pub report_type: String,
    pub note: Option<String>,
    pub status: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
pub struct MintedInvite {
    pub code: String,
    pub expires_at: u64,
    pub url: String,
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
    #[serde(default)]
    pub thinking_effort: Option<String>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
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
pub struct RelayAgentInfo {
    pub pubkey: String,
    #[serde(default)]
    pub owner_pubkey: Option<String>,
    pub name: String,
    pub agent_type: String,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub channel_ids: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub status: String,
    #[serde(default)]
    pub respond_to: Option<String>,
    #[serde(default)]
    pub respond_to_allowlist: Vec<String>,
    /// Model/provider/persona metadata from the published NIP-AP projection.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub persona_id: Option<String>,
    #[serde(default)]
    pub managed_by: Option<String>,
    #[serde(default)]
    pub management_mode: Option<String>,
    #[serde(default)]
    pub manager_pubkey: Option<String>,
    #[serde(default)]
    pub health_status: Option<String>,
    #[serde(default)]
    pub health_detail: Option<String>,
    #[serde(default)]
    pub health_restarts: Option<u64>,
    #[serde(default)]
    pub health_updated_at: Option<u64>,
    #[serde(default)]
    pub created_at: u64,
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
    pub thinking_effort: Option<String>,
    pub max_output_tokens: Option<u64>,
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
            edited_at: value.edited_at,
            edit_event_id: value.edit_event_id,
        }
    }
}

impl From<Message> for TuiMessageView {
    fn from(value: Message) -> Self {
        Self {
            id: value.id,
            pubkey: value.pubkey,
            kind: value.kind,
            content: value.content,
            created_at: value.created_at,
            channel_id: value.channel_id,
            thread_root_id: value.thread_root_id,
            edited_at: value.edited_at,
            edit_event_id: value.edit_event_id,
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
    #[serde(default)]
    pub edited_at: Option<u64>,
    #[serde(default)]
    pub edit_event_id: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelayLimits {
    #[serde(default)]
    pub max_message_length: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelayInformation {
    #[serde(default)]
    pub limitation: RelayLimits,
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

#[cfg(test)]
mod tests {
    use super::{
        huddle_started_description, is_conversational_unread_kind, timeline_profile_pubkeys,
        SystemMessage,
    };
    use buzz_core::kind::{
        KIND_HUDDLE_ENDED, KIND_HUDDLE_PARTICIPANT_JOINED, KIND_HUDDLE_PARTICIPANT_LEFT,
        KIND_HUDDLE_STARTED, KIND_STREAM_MESSAGE, KIND_SYSTEM_MESSAGE,
    };

    const ALICE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BOB: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const RELAY: &str = "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr";

    fn resolve_label(pubkey: &str) -> String {
        match pubkey {
            ALICE => "Alice".to_string(),
            BOB => "Bob".to_string(),
            _ => "unknown".to_string(),
        }
    }

    #[test]
    fn system_message_describes_membership_changes_with_names() {
        let joined = SystemMessage::parse(
            u64::from(KIND_SYSTEM_MESSAGE),
            &format!(r#"{{"type":"member_joined","actor":"{ALICE}","target":"{ALICE}"}}"#),
        )
        .expect("valid join event");
        let added = SystemMessage::parse(
            u64::from(KIND_SYSTEM_MESSAGE),
            &format!(r#"{{"type":"member_joined","actor":"{ALICE}","target":"{BOB}"}}"#),
        )
        .expect("valid add event");

        assert_eq!(
            joined.describe(resolve_label).as_deref(),
            Some("Alice joined the channel")
        );
        assert_eq!(
            added.describe(resolve_label).as_deref(),
            Some("Bob was added by Alice")
        );
    }

    #[test]
    fn system_message_profiles_use_payload_people_instead_of_relay_author() {
        let content = format!(r#"{{"type":"member_joined","actor":"{ALICE}","target":"{BOB}"}}"#);

        assert_eq!(
            timeline_profile_pubkeys(u64::from(KIND_SYSTEM_MESSAGE), RELAY, &content),
            vec![ALICE.to_string(), BOB.to_string()]
        );
        assert_eq!(
            timeline_profile_pubkeys(9, ALICE, "hello"),
            vec![ALICE.to_string()]
        );
    }

    #[test]
    fn malformed_or_unknown_system_messages_can_fall_back_to_raw_content() {
        assert!(SystemMessage::parse(u64::from(KIND_SYSTEM_MESSAGE), "not json").is_none());

        let unknown =
            SystemMessage::parse(u64::from(KIND_SYSTEM_MESSAGE), r#"{"type":"future_event"}"#)
                .expect("valid payload");
        assert_eq!(unknown.describe(resolve_label), None);
    }

    #[test]
    fn huddle_start_hides_internal_channel_id_behind_description() {
        let content = r#"{"ephemeral_channel_id":"d43c7a38-d117-4c4d-8739-3726b41c3f59"}"#;

        assert_eq!(
            huddle_started_description(u64::from(KIND_HUDDLE_STARTED), content),
            Some("started a huddle")
        );
        assert_eq!(huddle_started_description(9, content), None);
        assert_eq!(
            huddle_started_description(u64::from(KIND_HUDDLE_STARTED), "{}"),
            None
        );
    }

    #[test]
    fn dm_created_is_human_readable_and_hydrates_all_participants() {
        let message = SystemMessage::parse(
            u64::from(KIND_SYSTEM_MESSAGE),
            &format!(
                r#"{{"type":"dm_created","actor":"{ALICE}","participants":["{ALICE}","{BOB}"]}}"#
            ),
        )
        .expect("valid payload");

        assert_eq!(
            message.describe(|pubkey| match pubkey {
                ALICE => "Alice".to_string(),
                BOB => "Bob".to_string(),
                _ => "unknown".to_string(),
            }),
            Some("Alice started this direct message".to_string())
        );
        assert_eq!(
            message.profile_pubkeys(),
            vec![ALICE.to_string(), BOB.to_string()]
        );
    }

    #[test]
    fn huddle_lifecycle_kinds_are_non_conversational() {
        assert!(is_conversational_unread_kind(u64::from(
            KIND_STREAM_MESSAGE
        )));
        for kind in [
            KIND_SYSTEM_MESSAGE,
            KIND_HUDDLE_STARTED,
            KIND_HUDDLE_PARTICIPANT_JOINED,
            KIND_HUDDLE_PARTICIPANT_LEFT,
            KIND_HUDDLE_ENDED,
        ] {
            assert!(!is_conversational_unread_kind(u64::from(kind)));
        }
    }
}
