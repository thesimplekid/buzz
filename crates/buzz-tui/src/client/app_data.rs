use std::collections::BTreeMap;

use nostr::Tag;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CHANNEL_STARS_D_TAG: &str = "channel-stars";
pub const CHANNEL_MUTES_D_TAG: &str = "channel-mutes";
pub const CHANNEL_SECTIONS_D_TAG: &str = "channel-sections";
pub const CHANNEL_SORT_D_TAG: &str = "channel-sort";
pub const CHANNEL_SORT_MODE_ALPHA: &str = "alpha";
pub const CHANNEL_SORT_MODE_RECENT: &str = "recent";
pub const READ_STATE_D_TAG_PREFIX: &str = "read-state:";
pub const READ_STATE_TAG: &str = "read-state";
pub const MSG_READ_STATE_PREFIX: &str = "msg:";
pub const THREAD_READ_STATE_PREFIX: &str = "thread:";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelPreferenceStoreKind {
    Stars,
    Mutes,
}

impl ChannelPreferenceStoreKind {
    pub fn d_tag(self) -> &'static str {
        match self {
            Self::Stars => CHANNEL_STARS_D_TAG,
            Self::Mutes => CHANNEL_MUTES_D_TAG,
        }
    }

    pub fn field_name(self) -> &'static str {
        match self {
            Self::Stars => "starred",
            Self::Mutes => "muted",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelPreferenceEntry {
    pub enabled: bool,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelSectionRecord {
    pub id: String,
    pub name: String,
    pub order: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelSectionStore {
    pub version: u8,
    #[serde(default)]
    pub sections: Vec<ChannelSectionRecord>,
    #[serde(default)]
    pub assignments: BTreeMap<String, String>,
}

/// Desktop-compatible channel sort blob: `{"version":1,"groups":{"channels":
/// "recent",...}}` with group keys `starred|channels|forums|dms|section:<id>`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelSortStore {
    pub version: u8,
    #[serde(default)]
    pub groups: BTreeMap<String, String>,
}

impl ChannelSortStore {
    /// Keep only entries a desktop client would accept.
    pub fn sanitized(mut self) -> Option<Self> {
        if self.version != 1 {
            return None;
        }
        self.groups
            .retain(|_, mode| mode == CHANNEL_SORT_MODE_ALPHA || mode == CHANNEL_SORT_MODE_RECENT);
        Some(self)
    }
}

pub fn channel_sort_tags() -> Result<Vec<Tag>, nostr::event::tag::Error> {
    app_data_tags(CHANNEL_SORT_D_TAG)
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadStateBlob {
    pub v: u8,
    pub client_id: String,
    #[serde(default)]
    pub contexts: BTreeMap<String, u64>,
}

impl ReadStateBlob {
    pub fn new(client_id: &str, contexts: BTreeMap<String, u64>) -> Self {
        Self {
            v: 1,
            client_id: client_id.to_string(),
            contexts,
        }
    }
}

pub fn channel_preference_payload(
    kind: ChannelPreferenceStoreKind,
    entries: &BTreeMap<String, ChannelPreferenceEntry>,
) -> Value {
    let mut channels = Map::new();
    for (channel_id, entry) in entries {
        let mut value = Map::new();
        value.insert(kind.field_name().to_string(), Value::Bool(entry.enabled));
        value.insert("updatedAt".to_string(), Value::from(entry.updated_at));
        channels.insert(channel_id.clone(), Value::Object(value));
    }

    serde_json::json!({
        "version": 1,
        "channels": channels,
    })
}

pub fn channel_sections_payload(store: &ChannelSectionStore) -> Value {
    serde_json::to_value(store).unwrap_or_else(|_| {
        serde_json::json!({
            "version": 1,
            "sections": [],
            "assignments": {},
        })
    })
}

pub fn read_state_payload(client_id: &str, contexts: BTreeMap<String, u64>) -> Value {
    serde_json::to_value(ReadStateBlob::new(client_id, contexts)).unwrap_or_else(|_| {
        serde_json::json!({
            "v": 1,
            "client_id": client_id,
            "contexts": {},
        })
    })
}

pub fn msg_context_key(message_id: &str) -> String {
    format!("{MSG_READ_STATE_PREFIX}{message_id}")
}

pub fn thread_context_key(root_id: &str) -> String {
    format!("{THREAD_READ_STATE_PREFIX}{root_id}")
}

pub fn is_msg_context_key(value: &str) -> bool {
    let Some(id) = value.strip_prefix(MSG_READ_STATE_PREFIX) else {
        return false;
    };
    !id.is_empty() && !id.starts_with(THREAD_READ_STATE_PREFIX)
}

pub fn is_thread_context_key(value: &str) -> bool {
    let Some(id) = value.strip_prefix(THREAD_READ_STATE_PREFIX) else {
        return false;
    };
    !id.is_empty() && !id.starts_with(MSG_READ_STATE_PREFIX)
}

pub fn channel_preference_tags(
    kind: ChannelPreferenceStoreKind,
) -> Result<Vec<Tag>, nostr::event::tag::Error> {
    app_data_tags(kind.d_tag())
}

pub fn channel_section_tags() -> Result<Vec<Tag>, nostr::event::tag::Error> {
    app_data_tags(CHANNEL_SECTIONS_D_TAG)
}

pub fn read_state_tags(slot_id: &str) -> Result<Vec<Tag>, nostr::event::tag::Error> {
    Ok(vec![
        Tag::parse(["d", &format!("{READ_STATE_D_TAG_PREFIX}{slot_id}")])?,
        Tag::parse(["t", READ_STATE_TAG])?,
    ])
}

fn app_data_tags(d_tag: &str) -> Result<Vec<Tag>, nostr::event::tag::Error> {
    Ok(vec![Tag::parse(["d", d_tag])?, Tag::parse(["t", d_tag])?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_star_payload_matches_desktop_nip78_shape() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "channel-1".to_string(),
            ChannelPreferenceEntry {
                enabled: true,
                updated_at: 42,
            },
        );

        assert_eq!(
            channel_preference_payload(ChannelPreferenceStoreKind::Stars, &entries),
            serde_json::json!({
                "version": 1,
                "channels": {
                    "channel-1": { "starred": true, "updatedAt": 42 }
                }
            })
        );
        assert_eq!(
            channel_preference_tags(ChannelPreferenceStoreKind::Stars)
                .unwrap()
                .into_iter()
                .map(|tag| tag.as_slice().to_vec())
                .collect::<Vec<_>>(),
            vec![
                vec!["d".to_string(), CHANNEL_STARS_D_TAG.to_string()],
                vec!["t".to_string(), CHANNEL_STARS_D_TAG.to_string()],
            ]
        );
    }

    #[test]
    fn channel_mute_payload_uses_muted_field() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "channel-1".to_string(),
            ChannelPreferenceEntry {
                enabled: false,
                updated_at: 99,
            },
        );

        assert_eq!(
            channel_preference_payload(ChannelPreferenceStoreKind::Mutes, &entries),
            serde_json::json!({
                "version": 1,
                "channels": {
                    "channel-1": { "muted": false, "updatedAt": 99 }
                }
            })
        );
    }

    #[test]
    fn channel_sections_payload_matches_desktop_nip78_shape() {
        let mut assignments = BTreeMap::new();
        assignments.insert("channel-1".to_string(), "section-1".to_string());
        let store = ChannelSectionStore {
            version: 1,
            sections: vec![ChannelSectionRecord {
                id: "section-1".to_string(),
                name: "Core Work".to_string(),
                order: 0,
            }],
            assignments,
        };

        assert_eq!(
            channel_sections_payload(&store),
            serde_json::json!({
                "version": 1,
                "sections": [
                    { "id": "section-1", "name": "Core Work", "order": 0 }
                ],
                "assignments": {
                    "channel-1": "section-1"
                }
            })
        );
        assert_eq!(
            channel_section_tags()
                .unwrap()
                .into_iter()
                .map(|tag| tag.as_slice().to_vec())
                .collect::<Vec<_>>(),
            vec![
                vec!["d".to_string(), CHANNEL_SECTIONS_D_TAG.to_string()],
                vec!["t".to_string(), CHANNEL_SECTIONS_D_TAG.to_string()],
            ]
        );
    }

    #[test]
    fn read_state_payload_and_tags_match_niprs_shape() {
        let client_id = "11111111111111111111111111111111";
        let slot_id = "22222222222222222222222222222222";
        let contexts = BTreeMap::from([
            ("channel-1".to_string(), 42),
            ("thread:event-1".to_string(), 99),
        ]);

        assert_eq!(
            read_state_payload(client_id, contexts),
            serde_json::json!({
                "v": 1,
                "client_id": client_id,
                "contexts": {
                    "channel-1": 42,
                    "thread:event-1": 99
                }
            })
        );
        assert_eq!(
            read_state_tags(slot_id)
                .unwrap()
                .into_iter()
                .map(|tag| tag.as_slice().to_vec())
                .collect::<Vec<_>>(),
            vec![
                vec!["d".to_string(), format!("read-state:{slot_id}")],
                vec!["t".to_string(), READ_STATE_TAG.to_string()],
            ]
        );
    }

    #[test]
    fn msg_context_helpers_match_desktop_shape() {
        assert_eq!(msg_context_key("event-1"), "msg:event-1");
        assert_eq!(thread_context_key("event-1"), "thread:event-1");
        assert!(is_msg_context_key("msg:event-1"));
        assert!(!is_msg_context_key("msg:"));
        assert!(!is_msg_context_key("msg:thread:event-1"));
        assert!(!is_msg_context_key("thread:event-1"));
        assert!(is_thread_context_key("thread:event-1"));
        assert!(!is_thread_context_key("thread:"));
        assert!(!is_thread_context_key("thread:msg:event-1"));
        assert!(!is_thread_context_key("msg:event-1"));
    }
}
