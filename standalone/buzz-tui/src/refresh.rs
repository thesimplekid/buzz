use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::app::{ChannelScope, Focus, TimelineMode};
use crate::client::{
    timeline_profile_pubkeys, AgentTurnMetric, Channel, ChannelMember, ChannelPreferenceKind,
    ChannelSections, Message, Reaction, ReadState, RelayAgentInfo, Reminder, ThreadSummary,
    TuiRelayClient, UserProfile, WindowCursor, CHANNEL_WINDOW_HEAD_LIMIT,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshKind {
    Full,
    Active,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshTarget {
    pub relay_url: String,
    pub private_key: Option<String>,
    pub auth_tag: Option<String>,
    pub channel_scope: ChannelScope,
    pub selected_channel: usize,
    pub active_channel_id: Option<String>,
    pub active_channel_type: Option<String>,
    pub thread_root: Option<String>,
    pub timeline_mode: TimelineMode,
    pub focus: Focus,
    pub feed_type: Option<&'static str>,
    pub selected_message_id: Option<String>,
    pub known_author_pubkeys: BTreeSet<String>,
    pub read_frontiers: BTreeMap<String, u64>,
    pub channel_ids: Vec<String>,
}

#[derive(Debug)]
pub enum RefreshEvent {
    Primary {
        generation: u64,
        target: RefreshTarget,
        result: Box<Result<RefreshResult, String>>,
    },
    Hydrate {
        generation: u64,
        target: HydrateTarget,
        result: HydrateResult,
    },
}

#[derive(Debug, Default)]
pub struct RefreshResult {
    pub sidebar: Option<Result<SidebarData, String>>,
    pub read_state: Option<ReadState>,
    pub unread_counts: Option<BTreeMap<String, u64>>,
    pub starred_channel_ids: Option<BTreeSet<String>>,
    pub muted_channel_ids: Option<BTreeSet<String>>,
    pub channel_sections: Option<ChannelSections>,
    pub channel_detail_id: Option<String>,
    pub channel_detail: Option<Result<Option<Channel>, String>>,
    pub channel_members: Option<Result<Vec<ChannelMember>, String>>,
    pub message_channel_id: Option<String>,
    pub messages: Option<Result<Vec<Message>, String>>,
    pub channel_window: Option<ChannelWindowMeta>,
    pub feed: Option<Result<Vec<Message>, String>>,
    pub reminders: Option<Result<Vec<Reminder>, String>>,
    pub profiles: Vec<UserProfile>,
    pub reaction_event_id: Option<String>,
    pub reactions: Option<Result<Vec<Reaction>, String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadStateSyncTarget {
    pub relay_url: String,
    pub private_key: Option<String>,
    pub auth_tag: Option<String>,
    pub client_id: String,
    pub slot_id: String,
    pub contexts: BTreeMap<String, u64>,
}

#[derive(Debug)]
pub struct ReadStateSyncEvent {
    pub target: ReadStateSyncTarget,
    pub result: Result<(), String>,
}

/// Window overlay state fetched with a head channel window (NIP-CW).
#[derive(Debug)]
pub struct ChannelWindowMeta {
    pub active: bool,
    pub summaries: BTreeMap<String, ThreadSummary>,
    pub has_more: bool,
    pub next_cursor: Option<WindowCursor>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRefreshTarget {
    pub relay_url: String,
    pub private_key: Option<String>,
    pub auth_tag: Option<String>,
}

#[derive(Debug)]
pub struct AgentRefreshEvent {
    pub generation: u64,
    pub target: AgentRefreshTarget,
    pub result: Result<AgentRefreshResult, String>,
}

#[derive(Debug)]
pub struct AgentRefreshResult {
    pub agents: Result<Vec<RelayAgentInfo>, String>,
    pub metrics: Result<Vec<AgentTurnMetric>, String>,
}

pub struct AgentRefreshRuntime {
    generation: u64,
    target: Option<AgentRefreshTarget>,
    task: Option<JoinHandle<()>>,
    tx: UnboundedSender<AgentRefreshEvent>,
}

impl AgentRefreshRuntime {
    pub fn new(tx: UnboundedSender<AgentRefreshEvent>) -> Self {
        Self {
            generation: 0,
            target: None,
            task: None,
            tx,
        }
    }

    pub fn request(&mut self, target: AgentRefreshTarget) {
        if self.task.as_ref().is_some_and(|task| !task.is_finished())
            && self.target.as_ref() == Some(&target)
        {
            return;
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.generation = self.generation.saturating_add(1);
        self.target = Some(target.clone());
        let generation = self.generation;
        let tx = self.tx.clone();
        self.task = Some(tokio::spawn(async move {
            let result = run_agent_refresh(&target).await;
            let _ = tx.send(AgentRefreshEvent {
                generation,
                target,
                result,
            });
        }));
    }

    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }

    pub fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.target = None;
    }
}

impl Drop for AgentRefreshRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct RefreshRuntime {
    primary_generation: u64,
    hydrate_generation: u64,
    primary_request: Option<(RefreshKind, RefreshTarget)>,
    primary_task: Option<JoinHandle<()>>,
    hydrate_task: Option<JoinHandle<()>>,
    tx: UnboundedSender<RefreshEvent>,
}

impl RefreshRuntime {
    pub fn new(tx: UnboundedSender<RefreshEvent>) -> Self {
        Self {
            primary_generation: 0,
            hydrate_generation: 0,
            primary_request: None,
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
                result: Box::new(Err("BUZZ_PRIVATE_KEY is required".to_string())),
            });
            return;
        }
        if self
            .primary_task
            .as_ref()
            .is_some_and(|task| !task.is_finished())
            && self
                .primary_request
                .as_ref()
                .is_some_and(|(current_kind, current_target)| {
                    current_kind == &kind && current_target == &target
                })
        {
            return;
        }
        self.primary_generation = self.primary_generation.saturating_add(1);
        if let Some(task) = self.primary_task.take() {
            task.abort();
        }
        self.primary_request = Some((kind.clone(), target.clone()));
        let generation = self.primary_generation;
        let tx = self.tx.clone();
        self.primary_task = Some(tokio::spawn(async move {
            if kind == RefreshKind::Active {
                let result = run_active_timeline_refresh(&target).await;
                let _ = tx.send(RefreshEvent::Primary {
                    generation,
                    target: target.clone(),
                    result: Box::new(result),
                });
                let result = run_active_auxiliary_refresh(&target).await;
                let _ = tx.send(RefreshEvent::Primary {
                    generation,
                    target,
                    result: Box::new(result),
                });
            } else {
                let result = run_primary_refresh(&kind, &target).await;
                let _ = tx.send(RefreshEvent::Primary {
                    generation,
                    target,
                    result: Box::new(result),
                });
            }
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
        self.primary_request = None;
    }
}

impl Drop for RefreshRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run_agent_refresh(target: &AgentRefreshTarget) -> Result<AgentRefreshResult, String> {
    let private_key = target
        .private_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| "BUZZ_PRIVATE_KEY is required".to_string())?;
    let client = TuiRelayClient::new(
        target.relay_url.clone(),
        private_key,
        target.auth_tag.clone(),
    )
    .map_err(|error| error.to_string())?;
    let (agents, metrics) = tokio::join!(client.relay_agents(), client.agent_turn_metrics(100));
    Ok(AgentRefreshResult {
        agents: agents.map_err(|error| error.to_string()),
        metrics: metrics.map_err(|error| error.to_string()),
    })
}

pub struct ReadStateSyncRuntime {
    task: Option<JoinHandle<()>>,
    queued: Option<ReadStateSyncTarget>,
    tx: UnboundedSender<ReadStateSyncEvent>,
}

impl ReadStateSyncRuntime {
    pub fn new(tx: UnboundedSender<ReadStateSyncEvent>) -> Self {
        Self {
            task: None,
            queued: None,
            tx,
        }
    }

    /// Serialize read-state writes and retain only the newest queued snapshot.
    /// A write already accepted by the relay is never cancelled mid-flight.
    pub fn request(&mut self, target: ReadStateSyncTarget) {
        if self.task.is_some() {
            self.queued = Some(target);
            return;
        }
        self.start(target);
    }

    pub fn complete(&mut self) {
        self.task.take();
        if let Some(target) = self.queued.take() {
            self.start(target);
        }
    }

    pub fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.queued = None;
    }

    fn start(&mut self, target: ReadStateSyncTarget) {
        let tx = self.tx.clone();
        self.task = Some(tokio::spawn(async move {
            let result = sync_read_state(&target).await;
            let _ = tx.send(ReadStateSyncEvent { target, result });
        }));
    }
}

impl Drop for ReadStateSyncRuntime {
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
    let unread_counts = if target.channel_scope == ChannelScope::Conversations {
        let mut frontiers = target.read_frontiers.clone();
        if let Some(read_state) = &read_state {
            for (context, timestamp) in &read_state.contexts {
                let frontier = frontiers.entry(context.clone()).or_default();
                *frontier = (*frontier).max(*timestamp);
            }
        }
        let channel_ids = sidebar
            .as_ref()
            .and_then(|sidebar| sidebar.as_ref().ok())
            .map(|sidebar| {
                sidebar
                    .channels
                    .iter()
                    .map(|channel| channel.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| target.channel_ids.clone());
        let channel_frontiers = channel_ids
            .into_iter()
            .map(|channel_id| {
                let frontier = frontiers.get(&channel_id).copied().unwrap_or_default();
                (channel_id, frontier)
            })
            .collect();
        client
            .channel_unread_counts(channel_frontiers, &frontiers)
            .await
            .ok()
    } else {
        Some(BTreeMap::new())
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

    let (messages, channel_window) = match active_channel_id.as_deref() {
        Some(channel_id) => {
            let (messages, window) = fetch_messages(&client, channel_id, target).await;
            (Some(messages), window)
        }
        None => (None, None),
    };
    let feed = Some(fetch_feed(&client, target.feed_type).await);
    let reminders = Some(
        client
            .fetch_reminders()
            .await
            .map_err(|error| error.to_string()),
    );

    let mut author_pubkeys = BTreeSet::new();
    if let Some(Ok(messages)) = &messages {
        collect_author_pubkeys(messages, &mut author_pubkeys);
    }
    if let Some(Ok(feed)) = &feed {
        collect_author_pubkeys(feed, &mut author_pubkeys);
    }
    if let Some(Ok(Some(channel))) = &channel_detail {
        if !channel.owner_pubkey.trim().is_empty() {
            author_pubkeys.insert(channel.owner_pubkey.clone());
        }
        author_pubkeys.extend(channel.participant_pubkeys.iter().cloned());
    }
    if let Some(Ok(sidebar)) = &sidebar {
        author_pubkeys.extend(
            sidebar
                .channels
                .iter()
                .flat_map(|channel| channel.participant_pubkeys.iter().cloned()),
        );
    }
    if let Some(Ok(members)) = &channel_members {
        author_pubkeys.extend(
            members
                .iter()
                .map(|member| member.pubkey.trim())
                .filter(|pubkey| !pubkey.is_empty())
                .map(str::to_string),
        );
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
        unread_counts,
        starred_channel_ids,
        muted_channel_ids,
        channel_sections,
        channel_detail_id: active_channel_id.clone(),
        channel_detail,
        channel_members,
        message_channel_id: active_channel_id,
        messages,
        channel_window,
        feed,
        reminders,
        profiles,
        reaction_event_id,
        reactions,
    })
}

async fn run_active_timeline_refresh(target: &RefreshTarget) -> Result<RefreshResult, String> {
    let client = client_for(target)?;
    let Some(channel_id) = target.active_channel_id.as_deref() else {
        return Ok(RefreshResult::default());
    };
    let (messages, channel_window) = fetch_messages(&client, channel_id, target).await;
    Ok(RefreshResult {
        message_channel_id: Some(channel_id.to_string()),
        messages: Some(messages),
        channel_window,
        ..RefreshResult::default()
    })
}

async fn run_active_auxiliary_refresh(target: &RefreshTarget) -> Result<RefreshResult, String> {
    let client = client_for(target)?;
    let unread = async {
        if target.channel_scope != ChannelScope::Conversations {
            return Some(BTreeMap::new());
        }
        let channel_frontiers = target
            .channel_ids
            .iter()
            .map(|channel_id| {
                let frontier = target
                    .read_frontiers
                    .get(channel_id)
                    .copied()
                    .unwrap_or_default();
                (channel_id.clone(), frontier)
            })
            .collect();
        client
            .channel_unread_counts(channel_frontiers, &target.read_frontiers)
            .await
            .ok()
    };
    let reminders = async {
        client
            .fetch_reminders()
            .await
            .map_err(|error| error.to_string())
    };
    let (unread_counts, feed, reminders) =
        tokio::join!(unread, fetch_feed(&client, target.feed_type), reminders,);

    let mut author_pubkeys = BTreeSet::new();
    if let Ok(feed) = &feed {
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

    Ok(RefreshResult {
        unread_counts,
        feed: Some(feed),
        reminders: Some(reminders),
        profiles,
        ..RefreshResult::default()
    })
}

async fn sync_read_state(target: &ReadStateSyncTarget) -> Result<(), String> {
    let private_key = target
        .private_key
        .as_deref()
        .ok_or_else(|| "BUZZ_PRIVATE_KEY is required".to_string())?;
    if target.contexts.is_empty() {
        return Ok(());
    }
    let client = TuiRelayClient::new(&target.relay_url, private_key, target.auth_tag.clone())
        .map_err(|error| error.to_string())?;
    let remote_created_at = client
        .read_state_slot_created_at(&target.slot_id)
        .await
        .map_err(|error| error.to_string())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let event = client
        .build_read_state_event(
            &target.client_id,
            &target.slot_id,
            target.contexts.clone(),
            Some(now.max(remote_created_at.saturating_add(1))),
        )
        .map_err(|error| error.to_string())?;
    client
        .submit_event(&event)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
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
            let channels = client
                .list_channels(true)
                .await
                .map_err(|error| error.to_string())?;
            Ok(SidebarData {
                channels,
                warning: None,
            })
        }
        ChannelScope::OpenChannels => {
            let channels = client
                .list_channels(false)
                .await
                .map_err(|error| error.to_string())?;
            Ok(SidebarData {
                channels,
                warning: None,
            })
        }
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
) -> (Result<Vec<Message>, String>, Option<ChannelWindowMeta>) {
    if let Some(thread_root) = &target.thread_root {
        let result = client
            .query_messages(&[
                TuiRelayClient::thread_filter(channel_id, thread_root, 120),
                TuiRelayClient::event_id_filter(thread_root),
            ])
            .await
            .map(|messages| messages.into_iter().map(Message::from).collect())
            .map_err(|error| error.to_string());
        return (result, None);
    }

    // Forums have a distinct post/comment model on desktop; do not route
    // them through the stream-oriented NIP-CW top-level window.
    if target.active_channel_type.as_deref() != Some("forum") {
        if let Ok(page) = client
            .channel_window(channel_id, CHANNEL_WINDOW_HEAD_LIMIT, None)
            .await
        {
            if page.valid {
                let meta = ChannelWindowMeta {
                    active: true,
                    summaries: page.summaries,
                    has_more: page.has_more,
                    next_cursor: page.next_cursor,
                };
                return (Ok(page.messages), Some(meta));
            }
        }
    }
    let result = client
        .query_messages(&[TuiRelayClient::channel_history_filter(
            channel_id,
            CHANNEL_WINDOW_HEAD_LIMIT,
        )])
        .await
        .map(|messages| messages.into_iter().map(Message::from).collect())
        .map_err(|error| error.to_string());
    (
        result,
        Some(ChannelWindowMeta {
            active: false,
            summaries: BTreeMap::new(),
            has_more: false,
            next_cursor: None,
        }),
    )
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
        pubkeys.extend(timeline_profile_pubkeys(
            message.kind,
            &message.pubkey,
            &message.content,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refresh_target() -> RefreshTarget {
        RefreshTarget {
            relay_url: "http://localhost:3000".to_string(),
            private_key: Some("test-key".to_string()),
            auth_tag: None,
            channel_scope: ChannelScope::Conversations,
            selected_channel: 0,
            active_channel_id: Some("channel".to_string()),
            active_channel_type: None,
            thread_root: None,
            timeline_mode: TimelineMode::Channel,
            focus: Focus::Timeline,
            feed_type: None,
            selected_message_id: None,
            known_author_pubkeys: BTreeSet::new(),
            read_frontiers: BTreeMap::new(),
            channel_ids: vec!["channel".to_string()],
        }
    }

    #[tokio::test]
    async fn primary_refresh_deduplicates_identical_in_flight_request() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut runtime = RefreshRuntime::new(tx);
        let target = refresh_target();
        runtime.primary_generation = 7;
        runtime.primary_request = Some((RefreshKind::Full, target.clone()));
        runtime.primary_task = Some(tokio::spawn(std::future::pending()));

        runtime.request_primary(RefreshKind::Full, target);

        assert_eq!(runtime.primary_generation, 7);
        assert!(runtime
            .primary_task
            .as_ref()
            .is_some_and(|task| !task.is_finished()));
        runtime.stop();
    }

    fn agent_target() -> AgentRefreshTarget {
        AgentRefreshTarget {
            relay_url: "http://localhost:3000".to_string(),
            private_key: None,
            auth_tag: None,
        }
    }

    #[tokio::test]
    async fn agent_refresh_request_returns_before_relay_work_completes() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut runtime = AgentRefreshRuntime::new(tx);

        runtime.request(agent_target());

        assert!(runtime.task.is_some());
        let event = rx.recv().await.expect("agent refresh event");
        assert!(runtime.is_current(event.generation));
        assert_eq!(event.target, agent_target());
        assert_eq!(event.result.unwrap_err(), "BUZZ_PRIVATE_KEY is required");
    }

    fn read_target(value: u64) -> ReadStateSyncTarget {
        ReadStateSyncTarget {
            relay_url: "http://localhost:3000".to_string(),
            private_key: None,
            auth_tag: None,
            client_id: "client".to_string(),
            slot_id: "slot".to_string(),
            contexts: BTreeMap::from([("channel".to_string(), value)]),
        }
    }

    #[tokio::test]
    async fn read_state_runtime_coalesces_queued_snapshots() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut runtime = ReadStateSyncRuntime::new(tx);

        runtime.request(read_target(1));
        runtime.request(read_target(2));
        runtime.request(read_target(3));

        assert_eq!(
            runtime
                .queued
                .as_ref()
                .and_then(|target| target.contexts.get("channel")),
            Some(&3)
        );
        runtime.stop();
    }
}
