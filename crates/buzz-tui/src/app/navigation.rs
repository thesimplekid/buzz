use super::{App, ChannelScope, Focus, ThreadContext, TimelineMode};

/// A snapshot of a navigable view, used for back/forward history.
#[derive(Clone, Debug)]
pub struct NavigationEntry {
    pub focus: Focus,
    pub timeline_mode: TimelineMode,
    pub channel_scope: ChannelScope,
    pub selected_channel: usize,
    pub selected_message: usize,
    pub selected_search_result: usize,
    pub selected_feed: usize,
    pub selected_pulse: usize,
    pub thread_root: Option<String>,
    pub thread_context: Option<ThreadContext>,
}

/// Coarse view identity. Two entries with the same location key represent "the
/// same place" (e.g. scrolling within a channel) and do not create history.
fn location_key(entry: &NavigationEntry) -> String {
    let panel = match entry.focus {
        Focus::Search | Focus::ChannelSearch => "search",
        Focus::Feed => "feed",
        Focus::Pulse => "pulse",
        Focus::Profile | Focus::ProfileEdit | Focus::ProfileAvatarUpload => "profile",
        Focus::UserProfile | Focus::UserLookup => "user",
        Focus::Contacts | Focus::ContactAdd => "contacts",
        Focus::Agents | Focus::CreateAgent => "agents",
        Focus::Memory | Focus::MemoryEdit | Focus::MemoryPatch => "memory",
        Focus::Emoji | Focus::EmojiEdit | Focus::EmojiImport => "emoji",
        Focus::Notes | Focus::NoteEdit => "notes",
        Focus::Repos | Focus::RepoCreate | Focus::RepoIssueCreate | Focus::RepoPatchCreate => {
            "repos"
        }
        Focus::Workspaces | Focus::WorkspaceAdd => "workspaces",
        Focus::Canvas | Focus::CanvasEdit => "canvas",
        Focus::Workflows
        | Focus::WorkflowEdit
        | Focus::WorkflowInputs
        | Focus::WorkflowApproval => "workflows",
        Focus::Help => "help",
        // Chat surfaces (sidebar/timeline/composer + channel edit dialogs).
        _ => "chat",
    };
    let thread = entry.thread_root.as_deref().unwrap_or("");
    format!(
        "{panel}|{:?}|{}|{thread}",
        entry.timeline_mode, entry.selected_channel
    )
}

impl App {
    pub(super) fn nav_snapshot(&self) -> NavigationEntry {
        NavigationEntry {
            focus: self.focus,
            timeline_mode: self.timeline_mode,
            channel_scope: self.channel_scope,
            selected_channel: self.selected_channel,
            selected_message: self.selected_message,
            selected_search_result: self.selected_search_result,
            selected_feed: self.selected_feed,
            selected_pulse: self.selected_pulse,
            thread_root: self.thread_root.clone(),
            thread_context: self.thread_context.clone(),
        }
    }

    /// Record navigation after each input event. Pushes onto the back stack only
    /// when the view location changes; otherwise just tracks the latest scroll
    /// position so returning restores context.
    pub fn track_navigation(&mut self) {
        // Transient overlays are not navigable destinations.
        if matches!(self.focus, Focus::CommandPalette | Focus::Confirm) {
            return;
        }
        let snapshot = self.nav_snapshot();
        if self.nav_restoring {
            self.nav_restoring = false;
            self.nav_current = snapshot;
            return;
        }
        if location_key(&self.nav_current) != location_key(&snapshot) {
            self.nav_back.push(self.nav_current.clone());
            if self.nav_back.len() > 50 {
                self.nav_back.remove(0);
            }
            self.nav_forward.clear();
        }
        self.nav_current = snapshot;
    }

    pub async fn navigate_back(&mut self) {
        let Some(previous) = self.nav_back.pop() else {
            self.status = "No previous view".to_string();
            return;
        };
        self.nav_forward.push(self.nav_current.clone());
        self.nav_restoring = true;
        self.apply_nav(previous).await;
    }

    pub async fn navigate_forward(&mut self) {
        let Some(next) = self.nav_forward.pop() else {
            self.status = "No next view".to_string();
            return;
        };
        self.nav_back.push(self.nav_current.clone());
        self.nav_restoring = true;
        self.apply_nav(next).await;
    }

    async fn apply_nav(&mut self, entry: NavigationEntry) {
        self.channel_scope = entry.channel_scope;
        self.timeline_mode = entry.timeline_mode;
        self.thread_root = entry.thread_root.clone();
        self.thread_context = entry.thread_context.clone();
        self.selected_channel = entry.selected_channel;
        self.selected_search_result = entry.selected_search_result;
        self.selected_feed = entry.selected_feed;
        self.selected_pulse = entry.selected_pulse;

        match entry.timeline_mode {
            TimelineMode::Channel => {
                if let Some(channel) = self.active_channel() {
                    let reload = if let Some(root) = self.thread_root.clone() {
                        self.get_thread_messages(&channel.id, &root).await
                    } else {
                        self.get_channel_messages(&channel.id).await
                    };
                    if let Ok(messages) = reload {
                        self.remember_message_author_profiles(&messages).await;
                        self.messages = messages;
                    }
                }
                self.selected_message = entry
                    .selected_message
                    .min(self.messages.len().saturating_sub(1));
                self.reset_message_detail_scroll();
                self.refresh_selected_message_reactions().await;
            }
            TimelineMode::Search => {}
            TimelineMode::Feed => {
                if let Ok(feed) = self.get_feed_messages().await {
                    self.remember_message_author_profiles(&feed).await;
                    self.feed = feed;
                }
            }
            TimelineMode::Pulse => {
                self.focus_pulse().await;
            }
        }

        // Reload side-panel data where a destination view needs it.
        match entry.focus {
            Focus::Profile => self.focus_profile().await,
            Focus::Contacts => self.focus_contacts().await,
            Focus::Agents => self.focus_agents().await,
            Focus::Workflows => self.focus_workflows().await,
            _ => {}
        }

        self.focus = entry.focus;
        self.status = "Restored previous view".to_string();
    }
}
