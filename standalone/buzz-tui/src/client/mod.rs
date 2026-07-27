use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use buzz_client::{
    BuzzClient, BuzzClientConfig, BuzzIdentity, ClientError, RelayMessage, RelaySubscription,
};
use buzz_core::engram::{
    self, conversation_key, d_tag as engram_d_tag, normalize_slug, select_head,
    validate_and_decrypt, Body,
};
use buzz_core::kind::{
    KIND_AGENT_ENGRAM, KIND_AGENT_PROFILE, KIND_CANVAS, KIND_CONTACT_LIST, KIND_DM_HIDE,
    KIND_DM_OPEN, KIND_DM_VISIBILITY, KIND_EMOJI_SET, KIND_EVENT_REMINDER, KIND_GIT_ISSUE,
    KIND_GIT_PATCH, KIND_GIT_PR_UPDATE, KIND_GIT_PULL_REQUEST, KIND_GIT_REPO_ANNOUNCEMENT,
    KIND_GIT_STATUS_CLOSED, KIND_GIT_STATUS_DRAFT, KIND_GIT_STATUS_MERGED, KIND_GIT_STATUS_OPEN,
    KIND_HUDDLE_STARTED, KIND_LONG_FORM, KIND_MANAGED_AGENT, KIND_NIP29_GROUP_MEMBERS,
    KIND_NIP29_GROUP_METADATA, KIND_NIP43_MEMBERSHIP_LIST, KIND_PRESENCE_SNAPSHOT,
    KIND_PRESENCE_UPDATE, KIND_REACTION, KIND_READ_STATE, KIND_REPORT, KIND_STREAM_MESSAGE,
    KIND_STREAM_MESSAGE_EDIT, KIND_STREAM_MESSAGE_V2, KIND_SYSTEM_MESSAGE, KIND_TEXT_NOTE,
    KIND_TYPING_INDICATOR, KIND_USER_STATUS, KIND_WORKFLOW_CANCELLED, KIND_WORKFLOW_COMPLETED,
    KIND_WORKFLOW_DEF, KIND_WORKFLOW_FAILED, KIND_WORKFLOW_STEP_COMPLETED,
    KIND_WORKFLOW_STEP_FAILED, KIND_WORKFLOW_STEP_STARTED, KIND_WORKFLOW_TRIGGER,
    KIND_WORKFLOW_TRIGGERED, RELAY_ADMIN_ADD_MEMBER, RELAY_ADMIN_CHANGE_ROLE,
    RELAY_ADMIN_REMOVE_MEMBER,
};
use buzz_sdk::mentions::{extract_nostr_uris, normalize_mention_pubkeys, strip_code_regions};
use buzz_sdk::{
    ChannelKind, CustomEmoji, DiffMeta, GitIssueMeta, GitPatchMeta, GitRepoCoord, GitStatus,
    GitStatusMeta, MemberRole, ThreadRef, Visibility, VoteDirection,
};
use chrono::{Local, TimeZone};
use nostr::nips::nip01::Coordinate;
use nostr::nips::nip44::{self, Version};
use nostr::{
    Alphabet, Event, EventBuilder, EventId, Filter, FromBech32, Keys, Kind, PublicKey,
    SingleLetterTag, Tag, Timestamp, ToBech32,
};
use rand::RngExt;
use serde_json::json;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

pub mod app_data;
mod models;

pub use models::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ChannelMessageSurface {
    #[default]
    Stream,
    Forum,
}

impl ChannelMessageSurface {
    pub(crate) fn from_channel_type(channel_type: &str) -> Self {
        if channel_type.eq_ignore_ascii_case("forum") {
            Self::Forum
        } else {
            Self::Stream
        }
    }
}

use crate::client::app_data::{
    channel_preference_payload, channel_preference_tags, channel_section_tags,
    channel_sections_payload, msg_context_key, read_state_payload, read_state_tags,
    thread_context_key, ChannelPreferenceEntry, ChannelPreferenceStoreKind, ChannelSectionRecord,
    ChannelSectionStore, ReadStateBlob, CHANNEL_SECTIONS_D_TAG,
};

const HISTORY_PAGE_LIMIT: u32 = 500;

/// Timeline row kinds served through the NIP-CW window filter — mirrors the
/// desktop's `TIMELINE_KINDS` so both clients see identical windows.
const CHANNEL_WINDOW_KINDS: [u32; 11] = [
    9, 40002, 40008, 40099, 43001, 43002, 43003, 43004, 43005, 43006, 48100,
];

/// Top-level rows requested when opening a channel via a NIP-CW window.
pub const CHANNEL_WINDOW_HEAD_LIMIT: u32 = 80;
/// Top-level rows per older-history window page.
pub const CHANNEL_WINDOW_PAGE_LIMIT: u32 = 100;
const MAX_HISTORY_PAGE_LIMIT: u32 = 2000;

static RELAY_INFORMATION_CACHE: OnceLock<Mutex<BTreeMap<String, RelayInformation>>> =
    OnceLock::new();
static CHANNEL_WINDOW_SUPPORT_CACHE: OnceLock<Mutex<BTreeMap<String, bool>>> = OnceLock::new();

#[derive(Clone)]
pub struct TuiRelayClient {
    shared: BuzzClient,
    keys: Keys,
}

impl std::fmt::Debug for TuiRelayClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TuiRelayClient")
            .field("base_url", &self.shared.relay_http_url())
            .field("public_key", &self.public_key_hex())
            .field("has_auth_tag", &self.shared.auth_tag().is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub enum RelayClientError {
    #[error("invalid private key: {0}")]
    Key(String),
    #[error("NIP-98 signing failed: {0}")]
    Signing(String),
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("WebSocket relay failed: {0}")]
    WebSocket(String),
    #[error("operation timed out")]
    Timeout,
    #[error("invalid event builder input: {0}")]
    Builder(String),
    #[error("event too large: {actual} bytes exceeds relay max_message_length {max}")]
    EventTooLarge { actual: usize, max: u64 },
}

impl From<ClientError> for RelayClientError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::InvalidKey(message) => Self::Key(message),
            ClientError::InvalidAuthTag(message) | ClientError::Signing(message) => {
                Self::Signing(message)
            }
            ClientError::Network(error) => Self::Http(format_error_chain(&error)),
            ClientError::WebSocket(error) => Self::WebSocket(error.to_string()),
            ClientError::Serialization(error) => Self::Json(error),
            ClientError::Relay {
                status, message, ..
            } => Self::Http(if message.trim().is_empty() {
                format!("HTTP {status}")
            } else {
                format!("HTTP {status}: {message}")
            }),
            ClientError::Rejected { message, .. } => {
                Self::Builder(format!("relay rejected event: {message}"))
            }
            ClientError::InvalidUrl(message)
            | ClientError::InvalidMedia(message)
            | ClientError::Protocol(message) => Self::Builder(message),
            ClientError::Timeout => Self::Timeout,
            ClientError::DeliveryUnknown { event_id, reason } => {
                Self::Http(format!("delivery of event {event_id} is unknown: {reason}"))
            }
        }
    }
}

impl TuiRelayClient {
    pub fn new(
        relay_url: impl Into<String>,
        private_key: &str,
        auth_tag_json: Option<String>,
    ) -> Result<Self, RelayClientError> {
        let identity = BuzzIdentity::parse(private_key, auth_tag_json.as_deref())?;
        let mut config = BuzzClientConfig::new(relay_url.into());
        config.connect_timeout = Duration::from_secs(5);
        config.request_timeout = Duration::from_secs(10);
        config.retry_policy.max_attempts = 1;
        let shared = BuzzClient::new(config, identity)?;
        let keys = shared.keys().clone();
        Ok(Self { shared, keys })
    }

    pub fn public_key_hex(&self) -> String {
        self.shared.public_key().to_hex()
    }

    pub fn public_key(&self) -> PublicKey {
        self.shared.public_key()
    }

    pub fn channel_messages_filter(channel_id: Uuid, since: Option<u64>) -> Filter {
        let filter = Filter::new()
            .kinds([
                Kind::Custom(KIND_STREAM_MESSAGE as u16),
                Kind::Custom(KIND_STREAM_MESSAGE_V2 as u16),
                Kind::Custom(KIND_STREAM_MESSAGE_EDIT as u16),
                Kind::Custom(KIND_REACTION as u16),
                Kind::Custom(buzz_core::kind::KIND_DELETION as u16),
                Kind::Custom(buzz_core::kind::KIND_NIP29_DELETE_EVENT as u16),
                Kind::Custom(buzz_core::kind::KIND_STREAM_MESSAGE_DIFF as u16),
                Kind::Custom(buzz_core::kind::KIND_FORUM_POST as u16),
                Kind::Custom(buzz_core::kind::KIND_FORUM_COMMENT as u16),
                Kind::Custom(KIND_SYSTEM_MESSAGE as u16),
                Kind::Custom(KIND_HUDDLE_STARTED as u16),
            ])
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::H),
                channel_id.to_string(),
            );
        if let Some(since) = since {
            filter.since(Timestamp::from(since))
        } else {
            filter
        }
    }

    pub fn channel_typing_filter(channel_id: Uuid, since: Option<u64>) -> Filter {
        let filter = Filter::new()
            .kind(Kind::Custom(KIND_TYPING_INDICATOR as u16))
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::H),
                channel_id.to_string(),
            );
        if let Some(since) = since {
            filter.since(Timestamp::from(since))
        } else {
            filter
        }
    }

    /// Build a NIP-CW channel-window filter: extension fields on a normal
    /// NIP-01 filter served by `POST /query`. `top_level` selects the window
    /// path; the cursor is composite (`until` + `before_id`), both or neither.
    pub fn channel_window_filter(
        channel_id: &str,
        limit: u32,
        cursor: Option<&WindowCursor>,
    ) -> Value {
        let mut filter = json!({
            "kinds": CHANNEL_WINDOW_KINDS,
            "#h": [channel_id],
            "limit": limit.clamp(1, 200),
            "top_level": true,
            "include_summaries": true,
        });
        if let Some(cursor) = cursor {
            filter["until"] = json!(cursor.created_at);
            filter["before_id"] = json!(cursor.id);
        }
        filter
    }

    /// Fetch one server-assembled channel window. `page.valid == false` is the
    /// NIP-CW downgrade signal: fall back to plain REQ history.
    pub async fn channel_window(
        &self,
        channel_id: &str,
        limit: u32,
        cursor: Option<&WindowCursor>,
    ) -> Result<ChannelWindowPage, RelayClientError> {
        parse_uuid(channel_id, "channel id")?;
        let support_cache =
            CHANNEL_WINDOW_SUPPORT_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
        if cursor.is_none()
            && support_cache
                .lock()
                .ok()
                .and_then(|cache| cache.get(self.shared.relay_http_url()).copied())
                == Some(false)
        {
            return Ok(ChannelWindowPage::default());
        }
        let value = self
            .query_values(&[Self::channel_window_filter(channel_id, limit, cursor)])
            .await?;
        let mut page = parse_channel_window_response(&value, channel_id, cursor);
        if cursor.is_none() {
            if let Ok(mut cache) = support_cache.lock() {
                cache.insert(self.shared.relay_http_url().to_string(), page.valid);
            }
        }
        // An unsupported relay returns ordinary timeline rows without the
        // NIP-CW bounds event. Do not hydrate a page the caller will discard.
        if !page.valid {
            return Ok(page);
        }
        if !page.messages.is_empty() {
            let views = page
                .messages
                .into_iter()
                .map(TuiMessageView::from)
                .collect();
            page.messages = self
                .apply_structural_aux(views)
                .await?
                .into_iter()
                .map(Message::from)
                .collect();
        }
        Ok(page)
    }

    pub fn channel_history_filter(channel_id: &str, limit: u32) -> Value {
        Self::channel_history_page_filter(channel_id, limit, None)
    }

    pub fn channel_history_page_filter(channel_id: &str, limit: u32, until: Option<u64>) -> Value {
        let mut filter = json!({
            "kinds": [
                KIND_STREAM_MESSAGE,
                KIND_STREAM_MESSAGE_V2,
                buzz_core::kind::KIND_STREAM_MESSAGE_DIFF,
                buzz_core::kind::KIND_FORUM_POST,
                buzz_core::kind::KIND_FORUM_COMMENT,
                KIND_HUDDLE_STARTED,
            ],
            "#h": [channel_id],
            "limit": clamp_history_page_limit(limit),
        });
        if let Some(until) = until {
            filter["until"] = json!(until);
        }
        filter
    }

    pub fn thread_filter(channel_id: &str, event_id: &str, limit: u32) -> Value {
        Self::thread_page_filter(channel_id, event_id, limit, None)
    }

    pub fn thread_page_filter(
        channel_id: &str,
        event_id: &str,
        limit: u32,
        until: Option<u64>,
    ) -> Value {
        let mut filter = json!({
            "kinds": [
                KIND_STREAM_MESSAGE,
                KIND_STREAM_MESSAGE_V2,
                KIND_STREAM_MESSAGE_EDIT,
                buzz_core::kind::KIND_STREAM_MESSAGE_DIFF,
                buzz_core::kind::KIND_FORUM_COMMENT,
            ],
            "#h": [channel_id],
            "#e": [event_id],
            "limit": clamp_history_page_limit(limit),
        });
        if let Some(until) = until {
            filter["until"] = json!(until);
        }
        filter
    }

    pub fn event_id_filter(event_id: &str) -> Value {
        json!({
            "ids": [event_id],
            "kinds": [
                KIND_STREAM_MESSAGE,
                KIND_STREAM_MESSAGE_V2,
                KIND_STREAM_MESSAGE_EDIT,
                buzz_core::kind::KIND_STREAM_MESSAGE_DIFF,
                buzz_core::kind::KIND_FORUM_POST,
                buzz_core::kind::KIND_FORUM_COMMENT,
            ],
            "limit": 1,
        })
    }

    pub fn search_filter(query: &str, limit: u32) -> Value {
        json!({
            "kinds": [
                KIND_STREAM_MESSAGE,
                KIND_STREAM_MESSAGE_V2,
                buzz_core::kind::KIND_FORUM_POST,
                buzz_core::kind::KIND_FORUM_COMMENT,
            ],
            "search": query,
            "limit": limit.min(100),
        })
    }

    pub fn relay_agents_filter(limit: u32) -> Value {
        json!({
            "kinds": [KIND_AGENT_PROFILE, KIND_MANAGED_AGENT],
            "limit": limit.min(500),
        })
    }

    pub fn feed_filter(pubkey: PublicKey, feed_type: Option<&str>, limit: u32) -> Value {
        let mut filter = json!({
            "#p": [pubkey.to_hex()],
            "limit": limit.min(50),
        });
        if let Some(feed_type) = feed_type {
            filter["feed_types"] = json!([feed_type]);
        }
        filter
    }

    pub fn joined_channels_filter(pubkey: PublicKey, since: Option<u64>) -> Filter {
        let filter = Filter::new()
            .kind(Kind::Custom(KIND_NIP29_GROUP_MEMBERS as u16))
            .custom_tag(SingleLetterTag::lowercase(Alphabet::P), pubkey.to_hex());
        if let Some(since) = since {
            filter.since(Timestamp::from(since))
        } else {
            filter
        }
    }

    pub fn mentions_filter(pubkey: PublicKey, since: Option<u64>) -> Filter {
        let filter = Filter::new()
            .kinds([
                Kind::Custom(KIND_TEXT_NOTE as u16),
                Kind::Custom(KIND_STREAM_MESSAGE as u16),
                Kind::Custom(KIND_STREAM_MESSAGE_V2 as u16),
            ])
            .custom_tag(SingleLetterTag::lowercase(Alphabet::P), pubkey.to_hex());
        if let Some(since) = since {
            filter.since(Timestamp::from(since))
        } else {
            filter
        }
    }

    pub fn app_data_filter(pubkey: PublicKey, since: Option<u64>) -> Filter {
        let filter = Filter::new().author(pubkey).kinds([
            Kind::Custom(KIND_READ_STATE as u16),
            Kind::Custom(KIND_EMOJI_SET as u16),
        ]);
        if let Some(since) = since {
            filter.since(Timestamp::from(since))
        } else {
            filter
        }
    }

    pub fn custom_emoji_filter(since: Option<u64>) -> Filter {
        let filter = Filter::new().kind(Kind::Custom(KIND_EMOJI_SET as u16));
        if let Some(since) = since {
            filter.since(Timestamp::from(since))
        } else {
            filter
        }
    }

    pub fn contacts_filter(pubkey: PublicKey, since: Option<u64>) -> Filter {
        let filter = Filter::new()
            .author(pubkey)
            .kind(Kind::Custom(KIND_CONTACT_LIST as u16));
        if let Some(since) = since {
            filter.since(Timestamp::from(since))
        } else {
            filter
        }
    }

    pub fn presence_filter(pubkeys: Vec<PublicKey>, since: Option<u64>) -> Filter {
        let filter = Filter::new()
            .kind(Kind::Custom(KIND_PRESENCE_UPDATE as u16))
            .authors(pubkeys);
        if let Some(since) = since {
            filter.since(Timestamp::from(since))
        } else {
            filter
        }
    }

    pub fn channel_metadata_filter(since: Option<u64>) -> Filter {
        let filter = Filter::new().kind(Kind::Custom(KIND_NIP29_GROUP_METADATA as u16));
        if let Some(since) = since {
            filter.since(Timestamp::from(since))
        } else {
            filter
        }
    }

    pub fn build_message_event(
        &self,
        channel_id: Uuid,
        surface: ChannelMessageSurface,
        content: &str,
        explicit_mention_pubkeys: &[String],
        reply_to: Option<(EventId, EventId)>,
        media_tags: &[Vec<String>],
    ) -> Result<Event, RelayClientError> {
        let thread_ref = reply_to.map(|(root_event_id, parent_event_id)| ThreadRef {
            root_event_id,
            parent_event_id,
        });
        let stripped = strip_code_regions(content);
        let mut mention_pubkeys = explicit_mention_pubkeys.to_vec();
        mention_pubkeys.extend(extract_nostr_uris(&stripped));
        let mention_pubkeys =
            normalize_mention_pubkeys(&mention_pubkeys, Some(&self.public_key_hex()));
        let mention_refs: Vec<&str> = mention_pubkeys.iter().map(String::as_str).collect();
        let builder = match (surface, thread_ref.as_ref()) {
            (ChannelMessageSurface::Forum, Some(thread_ref)) => buzz_sdk::build_forum_comment(
                channel_id,
                content,
                thread_ref,
                &mention_refs,
                media_tags,
            ),
            (ChannelMessageSurface::Forum, None) => {
                buzz_sdk::build_forum_post(channel_id, content, &mention_refs, media_tags)
            }
            (ChannelMessageSurface::Stream, thread_ref) => buzz_sdk::build_message(
                channel_id,
                content,
                thread_ref,
                &mention_refs,
                false,
                media_tags,
            ),
        }
        .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        self.sign_event(builder)
    }

    pub fn build_channel_message_event(
        &self,
        channel_id: &str,
        surface: ChannelMessageSurface,
        content: &str,
        mention_pubkeys: &[String],
        reply_to: Option<&str>,
    ) -> Result<Event, RelayClientError> {
        let uuid = Uuid::parse_str(channel_id)
            .map_err(|error| RelayClientError::Builder(format!("channel id: {error}")))?;
        let reply = reply_to
            .map(|event_id| {
                let event_id = parse_event_id(event_id, "reply id")?;
                Ok::<_, RelayClientError>((event_id, event_id))
            })
            .transpose()?;
        self.build_message_event(uuid, surface, content, mention_pubkeys, reply, &[])
    }

    /// Build and submit a channel message directly to the relay, bypassing the
    /// CLI subprocess. `reply_to` is a thread root event id (hex).
    pub async fn send_channel_message(
        &self,
        channel_id: &str,
        surface: ChannelMessageSurface,
        content: &str,
        mention_pubkeys: &[String],
        reply_to: Option<&str>,
    ) -> Result<(), RelayClientError> {
        let uuid = Uuid::parse_str(channel_id)
            .map_err(|error| RelayClientError::Builder(format!("channel id: {error}")))?;
        let reply = self.resolve_reply_pair(reply_to).await?;
        let event =
            self.build_message_event(uuid, surface, content, mention_pubkeys, reply, &[])?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn send_channel_message_with_files(
        &self,
        channel_id: &str,
        surface: ChannelMessageSurface,
        content: &str,
        mention_pubkeys: &[String],
        reply_to: Option<&str>,
        files: &[String],
    ) -> Result<Value, RelayClientError> {
        let uuid = parse_uuid(channel_id, "channel id")?;
        let mut media_tags = Vec::new();
        let mut media_content = String::new();
        for file in files {
            let upload = self.upload_file(file).await?;
            media_tags.push(build_imeta_tag(&upload));
            if upload.mime_type.starts_with("video/") {
                media_content.push_str("\n![video](");
            } else {
                media_content.push_str("\n![image](");
            }
            media_content.push_str(&upload.url);
            media_content.push(')');
        }
        let final_content = if media_content.is_empty() {
            content.to_string()
        } else {
            format!("{content}{media_content}")
        };
        let reply = self.resolve_reply_pair(reply_to).await?;
        let event = self.build_message_event(
            uuid,
            surface,
            &final_content,
            mention_pubkeys,
            reply,
            &media_tags,
        )?;
        self.submit_event(&event).await
    }

    pub async fn send_diff(
        &self,
        channel_id: &str,
        options: &SendDiffOptions,
        reply_to: Option<&str>,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let reply =
            self.resolve_reply_pair(reply_to)
                .await?
                .map(|(root_event_id, parent_event_id)| ThreadRef {
                    root_event_id,
                    parent_event_id,
                });
        let file_path = non_empty(&options.file_path).map(str::to_string);
        let description = non_empty(&options.description).map(str::to_string);
        let alt_text = file_path
            .as_ref()
            .map(|file| match &description {
                Some(description) => format!("Diff: {file} — {description}"),
                None => format!("Diff: {file}"),
            })
            .unwrap_or_else(|| "Diff".to_string());
        let diff_meta = DiffMeta {
            repo_url: options.repo_url.clone(),
            commit_sha: options.commit_sha.clone(),
            file_path,
            parent_commit: None,
            branch: None,
            pr_number: None,
            language: options
                .file_path
                .rsplit_once('.')
                .map(|(_, ext)| ext.to_string()),
            description,
            truncated: false,
            alt_text: Some(alt_text),
        };
        let event = self.sign_event(
            buzz_sdk::build_diff_message(channel_id, &options.diff, &diff_meta, reply.as_ref())
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn vote_message(
        &self,
        event_id: &str,
        direction: &str,
    ) -> Result<Value, RelayClientError> {
        let channel_id = self.resolve_event_channel_id(event_id).await?;
        let target = parse_event_id(event_id, "event id")?;
        let direction = match direction {
            "up" => VoteDirection::Up,
            "down" => VoteDirection::Down,
            other => return Err(RelayClientError::Builder(format!("invalid vote {other:?}"))),
        };
        let event = self.sign_event(
            buzz_sdk::build_vote(channel_id, target, direction)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn edit_message(
        &self,
        event_id: &str,
        content: &str,
    ) -> Result<(), RelayClientError> {
        let channel_id = self.resolve_event_channel_id(event_id).await?;
        let target = EventId::from_hex(event_id)
            .map_err(|error| RelayClientError::Builder(format!("event id: {error}")))?;
        let builder = buzz_sdk::build_edit(channel_id, target, content)
            .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let event = self.sign_event(builder)?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn delete_message(&self, event_id: &str) -> Result<(), RelayClientError> {
        let channel_id = self.resolve_event_channel_id(event_id).await?;
        let target = EventId::from_hex(event_id)
            .map_err(|error| RelayClientError::Builder(format!("event id: {error}")))?;
        let builder = buzz_sdk::build_delete_message(channel_id, target)
            .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let event = self.sign_event(builder)?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn add_reaction(
        &self,
        event_id: &str,
        emoji: &str,
        emoji_url: Option<&str>,
    ) -> Result<(), RelayClientError> {
        let target = EventId::from_hex(event_id)
            .map_err(|error| RelayClientError::Builder(format!("event id: {error}")))?;
        let builder = if let Some(url) = emoji_url {
            buzz_sdk::build_custom_emoji_reaction(target, emoji, url)
        } else {
            buzz_sdk::build_reaction(target, emoji)
        }
        .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let event = self.sign_event(builder)?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn remove_reaction(
        &self,
        event_id: &str,
        emoji: &str,
    ) -> Result<(), RelayClientError> {
        let reaction_id = self.find_own_reaction(event_id, emoji).await?;
        let target = EventId::from_hex(&reaction_id)
            .map_err(|error| RelayClientError::Builder(format!("reaction id: {error}")))?;
        let builder = buzz_sdk::build_remove_reaction(target)
            .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let event = self.sign_event(builder)?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn query_reactions(&self, event_id: &str) -> Result<Vec<Reaction>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_REACTION],
                "#e": [event_id],
            })])
            .await?;
        Ok(group_reactions(value.as_array().into_iter().flatten()))
    }

    pub async fn current_profile(&self) -> Result<Option<UserProfile>, RelayClientError> {
        self.user_profile(&self.public_key_hex()).await
    }

    pub async fn user_profile(
        &self,
        pubkey: &str,
    ) -> Result<Option<UserProfile>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [0],
                "authors": [pubkey],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .find_map(parse_profile_event))
    }

    pub async fn user_profiles(
        &self,
        pubkeys: &[String],
    ) -> Result<Vec<UserProfile>, RelayClientError> {
        if pubkeys.is_empty() {
            return Ok(Vec::new());
        }

        let value = self
            .query_values(&[json!({
                "kinds": [0],
                "authors": pubkeys,
                "limit": pubkeys.len().max(1) * 4,
            })])
            .await?;
        let mut latest = BTreeMap::new();
        for event in value.as_array().into_iter().flatten() {
            let Some(profile) = parse_profile_event(event) else {
                continue;
            };
            let created_at = event
                .get("created_at")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let should_replace = latest
                .get(&profile.pubkey)
                .map(|(existing_created_at, _)| created_at >= *existing_created_at)
                .unwrap_or(true);
            if should_replace {
                latest.insert(profile.pubkey.clone(), (created_at, profile));
            }
        }
        Ok(latest.into_values().map(|(_, profile)| profile).collect())
    }

    pub async fn relay_agents(&self) -> Result<Vec<RelayAgentInfo>, RelayClientError> {
        let value = self.query_values(&[Self::relay_agents_filter(500)]).await?;
        let mut latest = BTreeMap::new();
        for event in parse_nostr_events(&value) {
            let Some(agent) = parse_relay_agent_event(&event) else {
                continue;
            };
            let should_replace = latest
                .get(&agent.pubkey)
                .map(|(existing_created_at, _)| agent.created_at >= *existing_created_at)
                .unwrap_or(true);
            if should_replace {
                latest.insert(agent.pubkey.clone(), (agent.created_at, agent));
            }
        }
        let mut agents = latest
            .into_values()
            .map(|(_, agent)| agent)
            .collect::<Vec<_>>();
        if agents.is_empty() {
            return Ok(agents);
        }

        let pubkeys = agents
            .iter()
            .map(|agent| agent.pubkey.clone())
            .collect::<Vec<_>>();
        let presence = self
            .query_values(&[json!({
                "kinds": [KIND_PRESENCE_SNAPSHOT],
                "authors": pubkeys,
                "limit": agents.len(),
            })])
            .await?;
        apply_presence_to_relay_agents(&mut agents, &presence);
        if let Ok(health) = self
            .query_values(&[json!({
                "kinds": [KIND_STREAM_MESSAGE],
                "#t": ["beekeeper-health"],
                "limit": 500,
            })])
            .await
        {
            apply_beekeeper_health_to_relay_agents(&mut agents, &health);
        }
        Ok(agents)
    }

    pub async fn search_user_profiles(
        &self,
        query: &str,
    ) -> Result<Vec<UserProfile>, RelayClientError> {
        let lower_query = query.to_ascii_lowercase();
        let value = self
            .query_values(&[json!({
                "kinds": [0],
                "search": query,
                "limit": 100,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse_profile_event)
            .filter(|profile| {
                profile
                    .display_name
                    .to_ascii_lowercase()
                    .contains(&lower_query)
                    || profile.name.to_ascii_lowercase().contains(&lower_query)
            })
            .collect())
    }

    pub async fn set_profile_field(
        &self,
        field: ProfileField,
        value: &str,
    ) -> Result<(), RelayClientError> {
        let current = self.current_profile().await?.unwrap_or_default();
        let event = self.build_profile_field_event(&current, field, value)?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn user_status(&self, pubkey: &str) -> Result<Option<UserStatus>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_USER_STATUS],
                "authors": [pubkey],
                "#d": ["general"],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .max_by_key(|event| {
                event
                    .get("created_at")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
            })
            .map(|event| UserStatus {
                text: event
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                emoji: tag_value(event, "emoji").unwrap_or_default(),
                updated_at: event
                    .get("created_at")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            })
            .filter(|status| !status.text.is_empty() || !status.emoji.is_empty()))
    }

    pub async fn set_user_status(&self, text: &str, emoji: &str) -> Result<(), RelayClientError> {
        let event = self.build_user_status_event(text, emoji)?;
        self.submit_event(&event).await.map(|_| ())
    }

    fn build_user_status_event(&self, text: &str, emoji: &str) -> Result<Event, RelayClientError> {
        let mut tags = vec![parse_tag(["d", "general"])?];
        if !emoji.trim().is_empty() {
            tags.push(parse_tag(["emoji", emoji.trim()])?);
        }
        self.sign_event(
            EventBuilder::new(Kind::Custom(KIND_USER_STATUS as u16), text.trim()).tags(tags),
        )
    }

    pub async fn report_message(
        &self,
        author_pubkey: &str,
        event_id: &str,
        report_type: &str,
        note: &str,
    ) -> Result<(), RelayClientError> {
        const REPORT_TYPES: [&str; 7] = [
            "illegal",
            "nudity",
            "malware",
            "spam",
            "impersonation",
            "profanity",
            "other",
        ];
        PublicKey::from_hex(author_pubkey)
            .map_err(|error| RelayClientError::Builder(format!("author pubkey: {error}")))?;
        parse_event_id(event_id, "report event id")?;
        if !REPORT_TYPES.contains(&report_type) {
            return Err(RelayClientError::Builder(format!(
                "unsupported report type: {report_type}"
            )));
        }
        let tags = vec![
            parse_tag(["p", &author_pubkey.to_ascii_lowercase()])?,
            parse_tag(["e", event_id, report_type])?,
        ];
        let event = self.sign_event(
            EventBuilder::new(Kind::Custom(KIND_REPORT as u16), note.trim()).tags(tags),
        )?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn moderation_reports(&self) -> Result<Vec<ModerationReport>, RelayClientError> {
        let body = self
            .shared
            .get_authenticated("/moderation/reports?status=open&limit=100")
            .await?;
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn dismiss_moderation_report(
        &self,
        report_event_id: &str,
    ) -> Result<(), RelayClientError> {
        let builder = buzz_sdk::build_moderation_resolve_report(
            report_event_id,
            "dismissed",
            "dismiss",
            None,
        )
        .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let event = self.sign_event(builder)?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn mint_invite(
        &self,
        ttl_secs: Option<u64>,
    ) -> Result<MintedInvite, RelayClientError> {
        let body = ttl_secs.map_or_else(|| json!({}), |ttl_secs| json!({ "ttl_secs": ttl_secs }));
        let response = self.shared.post_json_value("/api/invites", &body).await?;
        Ok(serde_json::from_value(response)?)
    }

    fn build_profile_field_event(
        &self,
        current: &UserProfile,
        field: ProfileField,
        value: &str,
    ) -> Result<Event, RelayClientError> {
        let display_name = match field {
            ProfileField::DisplayName => value,
            _ => current.display_name.as_str(),
        };
        let picture = match field {
            ProfileField::Picture => value,
            _ => current.picture.as_str(),
        };
        let about = match field {
            ProfileField::About => value,
            _ => current.about.as_str(),
        };
        let nip05 = match field {
            ProfileField::Nip05 => value,
            _ => current.nip05.as_str(),
        };
        let builder = buzz_sdk::build_profile(
            non_empty(display_name),
            non_empty(&current.name),
            non_empty(picture),
            non_empty(about),
            non_empty(nip05),
        )
        .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        self.sign_event(builder)
    }

    pub async fn presence(&self, pubkey: &str) -> Result<Option<PresenceInfo>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_PRESENCE_SNAPSHOT],
                "authors": [pubkey],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .find_map(parse_presence_event))
    }

    pub async fn set_presence(&self, status: PresenceStatus) -> Result<(), RelayClientError> {
        let builder = buzz_sdk::build_presence_update(status.as_str())
            .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let event = self.sign_event(builder)?;
        self.shared.publish_ephemeral(event).await?;
        Ok(())
    }

    pub async fn contact_list(&self, pubkey: &str) -> Result<Vec<Contact>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_CONTACT_LIST],
                "authors": [pubkey],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .next()
            .map(parse_contact_list_event)
            .unwrap_or_default())
    }

    pub async fn set_contact_list(&self, contacts: &[Contact]) -> Result<Value, RelayClientError> {
        let contacts = contacts
            .iter()
            .map(|contact| {
                (
                    contact.pubkey.as_str(),
                    non_empty(&contact.relay_url),
                    non_empty(&contact.petname),
                )
            })
            .collect::<Vec<_>>();
        let event = self.sign_event(
            buzz_sdk::build_contact_list(&contacts)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn publish_social_note(
        &self,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<Value, RelayClientError> {
        let reply_to = reply_to
            .map(|event_id| parse_event_id(event_id, "reply id"))
            .transpose()?;
        let event = self.sign_event(
            buzz_sdk::build_note(content, reply_to)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn social_user_notes(
        &self,
        pubkey: &str,
        limit: u32,
    ) -> Result<Vec<Message>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_TEXT_NOTE],
                "authors": [pubkey],
                "limit": limit.min(100),
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .map(Message::from)
            .filter(|message| !message.id.is_empty() || !message.content.is_empty())
            .collect())
    }

    pub async fn list_repos(&self) -> Result<Vec<RepoProject>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_GIT_REPO_ANNOUNCEMENT],
                "authors": [self.public_key_hex()],
                "limit": 100,
            })])
            .await?;
        let mut repos = value
            .as_array()
            .into_iter()
            .flatten()
            .map(parse_repo_event)
            .filter(|repo| !repo.dtag.is_empty())
            .collect::<Vec<_>>();
        repos.sort_by_key(|repo| std::cmp::Reverse(repo.created_at));
        Ok(repos)
    }

    pub async fn create_repo(
        &self,
        options: &CreateRepoOptions,
    ) -> Result<Value, RelayClientError> {
        let clone_urls = options
            .clone_urls
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let relays = options
            .relays
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let event = self.sign_event(
            buzz_sdk::build_repo_announcement(
                &options.id,
                non_empty(&options.name),
                non_empty(&options.description),
                &clone_urls,
                non_empty(&options.web_url),
                &relays,
            )
            .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn list_repo_issues(
        &self,
        repo_owner: &str,
        repo_id: &str,
        limit: u32,
    ) -> Result<Vec<GitIssue>, RelayClientError> {
        parse_pubkey(repo_owner, "repo owner")?;
        let filter = json!({
            "kinds": [KIND_GIT_ISSUE],
            "#a": [repo_coordinate(repo_owner, repo_id)],
            "limit": limit.min(200),
        });
        let value = self.query_values(&[filter]).await?;
        let mut issues = value
            .as_array()
            .into_iter()
            .flatten()
            .map(|event| parse_issue_event(event, repo_owner, repo_id))
            .collect::<Vec<_>>();
        issues.sort_by_key(|issue| std::cmp::Reverse(issue.created_at));
        Ok(issues)
    }

    pub async fn create_issue(
        &self,
        options: &CreateIssueOptions,
    ) -> Result<Value, RelayClientError> {
        parse_pubkey(&options.repo_owner, "repo owner")?;
        if options.repo_id.trim().is_empty() {
            return Err(RelayClientError::Builder("repo id is empty".to_string()));
        }
        let meta = GitIssueMeta {
            labels: options.labels.clone(),
            recipients: options.recipients.clone(),
        };
        let repo = GitRepoCoord {
            owner: options.repo_owner.clone(),
            id: options.repo_id.clone(),
        };
        let event = self.sign_event(
            buzz_sdk::build_git_issue(&repo, &options.title, &options.content, &meta)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn list_repo_patches(
        &self,
        repo_owner: &str,
        repo_id: &str,
        limit: u32,
    ) -> Result<Vec<GitPatch>, RelayClientError> {
        parse_pubkey(repo_owner, "repo owner")?;
        let filter = json!({
            "kinds": [KIND_GIT_PATCH],
            "#a": [repo_coordinate(repo_owner, repo_id)],
            "limit": limit.min(200),
        });
        let value = self.query_values(&[filter]).await?;
        let mut patches = value
            .as_array()
            .into_iter()
            .flatten()
            .map(|event| parse_patch_event(event, repo_owner, repo_id))
            .collect::<Vec<_>>();
        patches.sort_by_key(|patch| std::cmp::Reverse(patch.created_at));
        Ok(patches)
    }

    pub async fn list_repo_pull_requests(
        &self,
        repo_owner: &str,
        repo_id: &str,
        limit: u32,
    ) -> Result<Vec<GitPullRequest>, RelayClientError> {
        parse_pubkey(repo_owner, "repo owner")?;
        let coordinate = repo_coordinate(repo_owner, repo_id);
        let value = self
            .query_values(&[
                json!({ "kinds": [KIND_GIT_PULL_REQUEST], "#a": [coordinate.clone()], "limit": limit.min(200) }),
                json!({ "kinds": [KIND_GIT_PR_UPDATE], "#a": [coordinate.clone()], "limit": 500 }),
                json!({ "kinds": [KIND_GIT_STATUS_OPEN, KIND_GIT_STATUS_MERGED, KIND_GIT_STATUS_CLOSED, KIND_GIT_STATUS_DRAFT], "#a": [coordinate.clone()], "limit": 500 }),
                json!({ "kinds": [KIND_TEXT_NOTE], "#a": [coordinate], "limit": 1000 }),
            ])
            .await?;
        let events = value.as_array().cloned().unwrap_or_default();
        let mut pull_requests = events
            .iter()
            .filter(|event| {
                event.get("kind").and_then(Value::as_u64) == Some(u64::from(KIND_GIT_PULL_REQUEST))
            })
            .map(|root| parse_pull_request(root, &events, repo_owner, repo_id))
            .collect::<Vec<_>>();
        pull_requests.sort_by_key(|pull_request| std::cmp::Reverse(pull_request.updated_at));
        Ok(pull_requests)
    }

    pub async fn review_pull_request(
        &self,
        pull_request: &GitPullRequest,
        approve: bool,
    ) -> Result<(), RelayClientError> {
        let event = self.build_pull_request_review_event(pull_request, approve)?;
        self.submit_event(&event).await.map(|_| ())
    }

    fn build_pull_request_review_event(
        &self,
        pull_request: &GitPullRequest,
        approve: bool,
    ) -> Result<Event, RelayClientError> {
        if pull_request.commit.is_empty() {
            return Err(RelayClientError::Builder(
                "pull request has no commit to review".to_string(),
            ));
        }
        let label = if approve {
            "approval"
        } else {
            "changes-requested"
        };
        let content = if approve {
            "Approved these changes"
        } else {
            "Requested changes"
        };
        let repo_address = repo_coordinate(&pull_request.repo_owner, &pull_request.repo_id);
        let mut tags = vec![
            parse_tag(["e", pull_request.id.as_str(), "", "root"])?,
            parse_tag(["a", repo_address.as_str()])?,
        ];
        let recipients = [
            pull_request.repo_owner.as_str(),
            pull_request.author.as_str(),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        for recipient in recipients {
            tags.push(parse_tag(["p", recipient])?);
        }
        tags.push(parse_tag(["t", label])?);
        tags.push(parse_tag(["c", pull_request.commit.as_str()])?);
        let created_at = Timestamp::now().as_secs().max(
            pull_request
                .review_created_at
                .unwrap_or_default()
                .saturating_add(1),
        );
        self.sign_event(
            EventBuilder::new(Kind::TextNote, content)
                .tags(tags)
                .custom_created_at(Timestamp::from(created_at)),
        )
    }

    pub async fn set_pull_request_status(
        &self,
        pull_request: &GitPullRequest,
        status: GitStatus,
    ) -> Result<(), RelayClientError> {
        if status == GitStatus::AppliedOrResolved {
            return Err(RelayClientError::Builder(
                "merged status requires an actual git merge and is not published by the TUI"
                    .to_string(),
            ));
        }
        let repo = GitRepoCoord {
            owner: pull_request.repo_owner.clone(),
            id: pull_request.repo_id.clone(),
        };
        let meta = GitStatusMeta {
            root_event: pull_request.id.clone(),
            repo: Some(repo),
            recipients: vec![pull_request.author.clone()],
            ..GitStatusMeta::default()
        };
        let created_at = Timestamp::now().as_secs().max(
            pull_request
                .status_created_at
                .unwrap_or_default()
                .saturating_add(1),
        );
        let builder = buzz_sdk::build_git_status(status, "", &meta)
            .map_err(|error| RelayClientError::Builder(error.to_string()))?
            .custom_created_at(Timestamp::from(created_at));
        let event = self.sign_event(builder)?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn create_patch(
        &self,
        options: &CreatePatchOptions,
    ) -> Result<Value, RelayClientError> {
        parse_pubkey(&options.repo_owner, "repo owner")?;
        if options.repo_id.trim().is_empty() {
            return Err(RelayClientError::Builder("repo id is empty".to_string()));
        }
        let meta = GitPatchMeta {
            recipients: options.recipients.clone(),
            root: options.root,
            root_revision: options.root_revision,
            commit: non_empty(&options.commit).map(str::to_string),
            parent_commit: non_empty(&options.parent_commit).map(str::to_string),
            ..GitPatchMeta::default()
        };
        let repo = GitRepoCoord {
            owner: options.repo_owner.clone(),
            id: options.repo_id.clone(),
        };
        let event = self.sign_event(
            buzz_sdk::build_git_patch(&repo, &options.content, &meta)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn list_workflows(
        &self,
        channel_id: &str,
    ) -> Result<Vec<Workflow>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_WORKFLOW_DEF],
                "#h": [channel_id],
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .map(parse_workflow_event)
            .collect())
    }

    pub async fn get_workflow(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowDetail>, RelayClientError> {
        let workflow_id = parse_uuid(workflow_id, "workflow id")?.to_string();
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_WORKFLOW_DEF],
                "#d": [workflow_id],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .next()
            .map(parse_workflow_detail_event))
    }

    pub async fn get_workflow_runs(
        &self,
        workflow_id: &str,
    ) -> Result<Vec<WorkflowRun>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [
                    KIND_WORKFLOW_TRIGGERED,
                    KIND_WORKFLOW_STEP_STARTED,
                    KIND_WORKFLOW_STEP_COMPLETED,
                    KIND_WORKFLOW_STEP_FAILED,
                    KIND_WORKFLOW_COMPLETED,
                    KIND_WORKFLOW_FAILED,
                    KIND_WORKFLOW_CANCELLED,
                ],
                "#d": [workflow_id],
                "limit": 20,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .map(parse_workflow_run_event)
            .collect())
    }

    pub async fn trigger_workflow(
        &self,
        workflow_id: &str,
        inputs: Option<&str>,
    ) -> Result<Value, RelayClientError> {
        let workflow_id = parse_uuid(workflow_id, "workflow id")?;
        let builder = if let Some(inputs) = inputs {
            let parsed: Value = serde_json::from_str(inputs)?;
            if !parsed.is_object() {
                return Err(RelayClientError::Builder(
                    "workflow inputs must be a JSON object".to_string(),
                ));
            }
            EventBuilder::new(
                Kind::Custom(KIND_WORKFLOW_TRIGGER as u16),
                parsed.to_string(),
            )
            .tags([parse_tag(["d", workflow_id.to_string().as_str()])?])
        } else {
            buzz_sdk::build_workflow_trigger(workflow_id)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?
        };
        let event = self.sign_event(builder)?;
        self.submit_event(&event).await
    }

    pub async fn create_workflow(
        &self,
        channel_id: &str,
        yaml: &str,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let workflow_id = Uuid::new_v4();
        let event = self.sign_event(
            buzz_sdk::build_workflow_def(channel_id, workflow_id, yaml)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        let mut response = self.submit_event(&event).await?;
        response["workflow_id"] = json!(workflow_id.to_string());
        if let Some(secret) = workflow_webhook_secret(&response) {
            response["webhook_secret"] = json!(secret);
        }
        Ok(response)
    }

    pub async fn update_workflow(
        &self,
        channel_id: &str,
        workflow_id: &str,
        yaml: &str,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let workflow_id = parse_uuid(workflow_id, "workflow id")?;
        let event = self.sign_event(
            buzz_sdk::build_workflow_update(channel_id, workflow_id, yaml)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn delete_workflow(&self, workflow_id: &str) -> Result<Value, RelayClientError> {
        let workflow_id = parse_uuid(workflow_id, "workflow id")?;
        let event = self.sign_event(
            buzz_sdk::build_workflow_delete(&self.public_key_hex(), workflow_id)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn approve_workflow_step(
        &self,
        approval_token: &str,
        approved: bool,
        note: &str,
    ) -> Result<Value, RelayClientError> {
        parse_uuid(approval_token, "approval token")?;
        let token_hash = hex::encode(Sha256::digest(approval_token.as_bytes()));
        let event = self.sign_event(
            buzz_sdk::build_workflow_approval(&token_hash, approved, note)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn list_channels(&self, joined_only: bool) -> Result<Vec<Channel>, RelayClientError> {
        let metadata_filter = if joined_only {
            let member_events = self
                .query_values(&[json!({
                    "kinds": [KIND_NIP29_GROUP_MEMBERS],
                    "#p": [self.public_key_hex()],
                    "limit": 500,
                })])
                .await?;
            let channel_ids = member_events
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(d_tag)
                .collect::<Vec<_>>();
            if channel_ids.is_empty() {
                return Ok(Vec::new());
            }
            json!({
                "kinds": [KIND_NIP29_GROUP_METADATA],
                "#d": channel_ids,
                "limit": 500,
            })
        } else {
            json!({
                "kinds": [KIND_NIP29_GROUP_METADATA],
                "limit": 500,
            })
        };
        let value = self.query_values(&[metadata_filter]).await?;
        let hidden_dms = if joined_only {
            self.hidden_dm_ids().await.unwrap_or_default()
        } else {
            BTreeSet::new()
        };
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse_channel_metadata_event)
            .filter(|channel| {
                channel.kind != ConversationKind::DirectMessage || !hidden_dms.contains(&channel.id)
            })
            .collect())
    }

    async fn hidden_dm_ids(&self) -> Result<BTreeSet<String>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_DM_VISIBILITY],
                "#p": [self.public_key_hex()],
                "limit": 1,
            })])
            .await?;
        let newest = value.as_array().into_iter().flatten().max_by_key(|event| {
            event
                .get("created_at")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        });
        Ok(newest
            .map(|event| event_tag_targets(event, "h").into_iter().collect())
            .unwrap_or_default())
    }

    pub async fn channel(&self, channel_id: &str) -> Result<Option<Channel>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_NIP29_GROUP_METADATA],
                "#d": [channel_id],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .find_map(parse_channel_metadata_event))
    }

    pub async fn channel_members(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ChannelMember>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_NIP29_GROUP_MEMBERS],
                "#d": [channel_id],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .next()
            .map(parse_channel_members_event)
            .unwrap_or_default())
    }

    pub async fn list_relay_members(&self) -> Result<Vec<RelayMember>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_NIP43_MEMBERSHIP_LIST],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .next()
            .map(parse_relay_members_event)
            .unwrap_or_default())
    }

    pub async fn add_relay_member(
        &self,
        pubkey: &str,
        role: &str,
    ) -> Result<Value, RelayClientError> {
        let pubkey = normalize_pubkey(pubkey, "pubkey")?;
        let role = normalize_relay_member_role(role)?;
        let event = self.sign_event(
            EventBuilder::new(Kind::Custom(RELAY_ADMIN_ADD_MEMBER as u16), "").tags(vec![
                parse_tag(["p", pubkey.as_str()])?,
                parse_tag(["role", role])?,
            ]),
        )?;
        self.submit_event(&event).await
    }

    pub async fn remove_relay_member(&self, pubkey: &str) -> Result<Value, RelayClientError> {
        let pubkey = normalize_pubkey(pubkey, "pubkey")?;
        let event = self.sign_event(
            EventBuilder::new(Kind::Custom(RELAY_ADMIN_REMOVE_MEMBER as u16), "")
                .tags(vec![parse_tag(["p", pubkey.as_str()])?]),
        )?;
        self.submit_event(&event).await
    }

    pub async fn change_relay_member_role(
        &self,
        pubkey: &str,
        role: &str,
    ) -> Result<Value, RelayClientError> {
        let pubkey = normalize_pubkey(pubkey, "pubkey")?;
        let role = normalize_relay_member_role(role)?;
        let event = self.sign_event(
            EventBuilder::new(Kind::Custom(RELAY_ADMIN_CHANGE_ROLE as u16), "").tags(vec![
                parse_tag(["p", pubkey.as_str()])?,
                parse_tag(["role", role])?,
            ]),
        )?;
        self.submit_event(&event).await
    }

    pub async fn open_dm(&self, pubkey: &str) -> Result<Value, RelayClientError> {
        parse_pubkey(pubkey, "pubkey")?;
        let dm_id = Uuid::new_v4().to_string();
        let tags = vec![parse_tag(["p", pubkey])?, parse_tag(["d", dm_id.as_str()])?];
        let event =
            self.sign_event(EventBuilder::new(Kind::Custom(KIND_DM_OPEN as u16), "").tags(tags))?;
        let mut response = self.submit_event(&event).await?;
        let relay_dm_id = response
            .get("message")
            .and_then(Value::as_str)
            .and_then(|message| message.strip_prefix("response:"))
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
            .and_then(|value| {
                value
                    .get("channel_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or(dm_id);
        response["dm_id"] = json!(relay_dm_id);
        if response.get("accepted").is_none() {
            response["accepted"] = json!(true);
        }
        Ok(response)
    }

    pub async fn hide_dm(&self, channel_id: &str) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let event = self.sign_event(
            EventBuilder::new(Kind::Custom(KIND_DM_HIDE as u16), "")
                .tags([parse_tag(["h", channel_id.to_string().as_str()])?]),
        )?;
        self.submit_event(&event).await
    }

    pub async fn add_dm_member(
        &self,
        channel_id: &str,
        pubkey: &str,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        parse_pubkey(pubkey, "pubkey")?;
        let event = self.sign_event(
            buzz_sdk::build_dm_add_member(channel_id, pubkey)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn search_channels(&self, query: &str) -> Result<Vec<Channel>, RelayClientError> {
        let needle = query.to_ascii_lowercase();
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_NIP29_GROUP_METADATA],
                "limit": 500,
            })])
            .await?;
        let mut channels = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse_channel_metadata_event)
            .filter(|channel| !channel.archived)
            .filter(|channel| channel.name.to_ascii_lowercase().contains(&needle))
            .collect::<Vec<_>>();
        channels.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(channels)
    }

    pub async fn create_channel(
        &self,
        options: &CreateChannelOptions,
    ) -> Result<Value, RelayClientError> {
        let channel_id = Uuid::new_v4();
        let visibility = match options.visibility.as_str() {
            "open" => Visibility::Open,
            "private" => Visibility::Private,
            other => {
                return Err(RelayClientError::Builder(format!(
                    "invalid visibility {other:?}"
                )))
            }
        };
        let channel_type = match options.channel_type.as_str() {
            "stream" => ChannelKind::Stream,
            "forum" => ChannelKind::Forum,
            other => {
                return Err(RelayClientError::Builder(format!(
                    "invalid channel type {other:?}"
                )))
            }
        };
        let builder = buzz_sdk::build_create_channel(
            channel_id,
            &options.name,
            Some(visibility),
            Some(channel_type),
            non_empty(&options.description),
            options.ttl,
        )
        .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let event = self.sign_event(builder)?;
        let mut response = self.submit_event(&event).await?;
        response["channel_id"] = json!(channel_id.to_string());
        Ok(response)
    }

    pub async fn join_channel(&self, channel_id: &str) -> Result<Value, RelayClientError> {
        self.sign_and_submit_channel_builder(channel_id, buzz_sdk::build_join)
            .await
    }

    pub async fn leave_channel(&self, channel_id: &str) -> Result<Value, RelayClientError> {
        self.sign_and_submit_channel_builder(channel_id, buzz_sdk::build_leave)
            .await
    }

    pub async fn archive_channel(&self, channel_id: &str) -> Result<Value, RelayClientError> {
        self.sign_and_submit_channel_builder(channel_id, buzz_sdk::build_archive)
            .await
    }

    pub async fn unarchive_channel(&self, channel_id: &str) -> Result<Value, RelayClientError> {
        self.sign_and_submit_channel_builder(channel_id, buzz_sdk::build_unarchive)
            .await
    }

    pub async fn delete_channel(&self, channel_id: &str) -> Result<Value, RelayClientError> {
        self.sign_and_submit_channel_builder(channel_id, buzz_sdk::build_delete_channel)
            .await
    }

    pub async fn update_channel_name(
        &self,
        channel_id: &str,
        name: &str,
    ) -> Result<Value, RelayClientError> {
        self.update_channel(channel_id, Some(name), None, None)
            .await
    }

    pub async fn update_channel_description(
        &self,
        channel_id: &str,
        description: &str,
    ) -> Result<Value, RelayClientError> {
        self.update_channel(channel_id, None, Some(description), None)
            .await
    }

    pub async fn set_channel_topic(
        &self,
        channel_id: &str,
        topic: &str,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let event = self.sign_event(
            buzz_sdk::build_set_topic(channel_id, topic)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn set_channel_purpose(
        &self,
        channel_id: &str,
        purpose: &str,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let event = self.sign_event(
            buzz_sdk::build_set_purpose(channel_id, purpose)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn add_channel_member(
        &self,
        channel_id: &str,
        pubkey: &str,
        role: Option<&str>,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let role = match role {
            None => None,
            Some("owner") => Some(MemberRole::Owner),
            Some("admin") => Some(MemberRole::Admin),
            Some("member") => Some(MemberRole::Member),
            Some("guest") => Some(MemberRole::Guest),
            Some("bot") => Some(MemberRole::Bot),
            Some(other) => {
                return Err(RelayClientError::Builder(format!("invalid role {other:?}")))
            }
        };
        let event = self.sign_event(
            buzz_sdk::build_add_member(channel_id, pubkey, role)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn remove_channel_member(
        &self,
        channel_id: &str,
        pubkey: &str,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let event = self.sign_event(
            buzz_sdk::build_remove_member(channel_id, pubkey)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn set_channel_add_policy(&self, policy: &str) -> Result<Value, RelayClientError> {
        match policy {
            "anyone" | "owner_only" | "nobody" => {}
            _ => {
                return Err(RelayClientError::Builder(format!(
                    "policy must be anyone, owner_only, or nobody (got {policy:?})"
                )))
            }
        }
        let content = json!({ "channel_add_policy": policy }).to_string();
        let event = self.sign_event(EventBuilder::new(
            Kind::Custom(KIND_AGENT_PROFILE as u16),
            content,
        ))?;
        self.submit_event(&event).await
    }

    pub async fn get_canvas(&self, channel_id: &str) -> Result<CanvasDocument, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_CANVAS],
                "#h": [channel_id],
                "limit": 1,
            })])
            .await?;
        Ok(CanvasDocument {
            channel_id: channel_id.to_string(),
            content: value
                .as_array()
                .into_iter()
                .flatten()
                .next()
                .and_then(|event| event.get("content"))
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    pub async fn set_canvas(
        &self,
        channel_id: &str,
        content: &str,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let event = self.sign_event(
            buzz_sdk::build_set_canvas(channel_id, content)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    pub async fn read_state(&self) -> Result<ReadState, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_READ_STATE],
                "authors": [self.public_key_hex()],
                "#t": ["read-state"],
                "limit": 500,
            })])
            .await?;
        let mut contexts: BTreeMap<String, u64> = BTreeMap::new();
        for event in value.as_array().into_iter().flatten() {
            let Some(content) = event.get("content").and_then(Value::as_str) else {
                continue;
            };
            let Ok(plaintext) =
                nip44::decrypt(self.keys.secret_key(), &self.keys.public_key(), content)
            else {
                continue;
            };
            let Ok(blob) = serde_json::from_str::<ReadStateBlob>(&plaintext) else {
                continue;
            };
            if blob.v != 1 || blob.client_id.is_empty() {
                continue;
            }
            merge_read_state_contexts(&mut contexts, blob.contexts);
        }
        Ok(ReadState { contexts })
    }

    pub async fn read_state_slot_created_at(&self, slot_id: &str) -> Result<u64, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_READ_STATE],
                "authors": [self.public_key_hex()],
                "#d": [format!("read-state:{slot_id}")],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|event| event.get("created_at").and_then(Value::as_u64))
            .max()
            .unwrap_or_default())
    }

    pub async fn fetch_reminders(&self) -> Result<Vec<Reminder>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_EVENT_REMINDER],
                "authors": [self.public_key_hex()],
                "limit": 200,
            })])
            .await?;
        let mut reminders: Vec<Reminder> = parse_nostr_events(&value)
            .into_iter()
            .filter_map(|event| self.decrypt_reminder(&event))
            .collect();
        reminders
            .sort_by_key(|reminder| (reminder.not_before.unwrap_or(u64::MAX), reminder.id.clone()));
        Ok(reminders)
    }

    pub fn build_reminder_event(
        &self,
        id: &str,
        content: ReminderContent,
        not_before: Option<u64>,
        expiration: Option<u64>,
        created_at: Option<u64>,
    ) -> Result<Event, RelayClientError> {
        let plaintext = serde_json::to_string(&content)?;
        let ciphertext = nip44::encrypt(
            self.keys.secret_key(),
            &self.keys.public_key(),
            plaintext,
            Version::V2,
        )
        .map_err(|error| RelayClientError::Signing(error.to_string()))?;
        let mut tags =
            vec![Tag::parse(["d", id])
                .map_err(|error| RelayClientError::Signing(error.to_string()))?];
        if let Some(not_before) = not_before {
            tags.push(
                Tag::parse(["not_before", &not_before.to_string()])
                    .map_err(|error| RelayClientError::Signing(error.to_string()))?,
            );
        }
        if let Some(expiration) = expiration {
            tags.push(
                Tag::parse(["expiration", &expiration.to_string()])
                    .map_err(|error| RelayClientError::Signing(error.to_string()))?,
            );
        }
        let mut builder =
            EventBuilder::new(Kind::Custom(KIND_EVENT_REMINDER as u16), ciphertext).tags(tags);
        if let Some(created_at) = created_at {
            builder = builder.custom_created_at(Timestamp::from(created_at));
        }
        self.sign_event(builder)
    }

    pub async fn create_reminder(
        &self,
        target: ReminderTarget,
        not_before: u64,
        note: Option<String>,
    ) -> Result<Reminder, RelayClientError> {
        let id = random_reminder_id();
        let content = ReminderContent {
            target: Some(target),
            note: note.filter(|note| !note.trim().is_empty()),
            status: ReminderStatus::Pending,
        };
        let event =
            self.build_reminder_event(&id, content.clone(), Some(not_before), None, None)?;
        self.submit_event(&event).await?;
        Ok(Reminder {
            id,
            not_before: Some(not_before),
            content,
            created_at: event.created_at.as_secs(),
            event_id: event.id.to_hex(),
        })
    }

    pub async fn complete_reminder(
        &self,
        reminder: &Reminder,
    ) -> Result<Reminder, RelayClientError> {
        self.terminal_reminder(reminder, ReminderStatus::Done).await
    }

    pub async fn cancel_reminder(&self, reminder: &Reminder) -> Result<Reminder, RelayClientError> {
        self.terminal_reminder(reminder, ReminderStatus::Cancelled)
            .await
    }

    pub async fn snooze_reminder(
        &self,
        reminder: &Reminder,
        not_before: u64,
    ) -> Result<Reminder, RelayClientError> {
        let mut content = reminder.content.clone();
        content.status = ReminderStatus::Pending;
        let created_at = monotonic_reminder_created_at(reminder.created_at);
        let event = self.build_reminder_event(
            &reminder.id,
            content.clone(),
            Some(not_before),
            None,
            Some(created_at),
        )?;
        self.submit_event(&event).await?;
        Ok(Reminder {
            id: reminder.id.clone(),
            not_before: Some(not_before),
            content,
            created_at: event.created_at.as_secs(),
            event_id: event.id.to_hex(),
        })
    }

    async fn terminal_reminder(
        &self,
        reminder: &Reminder,
        status: ReminderStatus,
    ) -> Result<Reminder, RelayClientError> {
        let mut content = reminder.content.clone();
        content.status = status;
        let created_at = monotonic_reminder_created_at(reminder.created_at);
        let event = self.build_reminder_event(
            &reminder.id,
            content.clone(),
            None,
            Some(jittered_reminder_expiration()),
            Some(created_at),
        )?;
        self.submit_event(&event).await?;
        Ok(Reminder {
            id: reminder.id.clone(),
            not_before: None,
            content,
            created_at: event.created_at.as_secs(),
            event_id: event.id.to_hex(),
        })
    }

    fn decrypt_reminder(&self, event: &Event) -> Option<Reminder> {
        let id = find_tag_value(event.tags.iter(), "d")?;
        let plaintext = nip44::decrypt(
            self.keys.secret_key(),
            &self.keys.public_key(),
            event.content.as_str(),
        )
        .ok()?;
        let content = parse_reminder_content(&plaintext)?;
        Some(Reminder {
            id,
            not_before: find_tag_value(event.tags.iter(), "not_before")
                .as_deref()
                .and_then(parse_not_before),
            content,
            created_at: event.created_at.as_secs(),
            event_id: event.id.to_hex(),
        })
    }

    pub async fn channel_preference_ids(
        &self,
        kind: ChannelPreferenceKind,
    ) -> Result<BTreeSet<String>, RelayClientError> {
        let store_kind = match kind {
            ChannelPreferenceKind::Stars => ChannelPreferenceStoreKind::Stars,
            ChannelPreferenceKind::Mutes => ChannelPreferenceStoreKind::Mutes,
        };
        let (entries, _) = self.fetch_channel_preference_blob(store_kind).await?;
        Ok(entries
            .into_iter()
            .filter_map(|(channel_id, entry)| entry.enabled.then_some(channel_id))
            .collect())
    }

    async fn fetch_channel_preference_blob(
        &self,
        store_kind: ChannelPreferenceStoreKind,
    ) -> Result<(BTreeMap<String, ChannelPreferenceEntry>, u64), RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_READ_STATE],
                "authors": [self.public_key_hex()],
                "#d": [store_kind.d_tag()],
                "limit": 1,
            })])
            .await?;
        let Some(event) = value.as_array().into_iter().flatten().next() else {
            return Ok((BTreeMap::new(), 0));
        };
        let created_at = event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let Some(content) = event.get("content").and_then(Value::as_str) else {
            return Ok((BTreeMap::new(), created_at));
        };
        // Desktop encrypts these private preferences to self. Plaintext is
        // accepted only to migrate blobs written by older TUI releases.
        let plaintext = nip44::decrypt(self.keys.secret_key(), &self.keys.public_key(), content)
            .unwrap_or_else(|_| content.to_string());
        let parsed: Value = serde_json::from_str(&plaintext)?;
        let entries = parsed
            .get("channels")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|channels| channels.iter())
            .map(|(channel_id, entry)| {
                let enabled = entry
                    .get(store_kind.field_name())
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let updated_at = entry
                    .get("updatedAt")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                (
                    channel_id.clone(),
                    ChannelPreferenceEntry {
                        enabled,
                        updated_at,
                    },
                )
            })
            .collect();
        Ok((entries, created_at))
    }

    pub async fn set_channel_preference(
        &self,
        store_kind: ChannelPreferenceStoreKind,
        channel_id: &str,
        enabled: bool,
    ) -> Result<(), RelayClientError> {
        let (mut entries, remote_created_at) =
            self.fetch_channel_preference_blob(store_kind).await?;
        let updated_at = monotonic_reminder_created_at(
            entries
                .get(channel_id)
                .map(|entry| entry.updated_at)
                .unwrap_or_default(),
        );
        entries.insert(
            channel_id.to_string(),
            ChannelPreferenceEntry {
                enabled,
                updated_at,
            },
        );
        let event = self.build_channel_preference_event(
            store_kind,
            entries,
            Some(monotonic_reminder_created_at(remote_created_at)),
        )?;
        self.submit_event(&event).await?;
        Ok(())
    }

    pub async fn channel_sections(&self) -> Result<ChannelSections, RelayClientError> {
        let (store, _) = self.fetch_channel_section_store().await?;
        Ok(ChannelSections {
            version: store.version,
            sections: store
                .sections
                .into_iter()
                .map(|section| ChannelSection {
                    id: section.id,
                    name: section.name,
                    order: section.order,
                })
                .collect(),
            assignments: store.assignments,
        })
    }

    pub async fn create_channel_section(
        &self,
        name: &str,
    ) -> Result<ChannelSection, RelayClientError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RelayClientError::Builder(
                "section name cannot be empty".to_string(),
            ));
        }
        let (mut store, remote_created_at) = self.fetch_channel_section_store().await?;
        let next_order = store
            .sections
            .iter()
            .map(|section| section.order)
            .max()
            .unwrap_or(-1)
            + 1;
        let section = ChannelSectionRecord {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            order: next_order,
        };
        store.sections.push(section.clone());
        self.publish_channel_section_store(&store, remote_created_at)
            .await?;
        Ok(ChannelSection {
            id: section.id,
            name: section.name,
            order: section.order,
        })
    }

    pub async fn assign_channel_section(
        &self,
        channel_id: &str,
        section_id: &str,
    ) -> Result<Value, RelayClientError> {
        parse_uuid(channel_id, "channel id")?;
        parse_uuid(section_id, "section id")?;
        let (mut store, remote_created_at) = self.fetch_channel_section_store().await?;
        if !store
            .sections
            .iter()
            .any(|section| section.id == section_id)
        {
            return Err(RelayClientError::Builder(format!(
                "section {section_id} was not found"
            )));
        }
        store
            .assignments
            .insert(channel_id.to_string(), section_id.to_string());
        self.publish_channel_section_store(&store, remote_created_at)
            .await
    }

    pub async fn unassign_channel_section(
        &self,
        channel_id: &str,
    ) -> Result<Value, RelayClientError> {
        parse_uuid(channel_id, "channel id")?;
        let (mut store, remote_created_at) = self.fetch_channel_section_store().await?;
        store.assignments.remove(channel_id);
        self.publish_channel_section_store(&store, remote_created_at)
            .await
    }

    async fn fetch_channel_section_store(
        &self,
    ) -> Result<(ChannelSectionStore, u64), RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_READ_STATE],
                "authors": [self.public_key_hex()],
                "#d": [CHANNEL_SECTIONS_D_TAG],
                "limit": 1,
            })])
            .await?;
        let Some(content) = value
            .as_array()
            .into_iter()
            .flatten()
            .next()
            .and_then(|event| event.get("content"))
            .and_then(Value::as_str)
        else {
            return Ok((
                ChannelSectionStore {
                    version: 1,
                    ..ChannelSectionStore::default()
                },
                0,
            ));
        };
        let created_at = value
            .as_array()
            .into_iter()
            .flatten()
            .next()
            .and_then(|event| event.get("created_at"))
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let plaintext = nip44::decrypt(self.keys.secret_key(), &self.keys.public_key(), content)
            .unwrap_or_else(|_| content.to_string());
        let store = serde_json::from_str::<ChannelSectionStore>(&plaintext)?;
        Ok((store, created_at))
    }

    async fn publish_channel_section_store(
        &self,
        store: &ChannelSectionStore,
        remote_created_at: u64,
    ) -> Result<Value, RelayClientError> {
        let event = self
            .build_channel_section_event(store, monotonic_reminder_created_at(remote_created_at))?;
        self.submit_event(&event).await
    }

    fn build_channel_section_event(
        &self,
        store: &ChannelSectionStore,
        created_at: u64,
    ) -> Result<Event, RelayClientError> {
        let plaintext = channel_sections_payload(store).to_string();
        let content = nip44::encrypt(
            self.keys.secret_key(),
            &self.keys.public_key(),
            plaintext,
            Version::V2,
        )
        .map_err(|error| RelayClientError::Signing(error.to_string()))?;
        let tags =
            channel_section_tags().map_err(|error| RelayClientError::Builder(error.to_string()))?;
        self.sign_event(
            EventBuilder::new(Kind::Custom(KIND_READ_STATE as u16), content)
                .tags(tags)
                .custom_created_at(Timestamp::from(created_at)),
        )
    }

    /// Fetch and decrypt NIP-AM agent turn metrics (kind 44200) addressed to
    /// this key as the agent owner, newest first. Undecryptable or invalid
    /// payloads are skipped.
    pub async fn agent_turn_metrics(
        &self,
        limit: u32,
    ) -> Result<Vec<AgentTurnMetric>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [buzz_core::kind::KIND_AGENT_TURN_METRIC],
                "#p": [self.public_key_hex()],
                "limit": limit.min(200),
            })])
            .await?;
        let mut metrics: Vec<AgentTurnMetric> = parse_nostr_events(&value)
            .into_iter()
            .filter_map(|event| {
                let payload =
                    buzz_core::agent_turn_metric::decrypt_agent_turn_metric(&self.keys, &event)
                        .ok()?;
                let agent_pubkey = find_tag_value(event.tags.iter(), "agent")
                    .unwrap_or_else(|| event.pubkey.to_hex());
                let stop_reason = payload
                    .stop_reason
                    .as_ref()
                    .and_then(|reason| serde_json::to_value(reason).ok())
                    .and_then(|value| value.as_str().map(str::to_string));
                let turn = payload.turn;
                Some(AgentTurnMetric {
                    agent_pubkey,
                    created_at: event.created_at.as_secs(),
                    harness: payload.harness,
                    model: payload.model,
                    stop_reason,
                    input_tokens: turn.as_ref().and_then(|counts| counts.input_tokens),
                    output_tokens: turn.as_ref().and_then(|counts| counts.output_tokens),
                    total_tokens: turn.as_ref().and_then(|counts| counts.total_tokens),
                    cost_usd: turn.as_ref().and_then(|counts| counts.cost_usd),
                })
            })
            .collect();
        metrics.sort_by_key(|metric| std::cmp::Reverse(metric.created_at));
        Ok(metrics)
    }

    pub async fn list_notes_with(
        &self,
        options: &ListNotesOptions,
    ) -> Result<Vec<Note>, RelayClientError> {
        let limit = if options.limit == 0 {
            50
        } else {
            options.limit.min(200)
        };
        let mut filter = json!({
            "kinds": [KIND_LONG_FORM],
            "limit": limit,
        });
        match &options.author {
            NoteAuthor::Me => filter["authors"] = json!([self.public_key_hex()]),
            NoteAuthor::All => {}
        }
        if let Some(tag) = options.tag.as_deref().and_then(non_empty) {
            filter["#t"] = json!([tag]);
        }
        let value = self.query_values(&[filter]).await?;
        let mut notes = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse_note_event)
            .collect::<Vec<_>>();
        notes.sort_by_key(|note| std::cmp::Reverse(note.updated_at));
        Ok(notes)
    }

    pub async fn set_note(&self, options: &LongFormNoteOptions) -> Result<Value, RelayClientError> {
        let slug = normalize_note_slug(&options.name)?;
        let mut tags = vec![parse_tag(["d", slug.as_str()])?];
        if let Some(title) = non_empty(&options.title) {
            tags.push(parse_tag(["title", title])?);
        }
        if let Some(summary) = non_empty(&options.summary) {
            tags.push(parse_tag(["summary", summary])?);
        }
        for tag in &options.tags {
            if let Some(tag) = non_empty(tag) {
                tags.push(parse_tag(["t", tag])?);
            }
        }
        let event = self.sign_event(
            EventBuilder::new(Kind::Custom(KIND_LONG_FORM as u16), &options.content).tags(tags),
        )?;
        self.submit_event(&event).await
    }

    pub async fn delete_note(&self, slug: &str) -> Result<Value, RelayClientError> {
        let slug = normalize_note_slug(slug)?;
        let coord = format!("{}:{}:{slug}", KIND_LONG_FORM, self.public_key_hex());
        let event = self.sign_event(
            EventBuilder::new(Kind::EventDeletion, "").tags([parse_tag(["a", coord.as_str()])?]),
        )?;
        self.submit_event(&event).await
    }

    pub async fn workspace_emoji(&self) -> Result<Vec<CustomEmojiEntry>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_EMOJI_SET],
                "#d": [buzz_sdk::CUSTOM_EMOJI_SET_D_TAG],
            })])
            .await?;
        Ok(union_custom_emoji(value.as_array().into_iter().flatten()))
    }

    pub async fn own_emoji(&self) -> Result<Vec<CustomEmojiEntry>, RelayClientError> {
        self.fetch_own_emoji_entries().await
    }

    pub async fn set_emoji(&self, shortcode: &str, url: &str) -> Result<Value, RelayClientError> {
        let normalized = buzz_sdk::normalize_custom_emoji_shortcode(shortcode)
            .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let mut emojis = self.fetch_own_emoji().await?;
        emojis.retain(|emoji| emoji.shortcode != normalized);
        emojis.push(CustomEmoji {
            shortcode: normalized,
            url: url.to_string(),
        });
        self.publish_own_emoji_set(&emojis).await
    }

    pub async fn remove_emoji(&self, shortcode: &str) -> Result<Value, RelayClientError> {
        let normalized = buzz_sdk::normalize_custom_emoji_shortcode(shortcode)
            .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let mut emojis = self.fetch_own_emoji().await?;
        let before = emojis.len();
        emojis.retain(|emoji| emoji.shortcode != normalized);
        if emojis.len() == before {
            return Ok(json!({"accepted": true, "message": "not present"}));
        }
        self.publish_own_emoji_set(&emojis).await
    }

    pub async fn import_emoji(&self, file: &str, replace: bool) -> Result<Value, RelayClientError> {
        let raw = std::fs::read_to_string(file)?;
        let parsed: Value = serde_json::from_str(&raw)?;
        let items = parsed
            .get("emojis")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                RelayClientError::Builder("emoji import needs an emojis array".into())
            })?;
        let mut incoming = Vec::with_capacity(items.len());
        for item in items {
            let shortcode = item
                .get("shortcode")
                .and_then(Value::as_str)
                .ok_or_else(|| RelayClientError::Builder("emoji entry missing shortcode".into()))?;
            let url = item
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| RelayClientError::Builder("emoji entry missing url".into()))?;
            incoming.push(CustomEmoji {
                shortcode: buzz_sdk::normalize_custom_emoji_shortcode(shortcode)
                    .map_err(|error| RelayClientError::Builder(error.to_string()))?,
                url: url.to_string(),
            });
        }
        let mut seen = BTreeSet::new();
        incoming.retain(|emoji| seen.insert(emoji.shortcode.clone()));
        let final_set = if replace {
            incoming
        } else {
            let mut existing = self.fetch_own_emoji().await?;
            let existing_shortcodes = existing
                .iter()
                .map(|emoji| emoji.shortcode.clone())
                .collect::<BTreeSet<_>>();
            existing.extend(
                incoming
                    .into_iter()
                    .filter(|emoji| !existing_shortcodes.contains(&emoji.shortcode)),
            );
            existing
        };
        self.publish_own_emoji_set(&final_set).await
    }

    pub async fn export_emoji_json(&self) -> Result<String, RelayClientError> {
        let mut emojis = self.workspace_emoji().await?;
        emojis.sort_by(|a, b| a.shortcode.cmp(&b.shortcode).then(a.url.cmp(&b.url)));
        Ok(serde_json::to_string(&json!({ "emojis": emojis }))?)
    }

    pub async fn list_memory(
        &self,
        agent_pubkey: &str,
    ) -> Result<Vec<MemoryEntry>, RelayClientError> {
        let agent = PublicKey::from_hex(agent_pubkey)
            .map_err(|error| RelayClientError::Builder(format!("agent pubkey: {error}")))?;
        let owner = self.public_key();
        let their_pubkey = agent;
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_AGENT_ENGRAM],
                "authors": [agent.to_hex()],
                "#p": [owner.to_hex()],
                "limit": 5000,
            })])
            .await?;
        let events = parse_nostr_events(&value);
        let mut groups = BTreeMap::<String, Vec<(Event, Body)>>::new();
        for event in events {
            if event.verify().is_err() {
                continue;
            }
            let Some(d_value) = event
                .tags
                .iter()
                .find(|tag| tag.as_slice().first().map(String::as_str) == Some("d"))
                .and_then(|tag| tag.as_slice().get(1).cloned())
            else {
                continue;
            };
            if let Ok(body) = validate_and_decrypt(
                &event,
                &agent,
                &owner,
                self.keys.secret_key(),
                &their_pubkey,
            ) {
                groups.entry(d_value).or_default().push((event, body));
            }
        }
        let mut entries = Vec::new();
        for (_, members) in groups {
            let head = select_head(members.iter().map(|(event, _)| event.clone()));
            let Some(head) = head else {
                continue;
            };
            let Some((_, body)) = members.into_iter().find(|(event, _)| event.id == head.id) else {
                continue;
            };
            match body {
                Body::Core { .. } | Body::Memory { value: None, .. } => {}
                Body::Memory { slug, value } => entries.push(MemoryEntry {
                    slug,
                    event_id: head.id.to_hex(),
                    created_at: head.created_at.as_secs(),
                    value: value.unwrap_or_default(),
                }),
            }
        }
        entries.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(entries)
    }

    pub async fn get_memory(
        &self,
        agent_pubkey: &str,
        slug: &str,
    ) -> Result<String, RelayClientError> {
        let agent = PublicKey::from_hex(agent_pubkey)
            .map_err(|error| RelayClientError::Builder(format!("agent pubkey: {error}")))?;
        let owner = self.public_key();
        let (_, body) = self.fetch_memory_head(&agent, &owner, slug).await?;
        match body {
            Some(Body::Memory {
                value: Some(value), ..
            }) => Ok(value),
            Some(Body::Core { profile }) => Ok(profile),
            Some(Body::Memory { value: None, .. }) | None => Err(RelayClientError::Builder(
                format!("memory not found: {slug}"),
            )),
        }
    }

    pub async fn memory_hash(
        &self,
        agent_pubkey: &str,
        slug: &str,
    ) -> Result<String, RelayClientError> {
        let value = self.get_memory(agent_pubkey, slug).await?;
        Ok(sha256_hex(&value))
    }

    pub async fn patch_memory(
        &self,
        slug: &str,
        patch_text: &str,
        base_hash: &str,
        allow_empty: bool,
    ) -> Result<String, RelayClientError> {
        let owner = self.auth_tag_owner().unwrap_or_else(|| self.public_key());
        let slug = normalize_slug(slug)
            .map_err(|error| RelayClientError::Builder(format!("invalid slug: {error}")))?;
        if base_hash.len() != 64 || !base_hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(RelayClientError::Builder(
                "base hash must be a 64-character sha256 hex digest".to_string(),
            ));
        }
        if patch_text.trim().is_empty() {
            return Err(RelayClientError::Builder("patch is empty".to_string()));
        }
        let file_header_count = patch_text
            .lines()
            .filter(|line| line.starts_with("--- "))
            .count();
        if file_header_count > 1 {
            return Err(RelayClientError::Builder(format!(
                "multi-file patch not supported (found {file_header_count} file headers)"
            )));
        }

        let agent = self.public_key();
        let (head, body) = self.fetch_memory_head(&agent, &owner, &slug).await?;
        let current = match body {
            Some(Body::Memory {
                value: Some(value), ..
            }) => value,
            Some(Body::Core { profile }) => profile,
            Some(Body::Memory { value: None, .. }) | None => {
                return Err(RelayClientError::Builder(format!(
                    "memory not found: {slug}"
                )))
            }
        };
        let actual_hash = sha256_hex(&current);
        if actual_hash != base_hash.to_ascii_lowercase() {
            return Err(RelayClientError::Builder(format!(
                "memory changed since patch base (expected {base_hash}, got {actual_hash})"
            )));
        }

        let patch = diffy::Patch::from_str(patch_text)
            .map_err(|error| RelayClientError::Builder(format!("malformed patch: {error}")))?;
        let new_value = diffy::apply(&current, &patch).map_err(|error| {
            RelayClientError::Builder(format!("patch did not apply cleanly: {error}"))
        })?;
        if new_value.is_empty() && !allow_empty {
            return Err(RelayClientError::Builder(
                "refusing to write empty memory value".to_string(),
            ));
        }
        let body = if slug == engram::CORE_SLUG {
            Body::Core {
                profile: new_value.clone(),
            }
        } else {
            Body::Memory {
                slug: slug.clone(),
                value: Some(new_value.clone()),
            }
        };
        let created_at = engram::monotonic_created_at(
            Timestamp::now().as_secs(),
            head.map(|event| event.created_at.as_secs()),
        );
        let event = engram::build_event(&self.keys, &owner, &body, created_at)
            .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let response = self.submit_event(&event).await?;
        let accepted = response
            .get("accepted")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !accepted {
            return Err(RelayClientError::Builder(format!(
                "relay rejected memory patch: {message}"
            )));
        }
        if message == "duplicate" || message.starts_with("duplicate:") {
            return Err(RelayClientError::Builder(
                "memory patch was dominated by a newer head".to_string(),
            ));
        }
        Ok(sha256_hex(&new_value))
    }

    pub async fn set_memory(&self, slug: &str, value: &str) -> Result<(), RelayClientError> {
        let owner = self.auth_tag_owner().unwrap_or_else(|| self.public_key());
        let slug = normalize_slug(slug)
            .map_err(|error| RelayClientError::Builder(format!("invalid slug: {error}")))?;
        let body = if slug == engram::CORE_SLUG {
            Body::Core {
                profile: value.to_string(),
            }
        } else {
            Body::Memory {
                slug: slug.clone(),
                value: Some(value.to_string()),
            }
        };
        self.write_memory_body(&owner, &slug, body).await
    }

    pub async fn remove_memory(&self, slug: &str) -> Result<(), RelayClientError> {
        let owner = self.auth_tag_owner().unwrap_or_else(|| self.public_key());
        let slug = normalize_slug(slug)
            .map_err(|error| RelayClientError::Builder(format!("invalid slug: {error}")))?;
        if slug == engram::CORE_SLUG {
            return Err(RelayClientError::Builder(
                "core cannot be tombstoned".to_string(),
            ));
        }
        self.write_memory_body(
            &owner,
            &slug,
            Body::Memory {
                slug: slug.clone(),
                value: None,
            },
        )
        .await
    }

    pub fn build_read_state_event(
        &self,
        client_id: &str,
        slot_id: &str,
        contexts: BTreeMap<String, u64>,
        created_at: Option<u64>,
    ) -> Result<Event, RelayClientError> {
        let plaintext = serde_json::to_string(&read_state_payload(client_id, contexts))?;
        let ciphertext = nip44::encrypt(
            self.keys.secret_key(),
            &self.keys.public_key(),
            plaintext,
            Version::V2,
        )
        .map_err(|error| RelayClientError::Signing(error.to_string()))?;
        let mut builder = EventBuilder::new(Kind::Custom(KIND_READ_STATE as u16), ciphertext).tags(
            read_state_tags(slot_id)
                .map_err(|error| RelayClientError::Signing(error.to_string()))?,
        );
        if let Some(created_at) = created_at {
            builder = builder.custom_created_at(Timestamp::from(created_at));
        }
        self.sign_event(builder)
    }

    pub fn build_channel_preference_event(
        &self,
        kind: ChannelPreferenceStoreKind,
        entries: BTreeMap<String, ChannelPreferenceEntry>,
        created_at: Option<u64>,
    ) -> Result<Event, RelayClientError> {
        let plaintext = channel_preference_payload(kind, &entries).to_string();
        let content = nip44::encrypt(
            self.keys.secret_key(),
            &self.keys.public_key(),
            plaintext,
            Version::V2,
        )
        .map_err(|error| RelayClientError::Signing(error.to_string()))?;
        let mut builder = EventBuilder::new(Kind::Custom(KIND_READ_STATE as u16), content).tags(
            channel_preference_tags(kind)
                .map_err(|error| RelayClientError::Signing(error.to_string()))?,
        );
        if let Some(created_at) = created_at {
            builder = builder.custom_created_at(Timestamp::from(created_at));
        }
        self.sign_event(builder)
    }

    pub fn normalize_message_event(event: &Event) -> TuiMessageView {
        TuiMessageView {
            id: event.id.to_hex(),
            pubkey: event.pubkey.to_hex(),
            kind: u64::from(event.kind.as_u16()),
            content: event.content.clone(),
            created_at: event.created_at.as_secs(),
            channel_id: event
                .tags
                .iter()
                .find_map(|tag| {
                    let parts = tag.as_slice();
                    (parts.first().map(String::as_str) == Some("h"))
                        .then(|| parts.get(1).cloned())
                        .flatten()
                })
                .unwrap_or_default(),
            thread_root_id: find_root_from_event_tags(event.tags.iter()),
            edited_at: None,
            edit_event_id: None,
        }
    }

    pub fn normalize_message_value(value: &Value) -> Option<TuiMessageView> {
        let id = value.get("id")?.as_str()?.to_string();
        let pubkey = value.get("pubkey")?.as_str()?.to_string();
        let kind = value.get("kind")?.as_u64()?;
        let content = value
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let created_at = value
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let channel_id = value
            .get("channel_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| h_tag(value))
            .unwrap_or_default();

        Some(TuiMessageView {
            id,
            pubkey,
            kind,
            content,
            created_at,
            channel_id,
            thread_root_id: find_root_from_tags(value.get("tags")).map(|id| id.to_hex()),
            edited_at: None,
            edit_event_id: None,
        })
    }

    pub async fn query_messages(
        &self,
        filters: &[Value],
    ) -> Result<Vec<TuiMessageView>, RelayClientError> {
        let response = self.query_values(filters).await?;
        let mut messages: Vec<TuiMessageView> = response
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Self::normalize_message_value)
            .filter(|message| !message.id.is_empty() || !message.content.is_empty())
            .collect();
        messages.sort_by_key(|message| (message.created_at, message.id.clone()));
        let mut seen = BTreeSet::new();
        messages.retain(|message| message.id.is_empty() || seen.insert(message.id.clone()));
        self.apply_structural_aux(messages).await
    }

    async fn apply_structural_aux(
        &self,
        mut messages: Vec<TuiMessageView>,
    ) -> Result<Vec<TuiMessageView>, RelayClientError> {
        let message_ids = messages
            .iter()
            .map(|message| message.id.clone())
            .filter(|id| !id.is_empty())
            .collect::<Vec<_>>();
        if message_ids.is_empty() {
            return Ok(messages);
        }
        let aux = self
            .query_values(&[json!({
                "kinds": [
                    buzz_core::kind::KIND_DELETION,
                    buzz_core::kind::KIND_NIP29_DELETE_EVENT,
                    KIND_STREAM_MESSAGE_EDIT,
                ],
                "#e": message_ids,
                "limit": 1000,
            })])
            .await?;
        let mut aux_events = aux.as_array().cloned().unwrap_or_default();
        let edit_ids = aux_events
            .iter()
            .filter(|event| {
                event.get("kind").and_then(Value::as_u64)
                    == Some(u64::from(KIND_STREAM_MESSAGE_EDIT))
            })
            .filter_map(|event| event.get("id").and_then(Value::as_str).map(str::to_string))
            .collect::<Vec<_>>();
        if !edit_ids.is_empty() {
            let deleted_edits = self
                .query_values(&[json!({
                    "kinds": [
                        buzz_core::kind::KIND_DELETION,
                        buzz_core::kind::KIND_NIP29_DELETE_EVENT,
                    ],
                    "#e": edit_ids,
                    "limit": 1000,
                })])
                .await?;
            aux_events.extend(deleted_edits.as_array().cloned().unwrap_or_default());
        }
        apply_structural_aux_events(&mut messages, &aux_events);
        Ok(messages)
    }

    pub fn sign_event(&self, builder: EventBuilder) -> Result<Event, RelayClientError> {
        self.shared.sign_event(builder).map_err(Into::into)
    }

    pub async fn query_values(&self, filters: &[Value]) -> Result<Value, RelayClientError> {
        self.shared
            .query_values(filters)
            .await
            .map(Value::Array)
            .map_err(Into::into)
    }

    pub fn channel_unread_filter(channel_id: &str, read_frontier: u64) -> Value {
        json!({
            "kinds": [
                KIND_STREAM_MESSAGE,
                KIND_STREAM_MESSAGE_V2,
                buzz_core::kind::KIND_STREAM_MESSAGE_DIFF,
                buzz_core::kind::KIND_FORUM_POST,
                buzz_core::kind::KIND_FORUM_COMMENT,
            ],
            "#h": [channel_id],
            "since": read_frontier.saturating_add(1),
            "limit": 100,
        })
    }

    /// Count up to 100 unread timeline rows per channel in one relay query.
    /// The UI renders a saturated result as `99+`.
    pub async fn channel_unread_counts(
        &self,
        channel_frontiers: Vec<(String, u64)>,
        read_frontiers: &BTreeMap<String, u64>,
    ) -> Result<BTreeMap<String, u64>, RelayClientError> {
        if channel_frontiers.is_empty() {
            return Ok(BTreeMap::new());
        }
        let filters = channel_frontiers
            .iter()
            .map(|(channel_id, frontier)| Self::channel_unread_filter(channel_id, *frontier))
            .collect::<Vec<_>>();
        let channel_frontier_by_id = channel_frontiers
            .iter()
            .cloned()
            .collect::<BTreeMap<_, _>>();
        let mut counts = channel_frontiers
            .into_iter()
            .map(|(channel_id, _)| (channel_id, 0u64))
            .collect::<BTreeMap<_, _>>();
        // Count the same normalized, structurally filtered rows the timeline
        // can render. Raw query values can include deleted messages and other
        // rows removed by the timeline's auxiliary-event pass.
        let messages = self.query_messages(&filters).await?;
        let own_pubkey = self.public_key_hex();
        for message in &messages {
            if message.channel_id.is_empty() {
                continue;
            }
            let channel_id = &message.channel_id;
            let channel_frontier = channel_frontier_by_id
                .get(channel_id)
                .copied()
                .unwrap_or_default();
            if !message_is_unread(message, channel_frontier, read_frontiers, &own_pubkey) {
                continue;
            }
            if let Some(count) = counts.get_mut(channel_id) {
                *count = count.saturating_add(1);
            }
        }
        Ok(counts)
    }

    pub async fn relay_information(&self) -> Result<RelayInformation, RelayClientError> {
        let cache = RELAY_INFORMATION_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
        if let Some(info) = cache
            .lock()
            .expect("relay information cache poisoned")
            .get(self.shared.relay_http_url())
            .cloned()
        {
            return Ok(info);
        }

        let info = self.fetch_relay_information().await?;
        cache
            .lock()
            .expect("relay information cache poisoned")
            .insert(self.shared.relay_http_url().to_string(), info.clone());
        Ok(info)
    }

    async fn fetch_relay_information(&self) -> Result<RelayInformation, RelayClientError> {
        let body = self.shared.get_public("/").await?;
        Ok(serde_json::from_str(&body)?)
    }

    async fn resolve_event_channel_id(&self, event_id: &str) -> Result<Uuid, RelayClientError> {
        let value = self
            .query_values(&[Self::event_id_filter(event_id)])
            .await?
            .as_array()
            .and_then(|events| events.first().cloned())
            .ok_or_else(|| RelayClientError::Builder(format!("event {event_id} not found")))?;
        let channel_id = value
            .get("channel_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| h_tag(&value))
            .ok_or_else(|| {
                RelayClientError::Builder(format!("event {event_id} has no channel id"))
            })?;
        Uuid::parse_str(&channel_id)
            .map_err(|error| RelayClientError::Builder(format!("channel id: {error}")))
    }

    async fn resolve_reply_pair(
        &self,
        reply_to: Option<&str>,
    ) -> Result<Option<(EventId, EventId)>, RelayClientError> {
        let Some(parent_id) = reply_to else {
            return Ok(None);
        };
        let parent = parse_event_id(parent_id, "reply id")?;
        let value = self
            .query_values(&[Self::event_id_filter(parent_id)])
            .await?
            .as_array()
            .and_then(|events| events.first().cloned())
            .ok_or_else(|| {
                RelayClientError::Builder(format!("parent event {parent_id} not found"))
            })?;
        let root = find_root_from_tags(value.get("tags")).unwrap_or(parent);
        Ok(Some((root, parent)))
    }

    async fn update_channel(
        &self,
        channel_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        visibility: Option<&str>,
    ) -> Result<Value, RelayClientError> {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let event = self.sign_event(
            buzz_sdk::build_update_channel(channel_id, name, description, visibility, None)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    async fn sign_and_submit_channel_builder<F>(
        &self,
        channel_id: &str,
        build: F,
    ) -> Result<Value, RelayClientError>
    where
        F: FnOnce(Uuid) -> Result<EventBuilder, buzz_sdk::SdkError>,
    {
        let channel_id = parse_uuid(channel_id, "channel id")?;
        let event = self.sign_event(
            build(channel_id).map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    async fn fetch_own_emoji(&self) -> Result<Vec<CustomEmoji>, RelayClientError> {
        Ok(self
            .fetch_own_emoji_entries()
            .await?
            .into_iter()
            .map(|emoji| CustomEmoji {
                shortcode: emoji.shortcode,
                url: emoji.url,
            })
            .collect())
    }

    async fn fetch_own_emoji_entries(&self) -> Result<Vec<CustomEmojiEntry>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_EMOJI_SET],
                "#d": [buzz_sdk::CUSTOM_EMOJI_SET_D_TAG],
                "authors": [self.public_key_hex()],
                "limit": 1,
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .last()
            .map(emoji_tags_of)
            .unwrap_or_default())
    }

    async fn publish_own_emoji_set(
        &self,
        emojis: &[CustomEmoji],
    ) -> Result<Value, RelayClientError> {
        let event = self.sign_event(
            buzz_sdk::build_custom_emoji_set(emojis)
                .map_err(|error| RelayClientError::Builder(error.to_string()))?,
        )?;
        self.submit_event(&event).await
    }

    fn auth_tag_owner(&self) -> Option<PublicKey> {
        let owner = self.shared.auth_tag()?.as_slice().get(1)?;
        PublicKey::from_hex(owner).ok()
    }

    async fn fetch_memory_head(
        &self,
        agent: &PublicKey,
        owner: &PublicKey,
        slug: &str,
    ) -> Result<(Option<Event>, Option<Body>), RelayClientError> {
        let slug = normalize_slug(slug)
            .map_err(|error| RelayClientError::Builder(format!("invalid slug: {error}")))?;
        let their_pubkey = if self.public_key() == *agent {
            owner
        } else {
            agent
        };
        let conversation_key = conversation_key(self.keys.secret_key(), their_pubkey);
        let d = engram_d_tag(&conversation_key, &slug);
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_AGENT_ENGRAM],
                "authors": [agent.to_hex()],
                "#d": [d],
                "#p": [owner.to_hex()],
                "limit": 16,
            })])
            .await?;
        let mut valid = Vec::new();
        for event in parse_nostr_events(&value) {
            if event.verify().is_err() {
                continue;
            }
            if let Ok(body) =
                validate_and_decrypt(&event, agent, owner, self.keys.secret_key(), their_pubkey)
            {
                valid.push((event, body));
            }
        }
        if valid.is_empty() {
            return Ok((None, None));
        }
        let Some(head) = select_head(valid.iter().map(|(event, _)| event.clone())) else {
            return Ok((None, None));
        };
        let body = valid
            .into_iter()
            .find(|(event, _)| event.id == head.id)
            .map(|(_, body)| body);
        Ok((Some(head), body))
    }

    async fn write_memory_body(
        &self,
        owner: &PublicKey,
        slug: &str,
        body: Body,
    ) -> Result<(), RelayClientError> {
        let agent = self.public_key();
        let (head, _) = self.fetch_memory_head(&agent, owner, slug).await?;
        let created_at = engram::monotonic_created_at(
            Timestamp::now().as_secs(),
            head.map(|event| event.created_at.as_secs()),
        );
        let event = engram::build_event(&self.keys, owner, &body, created_at)
            .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let response = self.submit_event(&event).await?;
        let accepted = response
            .get("accepted")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !accepted {
            return Err(RelayClientError::Builder(format!(
                "relay rejected memory event: {message}"
            )));
        }
        if message == "duplicate" || message.starts_with("duplicate:") {
            return Err(RelayClientError::Builder(
                "memory write was dominated by a newer head".to_string(),
            ));
        }
        Ok(())
    }

    async fn find_own_reaction(
        &self,
        event_id: &str,
        emoji: &str,
    ) -> Result<String, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_REACTION],
                "#e": [event_id],
                "authors": [self.public_key_hex()],
            })])
            .await?;
        value
            .as_array()
            .into_iter()
            .flatten()
            .find(|event| event.get("content").and_then(Value::as_str) == Some(emoji))
            .and_then(|event| event.get("id").and_then(Value::as_str))
            .map(ToString::to_string)
            .ok_or_else(|| {
                RelayClientError::Builder(format!(
                    "no reaction {emoji:?} found for event {event_id}"
                ))
            })
    }

    pub async fn submit_event(&self, event: &Event) -> Result<Value, RelayClientError> {
        let body = serde_json::to_vec(event)?;
        self.validate_event_size(body.len()).await?;
        let response = self.shared.submit_event(event.clone()).await?;
        Ok(serde_json::to_value(response)?)
    }

    async fn validate_event_size(&self, actual: usize) -> Result<(), RelayClientError> {
        let Ok(info) = self.relay_information().await else {
            return Ok(());
        };
        validate_event_size_against_limit(actual, info.limitation.max_message_length)
    }

    pub async fn upload_bytes(
        &self,
        bytes: Vec<u8>,
        mime_type: &str,
    ) -> Result<UploadedFile, RelayClientError> {
        let upload = self.shared.upload_bytes(bytes, mime_type).await?;
        Ok(UploadedFile {
            url: upload.url,
            sha256: upload.sha256,
            size: upload.size,
            mime_type: upload.mime_type,
            uploaded: upload.uploaded,
            dim: upload.dim,
            blurhash: upload.blurhash,
            thumb: upload.thumb,
        })
    }

    pub async fn upload_file(&self, path: &str) -> Result<UploadedFile, RelayClientError> {
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() {
            return Err(RelayClientError::Builder(format!("{path} is not a file")));
        }
        let bytes = std::fs::read(path)?;
        let mime_type = infer::get(&bytes)
            .map(|kind| kind.mime_type().to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        if !ALLOWED_UPLOAD_MIMES.contains(&mime_type.as_str()) {
            return Err(RelayClientError::Builder(format!(
                "unsupported file type: {mime_type}"
            )));
        }
        let max_bytes = if mime_type.starts_with("video/") {
            MAX_VIDEO_BYTES
        } else {
            MAX_IMAGE_BYTES
        };
        if bytes.len() as u64 > max_bytes {
            return Err(RelayClientError::Builder(format!(
                "file too large: {} bytes (max {max_bytes})",
                bytes.len()
            )));
        }
        self.upload_bytes(bytes, &mime_type).await
    }

    pub async fn subscribe_live(
        &self,
        subscription_id: &str,
        filters: Vec<Filter>,
    ) -> Result<TuiRelaySubscription, RelayClientError> {
        let subscription = self.shared.subscribe(subscription_id, &filters).await?;
        Ok(TuiRelaySubscription { subscription })
    }
}

pub struct TuiRelaySubscription {
    subscription: RelaySubscription,
}

impl TuiRelaySubscription {
    pub async fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Result<RelayMessage, RelayClientError> {
        self.subscription
            .next_event(timeout)
            .await
            .map_err(Into::into)
    }

    pub async fn close(self, _subscription_id: &str) -> Result<(), RelayClientError> {
        self.subscription.cancel().await.map_err(Into::into)
    }
}

pub fn relay_http_to_ws_url(relay_url: &str) -> String {
    buzz_client::normalize_relay_ws_url(relay_url)
        .unwrap_or_else(|_| relay_url.trim().trim_end_matches('/').to_string())
}

fn format_error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        let detail = error.to_string();
        if !message.contains(&detail) {
            message.push_str(": ");
            message.push_str(&detail);
        }
        source = error.source();
    }
    message
}

pub fn clamp_history_page_limit(limit: u32) -> u32 {
    let limit = if limit == 0 {
        HISTORY_PAGE_LIMIT
    } else {
        limit
    };
    limit.min(MAX_HISTORY_PAGE_LIMIT)
}

pub fn validate_event_size_against_limit(
    actual: usize,
    max_message_length: Option<u64>,
) -> Result<(), RelayClientError> {
    let Some(max) = max_message_length else {
        return Ok(());
    };
    if actual as u64 > max {
        return Err(RelayClientError::EventTooLarge { actual, max });
    }
    Ok(())
}

fn h_tag(value: &Value) -> Option<String> {
    tag_value(value, "h")
}

fn message_is_unread(
    message: &TuiMessageView,
    channel_frontier: u64,
    read_frontiers: &BTreeMap<String, u64>,
    own_pubkey: &str,
) -> bool {
    if message.pubkey.eq_ignore_ascii_case(own_pubkey)
        || !is_conversational_unread_kind(message.kind)
    {
        return false;
    }
    let mut frontier = channel_frontier;
    if let Some(root_id) = message
        .thread_root_id
        .as_deref()
        .filter(|root_id| !root_id.is_empty())
    {
        frontier = frontier.max(
            read_frontiers
                .get(&thread_context_key(root_id))
                .copied()
                .unwrap_or_default(),
        );
    }
    if !message.id.is_empty() {
        frontier = frontier.max(
            read_frontiers
                .get(&msg_context_key(&message.id))
                .copied()
                .unwrap_or_default(),
        );
    }
    message.created_at > frontier
}

fn apply_structural_aux_events(messages: &mut Vec<TuiMessageView>, events: &[Value]) {
    let mut authors = messages
        .iter()
        .map(|message| (message.id.clone(), message.pubkey.clone()))
        .collect::<BTreeMap<_, _>>();
    for event in events {
        if event.get("kind").and_then(Value::as_u64) == Some(u64::from(KIND_STREAM_MESSAGE_EDIT)) {
            if let (Some(id), Some(pubkey)) = (
                event.get("id").and_then(Value::as_str),
                event.get("pubkey").and_then(Value::as_str),
            ) {
                authors.insert(id.to_string(), pubkey.to_string());
            }
        }
    }

    let mut deleted = BTreeSet::new();
    for event in events {
        let kind = event
            .get("kind")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if kind != u64::from(buzz_core::kind::KIND_DELETION)
            && kind != u64::from(buzz_core::kind::KIND_NIP29_DELETE_EVENT)
        {
            continue;
        }
        let deletion_author = event.get("pubkey").and_then(Value::as_str);
        for target in event_tag_targets(event, "e") {
            let authorized = kind == u64::from(buzz_core::kind::KIND_NIP29_DELETE_EVENT)
                || authors
                    .get(&target)
                    .is_some_and(|author| Some(author.as_str()) == deletion_author);
            if authorized {
                deleted.insert(target);
            }
        }
    }

    messages.retain(|message| !deleted.contains(&message.id));
    for message in messages {
        let newest = events
            .iter()
            .filter(|event| {
                event.get("kind").and_then(Value::as_u64)
                    == Some(u64::from(KIND_STREAM_MESSAGE_EDIT))
                    && event.get("pubkey").and_then(Value::as_str) == Some(message.pubkey.as_str())
                    && event_tag_targets(event, "e")
                        .iter()
                        .any(|id| id == &message.id)
                    && event
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !deleted.contains(id))
            })
            .max_by_key(|event| {
                (
                    event
                        .get("created_at")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    event.get("id").and_then(Value::as_str).unwrap_or_default(),
                )
            });
        if let Some(edit) = newest {
            message.content = edit
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            message.edited_at = edit.get("created_at").and_then(Value::as_u64);
            message.edit_event_id = edit.get("id").and_then(Value::as_str).map(str::to_string);
        }
    }
}

/// Partition a flat NIP-CW response by kind: timeline rows, kind-39005 thread
/// summaries (newest per root wins), and exactly one kind-39006 bounds event
/// whose `d` tag must echo the request cursor.
fn parse_channel_window_response(
    value: &Value,
    channel_id: &str,
    request_cursor: Option<&WindowCursor>,
) -> ChannelWindowPage {
    let mut page = ChannelWindowPage::default();
    let mut bounds_seen = 0usize;
    let mut bounds_ok = false;
    let mut summary_created: BTreeMap<String, u64> = BTreeMap::new();

    for event in value.as_array().into_iter().flatten() {
        let kind = event
            .get("kind")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if kind == u64::from(buzz_core::kind::KIND_THREAD_SUMMARY) {
            let Some(root_id) = tag_value(event, "e") else {
                continue;
            };
            let created_at = event
                .get("created_at")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let Some(summary) = event
                .get("content")
                .and_then(Value::as_str)
                .and_then(|content| serde_json::from_str::<ThreadSummary>(content).ok())
            else {
                continue;
            };
            let newest = summary_created.get(&root_id).copied().unwrap_or_default();
            if created_at >= newest {
                summary_created.insert(root_id.clone(), created_at);
                page.summaries.insert(root_id, summary);
            }
            continue;
        }
        if kind == u64::from(buzz_core::kind::KIND_WINDOW_BOUNDS) {
            bounds_seen += 1;
            let expected = match request_cursor {
                Some(cursor) => format!("{channel_id}:{}:{}", cursor.created_at, cursor.id),
                None => format!("{channel_id}:head"),
            };
            if d_tag(event).as_deref() != Some(expected.as_str()) {
                continue;
            }
            let Some(parsed) = event
                .get("content")
                .and_then(Value::as_str)
                .and_then(|content| serde_json::from_str::<Value>(content).ok())
            else {
                continue;
            };
            let has_more = parsed
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or_default();
            let next_cursor = parsed.get("next_cursor").and_then(|cursor| {
                Some(WindowCursor {
                    created_at: cursor.get("created_at").and_then(Value::as_u64)?,
                    id: cursor.get("id").and_then(Value::as_str)?.to_string(),
                })
            });
            if has_more == next_cursor.is_some() {
                page.has_more = has_more;
                page.next_cursor = next_cursor;
                bounds_ok = true;
            }
            continue;
        }
        if let Some(view) = TuiRelayClient::normalize_message_value(event) {
            if !view.id.is_empty() || !view.content.is_empty() {
                page.messages.push(Message::from(view));
            }
        }
    }

    page.messages
        .sort_by_key(|message| (message.created_at, message.id.clone()));
    let mut seen = BTreeSet::new();
    page.messages
        .retain(|message| message.id.is_empty() || seen.insert(message.id.clone()));
    page.valid = bounds_seen == 1 && bounds_ok;
    page
}

fn d_tag(value: &Value) -> Option<String> {
    tag_value(value, "d")
}

fn tag_value(value: &Value, key: &str) -> Option<String> {
    value.get("tags")?.as_array()?.iter().find_map(|tag| {
        let parts = tag.as_array()?;
        (parts.first()?.as_str()? == key)
            .then(|| parts.get(1)?.as_str().map(ToString::to_string))
            .flatten()
    })
}

fn event_tag_targets(value: &Value, key: &str) -> Vec<String> {
    value
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tag| {
            let parts = tag.as_array()?;
            (parts.first()?.as_str()? == key).then(|| parts.get(1)?.as_str().map(str::to_string))?
        })
        .collect()
}

fn has_marker_tag(value: &Value, key: &str) -> bool {
    value
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|tag| {
            tag.as_array()
                .and_then(|parts| parts.first())
                .and_then(Value::as_str)
                == Some(key)
        })
}

fn parse_channel_metadata_event(event: &Value) -> Option<Channel> {
    let id = d_tag(event)?;
    let name = tag_value(event, "name").filter(|name| !name.is_empty())?;
    let visibility = if has_marker_tag(event, "private") {
        "private"
    } else if has_marker_tag(event, "public") {
        "public"
    } else {
        ""
    };
    let channel_type = tag_value(event, "t").unwrap_or_default();
    let kind = if channel_type == "dm" {
        ConversationKind::DirectMessage
    } else {
        ConversationKind::Channel
    };
    Some(Channel {
        id,
        name,
        description: tag_value(event, "about").unwrap_or_default(),
        channel_type,
        participant_pubkeys: event_tag_targets(event, "p"),
        visibility: visibility.to_string(),
        archived: tag_value(event, "archived").as_deref() == Some("true"),
        topic: tag_value(event, "topic").unwrap_or_default(),
        purpose: tag_value(event, "purpose").unwrap_or_default(),
        owner_pubkey: event
            .get("pubkey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        created_at: event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        kind,
    })
}

fn parse_channel_members_event(event: &Value) -> Vec<ChannelMember> {
    event
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tag| {
            let parts = tag.as_array()?;
            (parts.first()?.as_str()? == "p").then(|| ChannelMember {
                pubkey: parts
                    .get(1)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                role: parts
                    .get(3)
                    .and_then(Value::as_str)
                    .filter(|role| !role.is_empty())
                    .unwrap_or("member")
                    .to_string(),
            })
        })
        .collect()
}

fn parse_relay_members_event(event: &Value) -> Vec<RelayMember> {
    let created_at = event
        .get("created_at")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut members = Vec::new();
    for tag in event
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(parts) = tag.as_array() else {
            continue;
        };
        let Some(name) = parts.first().and_then(Value::as_str) else {
            continue;
        };
        if name != "member" && name != "p" {
            continue;
        }
        let Some(pubkey) = parts
            .get(1)
            .and_then(Value::as_str)
            .and_then(|value| normalize_pubkey(value, "pubkey").ok())
        else {
            continue;
        };
        if !seen.insert(pubkey.clone()) {
            continue;
        }
        let role_index = if name == "member" { 2 } else { 3 };
        let role = parts
            .get(role_index)
            .and_then(Value::as_str)
            .filter(|role| matches!(*role, "owner" | "admin" | "member"))
            .unwrap_or("member")
            .to_string();
        members.push(RelayMember {
            pubkey,
            role,
            created_at,
        });
    }
    members.sort_by_key(|member| relay_role_order(&member.role));
    members
}

fn relay_role_order(role: &str) -> u8 {
    match role {
        "owner" => 0,
        "admin" => 1,
        _ => 2,
    }
}

fn group_reactions<'a>(events: impl IntoIterator<Item = &'a Value>) -> Vec<Reaction> {
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    for event in events {
        let emoji = event
            .get("content")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("+")
            .to_string();
        let pubkey = event
            .get("pubkey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        groups.entry(emoji).or_default().push(pubkey);
    }
    groups
        .into_iter()
        .map(|(emoji, pubkeys)| Reaction {
            emoji,
            count: pubkeys.len(),
            pubkeys,
        })
        .collect()
}

fn parse_profile_event(event: &Value) -> Option<UserProfile> {
    let pubkey = event.get("pubkey")?.as_str()?.to_string();
    let content = event.get("content")?.as_str()?;
    let profile = serde_json::from_str::<Value>(content).ok()?;
    Some(UserProfile {
        pubkey,
        display_name: profile
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        name: profile
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        picture: profile
            .get("picture")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        about: profile
            .get("about")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        nip05: profile
            .get("nip05")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn parse_relay_agent_event(event: &Event) -> Option<RelayAgentInfo> {
    let kind = event.kind.as_u16() as u32;
    if !matches!(kind, KIND_AGENT_PROFILE | KIND_MANAGED_AGENT) {
        return None;
    }

    let owner_pubkey = if kind == KIND_MANAGED_AGENT {
        Some(event.pubkey.to_hex())
    } else {
        event.tags.iter().find_map(|tag| {
            (tag.as_slice().first().map(String::as_str) == Some("auth"))
                .then(|| serde_json::to_string(tag).ok())
                .flatten()
                .and_then(|encoded| {
                    buzz_sdk::nip_oa::verify_auth_tag(&encoded, &event.pubkey)
                        .ok()
                        .map(|owner| owner.to_hex())
                })
        })
    };
    let pubkey = if kind == KIND_MANAGED_AGENT {
        event_tag_value(event, "d")?
    } else {
        event.pubkey.to_hex()
    };
    let short_pubkey = pubkey.chars().take(8).collect::<String>();
    let content = serde_json::from_str::<Value>(&event.content).unwrap_or_else(|_| json!({}));
    let object = content.as_object();
    let display_name = object
        .and_then(|object| object.get("display_name"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let name = object
        .and_then(|object| object.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| display_name.map(str::to_string))
        .unwrap_or_else(|| short_pubkey.clone());
    let agent_type = object
        .and_then(|object| object.get("agent_type"))
        .and_then(Value::as_str)
        .unwrap_or(if kind == KIND_MANAGED_AGENT {
            "managed-agent"
        } else {
            "agent"
        })
        .to_string();
    let status = object
        .and_then(|object| object.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("offline")
        .to_string();
    let respond_to = object
        .and_then(|object| object.get("respond_to"))
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "owner-only" | "allowlist" | "anyone" | "nobody"))
        .map(str::to_string);
    let respond_to_allowlist = string_array_field(object, "respond_to_allowlist");
    let string_field = |key: &str| {
        object
            .and_then(|object| object.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    };

    Some(RelayAgentInfo {
        pubkey,
        owner_pubkey,
        name,
        agent_type,
        channels: string_array_field(object, "channels"),
        channel_ids: string_array_field(object, "channel_ids"),
        capabilities: string_array_field(object, "capabilities"),
        status,
        respond_to,
        respond_to_allowlist,
        model: string_field("model"),
        provider: string_field("provider"),
        persona_id: string_field("persona_id"),
        managed_by: string_field("managed_by"),
        management_mode: string_field("management_mode"),
        manager_pubkey: string_field("manager_pubkey"),
        health_status: None,
        health_detail: None,
        health_restarts: None,
        health_updated_at: None,
        created_at: event.created_at.as_secs(),
    })
}

fn apply_beekeeper_health_to_relay_agents(agents: &mut [RelayAgentInfo], events: &Value) {
    let mut latest = BTreeMap::<String, (u64, String, String, Option<u64>)>::new();
    for event in parse_nostr_events(events) {
        if event.kind.as_u16() as u32 != KIND_STREAM_MESSAGE
            || event_tag_value(&event, "t").as_deref() != Some("beekeeper-health")
            || event_tag_value(&event, "managed_by").as_deref() != Some("beekeeper")
        {
            continue;
        }
        let Some(pubkey) = event_tag_value(&event, "agent_pubkey") else {
            continue;
        };
        let signer = event.pubkey.to_hex();
        if !agents.iter().any(|agent| {
            agent.pubkey == pubkey
                && agent.managed_by.as_deref() == Some("beekeeper")
                && agent.management_mode.as_deref() == Some("external")
                && agent.owner_pubkey.as_deref() == Some(signer.as_str())
                && agent.manager_pubkey.as_deref() == Some(signer.as_str())
        }) {
            continue;
        }
        let Some(status) = event_tag_value(&event, "status") else {
            continue;
        };
        if !matches!(
            status.as_str(),
            "healthy" | "unhealthy" | "restarting" | "unobservable"
        ) {
            continue;
        }
        let updated_at = event.created_at.as_secs();
        let restarts =
            event_tag_value(&event, "restarts").and_then(|value| value.parse::<u64>().ok());
        if latest
            .get(&pubkey)
            .is_none_or(|(current, _, _, _)| updated_at >= *current)
        {
            latest.insert(
                pubkey,
                (updated_at, status, event.content.clone(), restarts),
            );
        }
    }
    for agent in agents {
        let Some((updated_at, status, detail, restarts)) = latest.get(&agent.pubkey) else {
            continue;
        };
        agent.health_status = Some(status.clone());
        agent.health_detail = Some(detail.clone());
        agent.health_restarts = *restarts;
        agent.health_updated_at = Some(*updated_at);
    }
}

fn event_tag_value(event: &Event, key: &str) -> Option<String> {
    event.tags.iter().find_map(|tag| {
        let parts = tag.as_slice();
        (parts.first().map(String::as_str) == Some(key))
            .then(|| parts.get(1).cloned())
            .flatten()
    })
}

fn parse_presence_event(event: &Value) -> Option<PresenceInfo> {
    Some(PresenceInfo {
        pubkey: json_event_tag_value(event, "p")
            .or_else(|| event.get("pubkey")?.as_str().map(str::to_string))?,
        status: event
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        updated_at: event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    })
}

fn json_event_tag_value(event: &Value, key: &str) -> Option<String> {
    event
        .get("tags")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|tag| {
            let parts = tag.as_array()?;
            (parts.first().and_then(Value::as_str) == Some(key))
                .then(|| parts.get(1)?.as_str().map(str::to_string))
                .flatten()
        })
}

fn apply_presence_to_relay_agents(agents: &mut [RelayAgentInfo], events: &Value) {
    let mut latest = BTreeMap::<String, PresenceInfo>::new();
    for event in events.as_array().into_iter().flatten() {
        let Some(presence) = parse_presence_event(event) else {
            continue;
        };
        if !matches!(presence.status.as_str(), "online" | "away" | "offline") {
            continue;
        }
        let should_replace = latest
            .get(&presence.pubkey)
            .map(|current| presence.updated_at >= current.updated_at)
            .unwrap_or(true);
        if should_replace {
            latest.insert(presence.pubkey.clone(), presence);
        }
    }

    for agent in agents {
        agent.status = latest
            .get(&agent.pubkey)
            .map(|presence| presence.status.clone())
            .unwrap_or_else(|| "offline".to_string());
    }
}

fn string_array_field(object: Option<&serde_json::Map<String, Value>>, field: &str) -> Vec<String> {
    object
        .and_then(|object| object.get(field))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn parse_contact_list_event(event: &Value) -> Vec<Contact> {
    event
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .filter(|tag| tag.first().and_then(Value::as_str) == Some("p"))
        .filter_map(|tag| {
            Some(Contact {
                pubkey: tag.get(1)?.as_str()?.to_string(),
                relay_url: tag.get(2).and_then(Value::as_str).unwrap_or("").to_string(),
                petname: tag.get(3).and_then(Value::as_str).unwrap_or("").to_string(),
            })
        })
        .collect()
}

fn parse_repo_event(event: &Value) -> RepoProject {
    let dtag = d_tag(event)
        .or_else(|| event.get("id").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    let owner = event
        .get("pubkey")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    RepoProject {
        id: if owner.is_empty() {
            dtag.clone()
        } else {
            format!("{owner}:{dtag}")
        },
        dtag: dtag.clone(),
        name: tag_value(event, "name").unwrap_or_else(|| dtag.clone()),
        description: tag_value(event, "description")
            .or_else(|| {
                event
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default(),
        clone_urls: tag_values(event, "clone"),
        web_url: tag_value(event, "web"),
        owner,
        relays: tag_values(event, "relays"),
        created_at: event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    }
}

fn parse_issue_event(event: &Value, repo_owner: &str, repo_id: &str) -> GitIssue {
    GitIssue {
        id: event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        repo_owner: repo_owner.to_string(),
        repo_id: repo_id.to_string(),
        author: event
            .get("pubkey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        title: tag_value(event, "subject").unwrap_or_else(|| "(untitled issue)".to_string()),
        content: event
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        labels: tag_values(event, "t"),
        created_at: event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    }
}

fn parse_patch_event(event: &Value, repo_owner: &str, repo_id: &str) -> GitPatch {
    GitPatch {
        id: event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        repo_owner: repo_owner.to_string(),
        repo_id: repo_id.to_string(),
        author: event
            .get("pubkey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        content: event
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        commit: tag_value(event, "commit"),
        parent_commit: tag_value(event, "parent-commit"),
        root: tag_values(event, "t").iter().any(|tag| tag == "root"),
        root_revision: tag_values(event, "t")
            .iter()
            .any(|tag| tag == "root-revision"),
        created_at: event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    }
}

fn parse_pull_request(
    root: &Value,
    events: &[Value],
    repo_owner: &str,
    repo_id: &str,
) -> GitPullRequest {
    let id = root.get("id").and_then(Value::as_str).unwrap_or_default();
    let author = root
        .get("pubkey")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let trusted_root_actors = [repo_owner, author];
    let latest_update = events
        .iter()
        .filter(|event| {
            event.get("kind").and_then(Value::as_u64) == Some(u64::from(KIND_GIT_PR_UPDATE))
                && tag_value(event, "E").as_deref() == Some(id)
                && event
                    .get("pubkey")
                    .and_then(Value::as_str)
                    .is_some_and(|pubkey| trusted_root_actors.contains(&pubkey))
        })
        .max_by(|left, right| event_order(left).cmp(&event_order(right)));
    let current = latest_update.unwrap_or(root);
    let latest_status = events
        .iter()
        .filter(|event| {
            matches!(
                event.get("kind").and_then(Value::as_u64),
                Some(kind)
                    if [
                        u64::from(KIND_GIT_STATUS_OPEN),
                        u64::from(KIND_GIT_STATUS_MERGED),
                        u64::from(KIND_GIT_STATUS_CLOSED),
                        u64::from(KIND_GIT_STATUS_DRAFT),
                    ]
                    .contains(&kind)
            ) && (tag_value(event, "e").as_deref() == Some(id)
                || tag_value(event, "E").as_deref() == Some(id))
                && event
                    .get("pubkey")
                    .and_then(Value::as_str)
                    .is_some_and(|pubkey| trusted_root_actors.contains(&pubkey))
        })
        .max_by(|left, right| event_order(left).cmp(&event_order(right)));
    let status = match latest_status
        .and_then(|event| event.get("kind"))
        .and_then(Value::as_u64)
    {
        Some(kind) if kind == u64::from(KIND_GIT_STATUS_MERGED) => "merged",
        Some(kind) if kind == u64::from(KIND_GIT_STATUS_CLOSED) => "closed",
        Some(kind) if kind == u64::from(KIND_GIT_STATUS_DRAFT) => "draft",
        _ if tag_values(root, "t")
            .iter()
            .any(|label| label.eq_ignore_ascii_case("draft")) =>
        {
            "draft"
        }
        _ => "open",
    };
    let commit = tag_value(current, "c").unwrap_or_default();
    let initial_commit = tag_value(root, "c").unwrap_or_default();
    let mut trusted_reviewers = tag_values(root, "p")
        .into_iter()
        .filter(|pubkey| pubkey != author)
        .collect::<BTreeSet<_>>();
    for event in events.iter().filter(|event| {
        event.get("kind").and_then(Value::as_u64) == Some(u64::from(KIND_TEXT_NOTE))
            && tag_value(event, "e").as_deref() == Some(id)
            && tag_values(event, "t")
                .iter()
                .any(|label| label == "review-request")
            && event
                .get("pubkey")
                .and_then(Value::as_str)
                .is_some_and(|pubkey| trusted_root_actors.contains(&pubkey))
    }) {
        trusted_reviewers.extend(tag_values(event, "p"));
    }
    trusted_reviewers.remove(author);
    trusted_reviewers.insert(repo_owner.to_string());
    let mut decisions = BTreeMap::<String, (u64, String, String)>::new();
    for event in events.iter().filter(|event| {
        event.get("kind").and_then(Value::as_u64) == Some(u64::from(KIND_TEXT_NOTE))
            && tag_value(event, "e").as_deref() == Some(id)
            && tag_value(event, "c").unwrap_or_else(|| initial_commit.clone()) == commit
    }) {
        let Some(reviewer) = event.get("pubkey").and_then(Value::as_str) else {
            continue;
        };
        if !trusted_reviewers.contains(reviewer) {
            continue;
        }
        let labels = tag_values(event, "t");
        let decision = if labels.iter().any(|label| label == "approval") {
            Some("approval")
        } else if labels.iter().any(|label| label == "changes-requested") {
            Some("changes-requested")
        } else {
            None
        };
        let Some(decision) = decision else {
            continue;
        };
        let created_at = event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let event_id = event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let should_replace = decisions
            .get(reviewer)
            .is_none_or(|(existing_at, existing_id, _)| {
                (created_at, event_id.as_str()) >= (*existing_at, existing_id.as_str())
            });
        if should_replace {
            decisions.insert(
                reviewer.to_string(),
                (created_at, event_id, decision.to_string()),
            );
        }
    }
    let created_at = root
        .get("created_at")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let review_created_at = decisions
        .values()
        .map(|(created_at, _, _)| *created_at)
        .max();
    let status_created_at = latest_status
        .and_then(|event| event.get("created_at"))
        .and_then(Value::as_u64);
    let updated_at = latest_update
        .and_then(|event| event.get("created_at"))
        .and_then(Value::as_u64)
        .into_iter()
        .chain(status_created_at)
        .chain(review_created_at)
        .max()
        .unwrap_or(created_at);
    GitPullRequest {
        id: id.to_string(),
        repo_owner: repo_owner.to_string(),
        repo_id: repo_id.to_string(),
        author: author.to_string(),
        title: tag_value(root, "subject").unwrap_or_else(|| {
            root.get("content")
                .and_then(Value::as_str)
                .and_then(|content| content.lines().find(|line| !line.trim().is_empty()))
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .unwrap_or("Untitled pull request")
                .to_string()
        }),
        content: root
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        commit,
        branch_name: tag_value(root, "branch-name"),
        target_branch: tag_value(root, "target-branch"),
        clone_urls: tag_values(current, "clone"),
        status: status.to_string(),
        status_created_at,
        approval_count: decisions
            .values()
            .filter(|(_, _, decision)| decision == "approval")
            .count(),
        change_request_count: decisions
            .values()
            .filter(|(_, _, decision)| decision == "changes-requested")
            .count(),
        review_created_at,
        created_at,
        updated_at,
    }
}

fn event_order(event: &Value) -> (u64, &str) {
    (
        event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        event.get("id").and_then(Value::as_str).unwrap_or_default(),
    )
}

fn workflow_webhook_secret(response: &Value) -> Option<String> {
    response
        .get("message")
        .and_then(Value::as_str)
        .and_then(|message| message.strip_prefix("response:"))
        .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
        .and_then(|payload| {
            payload
                .get("webhook_secret")
                .and_then(Value::as_str)
                .filter(|secret| !secret.is_empty())
                .map(str::to_string)
        })
}

fn parse_workflow_event(event: &Value) -> Workflow {
    Workflow {
        workflow_id: d_tag(event).unwrap_or_default(),
        content: event
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        created_at: event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        pubkey: event
            .get("pubkey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

fn parse_workflow_detail_event(event: &Value) -> WorkflowDetail {
    WorkflowDetail {
        workflow_id: d_tag(event).unwrap_or_default(),
        content: event
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        created_at: event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        pubkey: event
            .get("pubkey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

fn parse_workflow_run_event(event: &Value) -> WorkflowRun {
    let kind = event
        .get("kind")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let content = event
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let content_json = serde_json::from_str::<Value>(&content).ok();
    let status = match u32::try_from(kind).unwrap_or_default() {
        KIND_WORKFLOW_TRIGGERED => "running",
        KIND_WORKFLOW_STEP_STARTED => "step running",
        KIND_WORKFLOW_STEP_COMPLETED => "step completed",
        KIND_WORKFLOW_STEP_FAILED => "step failed",
        KIND_WORKFLOW_COMPLETED => "completed",
        KIND_WORKFLOW_FAILED => "failed",
        KIND_WORKFLOW_CANCELLED => "cancelled",
        _ => "event",
    }
    .to_string();
    WorkflowRun {
        event_id: event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        kind,
        run_id: tag_value(event, "run")
            .or_else(|| tag_value(event, "run_id"))
            .unwrap_or_default(),
        step_id: tag_value(event, "step").or_else(|| tag_value(event, "step_id")),
        status,
        output: content_json
            .as_ref()
            .and_then(|value| value.get("output"))
            .cloned(),
        error: content_json
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(Value::as_str)
            .map(str::to_string),
        content,
        created_at: event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    }
}

fn parse_note_event(event: &Value) -> Option<Note> {
    let slug = d_tag(event)?;
    let pubkey = event.get("pubkey")?.as_str()?.to_string();
    let kind = event
        .get("kind")
        .and_then(Value::as_u64)
        .unwrap_or(KIND_LONG_FORM as u64);
    let coordinate = format!("{kind}:{pubkey}:{slug}");
    let naddr = PublicKey::from_hex(&pubkey)
        .ok()
        .and_then(|public_key| {
            Coordinate {
                kind: Kind::Custom(kind as u16),
                public_key,
                identifier: slug.clone(),
            }
            .to_bech32()
            .ok()
        })
        .unwrap_or_default();
    Some(Note {
        id: event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        pubkey,
        naddr,
        coordinate,
        slug,
        title: tag_value(event, "title").unwrap_or_default(),
        summary: tag_value(event, "summary"),
        tags: tag_values(event, "t"),
        published_at: tag_value(event, "published_at").and_then(|value| value.parse().ok()),
        updated_at: event
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        content: event
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn emoji_tags_of(event: &Value) -> Vec<CustomEmojiEntry> {
    event
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .filter(|parts| parts.first().and_then(Value::as_str) == Some("emoji"))
        .filter_map(|parts| {
            Some(CustomEmojiEntry {
                shortcode: parts.get(1)?.as_str()?.to_string(),
                url: parts.get(2)?.as_str()?.to_string(),
            })
        })
        .collect()
}

fn union_custom_emoji<'a>(events: impl IntoIterator<Item = &'a Value>) -> Vec<CustomEmojiEntry> {
    let mut by_shortcode = BTreeMap::<String, (String, i64)>::new();
    for event in events {
        let created_at = event
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        for entry in emoji_tags_of(event) {
            match by_shortcode.get(&entry.shortcode) {
                Some((url, at)) if *at > created_at || (*at == created_at && *url <= entry.url) => {
                }
                _ => {
                    by_shortcode.insert(entry.shortcode, (entry.url, created_at));
                }
            }
        }
    }
    by_shortcode
        .into_iter()
        .map(|(shortcode, (url, _))| CustomEmojiEntry { shortcode, url })
        .collect()
}

fn parse_nostr_events(value: &Value) -> Vec<Event> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| serde_json::from_value::<Event>(event.clone()).ok())
        .collect()
}

pub fn parse_not_before(raw: &str) -> Option<u64> {
    if raw.is_empty() {
        return None;
    }
    if raw.len() > 1 && raw.starts_with('0') {
        return None;
    }
    raw.chars()
        .all(|ch| ch.is_ascii_digit())
        .then(|| raw.parse::<u64>().ok())
        .flatten()
}

pub fn parse_reminder_content(plaintext: &str) -> Option<ReminderContent> {
    let value = serde_json::from_str::<Value>(plaintext).ok()?;
    let object = value.as_object()?;
    let status = match object.get("status")?.as_str()? {
        "pending" => ReminderStatus::Pending,
        "done" => ReminderStatus::Done,
        "cancelled" => ReminderStatus::Cancelled,
        _ => return None,
    };
    let note = match object.get("note") {
        Some(Value::String(note)) => Some(note.clone()),
        Some(_) => return None,
        None => None,
    };
    let target = match object.get("target") {
        Some(target) => Some(parse_reminder_target(target)?),
        None => None,
    };
    if target.is_none() && note.as_deref().is_none_or(str::is_empty) {
        return None;
    }
    Some(ReminderContent {
        target,
        note,
        status,
    })
}

fn parse_reminder_target(value: &Value) -> Option<ReminderTarget> {
    let object = value.as_object()?;
    Some(ReminderTarget {
        event_id: object.get("eventId")?.as_str()?.to_string(),
        channel_id: object.get("channelId")?.as_str()?.to_string(),
        preview: object.get("preview")?.as_str()?.to_string(),
        author_pubkey: object.get("authorPubkey")?.as_str()?.to_string(),
    })
}

pub fn count_due_reminders(reminders: &[Reminder], now: u64) -> usize {
    reminders
        .iter()
        .filter(|reminder| reminder_is_due(reminder, now))
        .count()
}

fn reminder_is_due(reminder: &Reminder, now: u64) -> bool {
    reminder.content.status == ReminderStatus::Pending
        && reminder
            .not_before
            .is_some_and(|not_before| not_before <= now)
}

pub fn group_reminders(reminders: &[Reminder], now: u64) -> Vec<ReminderGroup> {
    let end_of_today = local_end_of_today(now);
    let mut overdue = Vec::new();
    let mut today = Vec::new();
    let mut upcoming = Vec::new();

    for reminder in reminders {
        if reminder.content.status != ReminderStatus::Pending {
            continue;
        }
        let Some(not_before) = reminder.not_before else {
            continue;
        };
        if not_before <= now {
            overdue.push(reminder.clone());
        } else if not_before <= end_of_today {
            today.push(reminder.clone());
        } else {
            upcoming.push(reminder.clone());
        }
    }

    let mut groups = Vec::new();
    if !overdue.is_empty() {
        groups.push(ReminderGroup {
            label: "Overdue",
            reminders: overdue,
        });
    }
    if !today.is_empty() {
        groups.push(ReminderGroup {
            label: "Today",
            reminders: today,
        });
    }
    if !upcoming.is_empty() {
        groups.push(ReminderGroup {
            label: "Upcoming",
            reminders: upcoming,
        });
    }
    groups
}

fn local_end_of_today(now: u64) -> u64 {
    let Some(now_local) = Local.timestamp_opt(now as i64, 0).single() else {
        return now;
    };
    now_local
        .date_naive()
        .and_hms_opt(23, 59, 59)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .map(|local| local.timestamp().max(0) as u64)
        .unwrap_or(now)
}

fn find_tag_value<'a>(tags: impl IntoIterator<Item = &'a Tag>, key: &str) -> Option<String> {
    tags.into_iter().find_map(|tag| {
        let parts = tag.as_slice();
        (parts.first().map(String::as_str) == Some(key))
            .then(|| parts.get(1).cloned())
            .flatten()
    })
}

fn random_reminder_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    hex::encode(bytes)
}

fn jittered_reminder_expiration() -> u64 {
    let days = 30 + rand::rng().random_range(0..60);
    Timestamp::now().as_secs() + days * 86_400
}

fn monotonic_reminder_created_at(previous: u64) -> u64 {
    Timestamp::now().as_secs().max(previous.saturating_add(1))
}

fn merge_read_state_contexts(merged: &mut BTreeMap<String, u64>, incoming: BTreeMap<String, u64>) {
    for (context, timestamp) in incoming {
        merged
            .entry(context)
            .and_modify(|current| *current = (*current).max(timestamp))
            .or_insert(timestamp);
    }
}

fn find_root_from_tags(tags: Option<&Value>) -> Option<EventId> {
    let mut root = None;
    let mut reply = None;
    for tag in tags?.as_array()? {
        let parts = tag.as_array()?;
        if parts.first().and_then(Value::as_str) != Some("e") {
            continue;
        }
        let id = parts.get(1).and_then(Value::as_str)?;
        let id = EventId::from_hex(id).ok()?;
        match parts.get(3).and_then(Value::as_str) {
            Some("root") => root = Some(id),
            Some("reply") => reply = Some(id),
            _ => {}
        }
    }
    root.or(reply)
}

fn find_root_from_event_tags<'a>(tags: impl IntoIterator<Item = &'a Tag>) -> Option<String> {
    let mut root = None;
    let mut reply = None;
    for tag in tags {
        let parts = tag.as_slice();
        if parts.first().map(String::as_str) != Some("e") {
            continue;
        }
        let Some(id) = parts.get(1) else {
            continue;
        };
        match parts.get(3).map(String::as_str) {
            Some("root") => root = Some(id.clone()),
            Some("reply") => reply = Some(id.clone()),
            _ => {}
        }
    }
    root.or(reply)
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

fn repo_coordinate(repo_owner: &str, repo_id: &str) -> String {
    format!("{}:{repo_owner}:{repo_id}", KIND_GIT_REPO_ANNOUNCEMENT)
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RelayClientError> {
    Uuid::parse_str(value).map_err(|error| RelayClientError::Builder(format!("{field}: {error}")))
}

fn parse_event_id(value: &str, field: &str) -> Result<EventId, RelayClientError> {
    EventId::from_hex(value).map_err(|error| RelayClientError::Builder(format!("{field}: {error}")))
}

fn parse_pubkey(value: &str, field: &str) -> Result<PublicKey, RelayClientError> {
    PublicKey::from_hex(value)
        .map_err(|error| RelayClientError::Builder(format!("{field}: {error}")))
}

fn normalize_pubkey(value: &str, field: &str) -> Result<String, RelayClientError> {
    let trimmed = value.trim().to_ascii_lowercase();
    if trimmed.starts_with("npub1") {
        PublicKey::from_bech32(&trimmed)
            .map(|pubkey| pubkey.to_hex())
            .map_err(|error| RelayClientError::Builder(format!("{field}: {error}")))
    } else {
        PublicKey::from_hex(&trimmed)
            .map(|pubkey| pubkey.to_hex())
            .map_err(|error| RelayClientError::Builder(format!("{field}: {error}")))
    }
}

fn normalize_relay_member_role(role: &str) -> Result<&str, RelayClientError> {
    match role.trim().to_ascii_lowercase().as_str() {
        "admin" => Ok("admin"),
        "member" | "" => Ok("member"),
        other => Err(RelayClientError::Builder(format!(
            "relay role must be admin or member (got {other:?})"
        ))),
    }
}

fn parse_tag<const N: usize>(parts: [&str; N]) -> Result<Tag, RelayClientError> {
    Tag::parse(parts).map_err(|error| RelayClientError::Signing(error.to_string()))
}

fn tag_values(value: &Value, tag_name: &str) -> Vec<String> {
    value
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .filter(|tag| tag.first().and_then(Value::as_str) == Some(tag_name))
        .flat_map(|tag| tag.iter().skip(1))
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn normalize_note_slug(value: &str) -> Result<String, RelayClientError> {
    let slug = value.trim();
    if slug.is_empty() {
        return Err(RelayClientError::Builder("note name is empty".to_string()));
    }
    Ok(slug.to_string())
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

const ALLOWED_UPLOAD_MIMES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "video/mp4",
];
const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_VIDEO_BYTES: u64 = 500 * 1024 * 1024;

fn build_imeta_tag(file: &UploadedFile) -> Vec<String> {
    let mut tag = vec![
        "imeta".to_string(),
        format!("url {}", file.url),
        format!("m {}", file.mime_type),
        format!("x {}", file.sha256),
        format!("size {}", file.size),
    ];
    if let Some(dim) = &file.dim {
        tag.push(format!("dim {dim}"));
    }
    if let Some(blurhash) = &file.blurhash {
        tag.push(format!("blurhash {blurhash}"));
    }
    if let Some(thumb) = &file.thumb {
        tag.push(format!("thumb {thumb}"));
    }
    tag
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW_CHANNEL: &str = "9ba26a41-91b9-4c57-83a9-08afd46330d2";

    fn window_response(bounds_d: &str, bounds_content: &str) -> Value {
        json!([
            {
                "id": "root-1",
                "kind": 9,
                "pubkey": "author-1",
                "content": "top level",
                "created_at": 100,
                "tags": [["h", WINDOW_CHANNEL]],
            },
            {
                "kind": 39005,
                "pubkey": "relay",
                "created_at": 101,
                "content": "{\"reply_count\":2,\"descendant_count\":3,\"last_reply_at\":100}",
                "tags": [["e", "root-1"], ["d", "root-1"], ["h", WINDOW_CHANNEL]],
            },
            {
                "kind": 39006,
                "pubkey": "relay",
                "created_at": 101,
                "content": bounds_content,
                "tags": [["d", bounds_d], ["h", WINDOW_CHANNEL]],
            },
        ])
    }

    #[test]
    fn parses_channel_window_rows_summaries_and_bounds() {
        let value = window_response(
            &format!("{WINDOW_CHANNEL}:head"),
            "{\"has_more\":true,\"next_cursor\":{\"created_at\":90,\"id\":\"older-id\"}}",
        );

        let page = parse_channel_window_response(&value, WINDOW_CHANNEL, None);

        assert!(page.valid);
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].id, "root-1");
        let summary = page.summaries.get("root-1").expect("summary for root");
        assert_eq!(summary.reply_count, 2);
        assert_eq!(summary.descendant_count, 3);
        assert!(page.has_more);
        assert_eq!(
            page.next_cursor,
            Some(WindowCursor {
                created_at: 90,
                id: "older-id".to_string(),
            })
        );
    }

    #[test]
    fn channel_window_without_valid_bounds_is_downgrade_signal() {
        // Wrong d-tag (doesn't echo the request cursor) → structural failure.
        let value = window_response(
            "some-other-channel:head",
            "{\"has_more\":false,\"next_cursor\":null}",
        );
        let page = parse_channel_window_response(&value, WINDOW_CHANNEL, None);
        assert!(!page.valid);

        // Inconsistent has_more/next_cursor → structural failure.
        let value = window_response(
            &format!("{WINDOW_CHANNEL}:head"),
            "{\"has_more\":true,\"next_cursor\":null}",
        );
        let page = parse_channel_window_response(&value, WINDOW_CHANNEL, None);
        assert!(!page.valid);
    }

    #[test]
    fn channel_window_continuation_bounds_echo_composite_cursor() {
        let cursor = WindowCursor {
            created_at: 90,
            id: "older-id".to_string(),
        };
        let value = window_response(
            &format!("{WINDOW_CHANNEL}:90:older-id"),
            "{\"has_more\":false,\"next_cursor\":null}",
        );

        let page = parse_channel_window_response(&value, WINDOW_CHANNEL, Some(&cursor));

        assert!(page.valid);
        assert!(!page.has_more);
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn channel_window_filter_uses_composite_cursor_and_extensions() {
        let cursor = WindowCursor {
            created_at: 90,
            id: "older-id".to_string(),
        };
        let filter = TuiRelayClient::channel_window_filter(WINDOW_CHANNEL, 50, Some(&cursor));
        assert_eq!(filter["top_level"], json!(true));
        assert_eq!(filter["include_summaries"], json!(true));
        assert_eq!(filter["until"], json!(90));
        assert_eq!(filter["before_id"], json!("older-id"));
        assert_eq!(filter["#h"], json!([WINDOW_CHANNEL]));

        let head = TuiRelayClient::channel_window_filter(WINDOW_CHANNEL, 50, None);
        assert!(head.get("until").is_none());
        assert!(head.get("before_id").is_none());
    }

    fn client() -> TuiRelayClient {
        let keys = Keys::generate();
        let owner = Keys::generate();
        let auth_tag =
            buzz_sdk::nip_oa::compute_auth_tag(&owner, &keys.public_key(), "kind=9").unwrap();
        TuiRelayClient::new(
            "http://localhost:3000",
            &keys.secret_key().to_bech32().unwrap(),
            Some(auth_tag),
        )
        .unwrap()
    }

    fn agent_profile_event(content: &str) -> Event {
        client()
            .sign_event(EventBuilder::new(
                Kind::Custom(KIND_AGENT_PROFILE as u16),
                content,
            ))
            .unwrap()
    }

    fn managed_agent_event(d_tag: &str, content: &str) -> Event {
        client()
            .sign_event(
                EventBuilder::new(Kind::Custom(KIND_MANAGED_AGENT as u16), content)
                    .tags([Tag::parse(["d", d_tag]).unwrap()]),
            )
            .unwrap()
    }

    fn reminder_content() -> ReminderContent {
        ReminderContent {
            target: Some(ReminderTarget {
                event_id: "event-1".to_string(),
                channel_id: "channel-1".to_string(),
                preview: "hello".to_string(),
                author_pubkey: "author-1".to_string(),
            }),
            note: Some("note".to_string()),
            status: ReminderStatus::Pending,
        }
    }

    fn reminder(id: &str, not_before: Option<u64>, status: ReminderStatus) -> Reminder {
        let mut content = reminder_content();
        content.status = status;
        Reminder {
            id: id.to_string(),
            not_before,
            content,
            created_at: 10,
            event_id: format!("event-{id}"),
        }
    }

    #[test]
    fn parses_nip11_max_message_length() {
        let info: RelayInformation =
            serde_json::from_str(r#"{"name":"relay","limitation":{"max_message_length":1048576}}"#)
                .unwrap();

        assert_eq!(info.limitation.max_message_length, Some(1_048_576));
    }

    #[test]
    fn missing_nip11_message_limit_allows_publish_size() {
        assert!(validate_event_size_against_limit(10_000, None).is_ok());
    }

    #[test]
    fn oversized_event_is_rejected_locally() {
        let error = validate_event_size_against_limit(101, Some(100)).unwrap_err();

        assert!(matches!(
            error,
            RelayClientError::EventTooLarge {
                actual: 101,
                max: 100
            }
        ));
    }

    #[test]
    fn history_page_filters_clamp_limit_and_include_until() {
        let channel_filter =
            TuiRelayClient::channel_history_page_filter("channel-1", 5000, Some(42));
        assert_eq!(channel_filter["limit"], json!(2000));
        assert_eq!(channel_filter["until"], json!(42));
        let kinds = channel_filter["kinds"].as_array().unwrap();
        assert!(kinds.contains(&json!(buzz_core::kind::KIND_FORUM_POST)));
        assert!(kinds.contains(&json!(buzz_core::kind::KIND_FORUM_COMMENT)));
        assert!(!kinds.contains(&json!(KIND_STREAM_MESSAGE_EDIT)));
        assert_eq!(
            TuiRelayClient::thread_page_filter("channel-1", "event-1", 0, Some(7))["limit"],
            json!(500)
        );
    }

    #[test]
    fn relay_agents_filter_queries_agent_profile_and_nip_ap_managed_agents() {
        assert_eq!(
            TuiRelayClient::relay_agents_filter(900),
            json!({
                "kinds": [KIND_AGENT_PROFILE, KIND_MANAGED_AGENT],
                "limit": 500,
            })
        );
    }

    #[test]
    fn parse_relay_agent_event_overwrites_forged_pubkey_with_author() {
        let event = agent_profile_event(r#"{"pubkey":"forged","name":"agent-1"}"#);
        let agent = parse_relay_agent_event(&event).unwrap();

        assert_eq!(agent.pubkey, event.pubkey.to_hex());
        assert_eq!(agent.name, "agent-1");
    }

    #[test]
    fn parse_relay_agent_event_defaults_sparse_invalid_content() {
        let event = agent_profile_event("not-json");
        let agent = parse_relay_agent_event(&event).unwrap();

        assert_eq!(agent.pubkey, event.pubkey.to_hex());
        assert_eq!(agent.name, event.pubkey.to_hex()[..8]);
        assert_eq!(agent.agent_type, "agent");
        assert_eq!(agent.status, "offline");
        assert!(agent.channels.is_empty());
        assert!(agent.channel_ids.is_empty());
        assert!(agent.capabilities.is_empty());
    }

    #[test]
    fn relay_agent_presence_uses_snapshot_subject_and_defaults_missing_to_offline() {
        let mut agents = vec![
            RelayAgentInfo {
                pubkey: "agent-online".to_string(),
                status: "offline".to_string(),
                ..RelayAgentInfo::default()
            },
            RelayAgentInfo {
                pubkey: "agent-missing".to_string(),
                status: "online".to_string(),
                ..RelayAgentInfo::default()
            },
        ];
        let events = json!([
            {
                "pubkey": "relay-signing-key",
                "content": "online",
                "created_at": 42,
                "tags": [["p", "agent-online"]],
            },
        ]);

        apply_presence_to_relay_agents(&mut agents, &events);

        assert_eq!(agents[0].status, "online");
        assert_eq!(agents[1].status, "offline");
    }

    #[test]
    fn relay_agent_presence_keeps_latest_valid_snapshot() {
        let mut agents = vec![RelayAgentInfo {
            pubkey: "agent".to_string(),
            status: "offline".to_string(),
            ..RelayAgentInfo::default()
        }];
        let events = json!([
            {
                "pubkey": "agent",
                "content": "online",
                "created_at": 10,
                "tags": [],
            },
            {
                "pubkey": "agent",
                "content": "away",
                "created_at": 20,
                "tags": [],
            },
            {
                "pubkey": "agent",
                "content": "busy",
                "created_at": 30,
                "tags": [],
            },
        ]);

        apply_presence_to_relay_agents(&mut agents, &events);

        assert_eq!(agents[0].status, "away");
    }

    #[test]
    fn beekeeper_health_overlays_relay_agent_without_granting_controls() {
        let agent_pubkey = "a".repeat(64);
        let keys = Keys::generate();
        let mut agents = vec![RelayAgentInfo {
            pubkey: agent_pubkey.clone(),
            managed_by: Some("beekeeper".to_string()),
            management_mode: Some("external".to_string()),
            manager_pubkey: Some(keys.public_key().to_hex()),
            owner_pubkey: Some(keys.public_key().to_hex()),
            status: "online".to_string(),
            ..RelayAgentInfo::default()
        }];
        let event = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "crash loop")
            .tags([
                Tag::parse(["t", "beekeeper-health"]).unwrap(),
                Tag::parse(["managed_by", "beekeeper"]).unwrap(),
                Tag::parse(["agent_pubkey", agent_pubkey.as_str()]).unwrap(),
                Tag::parse(["status", "restarting"]).unwrap(),
                Tag::parse(["restarts", "4"]).unwrap(),
            ])
            .custom_created_at(Timestamp::from(42))
            .sign_with_keys(&keys)
            .unwrap();
        let forged = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "all good")
            .tags([
                Tag::parse(["t", "beekeeper-health"]).unwrap(),
                Tag::parse(["managed_by", "beekeeper"]).unwrap(),
                Tag::parse(["agent_pubkey", agent_pubkey.as_str()]).unwrap(),
                Tag::parse(["status", "healthy"]).unwrap(),
            ])
            .custom_created_at(Timestamp::from(43))
            .sign_with_keys(&Keys::generate())
            .unwrap();

        apply_beekeeper_health_to_relay_agents(&mut agents, &json!([event, forged]));

        assert_eq!(agents[0].health_status.as_deref(), Some("restarting"));
        assert_eq!(agents[0].health_detail.as_deref(), Some("crash loop"));
        assert_eq!(agents[0].health_restarts, Some(4));
        assert_eq!(agents[0].health_updated_at, Some(42));
        assert_eq!(agents[0].management_mode.as_deref(), Some("external"));
    }

    #[test]
    fn relay_agent_parser_preserves_external_management_metadata() {
        let event = agent_profile_event(
            r#"{"name":"Reviewer","agent_type":"codex-acp","managed_by":"beekeeper","management_mode":"external","manager_pubkey":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        );
        let agent = parse_relay_agent_event(&event).unwrap();

        assert_eq!(agent.agent_type, "codex-acp");
        assert_eq!(agent.managed_by.as_deref(), Some("beekeeper"));
        assert_eq!(agent.management_mode.as_deref(), Some("external"));
        assert_eq!(
            agent.manager_pubkey.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn relay_agent_parser_verifies_embedded_owner_attestation() {
        let owner = Keys::generate();
        let agent_keys = Keys::generate();
        let encoded =
            buzz_sdk::nip_oa::compute_auth_tag(&owner, &agent_keys.public_key(), "").unwrap();
        let auth_tag: Tag = serde_json::from_str(&encoded).unwrap();
        let event = EventBuilder::new(
            Kind::Custom(KIND_AGENT_PROFILE as u16),
            r#"{"name":"Reviewer","managed_by":"beekeeper","management_mode":"external"}"#,
        )
        .tags([auth_tag])
        .sign_with_keys(&agent_keys)
        .unwrap();

        let agent = parse_relay_agent_event(&event).unwrap();

        assert_eq!(
            agent.owner_pubkey.as_deref(),
            Some(owner.public_key().to_hex().as_str())
        );
    }

    #[test]
    fn parse_relay_agent_event_preserves_public_respond_to_mode() {
        let event = agent_profile_event(
            r#"{"name":"Scout","respond_to":"nobody","respond_to_allowlist":[]}"#,
        );
        let agent = parse_relay_agent_event(&event).unwrap();

        assert_eq!(agent.respond_to.as_deref(), Some("nobody"));
        assert!(agent.respond_to_allowlist.is_empty());
    }

    #[test]
    fn parse_relay_agent_event_accepts_nip_ap_managed_agent_projection() {
        let d_tag = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let event = managed_agent_event(
            d_tag,
            r#"{"name":"Scout","respond_to":"allowlist","respond_to_allowlist":["bbbb"]}"#,
        );
        let agent = parse_relay_agent_event(&event).unwrap();
        let owner_pubkey = event.pubkey.to_hex();

        assert_eq!(agent.pubkey, d_tag);
        assert_eq!(agent.owner_pubkey.as_deref(), Some(owner_pubkey.as_str()));
        assert_eq!(agent.name, "Scout");
        assert_eq!(agent.agent_type, "managed-agent");
        assert_eq!(agent.respond_to.as_deref(), Some("allowlist"));
        assert_eq!(agent.respond_to_allowlist, vec!["bbbb"]);
    }

    #[test]
    fn parse_relay_agent_event_rejects_malformed_nip_ap_projection_without_d_tag() {
        let event = client()
            .sign_event(EventBuilder::new(
                Kind::Custom(KIND_MANAGED_AGENT as u16),
                r#"{"name":"Scout"}"#,
            ))
            .unwrap();

        assert!(parse_relay_agent_event(&event).is_none());
    }

    #[test]
    fn parse_not_before_accepts_ascii_seconds_only() {
        assert_eq!(parse_not_before("0"), Some(0));
        assert_eq!(parse_not_before("42"), Some(42));
        assert_eq!(parse_not_before(""), None);
        assert_eq!(parse_not_before("042"), None);
        assert_eq!(parse_not_before("42s"), None);
    }

    #[test]
    fn parse_reminder_content_accepts_target_and_note_only_reminders() {
        let target = r#"{"status":"pending","target":{"eventId":"e","channelId":"c","preview":"p","authorPubkey":"a"}}"#;
        assert_eq!(
            parse_reminder_content(target)
                .and_then(|content| content.target)
                .map(|target| target.event_id),
            Some("e".to_string())
        );

        let note = r#"{"status":"pending","note":"standalone"}"#;
        assert_eq!(
            parse_reminder_content(note).and_then(|content| content.note),
            Some("standalone".to_string())
        );
    }

    #[test]
    fn parse_reminder_content_rejects_malformed_plaintext() {
        assert!(parse_reminder_content("not json").is_none());
        assert!(parse_reminder_content(r#"{"status":"waiting","note":"x"}"#).is_none());
        assert!(parse_reminder_content(r#"{"status":"pending"}"#).is_none());
        assert!(parse_reminder_content(r#"{"status":"pending","note":1}"#).is_none());
    }

    #[test]
    fn group_reminders_buckets_pending_and_hides_terminal_states() {
        let now = 86_400 * 10 + 12 * 60 * 60;
        let groups = group_reminders(
            &[
                reminder("over", Some(now - 1), ReminderStatus::Pending),
                reminder("today", Some(now + 60), ReminderStatus::Pending),
                reminder("upcoming", Some(now + 86_400), ReminderStatus::Pending),
                reminder("done", Some(now - 1), ReminderStatus::Done),
                reminder("cancelled", Some(now - 1), ReminderStatus::Cancelled),
                reminder("missing", None, ReminderStatus::Pending),
            ],
            now,
        );
        assert_eq!(
            groups
                .iter()
                .map(|group| (
                    group.label,
                    group
                        .reminders
                        .iter()
                        .map(|reminder| reminder.id.as_str())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("Overdue", vec!["over"]),
                ("Today", vec!["today"]),
                ("Upcoming", vec!["upcoming"]),
            ]
        );
        assert_eq!(
            count_due_reminders(
                &[
                    reminder("over", Some(now - 1), ReminderStatus::Pending),
                    reminder("future", Some(now + 1), ReminderStatus::Pending),
                ],
                now,
            ),
            1
        );
    }

    #[test]
    fn reminder_event_construction_uses_kind_tags_and_encrypted_content() {
        let client = client();
        let event = client
            .build_reminder_event(
                "reminder-id",
                reminder_content(),
                Some(123),
                None,
                Some(100),
            )
            .expect("build reminder");

        assert_eq!(event.kind.as_u16(), KIND_EVENT_REMINDER as u16);
        assert_eq!(
            find_tag_value(event.tags.iter(), "d").as_deref(),
            Some("reminder-id")
        );
        assert_eq!(
            find_tag_value(event.tags.iter(), "not_before").as_deref(),
            Some("123")
        );
        assert!(find_tag_value(event.tags.iter(), "expiration").is_none());
        let plaintext = nip44::decrypt(
            client.keys.secret_key(),
            &client.keys.public_key(),
            event.content.as_str(),
        )
        .expect("decrypt reminder");
        assert_eq!(
            parse_reminder_content(&plaintext).map(|content| content.status),
            Some(ReminderStatus::Pending)
        );
    }

    #[test]
    fn terminal_reminder_event_omits_not_before_and_adds_expiration() {
        let client = client();
        let mut content = reminder_content();
        content.status = ReminderStatus::Done;
        let event = client
            .build_reminder_event("same-d", content, None, Some(999), Some(501))
            .expect("build terminal reminder");

        assert_eq!(event.created_at.as_secs(), 501);
        assert_eq!(
            find_tag_value(event.tags.iter(), "d").as_deref(),
            Some("same-d")
        );
        assert!(find_tag_value(event.tags.iter(), "not_before").is_none());
        assert_eq!(
            find_tag_value(event.tags.iter(), "expiration").as_deref(),
            Some("999")
        );
    }

    #[test]
    fn snooze_reminder_event_reuses_d_and_updates_not_before() {
        let client = client();
        let event = client
            .build_reminder_event(
                "stable-d",
                reminder_content(),
                Some(777),
                None,
                Some(monotonic_reminder_created_at(500)),
            )
            .expect("build snooze reminder");

        assert!(event.created_at.as_secs() > 500);
        assert_eq!(
            find_tag_value(event.tags.iter(), "d").as_deref(),
            Some("stable-d")
        );
        assert_eq!(
            find_tag_value(event.tags.iter(), "not_before").as_deref(),
            Some("777")
        );
        assert!(find_tag_value(event.tags.iter(), "expiration").is_none());
    }

    #[test]
    fn relay_http_to_ws_url_accepts_http_inputs() {
        assert_eq!(
            relay_http_to_ws_url("https://relay.example/"),
            "wss://relay.example"
        );
        assert_eq!(
            relay_http_to_ws_url("http://localhost:3000"),
            "ws://127.0.0.1:3000"
        );
    }

    #[test]
    fn channel_message_filter_scopes_to_channel_and_message_kinds() {
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let filter = TuiRelayClient::channel_messages_filter(channel_id, Some(42));
        let value = serde_json::to_value(filter).unwrap();

        assert_eq!(
            value.get("#h").and_then(Value::as_array).cloned(),
            Some(vec![json!(channel_id.to_string())])
        );
        assert_eq!(value.get("since").and_then(Value::as_u64), Some(42));
        assert_eq!(
            value.get("kinds").and_then(Value::as_array).cloned(),
            Some(vec![
                json!(buzz_core::kind::KIND_DELETION),
                json!(KIND_REACTION),
                json!(KIND_STREAM_MESSAGE),
                json!(buzz_core::kind::KIND_NIP29_DELETE_EVENT),
                json!(KIND_STREAM_MESSAGE_V2),
                json!(KIND_STREAM_MESSAGE_EDIT),
                json!(buzz_core::kind::KIND_STREAM_MESSAGE_DIFF),
                json!(KIND_SYSTEM_MESSAGE),
                json!(buzz_core::kind::KIND_FORUM_POST),
                json!(buzz_core::kind::KIND_FORUM_COMMENT),
                json!(KIND_HUDDLE_STARTED),
            ])
        );
    }

    #[test]
    fn unread_filter_starts_after_read_frontier_and_counts_timeline_rows() {
        let filter =
            TuiRelayClient::channel_unread_filter("9ba26a41-91b9-4c57-83a9-08afd46330d2", 42);

        assert_eq!(
            filter["#h"],
            json!(["9ba26a41-91b9-4c57-83a9-08afd46330d2"])
        );
        assert_eq!(filter["since"], 43);
        assert_eq!(filter["limit"], 100);
        assert_eq!(
            filter["kinds"],
            json!([
                KIND_STREAM_MESSAGE,
                KIND_STREAM_MESSAGE_V2,
                buzz_core::kind::KIND_STREAM_MESSAGE_DIFF,
                buzz_core::kind::KIND_FORUM_POST,
                buzz_core::kind::KIND_FORUM_COMMENT,
            ])
        );
    }

    #[test]
    fn unread_message_respects_message_and_thread_read_frontiers() {
        let root_id = "11".repeat(32);
        let reply_id = "22".repeat(32);
        let reply = TuiMessageView {
            id: reply_id.clone(),
            created_at: 50,
            channel_id: WINDOW_CHANNEL.to_string(),
            thread_root_id: Some(root_id.clone()),
            ..TuiMessageView::default()
        };

        assert!(message_is_unread(
            &reply,
            42,
            &BTreeMap::new(),
            "current-user"
        ));

        let mut frontiers = BTreeMap::new();
        frontiers.insert(thread_context_key(&root_id), 50);
        assert!(!message_is_unread(&reply, 42, &frontiers, "current-user"));

        frontiers.clear();
        frontiers.insert(msg_context_key(&reply_id), 50);
        assert!(!message_is_unread(&reply, 42, &frontiers, "current-user"));

        let huddle = TuiMessageView {
            kind: u64::from(KIND_HUDDLE_STARTED),
            created_at: 51,
            ..TuiMessageView::default()
        };
        assert!(!message_is_unread(
            &huddle,
            42,
            &BTreeMap::new(),
            "current-user"
        ));
    }

    #[test]
    fn unread_message_ignores_current_users_events() {
        let message = TuiMessageView {
            pubkey: "CURRENT-USER".to_string(),
            created_at: 50,
            ..TuiMessageView::default()
        };

        assert!(!message_is_unread(
            &message,
            42,
            &BTreeMap::new(),
            "current-user"
        ));
    }

    #[test]
    fn build_message_event_uses_sdk_builder_and_tui_signer() {
        let client = client();
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let event = client
            .build_message_event(
                channel_id,
                ChannelMessageSurface::Stream,
                "hello from tui",
                &[],
                None,
                &[],
            )
            .unwrap();

        assert_eq!(event.kind, Kind::Custom(KIND_STREAM_MESSAGE as u16));
        assert_eq!(event.content, "hello from tui");
        assert_eq!(event.pubkey, client.keys.public_key());
        assert!(event
            .tags
            .iter()
            .any(
                |tag| tag.as_slice().first().map(String::as_str) == Some("h")
                    && tag.as_slice().get(1) == Some(&channel_id.to_string())
            ));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice().first().map(String::as_str) == Some("auth")));
    }

    #[test]
    fn build_message_event_tags_nip27_pubkey_mentions() {
        let client = client();
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let pubkey = "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e";
        let event = client
            .build_message_event(
                channel_id,
                ChannelMessageSurface::Stream,
                "hello nostr:npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg",
                &[],
                None,
                &[],
            )
            .unwrap();

        assert!(event.tags.iter().any(|tag| {
            let parts = tag.as_slice();
            parts.first().map(String::as_str) == Some("p") && parts.get(1) == Some(&pubkey.into())
        }));
    }

    #[test]
    fn build_message_event_tags_sidecar_mentions_without_changing_content() {
        let client = client();
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let pubkey = "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e";
        let event = client
            .build_message_event(
                channel_id,
                ChannelMessageSurface::Stream,
                "hello @Helper",
                &[pubkey.to_string()],
                None,
                &[],
            )
            .unwrap();

        assert_eq!(event.content, "hello @Helper");
        assert!(event.tags.iter().any(|tag| {
            let parts = tag.as_slice();
            parts.first().map(String::as_str) == Some("p") && parts.get(1) == Some(&pubkey.into())
        }));
    }

    #[test]
    fn build_message_event_ignores_nip27_mentions_inside_code() {
        let client = client();
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let event = client
            .build_message_event(
                channel_id,
                ChannelMessageSurface::Stream,
                "`nostr:npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg`",
                &[],
                None,
                &[],
            )
            .unwrap();

        assert!(!event
            .tags
            .iter()
            .any(|tag| tag.as_slice().first().map(String::as_str) == Some("p")));
    }

    #[test]
    fn normalize_message_event_projects_raw_event_to_tui_view() {
        let client = client();
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let event = client
            .build_message_event(
                channel_id,
                ChannelMessageSurface::Stream,
                "normalized",
                &[],
                None,
                &[],
            )
            .unwrap();

        let message = TuiRelayClient::normalize_message_event(&event);

        assert_eq!(message.id, event.id.to_hex());
        assert_eq!(message.pubkey, event.pubkey.to_hex());
        assert_eq!(message.kind, KIND_STREAM_MESSAGE as u64);
        assert_eq!(message.content, "normalized");
        assert_eq!(message.created_at, event.created_at.as_secs());
        assert_eq!(message.channel_id, channel_id.to_string());
        assert_eq!(message.thread_root_id, None);
    }

    #[test]
    fn build_message_event_uses_forum_kinds_for_posts_and_comments() {
        let client = client();
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let post = client
            .build_message_event(
                channel_id,
                ChannelMessageSurface::Forum,
                "forum post",
                &[],
                None,
                &[],
            )
            .unwrap();
        let comment = client
            .build_message_event(
                channel_id,
                ChannelMessageSurface::Forum,
                "forum comment",
                &[],
                Some((post.id, post.id)),
                &[],
            )
            .unwrap();

        assert_eq!(
            post.kind,
            Kind::Custom(buzz_core::kind::KIND_FORUM_POST as u16)
        );
        assert_eq!(
            comment.kind,
            Kind::Custom(buzz_core::kind::KIND_FORUM_COMMENT as u16)
        );
        assert!(comment.tags.iter().any(|tag| {
            let parts = tag.as_slice();
            parts.first().map(String::as_str) == Some("e")
                && parts.get(1) == Some(&post.id.to_hex())
                && parts.get(3).map(String::as_str) == Some("reply")
        }));
    }

    #[test]
    fn normalize_message_value_projects_thread_root() {
        let value = json!({
            "id": "reply",
            "pubkey": "author",
            "kind": KIND_STREAM_MESSAGE,
            "content": "reply",
            "created_at": 42,
            "tags": [
                ["h", "9ba26a41-91b9-4c57-83a9-08afd46330d2"],
                ["e", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "", "root"],
                ["e", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "", "reply"]
            ]
        });

        let message = TuiRelayClient::normalize_message_value(&value).expect("message");

        assert_eq!(message.channel_id, "9ba26a41-91b9-4c57-83a9-08afd46330d2");
        assert_eq!(
            message.thread_root_id.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn build_read_state_event_encrypts_niprs_payload() {
        let client = client();
        let client_id = "11111111111111111111111111111111";
        let slot_id = "22222222222222222222222222222222";
        let event = client
            .build_read_state_event(
                client_id,
                slot_id,
                BTreeMap::from([("channel-1".to_string(), 42)]),
                Some(100),
            )
            .unwrap();

        assert_eq!(event.kind, Kind::Custom(KIND_READ_STATE as u16));
        assert_eq!(event.created_at.as_secs(), 100);
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["d".to_string(), format!("read-state:{slot_id}")]));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["t", "read-state"]));

        let plaintext = nip44::decrypt(
            client.keys.secret_key(),
            &client.keys.public_key(),
            &event.content,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&plaintext).unwrap(),
            json!({
                "v": 1,
                "client_id": client_id,
                "contexts": {
                    "channel-1": 42
                }
            })
        );
    }

    #[test]
    fn read_state_slots_merge_by_max_frontier() {
        let mut merged =
            BTreeMap::from([("channel-a".to_string(), 10), ("channel-b".to_string(), 30)]);
        merge_read_state_contexts(
            &mut merged,
            BTreeMap::from([
                ("channel-a".to_string(), 20),
                ("channel-b".to_string(), 15),
                ("channel-c".to_string(), 40),
            ]),
        );
        assert_eq!(merged["channel-a"], 20);
        assert_eq!(merged["channel-b"], 30);
        assert_eq!(merged["channel-c"], 40);
    }

    #[test]
    fn channel_preferences_are_encrypted_and_preserve_all_entries() {
        let client = client();
        let entries = BTreeMap::from([
            (
                "channel-a".to_string(),
                ChannelPreferenceEntry {
                    enabled: true,
                    updated_at: 10,
                },
            ),
            (
                "channel-b".to_string(),
                ChannelPreferenceEntry {
                    enabled: false,
                    updated_at: 20,
                },
            ),
        ]);
        let event = client
            .build_channel_preference_event(ChannelPreferenceStoreKind::Stars, entries, Some(100))
            .unwrap();
        assert!(serde_json::from_str::<Value>(&event.content).is_err());
        let plaintext = nip44::decrypt(
            client.keys.secret_key(),
            &client.keys.public_key(),
            &event.content,
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&plaintext).unwrap();
        assert_eq!(payload["channels"]["channel-a"]["starred"], true);
        assert_eq!(payload["channels"]["channel-b"]["starred"], false);
    }

    #[test]
    fn channel_sections_are_encrypted_to_self() {
        let client = client();
        let store = ChannelSectionStore {
            version: 1,
            sections: vec![ChannelSectionRecord {
                id: "section-a".to_string(),
                name: "Important".to_string(),
                order: 0,
            }],
            assignments: BTreeMap::from([("channel-a".to_string(), "section-a".to_string())]),
        };
        let event = client.build_channel_section_event(&store, 100).unwrap();
        let plaintext = nip44::decrypt(
            client.keys.secret_key(),
            &client.keys.public_key(),
            &event.content,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<ChannelSectionStore>(&plaintext).unwrap(),
            store
        );
    }

    #[test]
    fn structural_aux_applies_latest_authorized_edit_and_deletion() {
        let mut messages = vec![
            TuiMessageView {
                id: "keep".to_string(),
                pubkey: "alice".to_string(),
                content: "original".to_string(),
                ..TuiMessageView::default()
            },
            TuiMessageView {
                id: "remove".to_string(),
                pubkey: "bob".to_string(),
                ..TuiMessageView::default()
            },
        ];
        let events = vec![
            json!({"id":"edit-old","pubkey":"alice","kind":KIND_STREAM_MESSAGE_EDIT,"content":"old","created_at":10,"tags":[["e","keep"]]}),
            json!({"id":"edit-new","pubkey":"alice","kind":KIND_STREAM_MESSAGE_EDIT,"content":"new","created_at":20,"tags":[["e","keep"]]}),
            json!({"id":"forged","pubkey":"mallory","kind":KIND_STREAM_MESSAGE_EDIT,"content":"forged","created_at":30,"tags":[["e","keep"]]}),
            json!({"id":"delete","pubkey":"bob","kind":buzz_core::kind::KIND_DELETION,"created_at":40,"tags":[["e","remove"]]}),
        ];
        apply_structural_aux_events(&mut messages, &events);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "new");
        assert_eq!(messages[0].edit_event_id.as_deref(), Some("edit-new"));
    }

    #[test]
    fn dm_metadata_is_classified_as_direct_message() {
        let channel = parse_channel_metadata_event(&json!({
            "pubkey": "owner",
            "created_at": 10,
            "tags": [
                ["d", WINDOW_CHANNEL],
                ["name", "DM"],
                ["t", "dm"],
                ["private"],
                ["p", "alice"],
                ["p", "bob"]
            ]
        }))
        .unwrap();
        assert_eq!(channel.kind, ConversationKind::DirectMessage);
        assert_eq!(channel.channel_type, "dm");
        assert_eq!(channel.participant_pubkeys, vec!["alice", "bob"]);
    }

    #[test]
    fn profile_field_update_preserves_legacy_name() {
        let client = client();
        let current = UserProfile {
            display_name: "Alice".to_string(),
            name: "alice-legacy".to_string(),
            about: "old".to_string(),
            ..UserProfile::default()
        };
        let event = client
            .build_profile_field_event(&current, ProfileField::About, "new")
            .unwrap();
        let content: Value = serde_json::from_str(&event.content).unwrap();
        assert_eq!(content["name"], "alice-legacy");
        assert_eq!(content["about"], "new");
    }

    #[test]
    fn user_status_event_uses_general_d_tag_and_optional_emoji() {
        let event = client()
            .build_user_status_event("  heads down  ", "  🚧  ")
            .unwrap();

        assert_eq!(event.kind, Kind::Custom(KIND_USER_STATUS as u16));
        assert_eq!(event.content, "heads down");
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["d", "general"]));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["emoji", "🚧"]));

        let cleared = client().build_user_status_event("", "").unwrap();
        assert_eq!(cleared.content, "");
        assert!(!cleared
            .tags
            .iter()
            .any(|tag| tag.as_slice().first().map(String::as_str) == Some("emoji")));
    }

    #[test]
    fn pull_request_review_event_targets_current_commit() {
        let owner = "a".repeat(64);
        let author = "b".repeat(64);
        let pull_request = GitPullRequest {
            id: "c".repeat(64),
            repo_owner: owner.clone(),
            repo_id: "buzz".to_string(),
            author: author.clone(),
            commit: "d".repeat(40),
            ..GitPullRequest::default()
        };

        let event = client()
            .build_pull_request_review_event(&pull_request, true)
            .unwrap();

        assert_eq!(event.kind, Kind::TextNote);
        assert_eq!(event.content, "Approved these changes");
        assert!(event
            .tags
            .iter()
            .any(|tag| { tag.as_slice() == ["e", pull_request.id.as_str(), "", "root"] }));
        assert!(event
            .tags
            .iter()
            .any(|tag| { tag.as_slice() == ["a", repo_coordinate(&owner, "buzz").as_str()] }));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["p", owner.as_str()]));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["p", author.as_str()]));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["t", "approval"]));
        assert!(event
            .tags
            .iter()
            .any(|tag| { tag.as_slice() == ["c", pull_request.commit.as_str()] }));
    }

    #[tokio::test]
    async fn tui_refuses_to_publish_merged_status_without_a_git_merge() {
        let error = client()
            .set_pull_request_status(&GitPullRequest::default(), GitStatus::AppliedOrResolved)
            .await
            .unwrap_err();

        assert!(
            matches!(error, RelayClientError::Builder(message) if message.contains("actual git merge"))
        );
    }

    #[test]
    fn pull_request_parser_trusts_lifecycle_and_current_commit_reviews() {
        let owner = "a".repeat(64);
        let author = "b".repeat(64);
        let reviewer = "c".repeat(64);
        let requested = "d".repeat(64);
        let outsider = "e".repeat(64);
        let root_id = "f".repeat(64);
        let root = json!({
            "id": root_id,
            "kind": KIND_GIT_PULL_REQUEST,
            "pubkey": author,
            "content": "Body",
            "created_at": 10,
            "tags": [
                ["a", repo_coordinate(&owner, "buzz")],
                ["p", reviewer],
                ["subject", "Safer PR parsing"],
                ["c", "old-commit"],
                ["clone", "https://example.test/buzz.git"],
                ["branch-name", "feature"]
            ]
        });
        let events = vec![
            root.clone(),
            json!({"id":"update-trusted","kind":KIND_GIT_PR_UPDATE,"pubkey":author,"created_at":20,"tags":[["E",root_id],["c","new-commit"],["clone","https://example.test/new.git"]]}),
            json!({"id":"update-forged","kind":KIND_GIT_PR_UPDATE,"pubkey":outsider,"created_at":99,"tags":[["E",root_id],["c","evil-commit"]]}),
            json!({"id":"request","kind":KIND_TEXT_NOTE,"pubkey":owner,"created_at":21,"tags":[["e",root_id],["p",requested],["t","review-request"]]}),
            json!({"id":"stale","kind":KIND_TEXT_NOTE,"pubkey":reviewer,"created_at":22,"tags":[["e",root_id],["t","approval"],["c","old-commit"]]}),
            json!({"id":"changes","kind":KIND_TEXT_NOTE,"pubkey":reviewer,"created_at":23,"tags":[["e",root_id],["t","changes-requested"],["c","new-commit"]]}),
            json!({"id":"approval","kind":KIND_TEXT_NOTE,"pubkey":reviewer,"created_at":24,"tags":[["e",root_id],["t","approval"],["c","new-commit"]]}),
            json!({"id":"requested-approval","kind":KIND_TEXT_NOTE,"pubkey":requested,"created_at":25,"tags":[["e",root_id],["t","approval"],["c","new-commit"]]}),
            json!({"id":"forged-review","kind":KIND_TEXT_NOTE,"pubkey":outsider,"created_at":100,"tags":[["e",root_id],["t","changes-requested"],["c","new-commit"]]}),
            json!({"id":"forged-status","kind":KIND_GIT_STATUS_MERGED,"pubkey":outsider,"created_at":100,"tags":[["e",root_id]]}),
            json!({"id":"closed","kind":KIND_GIT_STATUS_CLOSED,"pubkey":owner,"created_at":30,"tags":[["e",root_id]]}),
        ];

        let pull_request = parse_pull_request(&root, &events, &owner, "buzz");

        assert_eq!(pull_request.title, "Safer PR parsing");
        assert_eq!(pull_request.commit, "new-commit");
        assert_eq!(pull_request.clone_urls, ["https://example.test/new.git"]);
        assert_eq!(pull_request.status, "closed");
        assert_eq!(pull_request.status_created_at, Some(30));
        assert_eq!(pull_request.approval_count, 2);
        assert_eq!(pull_request.change_request_count, 0);
        assert_eq!(pull_request.review_created_at, Some(25));
        assert_eq!(pull_request.updated_at, 30);
    }

    #[test]
    fn workflow_secret_is_extracted_only_from_structured_response_message() {
        assert_eq!(
            workflow_webhook_secret(&json!({
                "message": "response:{\"webhook_secret\":\"one-time-secret\"}"
            })),
            Some("one-time-secret".to_string())
        );
        assert_eq!(
            workflow_webhook_secret(&json!({"message": "accepted"})),
            None
        );
        assert_eq!(
            workflow_webhook_secret(&json!({"message": "response:not-json"})),
            None
        );
    }
}
