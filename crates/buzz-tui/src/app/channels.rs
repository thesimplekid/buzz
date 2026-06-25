use crate::app::{App, Focus, TimelineMode};
use crate::client::ChannelPreferenceKind;

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
            self.channel_drafts.remove(&key);
        } else {
            self.channel_drafts.insert(key, self.composer.clone());
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
        self.channel_drafts.remove(&key);
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

        let Some(latest) = self.channel_latest_seen.get(channel_id).copied() else {
            return false;
        };
        let frontier = self
            .workspace_config
            .read_frontiers
            .get(workspace_id)
            .and_then(|channels| channels.get(channel_id))
            .copied()
            .unwrap_or_default();
        latest > frontier
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
