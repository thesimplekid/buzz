use super::{clamp_index, short_id, App, ChannelScope, Focus, ThreadContext, TimelineMode};
use crate::cli::{Channel, Message};

impl App {
    /// Load older history for the active channel/thread by growing the fetch
    /// window, preserving the selected message where possible.
    pub async fn load_older_messages(&mut self) {
        if self.timeline_mode != TimelineMode::Channel {
            self.status = "Older history is only available in the channel timeline".to_string();
            return;
        }
        let Some(channel) = self.active_channel() else {
            self.status = "No channel selected".to_string();
            return;
        };
        let current = self.messages.len();
        let new_limit = (current as u32 + 80).min(1000);
        let selected_id = self
            .selected_timeline_message()
            .map(|message| message.id.clone());
        let result = if let Some(root) = self.thread_root.clone() {
            self.get_thread_messages_with_limit(&channel.id, &root, new_limit)
                .await
        } else {
            self.get_channel_messages_with_limit(&channel.id, new_limit)
                .await
        };
        match result {
            Ok(messages) => {
                let added = messages.len().saturating_sub(current);
                self.remember_message_author_profiles(&messages).await;
                self.messages = messages;
                if let Some(id) = selected_id {
                    if let Some(position) = self.messages.iter().position(|m| m.id == id) {
                        self.selected_message = position;
                    } else {
                        self.selected_message = self
                            .selected_message
                            .saturating_add(added)
                            .min(self.messages.len().saturating_sub(1));
                    }
                }
                self.reset_message_detail_scroll();
                if added == 0 {
                    self.status = "No older messages".to_string();
                } else {
                    self.status = format!("Loaded {added} older ({} total)", self.messages.len());
                }
            }
            Err(error) => self.status = format!("older: {error}"),
        }
    }

    pub async fn delete_selected_message(&mut self) {
        let Some(message) = self.selected_timeline_message() else {
            self.status = "No message selected".to_string();
            return;
        };
        if message.id.is_empty() {
            self.status = "Selected message has no event id".to_string();
            return;
        }

        match self.delete_message(&message.id).await {
            Ok(_) => {
                let deleted = short_id(&message.id).to_string();
                self.remove_selected_timeline_message();
                self.selected_reactions.clear();
                self.status = format!("Deleted {deleted}");
            }
            Err(error) => self.status = format!("delete: {error}"),
        }
    }

    pub async fn vote_on_selected_message(&mut self, direction: &'static str) {
        let Some(message) = self.selected_timeline_message() else {
            self.status = "No message selected".to_string();
            return;
        };
        if message.id.is_empty() {
            self.status = "Selected message has no event id".to_string();
            return;
        }

        match self.vote_message_native(&message.id, direction).await {
            Ok(_) => {
                self.status = format!(
                    "{}voted {}",
                    if direction == "up" { "Up" } else { "Down" },
                    short_id(&message.id)
                );
            }
            Err(error) => self.status = format!("vote: {error}"),
        }
    }

    pub async fn add_reaction_to_selected_message(&mut self) {
        self.react_to_selected_message(true).await;
    }

    pub async fn remove_reaction_from_selected_message(&mut self) {
        self.react_to_selected_message(false).await;
    }

    pub(super) async fn load_selected_channel(&mut self) {
        let Some(channel) = self.active_channel() else {
            return;
        };
        self.thread_root = None;
        self.thread_context = None;
        self.timeline_mode = TimelineMode::Channel;
        self.edit_target = None;
        self.pulse_reply_target = None;
        match self.get_channel_messages(&channel.id).await {
            Ok(messages) => {
                self.remember_message_author_profiles(&messages).await;
                self.messages = messages;
                self.remember_latest_message_for(&channel.id);
                let latest = self.latest_active_message_at();
                let read_changed = self.mark_channel_read_at(&channel.id, latest);
                self.selected_message = self.messages.len().saturating_sub(1);
                self.reset_message_detail_scroll();
                self.restore_active_channel_draft();
                self.refresh_selected_message_reactions().await;
                self.refresh_selected_channel_details().await;
                if !read_changed || self.publish_channel_read_at(&channel.id, latest).await {
                    self.status = format!("Opened #{}", channel.name);
                }
            }
            Err(error) => {
                self.messages.clear();
                self.selected_reactions.clear();
                self.selected_channel_detail = Some(channel);
                self.channel_members.clear();
                self.status = format!("messages: {error}");
            }
        }
    }

    pub(super) async fn open_selected_channel_search_result(&mut self) {
        let Some(channel) = self
            .channel_search_results
            .get(self.selected_channel_search)
            .cloned()
        else {
            self.status = "No channel search result selected".to_string();
            return;
        };
        self.open_context_channel(channel).await;
    }

    async fn open_context_channel(&mut self, channel: Channel) {
        self.thread_root = None;
        self.thread_context = Some(ThreadContext {
            channel_id: channel.id.clone(),
            channel_name: channel.name.clone(),
            return_mode: TimelineMode::Channel,
        });
        self.timeline_mode = TimelineMode::Channel;
        self.edit_target = None;
        self.pulse_reply_target = None;
        self.selected_channel_detail = Some(channel.clone());
        match self.get_channel_messages(&channel.id).await {
            Ok(messages) => {
                self.remember_message_author_profiles(&messages).await;
                self.messages = messages;
                self.selected_message = self.messages.len().saturating_sub(1);
                self.reset_message_detail_scroll();
                self.focus = Focus::Timeline;
                self.refresh_channel_details_for(&channel).await;
                self.refresh_selected_message_reactions().await;
                self.status = format!("Opened #{}", channel.name);
            }
            Err(error) => {
                self.messages.clear();
                self.selected_reactions.clear();
                self.focus = Focus::ChannelSearch;
                self.status = format!("messages: {error}");
            }
        }
    }

    pub(super) async fn open_dm_pubkey(&mut self, pubkey: &str) {
        match self.open_dm_native(pubkey).await {
            Ok(value) => {
                let dm_id = value
                    .get("dm_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                self.channel_scope = ChannelScope::Conversations;
                self.thread_root = None;
                self.thread_context = None;
                self.timeline_mode = TimelineMode::Channel;
                self.edit_target = None;
                self.focus = Focus::Sidebar;
                self.refresh().await;
                if let Some(dm_id) = dm_id {
                    if let Some(index) =
                        self.channels.iter().position(|channel| channel.id == dm_id)
                    {
                        self.selected_channel = index;
                        self.load_selected_channel().await;
                    }
                }
                self.status = format!("Opened DM with {}", short_id(pubkey));
            }
            Err(error) => self.status = format!("dm: {error}"),
        }
    }

    pub(super) async fn open_selected_thread(&mut self) {
        if self.timeline_mode == TimelineMode::Search {
            self.open_selected_search_thread().await;
            return;
        }
        if self.timeline_mode == TimelineMode::Feed {
            self.open_selected_feed_thread().await;
            return;
        }
        if let Some(thread_root) = self.thread_root.as_deref() {
            self.status = format!("Showing thread {}", short_id(thread_root));
            return;
        }
        let Some(channel) = self.active_channel() else {
            return;
        };
        let Some(message) = self.messages.get(self.selected_message).cloned() else {
            return;
        };
        if message.id.is_empty() {
            return;
        }
        let thread_root = thread_root_for_message(&message);
        match self.get_thread_messages(&channel.id, thread_root).await {
            Ok(messages) => {
                self.remember_message_author_profiles(&messages).await;
                self.thread_root = Some(thread_root.to_string());
                self.thread_context = None;
                self.edit_target = None;
                self.messages = messages;
                self.selected_message = self.messages.len().saturating_sub(1);
                self.reset_message_detail_scroll();
                self.refresh_selected_message_reactions().await;
                self.status = format!("Opened thread {}", short_id(thread_root));
            }
            Err(error) => {
                self.status = format!("thread: {error}");
            }
        }
    }

    pub(super) async fn open_selected_feed_thread(&mut self) {
        let Some(message) = self.feed.get(self.selected_feed).cloned() else {
            return;
        };
        if message.id.is_empty() {
            return;
        }
        if message.channel_id.is_empty() {
            self.status = "Feed item has no channel id".to_string();
            return;
        }

        match self
            .get_thread_messages(&message.channel_id, &message.id)
            .await
        {
            Ok(messages) => {
                let channel_name = self
                    .channels
                    .iter()
                    .find(|channel| channel.id == message.channel_id)
                    .map(|channel| channel.name.clone())
                    .unwrap_or_else(|| short_id(&message.channel_id).to_string());
                self.remember_message_author_profiles(&messages).await;
                self.thread_root = Some(message.id.clone());
                self.thread_context = Some(ThreadContext {
                    channel_id: message.channel_id,
                    channel_name,
                    return_mode: TimelineMode::Feed,
                });
                self.edit_target = None;
                self.messages = messages;
                self.selected_message = self.messages.len().saturating_sub(1);
                self.reset_message_detail_scroll();
                self.timeline_mode = TimelineMode::Channel;
                self.focus = Focus::Timeline;
                self.refresh_selected_message_reactions().await;
                self.status = format!("Opened feed item {}", short_id(&message.id));
            }
            Err(error) => {
                self.status = format!("thread: {error}");
            }
        }
    }

    async fn open_selected_search_thread(&mut self) {
        let Some(message) = self
            .search_results
            .get(self.selected_search_result)
            .cloned()
        else {
            return;
        };
        if message.id.is_empty() {
            return;
        }
        if message.channel_id.is_empty() {
            self.status = "Search result has no channel id".to_string();
            return;
        }

        match self
            .get_thread_messages(&message.channel_id, &message.id)
            .await
        {
            Ok(messages) => {
                let channel_name = self
                    .channels
                    .iter()
                    .find(|channel| channel.id == message.channel_id)
                    .map(|channel| channel.name.clone())
                    .unwrap_or_else(|| short_id(&message.channel_id).to_string());
                self.remember_message_author_profiles(&messages).await;
                self.thread_root = Some(message.id.clone());
                self.thread_context = Some(ThreadContext {
                    channel_id: message.channel_id,
                    channel_name,
                    return_mode: TimelineMode::Search,
                });
                self.edit_target = None;
                self.messages = messages;
                self.selected_message = self.messages.len().saturating_sub(1);
                self.reset_message_detail_scroll();
                self.timeline_mode = TimelineMode::Channel;
                self.refresh_selected_message_reactions().await;
                self.status = format!("Opened search result {}", short_id(&message.id));
            }
            Err(error) => {
                self.status = format!("thread: {error}");
            }
        }
    }

    async fn react_to_selected_message(&mut self, add: bool) {
        let Some(message) = self.selected_timeline_message() else {
            self.status = "No message selected".to_string();
            return;
        };
        if message.id.is_empty() {
            self.status = "Selected message has no event id".to_string();
            return;
        }

        let emoji = "+";
        let result = if add {
            self.add_reaction_to_event(&message.id, emoji, None).await
        } else {
            self.remove_reaction_from_event(&message.id, emoji).await
        };
        match result {
            Ok(_) => {
                let action = if add { "Reacted" } else { "Removed reaction" };
                self.refresh_selected_message_reactions().await;
                self.status = format!("{action} {emoji} on {}", short_id(&message.id));
            }
            Err(error) => {
                let action = if add { "react" } else { "remove reaction" };
                self.status = format!("{action}: {error}");
            }
        }
    }

    pub fn selected_timeline_message(&self) -> Option<Message> {
        match self.timeline_mode {
            TimelineMode::Channel => self.messages.get(self.selected_message).cloned(),
            TimelineMode::Search => self
                .search_results
                .get(self.selected_search_result)
                .cloned(),
            TimelineMode::Feed => self.feed.get(self.selected_feed).cloned(),
            TimelineMode::Pulse => self.pulse.get(self.selected_pulse).cloned(),
        }
    }

    pub fn reset_message_detail_scroll(&mut self) {
        self.message_detail_scroll = 0;
    }

    pub fn scroll_message_detail_down(&mut self) {
        if self.selected_timeline_message().is_none() {
            self.status = "No message selected".to_string();
            return;
        }
        self.message_detail_scroll = self.message_detail_scroll.saturating_add(8);
        self.status = "Scrolled message detail".to_string();
    }

    pub fn scroll_message_detail_up(&mut self) {
        if self.selected_timeline_message().is_none() {
            self.status = "No message selected".to_string();
            return;
        }
        self.message_detail_scroll = self.message_detail_scroll.saturating_sub(8);
        self.status = if self.message_detail_scroll == 0 {
            "Message detail at top".to_string()
        } else {
            "Scrolled message detail".to_string()
        };
    }

    pub fn scroll_message_detail_top(&mut self) {
        self.reset_message_detail_scroll();
        self.status = "Message detail at top".to_string();
    }

    pub fn scroll_message_detail_bottom(&mut self) {
        if self.selected_timeline_message().is_none() {
            self.status = "No message selected".to_string();
            return;
        }
        self.message_detail_scroll = u16::MAX;
        self.status = "Message detail at bottom".to_string();
    }

    pub async fn refresh_selected_message_reactions(&mut self) {
        let Some(message) = self.selected_timeline_message() else {
            self.selected_reactions.clear();
            return;
        };
        if message.id.is_empty() {
            self.selected_reactions.clear();
            return;
        }

        match self.get_message_reactions(&message.id).await {
            Ok(reactions) => self.selected_reactions = reactions,
            Err(error) => {
                self.selected_reactions.clear();
                self.status = format!("reactions: {error}");
            }
        }
    }

    fn remove_selected_timeline_message(&mut self) {
        match self.timeline_mode {
            TimelineMode::Channel => {
                if self.selected_message < self.messages.len() {
                    self.messages.remove(self.selected_message);
                    clamp_index(&mut self.selected_message, self.messages.len());
                }
            }
            TimelineMode::Search => {
                if self.selected_search_result < self.search_results.len() {
                    self.search_results.remove(self.selected_search_result);
                    clamp_index(&mut self.selected_search_result, self.search_results.len());
                }
            }
            TimelineMode::Feed => {
                if self.selected_feed < self.feed.len() {
                    self.feed.remove(self.selected_feed);
                    clamp_index(&mut self.selected_feed, self.feed.len());
                }
            }
            TimelineMode::Pulse => {
                if self.selected_pulse < self.pulse.len() {
                    self.pulse.remove(self.selected_pulse);
                    clamp_index(&mut self.selected_pulse, self.pulse.len());
                }
            }
        }
    }

    pub(super) fn update_selected_timeline_message_content(
        &mut self,
        event_id: &str,
        content: &str,
    ) {
        for message in self
            .messages
            .iter_mut()
            .chain(self.search_results.iter_mut())
            .chain(self.feed.iter_mut())
            .chain(self.pulse.iter_mut())
            .filter(|message| message.id == event_id)
        {
            message.content = content.to_string();
        }
    }
}

pub(super) fn thread_root_for_message(message: &Message) -> &str {
    message
        .thread_root_id
        .as_deref()
        .filter(|root| !root.is_empty())
        .unwrap_or(&message.id)
}
