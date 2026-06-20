use std::time::Duration;

use buzz_core::kind::{
    KIND_CONTACT_LIST, KIND_DELETION, KIND_EMOJI_SET, KIND_FORUM_COMMENT, KIND_FORUM_POST,
    KIND_NIP29_DELETE_EVENT, KIND_NIP29_GROUP_MEMBERS, KIND_NIP29_GROUP_METADATA,
    KIND_PRESENCE_SNAPSHOT, KIND_PRESENCE_UPDATE, KIND_READ_STATE, KIND_STREAM_MESSAGE,
    KIND_STREAM_MESSAGE_DIFF, KIND_STREAM_MESSAGE_EDIT, KIND_STREAM_MESSAGE_V2, KIND_TEXT_NOTE,
    KIND_TYPING_INDICATOR,
};
use buzz_ws_client::{RelayMessage, WsClientError};
use nostr::{Filter, PublicKey};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::client::{RelayClientError, TuiMessageView, TuiRelayClient};

const ACTIVE_CHANNEL_SUBSCRIPTION_ID: &str = "buzz-tui-active-channel";
const WORKSPACE_SUBSCRIPTION_ID: &str = "buzz-tui-workspace";
const LIVE_RECV_TIMEOUT: Duration = Duration::from_secs(30);
const LIVE_RECONNECT_MAX_DELAY_SECS: u64 = 30;

/// Fast first retry, then exponential escalation capped at 30s — mirrors the
/// desktop's fast-path/escalation reconnect behavior.
fn reconnect_delay(attempt: u32) -> Duration {
    let secs = match attempt {
        0 | 1 => 1,
        attempt => (1u64 << (attempt - 1).min(6)).min(LIVE_RECONNECT_MAX_DELAY_SECS),
    };
    Duration::from_secs(secs)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveChannelTarget {
    pub relay_url: String,
    pub channel_id: String,
    pub presence_pubkeys: Vec<String>,
    pub since: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveEvent {
    Message(TuiMessageView),
    Edit {
        target_id: String,
        pubkey: String,
        content: String,
        created_at: u64,
        event_id: String,
    },
    Delete {
        target_ids: Vec<String>,
    },
    Typing {
        pubkey: String,
    },
    WorkspaceChanged,
    Notice(String),
    Error(String),
}

pub struct LiveRuntime {
    private_key: Option<String>,
    auth_tag: Option<String>,
    target: Option<LiveChannelTarget>,
    task: Option<JoinHandle<()>>,
    tx: UnboundedSender<LiveEvent>,
}

impl LiveRuntime {
    pub fn new(
        private_key: Option<String>,
        auth_tag: Option<String>,
        tx: UnboundedSender<LiveEvent>,
    ) -> Self {
        Self {
            private_key,
            auth_tag,
            target: None,
            task: None,
            tx,
        }
    }

    pub fn sync_active_channel(&mut self, target: Option<LiveChannelTarget>) {
        if same_live_stream(self.target.as_ref(), target.as_ref()) {
            return;
        }

        self.stop();
        self.target = target.clone();

        let Some(target) = target else {
            return;
        };
        let Some(private_key) = self.private_key.clone() else {
            return;
        };
        let channel_id = match Uuid::parse_str(&target.channel_id) {
            Ok(channel_id) => channel_id,
            Err(error) => {
                let _ = self.tx.send(LiveEvent::Error(format!(
                    "live channel subscription skipped: invalid channel id: {error}"
                )));
                return;
            }
        };
        let client =
            match TuiRelayClient::new(&target.relay_url, &private_key, self.auth_tag.clone()) {
                Ok(client) => client,
                Err(error) => {
                    let _ = self.tx.send(LiveEvent::Error(format!(
                        "live relay setup failed: {error}"
                    )));
                    return;
                }
            };
        let tx = self.tx.clone();
        self.task = Some(tokio::spawn(async move {
            run_active_channel_subscription(
                client,
                channel_id,
                target.presence_pubkeys,
                target.since,
                tx,
            )
            .await;
        }));
    }

    pub fn update_credentials(&mut self, private_key: Option<String>, auth_tag: Option<String>) {
        if self.private_key == private_key && self.auth_tag == auth_tag {
            return;
        }
        self.stop();
        self.target = None;
        self.private_key = private_key;
        self.auth_tag = auth_tag;
    }

    pub fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for LiveRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run_active_channel_subscription(
    client: TuiRelayClient,
    channel_id: Uuid,
    presence_pubkeys: Vec<String>,
    since: Option<u64>,
    tx: UnboundedSender<LiveEvent>,
) {
    let channel_client = client.clone();
    let channel_tx = tx.clone();
    let workspace_presence_pubkeys = presence_pubkeys.clone();

    tokio::select! {
        _ = run_live_subscription(
            channel_client,
            ACTIVE_CHANNEL_SUBSCRIPTION_ID,
            since,
            channel_tx,
            move |_client, since_cursor| channel_live_filters(channel_id, since_cursor),
        ) => {}
        _ = run_live_subscription(
            client,
            WORKSPACE_SUBSCRIPTION_ID,
            since,
            tx,
            move |client, since_cursor| {
                workspace_invalidation_filters(client, &workspace_presence_pubkeys, since_cursor)
            },
        ) => {}
    }
}

async fn run_live_subscription<F>(
    client: TuiRelayClient,
    subscription_id: &'static str,
    since: Option<u64>,
    tx: UnboundedSender<LiveEvent>,
    mut filters_for_since: F,
) where
    F: FnMut(&TuiRelayClient, Option<u64>) -> Vec<Filter>,
{
    let mut since_cursor = since;
    let mut attempt: u32 = 0;
    loop {
        let filters = filters_for_since(&client, since_cursor);
        let mut subscription = match client.subscribe_live(subscription_id, filters).await {
            Ok(subscription) => {
                if attempt > 0 {
                    let _ = tx.send(LiveEvent::Notice(format!("{subscription_id} reconnected")));
                }
                attempt = 0;
                subscription
            }
            Err(error) => {
                attempt = attempt.saturating_add(1);
                let delay = reconnect_delay(attempt);
                let _ = tx.send(LiveEvent::Error(format!(
                    "{subscription_id} subscription failed: {error}; retrying in {}s",
                    delay.as_secs()
                )));
                tokio::time::sleep(delay).await;
                continue;
            }
        };

        loop {
            match subscription.next_event(LIVE_RECV_TIMEOUT).await {
                Ok(RelayMessage::Event { event, .. }) => {
                    advance_since_cursor(&mut since_cursor, event.created_at.as_secs());
                    if let Some(event) = live_event_for_relay_event(&event) {
                        let _ = tx.send(event);
                    }
                }
                Ok(RelayMessage::Notice { message }) => {
                    let _ = tx.send(LiveEvent::Notice(message));
                }
                Ok(RelayMessage::Closed { message, .. }) => {
                    let _ = tx.send(LiveEvent::Error(format!(
                        "{subscription_id} subscription closed: {message}; reconnecting"
                    )));
                    break;
                }
                Ok(_) => {}
                Err(RelayClientError::WebSocket(WsClientError::Timeout)) => {}
                Err(error) => {
                    let _ = tx.send(LiveEvent::Error(format!(
                        "{subscription_id} subscription failed: {error}; reconnecting"
                    )));
                    break;
                }
            }
        }

        let _ = subscription.close(subscription_id).await;
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(reconnect_delay(attempt)).await;
    }
}

fn is_timeline_message_kind(kind: u16) -> bool {
    matches!(
        u32::from(kind),
        KIND_STREAM_MESSAGE
            | KIND_STREAM_MESSAGE_V2
            | KIND_STREAM_MESSAGE_DIFF
            | KIND_FORUM_POST
            | KIND_FORUM_COMMENT
    )
}

fn channel_live_filters(channel_id: Uuid, since: Option<u64>) -> Vec<Filter> {
    vec![
        TuiRelayClient::channel_messages_filter(channel_id, since),
        TuiRelayClient::channel_typing_filter(channel_id, since),
    ]
}

fn workspace_invalidation_filters(
    client: &TuiRelayClient,
    presence_pubkeys: &[String],
    since: Option<u64>,
) -> Vec<Filter> {
    let public_key = client.public_key();
    let presence_pubkeys = presence_public_keys(public_key, presence_pubkeys);
    vec![
        TuiRelayClient::joined_channels_filter(public_key, since),
        TuiRelayClient::mentions_filter(public_key, since),
        TuiRelayClient::app_data_filter(public_key, since),
        TuiRelayClient::contacts_filter(public_key, since),
        TuiRelayClient::presence_filter(presence_pubkeys, since),
        TuiRelayClient::channel_metadata_filter(since),
        TuiRelayClient::custom_emoji_filter(since),
    ]
}

fn presence_public_keys(own_pubkey: PublicKey, pubkeys: &[String]) -> Vec<PublicKey> {
    let mut parsed = pubkeys
        .iter()
        .filter_map(|pubkey| PublicKey::from_hex(pubkey).ok())
        .collect::<Vec<_>>();
    if !parsed.iter().any(|pubkey| pubkey == &own_pubkey) {
        parsed.push(own_pubkey);
    }
    parsed.sort_by_key(PublicKey::to_hex);
    parsed.dedup();
    parsed
}

fn live_event_for_relay_event(event: &nostr::Event) -> Option<LiveEvent> {
    let kind = event.kind.as_u16();
    if is_timeline_message_kind(kind) {
        return Some(LiveEvent::Message(TuiRelayClient::normalize_message_event(
            event,
        )));
    }
    if u32::from(kind) == KIND_STREAM_MESSAGE_EDIT {
        let target_id = event.tags.iter().find_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("e"))
                .then(|| parts.get(1).cloned())
                .flatten()
        })?;
        return Some(LiveEvent::Edit {
            target_id,
            pubkey: event.pubkey.to_hex(),
            content: event.content.clone(),
            created_at: event.created_at.as_secs(),
            event_id: event.id.to_hex(),
        });
    }
    if matches!(u32::from(kind), KIND_DELETION | KIND_NIP29_DELETE_EVENT) {
        let target_ids = event
            .tags
            .iter()
            .filter_map(|tag| {
                let parts = tag.as_slice();
                (parts.first().map(String::as_str) == Some("e"))
                    .then(|| parts.get(1).cloned())
                    .flatten()
            })
            .collect::<Vec<_>>();
        if !target_ids.is_empty() {
            return Some(LiveEvent::Delete { target_ids });
        }
    }
    if u32::from(kind) == KIND_TYPING_INDICATOR {
        return Some(LiveEvent::Typing {
            pubkey: event.pubkey.to_hex(),
        });
    }
    workspace_invalidation_kind(kind).then_some(LiveEvent::WorkspaceChanged)
}

fn workspace_invalidation_kind(kind: u16) -> bool {
    matches!(
        u32::from(kind),
        KIND_NIP29_GROUP_MEMBERS
            | KIND_TEXT_NOTE
            | KIND_READ_STATE
            | KIND_CONTACT_LIST
            | KIND_PRESENCE_UPDATE
            | KIND_PRESENCE_SNAPSHOT
            | KIND_NIP29_GROUP_METADATA
            | KIND_EMOJI_SET
    )
}

fn advance_since_cursor(cursor: &mut Option<u64>, created_at: u64) {
    if cursor.is_none_or(|current| created_at > current) {
        *cursor = Some(created_at);
    }
}

fn same_live_stream(left: Option<&LiveChannelTarget>, right: Option<&LiveChannelTarget>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.relay_url == right.relay_url
                && left.channel_id == right.channel_id
                && left.presence_pubkeys == right.presence_pubkeys
        }
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};
    use tokio::sync::mpsc;

    #[test]
    fn timeline_message_kind_excludes_non_timeline_events() {
        assert!(is_timeline_message_kind(KIND_STREAM_MESSAGE as u16));
        assert!(is_timeline_message_kind(KIND_STREAM_MESSAGE_V2 as u16));
        assert!(is_timeline_message_kind(KIND_FORUM_POST as u16));
        assert!(is_timeline_message_kind(KIND_FORUM_COMMENT as u16));
        assert!(!is_timeline_message_kind(KIND_STREAM_MESSAGE_EDIT as u16));
        assert!(!is_timeline_message_kind(7));
    }

    #[test]
    fn live_stream_identity_ignores_since_cursor() {
        let left = LiveChannelTarget {
            relay_url: "http://localhost:3000".to_string(),
            channel_id: "9ba26a41-91b9-4c57-83a9-08afd46330d2".to_string(),
            presence_pubkeys: Vec::new(),
            since: Some(10),
        };
        let right = LiveChannelTarget {
            since: Some(20),
            ..left.clone()
        };

        assert!(same_live_stream(Some(&left), Some(&right)));
        assert!(!same_live_stream(Some(&left), None));
    }

    #[test]
    fn relay_event_classifier_splits_timeline_and_workspace_events() {
        let keys = Keys::generate();
        let timeline = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "hello")
            .sign_with_keys(&keys)
            .unwrap();
        let read_state = EventBuilder::new(Kind::Custom(KIND_READ_STATE as u16), "{}")
            .sign_with_keys(&keys)
            .unwrap();
        let unknown = EventBuilder::new(Kind::Custom(12345), "ignored")
            .sign_with_keys(&keys)
            .unwrap();
        let edit = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE_EDIT as u16), "edited")
            .tags([Tag::parse(["e", "target"]).unwrap()])
            .sign_with_keys(&keys)
            .unwrap();
        let deletion = EventBuilder::new(Kind::Custom(KIND_NIP29_DELETE_EVENT as u16), "")
            .tags([Tag::parse(["e", "target"]).unwrap()])
            .sign_with_keys(&keys)
            .unwrap();

        assert!(matches!(
            live_event_for_relay_event(&timeline),
            Some(LiveEvent::Message(_))
        ));
        assert_eq!(
            live_event_for_relay_event(&read_state),
            Some(LiveEvent::WorkspaceChanged)
        );
        let typing = EventBuilder::new(Kind::Custom(KIND_TYPING_INDICATOR as u16), "")
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(
            live_event_for_relay_event(&typing),
            Some(LiveEvent::Typing {
                pubkey: keys.public_key().to_hex()
            })
        );
        assert_eq!(live_event_for_relay_event(&unknown), None);
        assert!(matches!(
            live_event_for_relay_event(&edit),
            Some(LiveEvent::Edit { target_id, content, .. })
                if target_id == "target" && content == "edited"
        ));
        assert_eq!(
            live_event_for_relay_event(&deletion),
            Some(LiveEvent::Delete {
                target_ids: vec!["target".to_string()]
            })
        );
    }

    #[test]
    fn presence_update_and_snapshot_invalidate_workspace() {
        let keys = Keys::generate();
        for kind in [KIND_PRESENCE_UPDATE, KIND_PRESENCE_SNAPSHOT] {
            let event = EventBuilder::new(Kind::Custom(kind as u16), "{}")
                .sign_with_keys(&keys)
                .unwrap();
            assert_eq!(
                live_event_for_relay_event(&event),
                Some(LiveEvent::WorkspaceChanged)
            );
        }
    }

    #[test]
    fn reconnect_delay_escalates_and_caps() {
        assert_eq!(reconnect_delay(1), Duration::from_secs(1));
        assert_eq!(reconnect_delay(2), Duration::from_secs(2));
        assert_eq!(reconnect_delay(3), Duration::from_secs(4));
        assert_eq!(reconnect_delay(5), Duration::from_secs(16));
        assert_eq!(
            reconnect_delay(6),
            Duration::from_secs(LIVE_RECONNECT_MAX_DELAY_SECS)
        );
        assert_eq!(
            reconnect_delay(60),
            Duration::from_secs(LIVE_RECONNECT_MAX_DELAY_SECS)
        );
    }

    #[test]
    fn since_cursor_only_moves_forward() {
        let mut cursor = Some(10);

        advance_since_cursor(&mut cursor, 9);
        assert_eq!(cursor, Some(10));

        advance_since_cursor(&mut cursor, 12);
        assert_eq!(cursor, Some(12));

        let mut empty = None;
        advance_since_cursor(&mut empty, 5);
        assert_eq!(empty, Some(5));
    }

    #[test]
    fn sync_active_channel_reports_invalid_channel_ids() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut runtime = LiveRuntime::new(Some("not-used".to_string()), None, tx);

        runtime.sync_active_channel(Some(LiveChannelTarget {
            relay_url: "http://localhost:3000".to_string(),
            channel_id: "not-a-uuid".to_string(),
            presence_pubkeys: Vec::new(),
            since: None,
        }));

        let event = rx.try_recv().expect("invalid target should emit an error");
        assert!(
            matches!(event, LiveEvent::Error(message) if message.contains("invalid channel id"))
        );
    }
}
