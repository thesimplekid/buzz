use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::time::Duration;

use base64::Engine;
use buzz_core::engram::{
    self, conversation_key, d_tag as engram_d_tag, normalize_slug, select_head,
    validate_and_decrypt, Body,
};
use buzz_core::kind::{
    KIND_AGENT_ENGRAM, KIND_AGENT_PROFILE, KIND_BLOSSOM_AUTH, KIND_CANVAS, KIND_CONTACT_LIST,
    KIND_DM_CREATED, KIND_DM_HIDE, KIND_DM_OPEN, KIND_EMOJI_SET, KIND_GIT_ISSUE, KIND_GIT_PATCH,
    KIND_GIT_REPO_ANNOUNCEMENT, KIND_HTTP_AUTH, KIND_LONG_FORM, KIND_NIP29_GROUP_MEMBERS,
    KIND_NIP29_GROUP_METADATA, KIND_PRESENCE_SNAPSHOT, KIND_PRESENCE_UPDATE, KIND_REACTION,
    KIND_READ_STATE, KIND_STREAM_MESSAGE, KIND_STREAM_MESSAGE_EDIT, KIND_STREAM_MESSAGE_V2,
    KIND_TEXT_NOTE, KIND_WORKFLOW_CANCELLED, KIND_WORKFLOW_COMPLETED, KIND_WORKFLOW_DEF,
    KIND_WORKFLOW_FAILED, KIND_WORKFLOW_STEP_COMPLETED, KIND_WORKFLOW_STEP_FAILED,
    KIND_WORKFLOW_STEP_STARTED, KIND_WORKFLOW_TRIGGER, KIND_WORKFLOW_TRIGGERED,
};
use buzz_sdk::mentions::{extract_nostr_uris, normalize_mention_pubkeys, strip_code_regions};
use buzz_sdk::{
    ChannelKind, CustomEmoji, DiffMeta, GitIssueMeta, GitPatchMeta, GitRepoCoord, MemberRole,
    ThreadRef, Visibility, VoteDirection,
};
use buzz_ws_client::{publish_event, NostrWsConnection, RelayMessage, WsClientError};
use nostr::nips::nip01::Coordinate;
use nostr::nips::nip44::{self, Version};
use nostr::{
    Alphabet, Event, EventBuilder, EventId, Filter, Keys, Kind, PublicKey, SingleLetterTag, Tag,
    Timestamp, ToBech32,
};
use reqwest::{Client, RequestBuilder};
use serde_json::json;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

pub mod app_data;
mod models;

pub use models::*;

use crate::app_data::{
    channel_preference_payload, channel_preference_tags, channel_section_tags,
    channel_sections_payload, read_state_payload, read_state_tags, ChannelPreferenceEntry,
    ChannelPreferenceStoreKind, ChannelSectionRecord, ChannelSectionStore, ReadStateBlob,
    CHANNEL_SECTIONS_D_TAG, TUI_READ_STATE_SLOT_ID,
};

#[derive(Clone, Debug)]
pub struct TuiRelayClient {
    http: Client,
    base_url: String,
    keys: Keys,
    auth_tag_json: Option<String>,
}

#[derive(Debug, Error)]
pub enum RelayClientError {
    #[error("invalid private key: {0}")]
    Key(String),
    #[error("NIP-98 signing failed: {0}")]
    Signing(String),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("WebSocket relay failed: {0}")]
    WebSocket(#[from] WsClientError),
    #[error("invalid relay auth tag: {0}")]
    AuthTag(String),
    #[error("invalid event builder input: {0}")]
    Builder(String),
}

impl TuiRelayClient {
    pub fn new(
        relay_url: impl Into<String>,
        private_key: &str,
        auth_tag_json: Option<String>,
    ) -> Result<Self, RelayClientError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        let keys =
            Keys::parse(private_key).map_err(|error| RelayClientError::Key(error.to_string()))?;
        Ok(Self {
            http,
            base_url: normalize_relay_http_url(&relay_url.into()),
            keys,
            auth_tag_json,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn public_key_hex(&self) -> String {
        self.keys.public_key().to_hex()
    }

    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }

    pub fn channel_messages_filter(channel_id: Uuid, since: Option<u64>) -> Filter {
        let filter = Filter::new()
            .kinds([
                Kind::Custom(KIND_STREAM_MESSAGE as u16),
                Kind::Custom(KIND_STREAM_MESSAGE_V2 as u16),
                Kind::Custom(KIND_STREAM_MESSAGE_EDIT as u16),
                Kind::Custom(KIND_REACTION as u16),
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

    pub fn channel_history_filter(channel_id: &str, limit: u32) -> Value {
        json!({
            "kinds": [
                KIND_STREAM_MESSAGE,
                KIND_STREAM_MESSAGE_V2,
                KIND_STREAM_MESSAGE_EDIT,
                buzz_core::kind::KIND_STREAM_MESSAGE_DIFF,
                buzz_core::kind::KIND_FORUM_POST,
                buzz_core::kind::KIND_FORUM_COMMENT,
            ],
            "#h": [channel_id],
            "limit": limit.min(1000),
        })
    }

    pub fn thread_filter(channel_id: &str, event_id: &str, limit: u32) -> Value {
        json!({
            "kinds": [
                KIND_STREAM_MESSAGE,
                KIND_STREAM_MESSAGE_V2,
                KIND_STREAM_MESSAGE_EDIT,
                buzz_core::kind::KIND_STREAM_MESSAGE_DIFF,
                buzz_core::kind::KIND_FORUM_COMMENT,
            ],
            "#h": [channel_id],
            "#e": [event_id],
            "limit": limit.min(500),
        })
    }

    pub fn event_id_filter(event_id: &str) -> Value {
        json!({
            "ids": [event_id],
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

    pub fn workflow_filter(channel_id: Uuid, since: Option<u64>) -> Filter {
        let filter = Filter::new()
            .kind(Kind::Custom(KIND_WORKFLOW_DEF as u16))
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

    pub fn build_message_event(
        &self,
        channel_id: Uuid,
        content: &str,
        reply_to: Option<(EventId, EventId)>,
        media_tags: &[Vec<String>],
    ) -> Result<Event, RelayClientError> {
        let thread_ref = reply_to.map(|(root_event_id, parent_event_id)| ThreadRef {
            root_event_id,
            parent_event_id,
        });
        let stripped = strip_code_regions(content);
        let mention_pubkeys =
            normalize_mention_pubkeys(&extract_nostr_uris(&stripped), Some(&self.public_key_hex()));
        let mention_refs: Vec<&str> = mention_pubkeys.iter().map(String::as_str).collect();
        let builder = buzz_sdk::build_message(
            channel_id,
            content,
            thread_ref.as_ref(),
            &mention_refs,
            false,
            media_tags,
        )
        .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        self.sign_event(builder)
    }

    /// Build and submit a channel message directly to the relay, bypassing the
    /// CLI subprocess. `reply_to` is a thread root event id (hex).
    pub async fn send_channel_message(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<(), RelayClientError> {
        let uuid = Uuid::parse_str(channel_id)
            .map_err(|error| RelayClientError::Builder(format!("channel id: {error}")))?;
        let reply = self.resolve_reply_pair(reply_to).await?;
        let event = self.build_message_event(uuid, content, reply, &[])?;
        self.submit_event(&event).await.map(|_| ())
    }

    pub async fn send_channel_message_with_files(
        &self,
        channel_id: &str,
        content: &str,
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
        let event = self.build_message_event(uuid, &final_content, reply, &media_tags)?;
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
            None,
            non_empty(picture),
            non_empty(about),
            non_empty(nip05),
        )
        .map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let event = self.sign_event(builder)?;
        self.submit_event(&event).await.map(|_| ())
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
        let auth_tag = self.auth_tag()?;
        let ok = publish_event(
            &relay_http_to_ws_url(&self.base_url),
            event,
            &self.keys,
            auth_tag.as_ref(),
            10,
        )
        .await?;
        if ok.accepted {
            Ok(())
        } else {
            Err(RelayClientError::Builder(format!(
                "presence rejected: {}",
                ok.message
            )))
        }
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
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse_channel_metadata_event)
            .collect())
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

    pub async fn list_dms(&self, limit: u32) -> Result<Vec<Channel>, RelayClientError> {
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_DM_CREATED],
                "#p": [self.public_key_hex()],
                "limit": limit.min(200),
            })])
            .await?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .map(parse_dm_event)
            .filter(|channel| !channel.id.is_empty())
            .collect())
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
                "#d": [format!("read-state:{TUI_READ_STATE_SLOT_ID}")],
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
            return Ok(ReadState::default());
        };
        let plaintext = nip44::decrypt(self.keys.secret_key(), &self.keys.public_key(), content)
            .map_err(|error| RelayClientError::Signing(error.to_string()))?;
        let blob = serde_json::from_str::<ReadStateBlob>(&plaintext)?;
        Ok(ReadState {
            contexts: blob.contexts,
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
        let value = self
            .query_values(&[json!({
                "kinds": [KIND_READ_STATE],
                "authors": [self.public_key_hex()],
                "#d": [store_kind.d_tag()],
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
            return Ok(BTreeSet::new());
        };
        let parsed: Value = serde_json::from_str(content)?;
        Ok(parsed
            .get("channels")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|channels| channels.iter())
            .filter_map(|(channel_id, entry)| {
                entry
                    .get(store_kind.field_name())
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    .then_some(channel_id.clone())
            })
            .collect())
    }

    pub async fn channel_sections(&self) -> Result<ChannelSections, RelayClientError> {
        let store = self.fetch_channel_section_store().await?;
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
        let mut store = self.fetch_channel_section_store().await?;
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
        self.publish_channel_section_store(&store).await?;
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
        let mut store = self.fetch_channel_section_store().await?;
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
        self.publish_channel_section_store(&store).await
    }

    pub async fn unassign_channel_section(
        &self,
        channel_id: &str,
    ) -> Result<Value, RelayClientError> {
        parse_uuid(channel_id, "channel id")?;
        let mut store = self.fetch_channel_section_store().await?;
        store.assignments.remove(channel_id);
        self.publish_channel_section_store(&store).await
    }

    async fn fetch_channel_section_store(&self) -> Result<ChannelSectionStore, RelayClientError> {
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
            return Ok(ChannelSectionStore {
                version: 1,
                ..ChannelSectionStore::default()
            });
        };
        serde_json::from_str::<ChannelSectionStore>(content).map_err(Into::into)
    }

    async fn publish_channel_section_store(
        &self,
        store: &ChannelSectionStore,
    ) -> Result<Value, RelayClientError> {
        let content = channel_sections_payload(store).to_string();
        let tags =
            channel_section_tags().map_err(|error| RelayClientError::Builder(error.to_string()))?;
        let event = self.sign_event(
            EventBuilder::new(Kind::Custom(KIND_READ_STATE as u16), content).tags(tags),
        )?;
        self.submit_event(&event).await
    }

    pub async fn list_notes(&self) -> Result<Vec<Note>, RelayClientError> {
        self.list_notes_with(&ListNotesOptions::default()).await
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
            NoteAuthor::Pubkey(pubkey) => {
                parse_pubkey(pubkey, "note author")?;
                filter["authors"] = json!([pubkey]);
            }
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

    pub async fn get_note(&self, lookup: &NoteLookup) -> Result<Note, RelayClientError> {
        match lookup {
            NoteLookup::Naddr(raw) => {
                let coord = parse_note_coordinate(raw)?;
                let value = self
                    .query_values(&[json!({
                        "kinds": [KIND_LONG_FORM],
                        "authors": [coord.public_key.to_hex()],
                        "#d": [coord.identifier],
                        "limit": 1,
                    })])
                    .await?;
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .next()
                    .and_then(parse_note_event)
                    .ok_or_else(|| RelayClientError::Builder("note not found".to_string()))
            }
            NoteLookup::Name {
                slug,
                author,
                latest,
            } => {
                let slug = normalize_note_slug(slug)?;
                let mut filter = json!({
                    "kinds": [KIND_LONG_FORM],
                    "#d": [slug],
                    "limit": 200,
                });
                if let Some(author) = author.as_deref().and_then(non_empty) {
                    let pubkey = if author == "me" {
                        self.public_key_hex()
                    } else {
                        parse_pubkey(author, "note author")?;
                        author.to_string()
                    };
                    filter["authors"] = json!([pubkey]);
                }
                let value = self.query_values(&[filter]).await?;
                let mut notes = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(parse_note_event)
                    .collect::<Vec<_>>();
                notes.sort_by_key(|note| std::cmp::Reverse(note.updated_at));
                match notes.len() {
                    0 => Err(RelayClientError::Builder(format!("note not found: {slug}"))),
                    1 => Ok(notes.remove(0)),
                    _ if *latest => Ok(notes.remove(0)),
                    _ => Err(RelayClientError::Builder(format!(
                        "note name {slug:?} is ambiguous; specify an author or latest"
                    ))),
                }
            }
        }
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

    pub async fn export_emoji_json(
        &self,
        scope: EmojiExportScope,
    ) -> Result<String, RelayClientError> {
        let mut emojis = match scope {
            EmojiExportScope::Own => self.own_emoji().await?,
            EmojiExportScope::Workspace => self.workspace_emoji().await?,
        };
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
        let owner = self
            .auth_tag_owner()
            .or_else(|| Some(self.public_key()))
            .ok_or_else(|| {
                RelayClientError::Builder("owner pubkey required for memory write".to_string())
            })?;
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
        let owner = self
            .auth_tag_owner()
            .or_else(|| Some(self.public_key()))
            .ok_or_else(|| {
                RelayClientError::Builder("owner pubkey required for memory write".to_string())
            })?;
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
        let owner = self
            .auth_tag_owner()
            .or_else(|| Some(self.public_key()))
            .ok_or_else(|| {
                RelayClientError::Builder("owner pubkey required for memory write".to_string())
            })?;
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
        contexts: BTreeMap<String, u64>,
        created_at: Option<u64>,
    ) -> Result<Event, RelayClientError> {
        let plaintext = serde_json::to_string(&read_state_payload(contexts))?;
        let ciphertext = nip44::encrypt(
            self.keys.secret_key(),
            &self.keys.public_key(),
            plaintext,
            Version::V2,
        )
        .map_err(|error| RelayClientError::Signing(error.to_string()))?;
        let mut builder = EventBuilder::new(Kind::Custom(KIND_READ_STATE as u16), ciphertext).tags(
            read_state_tags(TUI_READ_STATE_SLOT_ID)
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
        let content = channel_preference_payload(kind, &entries).to_string();
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
        Ok(messages)
    }

    pub fn sign_event(&self, builder: EventBuilder) -> Result<Event, RelayClientError> {
        let builder = if let Some(auth_tag) = self.auth_tag()? {
            builder.tags([auth_tag])
        } else {
            builder
        };
        builder
            .sign_with_keys(&self.keys)
            .map_err(|error| RelayClientError::Signing(error.to_string()))
    }

    pub async fn query(&self, filters: &[nostr::Filter]) -> Result<Value, RelayClientError> {
        let body = serde_json::to_vec(filters)?;
        let response = self.bridge_post("/query", body).await?;
        Ok(response.json().await?)
    }

    pub async fn query_values(&self, filters: &[Value]) -> Result<Value, RelayClientError> {
        let body = serde_json::to_vec(filters)?;
        let response = self.bridge_post("/query", body).await?;
        Ok(response.json().await?)
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
        let raw = self.auth_tag_json.as_deref()?;
        let parts = serde_json::from_str::<Vec<String>>(raw).ok()?;
        let owner = parts.get(1)?;
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

    pub async fn count(&self, filters: &[nostr::Filter]) -> Result<u64, RelayClientError> {
        let body = serde_json::to_vec(filters)?;
        let response = self.bridge_post("/count", body).await?;
        let value: Value = response.json().await?;
        Ok(value
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default())
    }

    pub async fn submit_event(&self, event: &Event) -> Result<Value, RelayClientError> {
        let body = serde_json::to_vec(event)?;
        let response = self.bridge_post("/events", body).await?;
        let text = response.text().await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn upload_bytes(
        &self,
        bytes: Vec<u8>,
        mime_type: &str,
    ) -> Result<UploadedFile, RelayClientError> {
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let url = self.upload_url();
        let mut request = self
            .http
            .put(&url)
            .timeout(upload_timeout(mime_type))
            .header(
                "Authorization",
                self.blossom_upload_auth_header(&sha256, upload_auth_expiration(mime_type))?,
            )
            .header("Content-Type", mime_type)
            .header("X-SHA-256", &sha256);
        if let Some(auth_tag_json) = &self.auth_tag_json {
            request = request.header("x-auth-tag", auth_tag_json);
        }

        let response = request.body(bytes).send().await?.error_for_status()?;
        Ok(response.json().await?)
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
        let ws_url = relay_http_to_ws_url(&self.base_url);
        let auth_tag = self.auth_tag()?;
        let mut connection =
            NostrWsConnection::connect_authenticated(&ws_url, &self.keys, auth_tag.as_ref())
                .await?;
        connection
            .send_raw(&subscription_request(subscription_id, &filters))
            .await?;
        Ok(TuiRelaySubscription { connection })
    }

    fn auth_tag(&self) -> Result<Option<Tag>, RelayClientError> {
        self.auth_tag_json
            .as_deref()
            .map(|json| serde_json::from_str::<Vec<String>>(json).map_err(RelayClientError::Json))
            .transpose()?
            .map(Tag::parse)
            .transpose()
            .map_err(|error| RelayClientError::Signing(error.to_string()))
    }

    async fn bridge_post(
        &self,
        path: &str,
        body: Vec<u8>,
    ) -> Result<reqwest::Response, RelayClientError> {
        let url = self.bridge_url(path);
        Ok(self
            .with_bridge_headers(self.http.post(&url), "POST", &url, Some(&body))?
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await?
            .error_for_status()?)
    }

    fn with_bridge_headers(
        &self,
        request: RequestBuilder,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<RequestBuilder, RelayClientError> {
        let request = request.header("Authorization", self.nip98_header(method, url, body)?);
        Ok(match &self.auth_tag_json {
            Some(auth_tag_json) => request.header("x-auth-tag", auth_tag_json),
            None => request,
        })
    }

    fn nip98_header(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<String, RelayClientError> {
        Ok(format!("Nostr {}", self.sign_nip98(method, url, body)?))
    }

    fn blossom_upload_auth_header(
        &self,
        sha256: &str,
        expires_at: u64,
    ) -> Result<String, RelayClientError> {
        let expires_at = expires_at.to_string();
        let mut tags = vec![
            Tag::parse(["t", "upload"])
                .map_err(|error| RelayClientError::Signing(error.to_string()))?,
            Tag::parse(["x", sha256])
                .map_err(|error| RelayClientError::Signing(error.to_string()))?,
            Tag::parse(["expiration", &expires_at])
                .map_err(|error| RelayClientError::Signing(error.to_string()))?,
        ];
        if let Some(server) = relay_server_tag(&self.base_url) {
            tags.push(
                Tag::parse(["server", &server])
                    .map_err(|error| RelayClientError::Signing(error.to_string()))?,
            );
        }

        let event = EventBuilder::new(Kind::Custom(KIND_BLOSSOM_AUTH as u16), "Upload file")
            .tags(tags)
            .sign_with_keys(&self.keys)
            .map_err(|error| RelayClientError::Signing(error.to_string()))?;
        let event_json = serde_json::to_string(&event)?;
        Ok(format!(
            "Nostr {}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(event_json)
        ))
    }

    fn sign_nip98(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<String, RelayClientError> {
        let mut tags = vec![
            Tag::parse(["u", url]).map_err(|error| RelayClientError::Signing(error.to_string()))?,
            Tag::parse(["method", method])
                .map_err(|error| RelayClientError::Signing(error.to_string()))?,
            Tag::parse(["nonce", &uuid::Uuid::new_v4().to_string()])
                .map_err(|error| RelayClientError::Signing(error.to_string()))?,
        ];
        if let Some(body) = body {
            let hash = hex::encode(Sha256::digest(body));
            tags.push(
                Tag::parse(["payload", &hash])
                    .map_err(|error| RelayClientError::Signing(error.to_string()))?,
            );
        }

        let event = EventBuilder::new(Kind::Custom(KIND_HTTP_AUTH as u16), "")
            .tags(tags)
            .sign_with_keys(&self.keys)
            .map_err(|error| RelayClientError::Signing(error.to_string()))?;
        let event_json = serde_json::to_string(&event)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(event_json))
    }

    fn bridge_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn upload_url(&self) -> String {
        self.bridge_url("/media/upload")
    }
}

pub struct TuiRelaySubscription {
    connection: NostrWsConnection,
}

impl TuiRelaySubscription {
    pub async fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Result<RelayMessage, RelayClientError> {
        Ok(self.connection.next_event(timeout).await?)
    }

    pub async fn close(mut self, subscription_id: &str) -> Result<(), RelayClientError> {
        self.connection
            .send_raw(&json!(["CLOSE", subscription_id]))
            .await?;
        self.connection.disconnect().await?;
        Ok(())
    }
}

pub fn subscription_request(subscription_id: &str, filters: &[Filter]) -> Value {
    let mut request = Vec::with_capacity(filters.len() + 2);
    request.push(json!("REQ"));
    request.push(json!(subscription_id));
    request.extend(filters.iter().map(|filter| json!(filter)));
    Value::Array(request)
}

pub fn normalize_relay_http_url(relay_url: &str) -> String {
    let relay_url = relay_url.trim().trim_end_matches('/');
    if let Some(rest) = relay_url.strip_prefix("ws://") {
        return format!("http://{rest}");
    }
    if let Some(rest) = relay_url.strip_prefix("wss://") {
        return format!("https://{rest}");
    }
    relay_url.to_string()
}

pub fn relay_http_to_ws_url(relay_url: &str) -> String {
    let relay_url = relay_url.trim().trim_end_matches('/');
    if let Some(rest) = relay_url.strip_prefix("https://") {
        return format!("wss://{rest}");
    }
    if let Some(rest) = relay_url.strip_prefix("http://") {
        return format!("ws://{rest}");
    }
    relay_url.to_string()
}

fn upload_timeout(mime_type: &str) -> Duration {
    if mime_type.starts_with("video/") {
        Duration::from_secs(600)
    } else {
        Duration::from_secs(120)
    }
}

fn upload_auth_expiration(mime_type: &str) -> u64 {
    let now = Timestamp::now().as_secs();
    now + if mime_type.starts_with("video/") {
        3600
    } else {
        600
    }
}

fn relay_server_tag(relay_url: &str) -> Option<String> {
    let parsed = url::Url::parse(relay_url).ok()?;
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn h_tag(value: &Value) -> Option<String> {
    tag_value(value, "h")
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
    Some(Channel {
        id,
        name,
        description: tag_value(event, "about").unwrap_or_default(),
        channel_type: tag_value(event, "t").unwrap_or_default(),
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
        kind: ConversationKind::Channel,
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

fn parse_presence_event(event: &Value) -> Option<PresenceInfo> {
    Some(PresenceInfo {
        pubkey: event.get("pubkey")?.as_str()?.to_string(),
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

fn parse_dm_event(event: &Value) -> Channel {
    let participants = event
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tag| {
            let parts = tag.as_array()?;
            (parts.first()?.as_str()? == "p").then(|| parts.get(1)?.as_str().map(str::to_string))?
        })
        .collect::<Vec<_>>();
    Channel::from_dm(&json!({
        "dm_id": d_tag(event).unwrap_or_default(),
        "participants": participants,
        "created_at": event.get("created_at").and_then(Value::as_u64).unwrap_or_default(),
    }))
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
    WorkflowRun {
        event_id: event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        kind: event
            .get("kind")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        content: event
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
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

fn parse_note_coordinate(raw: &str) -> Result<Coordinate, RelayClientError> {
    let coord = Coordinate::from_str(raw)
        .map_err(|error| RelayClientError::Builder(format!("invalid note coordinate: {error}")))?;
    if coord.kind != Kind::Custom(KIND_LONG_FORM as u16) {
        return Err(RelayClientError::Builder(format!(
            "coordinate kind is {}, expected {KIND_LONG_FORM}",
            coord.kind.as_u16()
        )));
    }
    if coord.identifier.is_empty() {
        return Err(RelayClientError::Builder(
            "note coordinate is missing its slug".to_string(),
        ));
    }
    Ok(coord)
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
    use nostr::JsonUtil;

    fn client() -> TuiRelayClient {
        let keys = Keys::generate();
        TuiRelayClient {
            http: Client::new(),
            base_url: "http://localhost:3000".to_string(),
            keys,
            auth_tag_json: Some(r#"["auth","owner","kind=9","sig"]"#.to_string()),
        }
    }

    #[test]
    fn normalize_relay_http_url_accepts_websocket_inputs() {
        assert_eq!(
            normalize_relay_http_url("wss://relay.example/"),
            "https://relay.example"
        );
        assert_eq!(
            normalize_relay_http_url("ws://localhost:3000"),
            "http://localhost:3000"
        );
    }

    #[test]
    fn relay_http_to_ws_url_accepts_http_inputs() {
        assert_eq!(
            relay_http_to_ws_url("https://relay.example/"),
            "wss://relay.example"
        );
        assert_eq!(
            relay_http_to_ws_url("http://localhost:3000"),
            "ws://localhost:3000"
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
                json!(KIND_REACTION),
                json!(KIND_STREAM_MESSAGE),
                json!(KIND_STREAM_MESSAGE_V2),
                json!(KIND_STREAM_MESSAGE_EDIT),
            ])
        );
    }

    #[test]
    fn subscription_request_uses_nip01_req_shape() {
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let filter = TuiRelayClient::channel_messages_filter(channel_id, None);
        let request = subscription_request("active-channel", &[filter]);

        assert_eq!(request[0], "REQ");
        assert_eq!(request[1], "active-channel");
        assert_eq!(request.as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn build_message_event_uses_sdk_builder_and_tui_signer() {
        let client = client();
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let event = client
            .build_message_event(channel_id, "hello from tui", None, &[])
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
                "hello nostr:npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg",
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
    fn build_message_event_ignores_nip27_mentions_inside_code() {
        let client = client();
        let channel_id = Uuid::parse_str("9ba26a41-91b9-4c57-83a9-08afd46330d2").unwrap();
        let event = client
            .build_message_event(
                channel_id,
                "`nostr:npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg`",
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
            .build_message_event(channel_id, "normalized", None, &[])
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
        let event = client
            .build_read_state_event(BTreeMap::from([("channel-1".to_string(), 42)]), Some(100))
            .unwrap();

        assert_eq!(event.kind, Kind::Custom(KIND_READ_STATE as u16));
        assert_eq!(event.created_at.as_secs(), 100);
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["d", "read-state:buzz-tui"]));
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
                "client_id": "buzz-tui",
                "contexts": {
                    "channel-1": 42
                }
            })
        );
    }

    #[test]
    fn nip98_header_contains_method_url_nonce_and_payload_hash() {
        let client = client();
        let header = client
            .nip98_header("POST", "http://localhost:3000/query", Some(br#"[]"#))
            .unwrap();
        let encoded = header.strip_prefix("Nostr ").unwrap();
        let json = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
        )
        .unwrap();
        let event = Event::from_json(json).unwrap();

        assert_eq!(event.kind, Kind::Custom(27235));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["u", "http://localhost:3000/query"]));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["method", "POST"]));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice().first().map(String::as_str) == Some("nonce")));
        assert!(event.tags.iter().any(|tag| {
            tag.as_slice()
                == [
                    "payload",
                    "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945",
                ]
        }));
    }

    #[test]
    fn bridge_headers_include_nip98_and_auth_tag_without_query_params() {
        let client = client();
        let request = client
            .with_bridge_headers(
                client.http.post("http://localhost:3000/events"),
                "POST",
                "http://localhost:3000/events",
                Some(b"{}"),
            )
            .unwrap()
            .build()
            .unwrap();

        assert!(request
            .headers()
            .get("Authorization")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("Nostr ")));
        assert_eq!(
            request
                .headers()
                .get("x-auth-tag")
                .and_then(|value| value.to_str().ok()),
            Some(r#"["auth","owner","kind=9","sig"]"#)
        );
        assert!(request.url().query().is_none());
    }

    #[test]
    fn blossom_upload_auth_header_contains_upload_hash_and_server_tags() {
        let client = client();
        let header = client
            .blossom_upload_auth_header(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1234,
            )
            .unwrap();
        let encoded = header.strip_prefix("Nostr ").unwrap();
        let json = String::from_utf8(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .unwrap(),
        )
        .unwrap();
        let event = Event::from_json(json).unwrap();

        assert_eq!(event.kind, Kind::Custom(KIND_BLOSSOM_AUTH as u16));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["t", "upload"]));
        assert!(event.tags.iter().any(|tag| {
            tag.as_slice()
                == [
                    "x",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ]
        }));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["expiration", "1234"]));
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["server", "localhost:3000"]));
    }
}
