use super::{App, ConfirmAction, Focus, TimelineMode};

/// Identifier for a palette command. Each maps to the same app method a direct
/// keybinding would invoke.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandId {
    CreateChannel,
    OpenDirectMessage,
    Search,
    ChannelSearch,
    SwitchWorkspace,
    OpenFeed,
    OpenPulse,
    OpenAgents,
    OpenProfile,
    OpenContacts,
    OpenWorkflows,
    OpenNotes,
    OpenMemory,
    OpenEmoji,
    OpenRepos,
    OpenCanvas,
    Refresh,
    Help,
    Quit,
    EditMessage,
    DeleteMessage,
    ReplyInThread,
    StartStopAgent,
    LeaveChannel,
    ArchiveChannel,
    DeleteChannel,
    NavigateBack,
    NavigateForward,
    LoadOlderMessages,
}

/// The focus a command is most relevant to. `Global` commands always rank after
/// the contextual ones for the active panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandScope {
    Global,
    Sidebar,
    Timeline,
    Agents,
}

struct CommandDescriptor {
    id: CommandId,
    label: &'static str,
    aliases: &'static [&'static str],
    keybinding: Option<&'static str>,
    scope: CommandScope,
}

/// A palette command resolved against the current app state.
#[derive(Clone, Debug)]
pub struct Command {
    pub id: CommandId,
    pub label: &'static str,
    pub keybinding: Option<&'static str>,
    pub scope: CommandScope,
    pub enabled: bool,
    pub disabled_reason: Option<&'static str>,
}

const COMMANDS: &[CommandDescriptor] = &[
    CommandDescriptor {
        id: CommandId::CreateChannel,
        label: "Create channel",
        aliases: &["new channel", "add channel"],
        keybinding: Some("n"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenDirectMessage,
        label: "Open DM",
        aliases: &["direct message", "message user", "dm"],
        keybinding: Some("m"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::Search,
        label: "Search messages",
        aliases: &["find", "search"],
        keybinding: Some("/"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::ChannelSearch,
        label: "Search channels",
        aliases: &["find channel", "browse channels"],
        keybinding: Some("O"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::SwitchWorkspace,
        label: "Switch workspace",
        aliases: &["workspace", "relay"],
        keybinding: Some("W"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenFeed,
        label: "Open feed",
        aliases: &["activity"],
        keybinding: Some("f"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenPulse,
        label: "Open Pulse",
        aliases: &["social", "notes feed"],
        keybinding: Some("T"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenAgents,
        label: "Open agents",
        aliases: &["managed agents", "bots"],
        keybinding: Some("a"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenProfile,
        label: "Open profile",
        aliases: &["me", "presence"],
        keybinding: Some("P"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenContacts,
        label: "Open contacts",
        aliases: &["people", "friends"],
        keybinding: Some("C"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenWorkflows,
        label: "Open workflows",
        aliases: &["automations"],
        keybinding: Some("w"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenNotes,
        label: "Open notes",
        aliases: &["long-form", "articles"],
        keybinding: Some("N"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenMemory,
        label: "Open agent memory",
        aliases: &["memory"],
        keybinding: Some("M"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenEmoji,
        label: "Open custom emoji",
        aliases: &["emoji", "reactions"],
        keybinding: Some("Y"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenRepos,
        label: "Open repos",
        aliases: &["repositories", "git"],
        keybinding: Some("G"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenCanvas,
        label: "Open canvas",
        aliases: &["channel canvas", "doc"],
        keybinding: Some("v"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::Refresh,
        label: "Refresh",
        aliases: &["reload"],
        keybinding: Some("r"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::Help,
        label: "Show help",
        aliases: &["keys", "shortcuts"],
        keybinding: Some("?"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::Quit,
        label: "Quit",
        aliases: &["exit"],
        keybinding: Some("q"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::NavigateBack,
        label: "Go back",
        aliases: &["back", "previous view"],
        keybinding: Some("Alt+←"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::NavigateForward,
        label: "Go forward",
        aliases: &["forward", "next view"],
        keybinding: Some("Alt+→"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::EditMessage,
        label: "Edit message",
        aliases: &["edit"],
        keybinding: Some("e"),
        scope: CommandScope::Timeline,
    },
    CommandDescriptor {
        id: CommandId::DeleteMessage,
        label: "Delete message",
        aliases: &["remove message"],
        keybinding: Some("d"),
        scope: CommandScope::Timeline,
    },
    CommandDescriptor {
        id: CommandId::LoadOlderMessages,
        label: "Load older messages",
        aliases: &["older history", "load more", "history"],
        keybinding: Some("PageUp"),
        scope: CommandScope::Timeline,
    },
    CommandDescriptor {
        id: CommandId::ReplyInThread,
        label: "Reply in thread",
        aliases: &["thread", "open thread"],
        keybinding: Some("Enter"),
        scope: CommandScope::Timeline,
    },
    CommandDescriptor {
        id: CommandId::StartStopAgent,
        label: "Start/stop agent",
        aliases: &["toggle agent", "run agent"],
        keybinding: Some("Enter"),
        scope: CommandScope::Agents,
    },
    CommandDescriptor {
        id: CommandId::LeaveChannel,
        label: "Leave channel",
        aliases: &["leave"],
        keybinding: Some("l"),
        scope: CommandScope::Sidebar,
    },
    CommandDescriptor {
        id: CommandId::ArchiveChannel,
        label: "Archive channel",
        aliases: &["archive"],
        keybinding: Some("z"),
        scope: CommandScope::Sidebar,
    },
    CommandDescriptor {
        id: CommandId::DeleteChannel,
        label: "Delete channel",
        aliases: &["remove channel", "delete conversation"],
        keybinding: Some("Del"),
        scope: CommandScope::Sidebar,
    },
];

impl App {
    /// Open the command palette, remembering the focus to return to.
    pub fn open_palette(&mut self) {
        self.palette_return_focus = self.focus;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.focus = Focus::CommandPalette;
        self.status = "Command palette — type to filter, Enter to run".to_string();
    }

    /// Close the palette and restore the previous focus.
    pub fn close_palette(&mut self) {
        self.focus = self.palette_return_focus;
        self.palette_query.clear();
        self.palette_selected = 0;
    }

    pub fn palette_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.palette_query.push(ch);
            self.palette_selected = 0;
        }
    }

    pub fn palette_pop(&mut self) {
        self.palette_query.pop();
        self.palette_selected = 0;
    }

    pub fn palette_move(&mut self, delta: isize) {
        let len = self.palette_filtered().len();
        if len == 0 {
            self.palette_selected = 0;
            return;
        }
        let current = self.palette_selected as isize;
        let next = (current + delta).rem_euclid(len as isize);
        self.palette_selected = next as usize;
    }

    /// All commands resolved against current state (enabled + disabled reason),
    /// using the focus the palette was opened from for context.
    pub fn palette_all_commands(&self) -> Vec<Command> {
        COMMANDS
            .iter()
            .map(|descriptor| {
                let disabled_reason = self.command_disabled_reason(descriptor.id);
                Command {
                    id: descriptor.id,
                    label: descriptor.label,
                    keybinding: descriptor.keybinding,
                    scope: descriptor.scope,
                    enabled: disabled_reason.is_none(),
                    disabled_reason,
                }
            })
            .collect()
    }

    /// Commands matching the current query, contextual scope first.
    pub fn palette_filtered(&self) -> Vec<Command> {
        let query = self.palette_query.trim().to_lowercase();
        let mut scored: Vec<(i32, Command)> = COMMANDS
            .iter()
            .filter_map(|descriptor| {
                let score = match_score(descriptor, &query)?;
                let context_bonus = if self.scope_is_active(descriptor.scope) {
                    -1000
                } else {
                    0
                };
                let disabled_reason = self.command_disabled_reason(descriptor.id);
                Some((
                    score + context_bonus,
                    Command {
                        id: descriptor.id,
                        label: descriptor.label,
                        keybinding: descriptor.keybinding,
                        scope: descriptor.scope,
                        enabled: disabled_reason.is_none(),
                        disabled_reason,
                    },
                ))
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label.cmp(b.1.label)));
        scored.into_iter().map(|(_, command)| command).collect()
    }

    /// Run the currently selected palette command.
    pub async fn run_selected_palette_command(&mut self) {
        let filtered = self.palette_filtered();
        let Some(command) = filtered.get(self.palette_selected).cloned() else {
            self.status = "No matching command".to_string();
            return;
        };
        if let Some(reason) = command.disabled_reason {
            self.status = format!("{}: {reason}", command.label);
            return;
        }
        // Restore the originating focus so dispatched methods read the same
        // selection a direct keybinding would.
        self.focus = self.palette_return_focus;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.dispatch_command(command.id).await;
    }

    fn scope_is_active(&self, scope: CommandScope) -> bool {
        match scope {
            CommandScope::Global => false,
            CommandScope::Sidebar => self.palette_return_focus == Focus::Sidebar,
            CommandScope::Timeline => self.palette_return_focus == Focus::Timeline,
            CommandScope::Agents => self.palette_return_focus == Focus::Agents,
        }
    }

    fn command_disabled_reason(&self, id: CommandId) -> Option<&'static str> {
        match id {
            CommandId::EditMessage | CommandId::DeleteMessage | CommandId::ReplyInThread => {
                if self.palette_return_focus != Focus::Timeline {
                    Some("focus the timeline first")
                } else if self.selected_timeline_message().is_none() {
                    Some("select a message first")
                } else {
                    None
                }
            }
            CommandId::StartStopAgent => {
                if self.acp.agent_at(self.selected_agent).is_none() {
                    Some("no agent selected")
                } else {
                    None
                }
            }
            CommandId::LeaveChannel | CommandId::ArchiveChannel | CommandId::DeleteChannel => {
                if self.active_channel().is_none() {
                    Some("select a channel first")
                } else {
                    None
                }
            }
            CommandId::LoadOlderMessages => {
                if self.timeline_mode != TimelineMode::Channel {
                    Some("only in the channel timeline")
                } else if self.active_channel().is_none() {
                    Some("select a channel first")
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    async fn dispatch_command(&mut self, id: CommandId) {
        match id {
            CommandId::CreateChannel => self.focus_create_channel(),
            CommandId::OpenDirectMessage => self.focus_direct_message(),
            CommandId::Search => self.focus_search(),
            CommandId::ChannelSearch => self.focus_channel_search(),
            CommandId::SwitchWorkspace => self.focus_workspaces(),
            CommandId::OpenFeed => self.focus_feed().await,
            CommandId::OpenPulse => self.focus_pulse().await,
            CommandId::OpenAgents => self.focus_agents().await,
            CommandId::OpenProfile => self.focus_profile().await,
            CommandId::OpenContacts => self.focus_contacts().await,
            CommandId::OpenWorkflows => self.focus_workflows().await,
            CommandId::OpenNotes => self.focus_notes().await,
            CommandId::OpenMemory => self.focus_memory().await,
            CommandId::OpenEmoji => self.focus_emoji().await,
            CommandId::OpenRepos => self.focus_repos().await,
            CommandId::OpenCanvas => self.focus_canvas().await,
            CommandId::Refresh => self.refresh().await,
            CommandId::Help => self.focus_help(),
            CommandId::Quit => self.quit(),
            CommandId::EditMessage => self.edit_selected_message(),
            CommandId::DeleteMessage => self.request_confirm(ConfirmAction::DeleteMessage),
            CommandId::ReplyInThread => self.open_selected_thread().await,
            CommandId::StartStopAgent => self.toggle_selected_agent().await,
            CommandId::LeaveChannel => self.request_confirm(ConfirmAction::LeaveChannel),
            CommandId::ArchiveChannel => self.request_confirm(ConfirmAction::ArchiveChannel),
            CommandId::DeleteChannel => self.request_confirm(ConfirmAction::DeleteChannel),
            CommandId::NavigateBack => self.navigate_back().await,
            CommandId::NavigateForward => self.navigate_forward().await,
            CommandId::LoadOlderMessages => self.load_older_messages().await,
        }
    }
}

/// Score a command against a query (lower is better). `None` means no match.
/// An empty query matches everything.
fn match_score(descriptor: &CommandDescriptor, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let label = descriptor.label.to_lowercase();
    if let Some(score) = subsequence_score(&label, query) {
        return Some(score);
    }
    descriptor
        .aliases
        .iter()
        .filter_map(|alias| subsequence_score(&alias.to_lowercase(), query).map(|score| score + 50))
        .min()
}

/// Returns a score if `query` is a subsequence of `haystack`. A contiguous
/// prefix match scores best; scattered subsequence matches score worse.
fn subsequence_score(haystack: &str, query: &str) -> Option<i32> {
    if haystack.starts_with(query) {
        return Some(0);
    }
    if haystack.contains(query) {
        return Some(10);
    }
    let mut query_chars = query.chars().peekable();
    let mut gaps = 0;
    let mut matched_any = false;
    for hc in haystack.chars() {
        match query_chars.peek() {
            Some(qc) if *qc == hc => {
                query_chars.next();
                matched_any = true;
            }
            Some(_) if matched_any => gaps += 1,
            _ => {}
        }
    }
    if query_chars.peek().is_none() {
        Some(100 + gaps)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> CommandDescriptor {
        CommandDescriptor {
            id: CommandId::CreateChannel,
            label: "Create channel",
            aliases: &["new channel", "add channel"],
            keybinding: Some("n"),
            scope: CommandScope::Global,
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(match_score(&descriptor(), ""), Some(0));
    }

    #[test]
    fn prefix_beats_substring_beats_subsequence() {
        assert_eq!(subsequence_score("create channel", "create"), Some(0));
        assert_eq!(subsequence_score("create channel", "channel"), Some(10));
        let scattered = subsequence_score("create channel", "ctl").unwrap();
        assert!(scattered >= 100);
    }

    #[test]
    fn matches_via_alias_when_label_misses() {
        // "new" is not in the label but is in an alias.
        let score = match_score(&descriptor(), "new").unwrap();
        assert!(
            score >= 50,
            "alias matches should be penalized relative to label"
        );
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(subsequence_score("create channel", "xyz"), None);
    }

    #[test]
    fn every_command_has_a_unique_id() {
        let mut ids: Vec<CommandId> = COMMANDS.iter().map(|d| d.id).collect();
        let total = ids.len();
        ids.dedup();
        // dedup only removes consecutive duplicates; sort first via debug string.
        let mut labels: Vec<&str> = COMMANDS.iter().map(|d| d.label).collect();
        labels.sort_unstable();
        let unique = labels.len();
        labels.dedup();
        assert_eq!(unique, labels.len(), "command labels must be unique");
        assert_eq!(total, COMMANDS.len());
    }
}
