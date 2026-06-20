use std::collections::BTreeMap;

use nostr::Tag;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CHANNEL_STARS_D_TAG: &str = "channel-stars";
pub const CHANNEL_MUTES_D_TAG: &str = "channel-mutes";
pub const CHANNEL_SECTIONS_D_TAG: &str = "channel-sections";
pub const READ_STATE_D_TAG_PREFIX: &str = "read-state:";
pub const READ_STATE_TAG: &str = "read-state";
pub const TUI_READ_STATE_CLIENT_ID: &str = "buzz-tui";
pub const TUI_READ_STATE_SLOT_ID: &str = "buzz-tui";

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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadStateBlob {
    pub v: u8,
    pub client_id: String,
    #[serde(default)]
    pub contexts: BTreeMap<String, u64>,
}

impl ReadStateBlob {
    pub fn new(contexts: BTreeMap<String, u64>) -> Self {
        Self {
            v: 1,
            client_id: TUI_READ_STATE_CLIENT_ID.to_string(),
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

pub fn read_state_payload(contexts: BTreeMap<String, u64>) -> Value {
    serde_json::to_value(ReadStateBlob::new(contexts)).unwrap_or_else(|_| {
        serde_json::json!({
            "v": 1,
            "client_id": TUI_READ_STATE_CLIENT_ID,
            "contexts": {},
        })
    })
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
        let contexts = BTreeMap::from([
            ("channel-1".to_string(), 42),
            ("thread:event-1".to_string(), 99),
        ]);

        assert_eq!(
            read_state_payload(contexts),
            serde_json::json!({
                "v": 1,
                "client_id": "buzz-tui",
                "contexts": {
                    "channel-1": 42,
                    "thread:event-1": 99
                }
            })
        );
        assert_eq!(
            read_state_tags(TUI_READ_STATE_SLOT_ID)
                .unwrap()
                .into_iter()
                .map(|tag| tag.as_slice().to_vec())
                .collect::<Vec<_>>(),
            vec![
                vec!["d".to_string(), "read-state:buzz-tui".to_string()],
                vec!["t".to_string(), READ_STATE_TAG.to_string()],
            ]
        );
    }
}
