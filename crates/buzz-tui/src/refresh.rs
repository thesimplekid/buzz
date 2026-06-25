use std::collections::BTreeSet;

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::app::{ChannelScope, Focus, TimelineMode};
use crate::client::{
    Channel, ChannelMember, ChannelPreferenceKind, ChannelSections, Message, Reaction, ReadState,
    TuiRelayClient, UserProfile,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshKind {
    Full,
    Active,
}

#[derive(Clone, Debug)]
pub struct RefreshTarget {
    pub relay_url: String,
    pub private_key: Option<String>,
    pub auth_tag: Option<String>,
    pub channel_scope: ChannelScope,
    pub selected_channel: usize,
    pub active_channel_id: Option<String>,
    pub thread_root: Option<String>,
    pub timeline_mode: TimelineMode,
    pub focus: Focus,
    pub feed_type: Option<&'static str>,
    pub selected_message_id: Option<String>,
    pub known_author_pubkeys: BTreeSet<String>,
}

#[derive(Debug)]
pub enum RefreshEvent {
    Primary {
        generation: u64,
        target: RefreshTarget,
        result: Result<RefreshResult, String>,
    },
    Hydrate {
        generation: u64,
        target: HydrateTarget,
        result: HydrateResult,
    },
}

#[derive(Debug)]
pub struct RefreshResult {
    pub sidebar: Option<Result<SidebarData, String>>,
    pub read_state: Option<ReadState>,
    pub starred_channel_ids: Option<BTreeSet<String>>,
    pub muted_channel_ids: Option<BTreeSet<String>>,
    pub channel_sections: Option<ChannelSections>,
    pub channel_detail_id: Option<String>,
    pub channel_detail: Option<Result<Option<Channel>, String>>,
    pub channel_members: Option<Result<Vec<ChannelMember>, String>>,
    pub message_channel_id: Option<String>,
    pub messages: Option<Result<Vec<Message>, String>>,
    pub feed: Option<Result<Vec<Message>, String>>,
    pub profiles: Vec<UserProfile>,
    pub reaction_event_id: Option<String>,
    pub reactions: Option<Result<Vec<Reaction>, String>>,
}

#[derive(Debug)]
pub struct SidebarData {
    pub channels: Vec<Channel>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HydrateTarget {
    pub relay_url: String,
    pub private_key: Option<String>,
    pub auth_tag: Option<String>,
    pub selected_message_id: Option<String>,
    pub author_pubkeys: BTreeSet<String>,
    pub known_author_pubkeys: BTreeSet<String>,
}

#[derive(Debug, Default)]
pub struct HydrateResult {
    pub profiles: Vec<UserProfile>,
    pub reactions: Option<Result<Vec<Reaction>, String>>,
}

pub struct RefreshRuntime {
    primary_generation: u64,
    hydrate_generation: u64,
    primary_task: Option<JoinHandle<()>>,
    hydrate_task: Option<JoinHandle<()>>,
    tx: UnboundedSender<RefreshEvent>,
}

impl RefreshRuntime {
    pub fn new(tx: UnboundedSender<RefreshEvent>) -> Self {
        Self {
            primary_generation: 0,
            hydrate_generation: 0,
            primary_task: None,
            hydrate_task: None,
            tx,
        }
    }

    pub fn request_primary(&mut self, kind: RefreshKind, target: RefreshTarget) {
        if target
            .private_key
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            let _ = self.tx.send(RefreshEvent::Primary {
                generation: self.primary_generation,
                target,
                result: Err("BUZZ_PRIVATE_KEY is required".to_string()),
            });
            return;
        }
        self.primary_generation = self.primary_generation.saturating_add(1);
        if let Some(task) = self.primary_task.take() {
            task.abort();
        }
        let generation = self.primary_generation;
        let tx = self.tx.clone();
        self.primary_task = Some(tokio::spawn(async move {
            let result = run_primary_refresh(&kind, &target).await;
            let _ = tx.send(RefreshEvent::Primary {
                generation,
                target,
                result,
            });
        }));
    }

    pub fn request_hydrate(&mut self, target: HydrateTarget) {
        if target
            .private_key
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            return;
        }
        if target.author_pubkeys.is_empty() && target.selected_message_id.is_none() {
            return;
        }
        self.hydrate_generation = self.hydrate_generation.saturating_add(1);
        if let Some(task) = self.hydrate_task.take() {
            task.abort();
        }
        let generation = self.hydrate_generation;
        let tx = self.tx.clone();
        self.hydrate_task = Some(tokio::spawn(async move {
            let result = run_hydrate(&target).await;
            let _ = tx.send(RefreshEvent::Hydrate {
                generation,
                target,
                result,
            });
        }));
    }

    pub fn is_current_primary(&self, generation: u64) -> bool {
        generation == self.primary_generation
    }

    pub fn is_current_hydrate(&self, generation: u64) -> bool {
        generation == self.hydrate_generation
    }

    pub fn stop(&mut self) {
        if let Some(task) = self.primary_task.take() {
            task.abort();
        }
        if let Some(task) = self.hydrate_task.take() {
            task.abort();
        }
    }
}

impl Drop for RefreshRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run_primary_refresh(
    kind: &RefreshKind,
    target: &RefreshTarget,
) -> Result<RefreshResult, String> {
    let client = client_for(target)?;
    let sidebar = if *kind == RefreshKind::Full {
        Some(fetch_sidebar(&client, target.channel_scope).await)
    } else {
        None
    };

    let mut active_channel_id = target.active_channel_id.clone();
    if active_channel_id.is_none() {
        active_channel_id = sidebar
            .as_ref()
            .and_then(|sidebar| sidebar.as_ref().ok())
            .and_then(|sidebar| sidebar.channels.get(target.selected_channel))
            .map(|channel| channel.id.clone());
    }

    let read_state = if *kind == RefreshKind::Full {
        client.read_state().await.ok()
    } else {
        None
    };
    let starred_channel_ids = if *kind == RefreshKind::Full {
        client
            .channel_preference_ids(ChannelPreferenceKind::Stars)
            .await
            .ok()
    } else {
        None
    };
    let muted_channel_ids = if *kind == RefreshKind::Full {
        client
            .channel_preference_ids(ChannelPreferenceKind::Mutes)
            .await
            .ok()
    } else {
        None
    };
    let channel_sections = if *kind == RefreshKind::Full {
        client.channel_sections().await.ok()
    } else {
        None
    };
    let (channel_detail, channel_members) = match active_channel_id.as_deref() {
        Some(channel_id) if *kind == RefreshKind::Full => (
            Some(fetch_channel_detail(&client, channel_id).await),
            Some(fetch_channel_members(&client, channel_id).await),
        ),
        _ => (None, None),
    };

    let messages = match active_channel_id.as_deref() {
        Some(channel_id) => Some(fetch_messages(&client, channel_id, target).await),
        None => None,
    };
    let feed = Some(fetch_feed(&client, target.feed_type).await);

    let mut author_pubkeys = BTreeSet::new();
    if let Some(Ok(messages)) = &messages {
        collect_author_pubkeys(messages, &mut author_pubkeys);
    }
    if let Some(Ok(feed)) = &feed {
        collect_author_pubkeys(feed, &mut author_pubkeys);
    }
    for known in &target.known_author_pubkeys {
        author_pubkeys.remove(known);
    }
    let profiles = if author_pubkeys.is_empty() {
        Vec::new()
    } else {
        client
            .user_profiles(&author_pubkeys.into_iter().collect::<Vec<_>>())
            .await
            .unwrap_or_default()
    };

    let reaction_event_id = target
        .selected_message_id
        .as_deref()
        .filter(|event_id| !event_id.is_empty())
        .map(str::to_string)
        .or_else(|| {
            (target.focus == Focus::Timeline)
                .then(|| {
                    messages
                        .as_ref()
                        .and_then(|messages| messages.as_ref().ok())
                        .and_then(|messages| messages.last())
                        .map(|message| message.id.clone())
                })
                .flatten()
                .filter(|event_id| !event_id.is_empty())
        });
    let reactions = match reaction_event_id.as_deref() {
        Some(event_id) => Some(
            client
                .query_reactions(event_id)
                .await
                .map_err(|error| error.to_string()),
        ),
        None => None,
    };

    Ok(RefreshResult {
        sidebar,
        read_state,
        starred_channel_ids,
        muted_channel_ids,
        channel_sections,
        channel_detail_id: active_channel_id.clone(),
        channel_detail,
        channel_members,
        message_channel_id: active_channel_id,
        messages,
        feed,
        profiles,
        reaction_event_id,
        reactions,
    })
}

async fn run_hydrate(target: &HydrateTarget) -> HydrateResult {
    let Some(private_key) = target.private_key.as_deref() else {
        return HydrateResult::default();
    };
    let Ok(client) = TuiRelayClient::new(&target.relay_url, private_key, target.auth_tag.clone())
    else {
        return HydrateResult::default();
    };

    let mut author_pubkeys = target.author_pubkeys.clone();
    for known in &target.known_author_pubkeys {
        author_pubkeys.remove(known);
    }
    let profiles = if author_pubkeys.is_empty() {
        Vec::new()
    } else {
        client
            .user_profiles(&author_pubkeys.into_iter().collect::<Vec<_>>())
            .await
            .unwrap_or_default()
    };
    let reactions = match target
        .selected_message_id
        .as_deref()
        .filter(|event_id| !event_id.is_empty())
    {
        Some(event_id) => Some(
            client
                .query_reactions(event_id)
                .await
                .map_err(|error| error.to_string()),
        ),
        None => None,
    };

    HydrateResult {
        profiles,
        reactions,
    }
}

fn client_for(target: &RefreshTarget) -> Result<TuiRelayClient, String> {
    let private_key = target
        .private_key
        .as_deref()
        .ok_or_else(|| "BUZZ_PRIVATE_KEY is required".to_string())?;
    TuiRelayClient::new(&target.relay_url, private_key, target.auth_tag.clone())
        .map_err(|error| error.to_string())
}

async fn fetch_sidebar(
    client: &TuiRelayClient,
    channel_scope: ChannelScope,
) -> Result<SidebarData, String> {
    match channel_scope {
        ChannelScope::Conversations => {
            let mut channels = client
                .list_channels(true)
                .await
                .map_err(|error| error.to_string())?;
            let mut warning = None;
            match client.list_dms(50).await {
                Ok(mut dms) => channels.append(&mut dms),
                Err(error) => warning = Some(format!("dms: {error}")),
            }
            Ok(SidebarData { channels, warning })
        }
        ChannelScope::OpenChannels => client
            .list_channels(false)
            .await
            .map(|channels| SidebarData {
                channels,
                warning: None,
            })
            .map_err(|error| error.to_string()),
    }
}

async fn fetch_channel_detail(
    client: &TuiRelayClient,
    channel_id: &str,
) -> Result<Option<Channel>, String> {
    client
        .channel(channel_id)
        .await
        .map_err(|error| error.to_string())
}

async fn fetch_channel_members(
    client: &TuiRelayClient,
    channel_id: &str,
) -> Result<Vec<ChannelMember>, String> {
    client
        .channel_members(channel_id)
        .await
        .map_err(|error| error.to_string())
}

async fn fetch_messages(
    client: &TuiRelayClient,
    channel_id: &str,
    target: &RefreshTarget,
) -> Result<Vec<Message>, String> {
    let result = if let Some(thread_root) = &target.thread_root {
        client
            .query_messages(&[
                TuiRelayClient::thread_filter(channel_id, thread_root, 120),
                TuiRelayClient::event_id_filter(thread_root),
            ])
            .await
    } else {
        client
            .query_messages(&[TuiRelayClient::channel_history_filter(channel_id, 80)])
            .await
    };
    result
        .map(|messages| messages.into_iter().map(Message::from).collect())
        .map_err(|error| error.to_string())
}

async fn fetch_feed(
    client: &TuiRelayClient,
    feed_type: Option<&'static str>,
) -> Result<Vec<Message>, String> {
    client
        .query_messages(&[TuiRelayClient::feed_filter(
            client.public_key(),
            feed_type,
            50,
        )])
        .await
        .map(|mut messages| {
            messages.sort_by_key(|message| std::cmp::Reverse(message.created_at));
            messages.into_iter().map(Message::from).collect()
        })
        .map_err(|error| error.to_string())
}

fn collect_author_pubkeys(messages: &[Message], pubkeys: &mut BTreeSet<String>) {
    for message in messages {
        let pubkey = message.pubkey.trim();
        if !pubkey.is_empty() {
            pubkeys.insert(pubkey.to_string());
        }
    }
}
