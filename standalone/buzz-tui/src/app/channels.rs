use std::cmp::Ordering;

use crate::app::{App, Focus, TimelineMode};
use crate::client::app_data::{msg_context_key, thread_context_key};
use crate::client::is_conversational_unread_kind;
use crate::client::{Channel, ChannelPreferenceKind, ConversationKind};

impl App {
    pub(super) fn active_workspace_id(&self) -> &str {
        self.workspace_config.active_id.as_str()
    }

    fn draft_key(&self, channel_id: &str) -> String {
        let thread = self.thread_root.as_deref().unwrap_or("channel");
        format!("{}::{channel_id}::{thread}", self.active_workspace_id())
    }

    pub(super) fn save_active_channel_draft(&mut self) {
        if self.edit_target.is_some() || self.timeline_mode == TimelineMode::Pulse {
            return;
        }
        let Some(channel) = self.active_channel() else {
            return;
        };
        let key = self.draft_key(&channel.id);
        if self.composer.is_empty() {
            if self.channel_drafts.remove(&key).is_some() {
                self.drafts_dirty = true;
            }
            if self.channel_draft_mentions.remove(&key).is_some() {
                self.drafts_dirty = true;
            }
        } else if self
            .channel_drafts
            .insert(key.clone(), self.composer.clone())
            .as_deref()
            != Some(self.composer.as_str())
        {
            self.drafts_dirty = true;
        }
        let mentions = self.composer_mention_refs_for_content(&self.composer);
        if mentions.is_empty() {
            if self.channel_draft_mentions.remove(&key).is_some() {
                self.drafts_dirty = true;
            }
        } else if self.channel_draft_mentions.get(&key) != Some(&mentions) {
            self.channel_draft_mentions.insert(key, mentions);
            self.drafts_dirty = true;
        }
    }

    pub(super) fn restore_active_channel_draft(&mut self) {
        if self.edit_target.is_some() || self.timeline_mode == TimelineMode::Pulse {
            return;
        }
        let Some(channel) = self.active_channel() else {
            return;
        };
        self.composer = self
            .channel_drafts
            .get(&self.draft_key(&channel.id))
            .cloned()
            .unwrap_or_default();
        self.composer_mentions = self
            .channel_draft_mentions
            .get(&self.draft_key(&channel.id))
            .cloned()
            .unwrap_or_default();
        self.composer_cursor = self.composer.len();
    }

    pub(super) fn clear_channel_draft(&mut self, channel_id: &str) {
        let key = self.draft_key(channel_id);
        if self.channel_drafts.remove(&key).is_some() {
            self.drafts_dirty = true;
        }
        if self.channel_draft_mentions.remove(&key).is_some() {
            self.drafts_dirty = true;
        }
    }

    pub fn has_channel_draft(&self, channel_id: &str) -> bool {
        let prefix = format!("{}::{channel_id}::", self.active_workspace_id());
        self.channel_drafts
            .iter()
            .any(|(key, value)| key.starts_with(&prefix) && !value.is_empty())
    }

    pub fn channel_has_unread(&self, channel_id: &str) -> bool {
        let workspace_id = self.active_workspace_id();
        if self
            .workspace_config
            .manual_unread
            .get(workspace_id)
            .is_some_and(|channels| channels.contains(channel_id))
        {
            return true;
        }
        if let Some(count) = self.channel_unread_counts.get(channel_id) {
            return *count > 0;
        }

        let frontiers = self.workspace_config.read_frontiers.get(workspace_id);
        let channel_frontier = frontiers
            .and_then(|channels| channels.get(channel_id))
            .copied()
            .unwrap_or_default();

        let has_loaded_channel_messages = self
            .messages
            .iter()
            .any(|message| message.channel_id == channel_id);
        let is_loaded_channel_active = self
            .active_channel_id
            .as_deref()
            .is_some_and(|active_id| active_id == channel_id)
            || self
                .thread_context
                .as_ref()
                .is_some_and(|context| context.channel_id == channel_id);
        if has_loaded_channel_messages && is_loaded_channel_active {
            return self.messages.iter().any(|message| {
                message.channel_id == channel_id
                    && !self.is_own_pubkey(&message.pubkey)
                    && is_conversational_unread_kind(message.kind)
                    && message.created_at
                        > self.effective_read_frontier_for_message(
                            frontiers,
                            channel_frontier,
                            message,
                        )
            });
        }

        let Some(latest) = self.channel_latest_seen.get(channel_id).copied() else {
            return false;
        };
        latest > channel_frontier
    }

    pub fn channel_unread_count(&self, channel_id: &str) -> Option<u64> {
        self.channel_unread_counts
            .get(channel_id)
            .copied()
            .filter(|count| *count > 0)
    }

    /// Index of the first message that was unread when this channel or thread
    /// was opened. The snapshot survives the immediate mark-read update.
    pub fn first_unread_message_index(&self) -> Option<usize> {
        if self.timeline_mode != TimelineMode::Channel {
            return None;
        }
        let snapshot = self.unread_snapshot.as_ref()?;
        if snapshot.suppressed || snapshot.thread_root != self.thread_root {
            return None;
        }
        let channel = self.active_channel()?;
        if channel.id != snapshot.channel_id {
            return None;
        }
        let channel_frontier = snapshot
            .frontiers
            .get(&snapshot.channel_id)
            .copied()
            .unwrap_or_default();

        self.messages.iter().position(|message| {
            message.channel_id == snapshot.channel_id
                && !self.is_own_pubkey(&message.pubkey)
                && snapshot
                    .thread_root
                    .as_deref()
                    .is_none_or(|root_id| message.id != root_id)
                && is_conversational_unread_kind(message.kind)
                && message.created_at
                    > self.effective_read_frontier_for_message(
                        Some(&snapshot.frontiers),
                        channel_frontier,
                        message,
                    )
        })
    }

    fn effective_read_frontier_for_message(
        &self,
        frontiers: Option<&std::collections::BTreeMap<String, u64>>,
        channel_frontier: u64,
        message: &crate::client::Message,
    ) -> u64 {
        let mut frontier = channel_frontier;
        let root_id = message
            .thread_root_id
            .as_deref()
            .filter(|root_id| !root_id.is_empty())
            .or_else(|| (!message.id.is_empty()).then_some(message.id.as_str()));
        if let Some(root_id) = root_id {
            frontier = frontier.max(
                frontiers
                    .and_then(|contexts| contexts.get(&thread_context_key(root_id)))
                    .copied()
                    .unwrap_or_default(),
            );
        }
        if !message.id.is_empty() {
            frontier = frontier.max(
                frontiers
                    .and_then(|contexts| contexts.get(&msg_context_key(&message.id)))
                    .copied()
                    .unwrap_or_default(),
            );
        }
        frontier
    }

    pub fn channel_is_starred(&self, channel_id: &str) -> bool {
        self.starred_channel_ids.contains(channel_id)
    }

    pub fn channel_is_muted(&self, channel_id: &str) -> bool {
        self.muted_channel_ids.contains(channel_id)
    }

    pub fn copy_selected_channel_id_to_clipboard(&mut self) {
        let Some(channel_id) = self.selected_sidebar_channel_id().map(str::to_owned) else {
            self.status = "No channel selected".to_string();
            return;
        };
        match crate::clipboard::copy_text(&channel_id) {
            Ok(()) => {
                self.status = format!("Copied channel ID {channel_id} to clipboard");
            }
            Err(error) => {
                self.status = format!("copy channel ID: {error}");
            }
        }
    }

    fn selected_sidebar_channel_id(&self) -> Option<&str> {
        self.channels
            .get(self.selected_channel)
            .map(|channel| channel.id.as_str())
    }

    pub fn channel_section_name(&self, channel_id: &str) -> Option<&str> {
        let section_id = self.channel_section_assignments.get(channel_id)?;
        self.channel_sections
            .iter()
            .find(|section| section.id == *section_id)
            .map(|section| section.name.as_str())
    }

    /// Resolve a generic relay DM name to the other participants' human labels.
    pub fn channel_display_label(&self, channel: &Channel) -> String {
        if channel.kind != ConversationKind::DirectMessage || !is_generic_dm_name(&channel.name) {
            return channel.name.clone();
        }

        let own_pubkey = self.own_pubkey_hex();
        let mut labels = channel
            .participant_pubkeys
            .iter()
            .filter(|pubkey| {
                own_pubkey
                    .as_deref()
                    .is_none_or(|own| !pubkey.eq_ignore_ascii_case(own))
            })
            .map(|pubkey| self.author_label(pubkey))
            .fold(Vec::<String>::new(), |mut labels, label| {
                if !labels
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(&label))
                {
                    labels.push(label);
                }
                labels
            });

        if labels.is_empty() {
            return channel.name.clone();
        }
        let hidden = labels.len().saturating_sub(3);
        labels.truncate(3);
        if hidden > 0 {
            labels.push(format!("+{hidden} more"));
        }
        labels.join(", ")
    }

    pub async fn toggle_selected_channel_read_marker(&mut self) {
        if self.focus != Focus::Sidebar {
            return;
        }
        let Some(channel) = self.active_channel() else {
            return;
        };
        if self.channel_has_unread(&channel.id) {
            let latest = self
                .channel_latest_seen
                .get(&channel.id)
                .copied()
                .unwrap_or_else(|| self.latest_active_message_at());
            self.mark_channel_read_at(&channel.id, latest);
            self.status = format!("Marked {} read", channel.name);
        } else {
            self.mark_channel_unread(&channel.id);
            self.status = format!("Marked {} unread", channel.name);
        }
    }

    pub async fn toggle_selected_channel_star(&mut self) {
        self.toggle_selected_channel_preference(ChannelPreferenceKind::Stars)
            .await;
    }

    pub async fn toggle_selected_channel_mute(&mut self) {
        self.toggle_selected_channel_preference(ChannelPreferenceKind::Mutes)
            .await;
    }

    async fn toggle_selected_channel_preference(&mut self, kind: ChannelPreferenceKind) {
        if self.focus != Focus::Sidebar {
            return;
        }
        let Some(channel) = self.active_channel() else {
            return;
        };
        let enabled = !self.preference_ids(kind).contains(&channel.id);
        let result = self
            .publish_channel_preference_native(kind, &channel.id, enabled)
            .await;
        match result {
            Ok(_) => {
                self.set_local_channel_preference(kind, &channel.id, enabled);
                let label = match (kind, enabled) {
                    (ChannelPreferenceKind::Stars, true) => "Starred",
                    (ChannelPreferenceKind::Stars, false) => "Unstarred",
                    (ChannelPreferenceKind::Mutes, true) => "Muted",
                    (ChannelPreferenceKind::Mutes, false) => "Unmuted",
                };
                self.status = format!("{label} {}", channel.name);
            }
            Err(error) => self.status = format!("channel prefs: {error}"),
        }
    }

    fn preference_ids(&self, kind: ChannelPreferenceKind) -> &std::collections::BTreeSet<String> {
        match kind {
            ChannelPreferenceKind::Stars => &self.starred_channel_ids,
            ChannelPreferenceKind::Mutes => &self.muted_channel_ids,
        }
    }

    pub(super) fn set_local_channel_preference(
        &mut self,
        kind: ChannelPreferenceKind,
        channel_id: &str,
        enabled: bool,
    ) {
        let ids = match kind {
            ChannelPreferenceKind::Stars => &mut self.starred_channel_ids,
            ChannelPreferenceKind::Mutes => &mut self.muted_channel_ids,
        };
        if enabled {
            ids.insert(channel_id.to_string());
        } else {
            ids.remove(channel_id);
        }
    }

    /// Sidebar class rank: starred, sections (by section order), channels,
    /// forums, DMs, with archived channels sinking to the bottom.
    fn sidebar_group_rank(&self, channel: &Channel) -> (u8, i64) {
        if channel.archived {
            return (9, 0);
        }
        if self.channel_is_starred(&channel.id) {
            return (0, 0);
        }
        if let Some(section_id) = self.channel_section_assignments.get(&channel.id) {
            let order = self
                .channel_sections
                .iter()
                .find(|section| section.id == *section_id)
                .map(|section| section.order)
                .unwrap_or(i64::MAX);
            return (1, order);
        }
        match channel.kind {
            ConversationKind::DirectMessage => (4, 0),
            ConversationKind::Channel if channel.channel_type == "forum" => (3, 0),
            ConversationKind::Channel => (2, 0),
        }
    }

    fn compare_sidebar_channels(&self, a: &Channel, b: &Channel) -> Ordering {
        let rank_a = self.sidebar_group_rank(a);
        let rank_b = self.sidebar_group_rank(b);
        rank_a
            .cmp(&rank_b)
            .then_with(|| self.alpha_channel_order(a, b))
    }

    fn alpha_channel_order(&self, a: &Channel, b: &Channel) -> Ordering {
        self.channel_display_label(a)
            .to_lowercase()
            .cmp(&self.channel_display_label(b).to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    }

    /// Replace the sidebar data while keeping the highlight on the same
    /// conversation, even when the relay returns channels in a different order.
    pub(super) fn replace_sidebar_channels(&mut self, channels: Vec<Channel>) {
        let selected_id = self
            .channels
            .get(self.selected_channel)
            .map(|channel| channel.id.clone());
        let previous_index = self.selected_channel;
        self.channels = channels;
        self.selected_channel = selected_id
            .and_then(|id| self.channels.iter().position(|channel| channel.id == id))
            .unwrap_or(previous_index);
        super::clamp_index(&mut self.selected_channel, self.channels.len());
    }

    /// Re-order the sidebar by stable group and alphabetical label, keeping
    /// the selection on the same channel.
    pub(super) fn sort_sidebar_channels(&mut self) {
        if self.channels.len() < 2 {
            return;
        }
        let selected_id = self
            .channels
            .get(self.selected_channel)
            .map(|channel| channel.id.clone());
        let mut channels = std::mem::take(&mut self.channels);
        channels.sort_by(|a, b| self.compare_sidebar_channels(a, b));
        self.channels = channels;
        if let Some(id) = selected_id {
            if let Some(index) = self.channels.iter().position(|channel| channel.id == id) {
                self.selected_channel = index;
            }
        }
    }

    pub(super) fn remember_latest_message_for(&mut self, channel_id: &str) {
        let latest = self
            .messages
            .iter()
            .filter(|message| is_conversational_unread_kind(message.kind))
            .map(|message| message.created_at)
            .max()
            .unwrap_or_default();
        if latest > 0 {
            self.channel_latest_seen
                .insert(channel_id.to_string(), latest);
        }
    }

    pub(super) fn latest_active_message_at(&self) -> u64 {
        self.messages
            .iter()
            .map(|message| message.created_at)
            .max()
            .unwrap_or_default()
    }
}

fn is_generic_dm_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "" | "dm" | "direct message" | "direct messages" | "group dm"
    ) {
        return true;
    }
    normalized
        .strip_prefix("group dm (")
        .and_then(|suffix| suffix.strip_suffix(')'))
        .is_some_and(|count| !count.is_empty() && count.chars().all(|ch| ch.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use crate::app::tests::test_app;
    use crate::client::{Channel, ConversationKind, UserProfile};

    fn channel(id: &str, name: &str) -> Channel {
        Channel {
            id: id.to_string(),
            name: name.to_string(),
            ..Channel::default()
        }
    }

    #[test]
    fn sidebar_sorts_groups_then_alpha_by_default() {
        let mut app = test_app();
        let mut dm = channel("dm-1", "zoe");
        dm.kind = ConversationKind::DirectMessage;
        let mut forum = channel("forum-1", "ideas");
        forum.channel_type = "forum".to_string();
        let mut archived = channel("chan-3", "attic");
        archived.archived = true;
        app.channels = vec![
            channel("chan-2", "beta"),
            dm,
            archived,
            channel("chan-1", "alpha"),
            forum,
            channel("chan-4", "favorite"),
        ];
        app.starred_channel_ids.insert("chan-4".to_string());

        app.sort_sidebar_channels();

        let names: Vec<&str> = app
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["favorite", "alpha", "beta", "ideas", "zoe", "attic"]
        );
    }

    #[test]
    fn generic_dm_labels_use_other_participant_profiles_and_sort_by_them() {
        let mut app = test_app();
        let alice = "aa".repeat(32);
        let bob = "bb".repeat(32);
        app.author_profiles.insert(
            alice.clone(),
            UserProfile {
                pubkey: alice.clone(),
                display_name: "Alice".to_string(),
                ..UserProfile::default()
            },
        );
        app.author_profiles.insert(
            bob.clone(),
            UserProfile {
                pubkey: bob.clone(),
                display_name: "Bob".to_string(),
                ..UserProfile::default()
            },
        );
        let mut bob_dm = channel("dm-bob", "DM");
        bob_dm.kind = ConversationKind::DirectMessage;
        bob_dm.participant_pubkeys = vec![bob];
        let mut alice_dm = channel("dm-alice", "DM");
        alice_dm.kind = ConversationKind::DirectMessage;
        alice_dm.participant_pubkeys = vec![alice];
        app.channels = vec![bob_dm, alice_dm];

        app.sort_sidebar_channels();

        assert_eq!(app.channel_display_label(&app.channels[0]), "Alice");
        assert_eq!(app.channel_display_label(&app.channels[1]), "Bob");
    }

    #[test]
    fn custom_dm_names_are_preserved() {
        let app = test_app();
        let mut dm = channel("dm-project", "Launch crew");
        dm.kind = ConversationKind::DirectMessage;
        dm.participant_pubkeys = vec!["aa".repeat(32), "bb".repeat(32)];

        assert_eq!(app.channel_display_label(&dm), "Launch crew");
    }

    #[test]
    fn sidebar_order_does_not_change_with_message_activity() {
        let mut app = test_app();
        app.channels = vec![
            channel("chan-2", "beta"),
            channel("chan-1", "alpha"),
            channel("chan-3", "quiet"),
        ];

        app.sort_sidebar_channels();
        let before: Vec<String> = app
            .channels
            .iter()
            .map(|channel| channel.id.clone())
            .collect();

        app.channel_latest_seen.insert("chan-2".to_string(), 30);
        app.channel_latest_seen.insert("chan-1".to_string(), 10);
        app.sort_sidebar_channels();

        assert_eq!(
            app.channels
                .iter()
                .map(|channel| channel.id.clone())
                .collect::<Vec<_>>(),
            before
        );
    }

    #[test]
    fn sort_keeps_selection_on_same_channel() {
        let mut app = test_app();
        app.channels = vec![channel("chan-2", "beta"), channel("chan-1", "alpha")];
        app.selected_channel = 0;

        app.sort_sidebar_channels();

        assert_eq!(app.channels[app.selected_channel].id, "chan-2");
    }

    #[test]
    fn sidebar_copy_target_follows_highlighted_channel() {
        let mut app = test_app();
        app.channels = vec![
            channel("active-id", "active"),
            channel("selected-id", "selected"),
        ];
        app.active_channel_id = Some("active-id".to_string());
        app.selected_channel = 1;

        assert_eq!(app.selected_sidebar_channel_id(), Some("selected-id"));
    }

    #[test]
    fn sidebar_replacement_keeps_selection_across_relay_reordering() {
        let mut app = test_app();
        app.channels = vec![
            channel("chan-1", "alpha"),
            channel("chan-2", "beta"),
            channel("chan-3", "gamma"),
        ];
        app.selected_channel = 1;

        app.replace_sidebar_channels(vec![
            channel("chan-3", "gamma"),
            channel("chan-1", "alpha"),
            channel("chan-2", "beta"),
        ]);
        app.sort_sidebar_channels();

        assert_eq!(app.channels[app.selected_channel].id, "chan-2");
    }
}
