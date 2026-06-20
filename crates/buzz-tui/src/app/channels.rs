use std::cmp::Ordering;

use crate::app::{App, Focus, TimelineMode};
use crate::client::app_data::{
    msg_context_key, thread_context_key, CHANNEL_SORT_MODE_ALPHA, CHANNEL_SORT_MODE_RECENT,
};
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
        } else if self
            .channel_drafts
            .insert(key, self.composer.clone())
            .as_deref()
            != Some(self.composer.as_str())
        {
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
        self.composer_cursor = self.composer.len();
    }

    pub(super) fn clear_channel_draft(&mut self, channel_id: &str) {
        let key = self.draft_key(channel_id);
        if self.channel_drafts.remove(&key).is_some() {
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

    pub fn channel_section_name(&self, channel_id: &str) -> Option<&str> {
        let section_id = self.channel_section_assignments.get(channel_id)?;
        self.channel_sections
            .iter()
            .find(|section| section.id == *section_id)
            .map(|section| section.name.as_str())
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
            if self.publish_channel_read_at(&channel.id, latest).await {
                self.status = format!("Marked {} read", channel.name);
            }
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

    /// The desktop sort-preference group this channel belongs to.
    pub(super) fn channel_sort_group_key(&self, channel: &Channel) -> String {
        if self.channel_is_starred(&channel.id) {
            return "starred".to_string();
        }
        if let Some(section_id) = self.channel_section_assignments.get(&channel.id) {
            return format!("section:{section_id}");
        }
        if channel.kind == ConversationKind::DirectMessage {
            return "dms".to_string();
        }
        if channel.channel_type == "forum" {
            return "forums".to_string();
        }
        "channels".to_string()
    }

    pub fn channel_sort_mode_for_group(&self, group_key: &str) -> &str {
        self.channel_sort_modes
            .get(group_key)
            .map(String::as_str)
            .unwrap_or(CHANNEL_SORT_MODE_ALPHA)
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
        rank_a.cmp(&rank_b).then_with(|| {
            let mode = self.channel_sort_mode_for_group(&self.channel_sort_group_key(a));
            if mode == CHANNEL_SORT_MODE_RECENT {
                // Newest first; channels with no activity sink, alphabetically.
                match (a.last_message_at, b.last_message_at) {
                    (0, 0) => alpha_channel_order(a, b),
                    (0, _) => Ordering::Greater,
                    (_, 0) => Ordering::Less,
                    (at_a, at_b) => at_b.cmp(&at_a).then_with(|| alpha_channel_order(a, b)),
                }
            } else {
                alpha_channel_order(a, b)
            }
        })
    }

    /// Re-order the sidebar by group and each group's synced sort mode,
    /// keeping the selection on the same channel.
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

    /// Flip the sort mode of the selected channel's group between alphabetical
    /// and recent-activity, and publish it so other clients follow.
    pub async fn toggle_selected_channel_sort(&mut self) {
        if self.focus != Focus::Sidebar {
            return;
        }
        let Some(channel) = self.active_channel() else {
            self.status = "No channel selected".to_string();
            return;
        };
        let group_key = self.channel_sort_group_key(&channel);
        let mode = if self.channel_sort_mode_for_group(&group_key) == CHANNEL_SORT_MODE_RECENT {
            CHANNEL_SORT_MODE_ALPHA
        } else {
            CHANNEL_SORT_MODE_RECENT
        };
        let client = match self.native_relay_client() {
            Ok(client) => client,
            Err(error) => {
                self.status = format!("channel sort: {error}");
                return;
            }
        };
        match client.set_channel_sort_mode(&group_key, mode).await {
            Ok(store) => {
                self.channel_sort_modes = store.groups;
                self.sort_sidebar_channels();
                let label = match mode {
                    CHANNEL_SORT_MODE_RECENT => "recent activity",
                    _ => "alphabetical",
                };
                self.status = format!("Sorting {group_key} by {label}");
            }
            Err(error) => self.status = format!("channel sort: {error}"),
        }
    }

    pub(super) fn remember_latest_message_for(&mut self, channel_id: &str) {
        let latest = self.latest_active_message_at();
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

fn alpha_channel_order(a: &Channel, b: &Channel) -> Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.id.cmp(&b.id))
}

#[cfg(test)]
mod tests {
    use crate::app::tests::test_app;
    use crate::client::{Channel, ConversationKind};

    fn channel(id: &str, name: &str, last_message_at: u64) -> Channel {
        Channel {
            id: id.to_string(),
            name: name.to_string(),
            last_message_at,
            ..Channel::default()
        }
    }

    #[test]
    fn sidebar_sorts_groups_then_alpha_by_default() {
        let mut app = test_app();
        let mut dm = channel("dm-1", "zoe", 50);
        dm.kind = ConversationKind::DirectMessage;
        let mut forum = channel("forum-1", "ideas", 40);
        forum.channel_type = "forum".to_string();
        let mut archived = channel("chan-3", "attic", 99);
        archived.archived = true;
        app.channels = vec![
            channel("chan-2", "beta", 10),
            dm,
            archived,
            channel("chan-1", "alpha", 20),
            forum,
            channel("chan-4", "favorite", 5),
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
    fn sidebar_recent_mode_orders_by_activity_and_sinks_quiet_channels() {
        let mut app = test_app();
        app.channels = vec![
            channel("chan-1", "alpha", 10),
            channel("chan-2", "beta", 30),
            channel("chan-3", "quiet", 0),
        ];
        app.channel_sort_modes
            .insert("channels".to_string(), "recent".to_string());

        app.sort_sidebar_channels();

        let names: Vec<&str> = app
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        assert_eq!(names, vec!["beta", "alpha", "quiet"]);
    }

    #[test]
    fn sort_keeps_selection_on_same_channel() {
        let mut app = test_app();
        app.channels = vec![channel("chan-2", "beta", 0), channel("chan-1", "alpha", 0)];
        app.selected_channel = 0;

        app.sort_sidebar_channels();

        assert_eq!(app.channels[app.selected_channel].id, "chan-2");
    }
}
