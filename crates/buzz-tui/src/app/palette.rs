use std::collections::BTreeSet;

use super::{App, ConfirmAction, Focus, TimelineMode};
use crate::client::{Channel, Message, RelayAgentInfo, UserProfile};

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
    OpenRelayMembers,
    OpenWorkflows,
    OpenNotes,
    OpenReminders,
    OpenSnippets,
    OpenDrafts,
    OpenMemory,
    OpenEmoji,
    OpenRepos,
    OpenCanvas,
    OpenModeration,
    MintInvite,
    Refresh,
    Help,
    Quit,
    CopyMessage,
    EditMessage,
    DeleteMessage,
    ReplyInThread,
    ToggleThreadFollow,
    ReportMessage,
    StartStopAgent,
    LeaveChannel,
    ArchiveChannel,
    DeleteChannel,
    NavigateBack,
    NavigateForward,
    LoadOlderMessages,
    ToggleDetailPanel,
    ShowDetailPanel,
    HideDetailPanel,
    ToggleAgentPanel,
    ShowAgentPanel,
    HideAgentPanel,
    ResetPanelLayout,
    ViewIdentity,
    ExportIdentity,
    ReplaceIdentity,
    WorkspaceAuthorization,
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
    pub enabled: bool,
    pub disabled_reason: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub enum PaletteResult {
    Action(Command),
    Channel(Channel),
    User(UserProfile),
    Agent(PaletteAgentResult),
    Message(Message),
}

#[derive(Clone, Debug)]
pub struct PaletteAgentResult {
    pub pubkey: String,
    pub name: String,
    pub managed: bool,
    pub status: String,
}

impl PaletteResult {
    pub fn label(&self) -> String {
        match self {
            Self::Action(command) => command.label.to_string(),
            Self::Channel(channel) => format!("#{}", channel.name),
            Self::User(profile) => profile_label(profile),
            Self::Agent(agent) => {
                if agent.managed {
                    format!("{} (managed agent)", agent.name)
                } else {
                    format!("{} (relay agent)", agent.name)
                }
            }
            Self::Message(message) => {
                let preview = compact_result_text(&message.content, 72);
                format!("{}  {}", short_result_id(&message.pubkey), preview)
            }
        }
    }

    pub fn group(&self) -> &'static str {
        match self {
            Self::Action(_) => "Actions",
            Self::Channel(_) => "Channels",
            Self::User(_) | Self::Agent(_) => "People and agents",
            Self::Message(_) => "Messages",
        }
    }

    pub fn detail(&self) -> Option<String> {
        match self {
            Self::Action(command) => {
                command
                    .keybinding
                    .map(|key| format!("[{key}]"))
                    .or_else(|| {
                        command
                            .disabled_reason
                            .map(|reason| format!("disabled: {reason}"))
                    })
            }
            Self::Channel(channel) => Some(channel.description.clone()).filter(|s| !s.is_empty()),
            Self::User(profile) => Some(short_result_id(&profile.pubkey)),
            Self::Agent(agent) => Some(format!(
                "{} {}",
                agent.status,
                short_result_id(&agent.pubkey)
            )),
            Self::Message(message) => Some(format!(
                "#{} {}",
                short_result_id(&message.channel_id),
                short_result_id(&message.id)
            )),
        }
    }

    pub fn enabled(&self) -> bool {
        !matches!(self, Self::Action(Command { enabled: false, .. }))
    }
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
        id: CommandId::OpenRelayMembers,
        label: "Open relay access",
        aliases: &["relay members", "relay membership", "access", "admins"],
        keybinding: None,
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
        id: CommandId::OpenReminders,
        label: "Open reminders",
        aliases: &["remind me", "later", "todos"],
        keybinding: Some("L"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenSnippets,
        label: "Open snippets",
        aliases: &["saved snippets", "canned responses", "templates"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::OpenDrafts,
        label: "Open drafts",
        aliases: &["unsent messages", "drafts inbox"],
        keybinding: None,
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
        id: CommandId::OpenModeration,
        label: "Open moderation queue",
        aliases: &["reports", "trust and safety", "admin queue"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::MintInvite,
        label: "Create community invite",
        aliases: &["invite link", "invite member"],
        keybinding: None,
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
        id: CommandId::ToggleDetailPanel,
        label: "Toggle detail panel",
        aliases: &["show details", "hide details", "message details"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::ShowDetailPanel,
        label: "Show detail panel",
        aliases: &["open details", "message details"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::HideDetailPanel,
        label: "Hide detail panel",
        aliases: &["close details", "collapse details"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::ToggleAgentPanel,
        label: "Toggle agent panel",
        aliases: &["show agents", "hide agents", "agent list"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::ShowAgentPanel,
        label: "Show agent panel",
        aliases: &["open agents", "agent list"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::HideAgentPanel,
        label: "Hide agent panel",
        aliases: &["close agents", "collapse agents"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::ResetPanelLayout,
        label: "Reset panel layout",
        aliases: &["reset layout", "collapse panels"],
        keybinding: Some("0"),
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::ViewIdentity,
        label: "View identity",
        aliases: &["public key", "npub", "who am i"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::ExportIdentity,
        label: "Export identity secret",
        aliases: &["backup key", "export identity", "show nsec"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::ReplaceIdentity,
        label: "Replace identity",
        aliases: &["change identity", "import key", "new identity"],
        keybinding: None,
        scope: CommandScope::Global,
    },
    CommandDescriptor {
        id: CommandId::WorkspaceAuthorization,
        label: "Set workspace authorization",
        aliases: &["auth tag", "clear workspace authorization", "relay access"],
        keybinding: None,
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
        id: CommandId::CopyMessage,
        label: "Copy message",
        aliases: &["copy selected message", "clipboard", "yank"],
        keybinding: Some("y"),
        scope: CommandScope::Timeline,
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
        id: CommandId::ToggleThreadFollow,
        label: "Follow/unfollow thread",
        aliases: &["watch thread", "thread notifications"],
        keybinding: Some("Ctrl-F"),
        scope: CommandScope::Timeline,
    },
    CommandDescriptor {
        id: CommandId::ReportMessage,
        label: "Report message",
        aliases: &["flag message", "abuse report"],
        keybinding: Some("!"),
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
        let len = self.palette_results().len();
        if len == 0 {
            self.palette_selected = 0;
            return;
        }
        let current = self.palette_selected as isize;
        let next = (current + delta).rem_euclid(len as isize);
        self.palette_selected = next as usize;
    }

    /// Unified palette results matching the current query.
    pub fn palette_results(&self) -> Vec<PaletteResult> {
        let query = self.palette_query.trim().to_lowercase();
        let mut results = Vec::new();

        results.extend(
            self.palette_actions(&query)
                .into_iter()
                .map(PaletteResult::Action),
        );
        results.extend(
            self.recent_palette_channels(&query)
                .into_iter()
                .map(PaletteResult::Channel),
        );
        results.extend(
            self.local_palette_channels(&query)
                .into_iter()
                .map(PaletteResult::Channel),
        );
        results.extend(self.local_palette_people_and_agents(&query));
        results.extend(
            self.search_results
                .iter()
                .filter(|message| message_matches_query(message, &query))
                .take(12)
                .cloned()
                .map(PaletteResult::Message),
        );
        dedupe_palette_results(results)
    }

    fn palette_actions(&self, query: &str) -> Vec<Command> {
        let mut scored: Vec<(i32, Command)> = COMMANDS
            .iter()
            .filter_map(|descriptor| {
                let score = match_score(descriptor, query)?;
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
        let results = self.palette_results();
        let Some(result) = results.get(self.palette_selected).cloned() else {
            self.status = "No matching command".to_string();
            return;
        };
        if let PaletteResult::Action(Command {
            disabled_reason: Some(reason),
            label,
            ..
        }) = &result
        {
            self.status = format!("{label}: {reason}");
            return;
        }
        // Restore the originating focus so dispatched methods read the same
        // selection a direct keybinding would.
        self.focus = self.palette_return_focus;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.dispatch_palette_result(result).await;
    }

    async fn dispatch_palette_result(&mut self, result: PaletteResult) {
        match result {
            PaletteResult::Action(command) => {
                if let Some(reason) = command.disabled_reason {
                    self.status = format!("{}: {reason}", command.label);
                    return;
                }
                self.dispatch_command(command.id).await;
            }
            PaletteResult::Channel(channel) => self.open_context_channel(channel).await,
            PaletteResult::User(profile) => self.open_dm_pubkey(&profile.pubkey).await,
            PaletteResult::Agent(agent) => {
                if agent.managed {
                    if let Some(index) = self.acp.position_of(&agent.pubkey) {
                        self.selected_agent = index;
                    }
                    self.focus_agents().await;
                } else {
                    self.open_dm_pubkey(&agent.pubkey).await;
                }
            }
            PaletteResult::Message(message) => {
                self.search_results = vec![message];
                self.selected_search_result = 0;
                self.timeline_mode = TimelineMode::Search;
                self.open_selected_search_thread().await;
            }
        }
    }

    fn scope_is_active(&self, scope: CommandScope) -> bool {
        match scope {
            CommandScope::Global => false,
            CommandScope::Sidebar => self.palette_return_focus == Focus::Sidebar,
            CommandScope::Timeline => matches!(
                self.palette_return_focus,
                Focus::Timeline | Focus::Feed | Focus::Pulse
            ),
            CommandScope::Agents => self.palette_return_focus == Focus::Agents,
        }
    }

    fn recent_palette_channels(&self, query: &str) -> Vec<Channel> {
        if !query.is_empty() {
            return Vec::new();
        }
        let mut channels = self.channels.clone();
        channels.sort_by_key(|channel| {
            std::cmp::Reverse(
                self.channel_latest_seen
                    .get(&channel.id)
                    .copied()
                    .unwrap_or(channel.created_at),
            )
        });
        channels.into_iter().take(5).collect()
    }

    fn local_palette_channels(&self, query: &str) -> Vec<Channel> {
        if query.is_empty() {
            return Vec::new();
        }
        self.channels
            .iter()
            .filter(|channel| channel_matches_query(channel, query))
            .take(12)
            .cloned()
            .collect()
    }

    fn local_palette_people_and_agents(&self, query: &str) -> Vec<PaletteResult> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut seen = BTreeSet::new();
        let mut results = Vec::new();
        for agent in self.acp.agents().filter(|agent| agent.runtime.managed) {
            let result = PaletteAgentResult {
                pubkey: agent.runtime.id.clone(),
                name: agent.runtime.label.clone(),
                managed: true,
                status: format!("{:?}", agent.status).to_ascii_lowercase(),
            };
            if agent_result_matches_query(&result, query)
                && seen.insert(normalized_pubkey(&result.pubkey))
            {
                results.push(PaletteResult::Agent(result));
            }
        }
        for agent in eligible_relay_agents(&self.relay_agents) {
            let result = PaletteAgentResult {
                pubkey: agent.pubkey,
                name: agent.name,
                managed: false,
                status: agent.status,
            };
            if agent_result_matches_query(&result, query)
                && seen.insert(normalized_pubkey(&result.pubkey))
            {
                results.push(PaletteResult::Agent(result));
            }
        }
        for profile in self.author_profiles.values() {
            if profile_matches_query(profile, query)
                && seen.insert(normalized_pubkey(&profile.pubkey))
            {
                results.push(PaletteResult::User(profile.clone()));
            }
        }
        results
    }

    fn command_disabled_reason(&self, id: CommandId) -> Option<&'static str> {
        match id {
            CommandId::CopyMessage => {
                if !matches!(
                    self.palette_return_focus,
                    Focus::Timeline | Focus::Feed | Focus::Pulse
                ) {
                    Some("focus a message list first")
                } else if self.selected_timeline_message().is_none() {
                    Some("select a message first")
                } else {
                    None
                }
            }
            CommandId::EditMessage
            | CommandId::DeleteMessage
            | CommandId::ReplyInThread
            | CommandId::ToggleThreadFollow
            | CommandId::ReportMessage => {
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
            CommandId::ExportIdentity | CommandId::ReplaceIdentity if self.identity_from_env => {
                Some("Identity is controlled by BUZZ_PRIVATE_KEY for this session")
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
            CommandId::OpenRelayMembers => self.focus_relay_members().await,
            CommandId::OpenWorkflows => self.focus_workflows().await,
            CommandId::OpenNotes => self.focus_notes().await,
            CommandId::OpenReminders => self.focus_reminders().await,
            CommandId::OpenSnippets => self.focus_snippets(),
            CommandId::OpenDrafts => self.focus_drafts(),
            CommandId::OpenMemory => self.focus_memory().await,
            CommandId::OpenEmoji => self.focus_emoji().await,
            CommandId::OpenRepos => self.focus_repos().await,
            CommandId::OpenCanvas => self.focus_canvas().await,
            CommandId::OpenModeration => self.focus_moderation().await,
            CommandId::MintInvite => self.mint_community_invite().await,
            CommandId::Refresh => self.refresh().await,
            CommandId::Help => self.focus_help(),
            CommandId::Quit => self.quit(),
            CommandId::CopyMessage => self.copy_selected_message_to_clipboard(),
            CommandId::EditMessage => self.edit_selected_message(),
            CommandId::DeleteMessage => self.request_confirm(ConfirmAction::DeleteMessage),
            CommandId::ReplyInThread => self.open_selected_thread().await,
            CommandId::ToggleThreadFollow => self.toggle_selected_thread_follow(),
            CommandId::ReportMessage => self.focus_message_report(),
            CommandId::StartStopAgent => self.toggle_selected_agent().await,
            CommandId::LeaveChannel => self.request_confirm(ConfirmAction::LeaveChannel),
            CommandId::ArchiveChannel => self.request_confirm(ConfirmAction::ArchiveChannel),
            CommandId::DeleteChannel => self.request_confirm(ConfirmAction::DeleteChannel),
            CommandId::NavigateBack => self.navigate_back().await,
            CommandId::NavigateForward => self.navigate_forward().await,
            CommandId::LoadOlderMessages => self.load_older_messages().await,
            CommandId::ToggleDetailPanel => self.toggle_detail_panel(),
            CommandId::ShowDetailPanel => self.set_detail_panel_visible(true),
            CommandId::HideDetailPanel => self.set_detail_panel_visible(false),
            CommandId::ToggleAgentPanel => self.toggle_agent_panel(),
            CommandId::ShowAgentPanel => self.set_agent_panel_visible(true),
            CommandId::HideAgentPanel => self.set_agent_panel_visible(false),
            CommandId::ResetPanelLayout => self.reset_panel_sizes(),
            CommandId::ViewIdentity => self.focus_identity(),
            CommandId::ExportIdentity => self.request_confirm(ConfirmAction::ExportIdentity),
            CommandId::ReplaceIdentity => self.focus_identity_replace(),
            CommandId::WorkspaceAuthorization => self.focus_workspace_authorization(),
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

fn eligible_relay_agents(agents: &[RelayAgentInfo]) -> Vec<RelayAgentInfo> {
    agents
        .iter()
        .filter(|agent| agent.respond_to.as_deref() == Some("anyone"))
        .cloned()
        .collect()
}

fn dedupe_palette_results(results: Vec<PaletteResult>) -> Vec<PaletteResult> {
    let mut seen = BTreeSet::new();
    results
        .into_iter()
        .filter(|result| {
            let key = match result {
                PaletteResult::Action(command) => format!("action:{:?}", command.id),
                PaletteResult::Channel(channel) => format!("channel:{}", channel.id),
                PaletteResult::User(profile) => {
                    format!("person:{}", normalized_pubkey(&profile.pubkey))
                }
                PaletteResult::Agent(agent) => {
                    format!("person:{}", normalized_pubkey(&agent.pubkey))
                }
                PaletteResult::Message(message) => format!("message:{}", message.id),
            };
            seen.insert(key)
        })
        .collect()
}

fn channel_matches_query(channel: &Channel, query: &str) -> bool {
    contains_query(&channel.name, query)
        || contains_query(&channel.description, query)
        || contains_query(&channel.topic, query)
        || contains_query(&channel.purpose, query)
}

fn profile_matches_query(profile: &UserProfile, query: &str) -> bool {
    contains_query(&profile.display_name, query)
        || contains_query(&profile.name, query)
        || contains_query(&profile.nip05, query)
        || contains_query(&profile.pubkey, query)
}

fn agent_result_matches_query(agent: &PaletteAgentResult, query: &str) -> bool {
    contains_query(&agent.name, query) || contains_query(&agent.pubkey, query)
}

fn message_matches_query(message: &Message, query: &str) -> bool {
    query.is_empty()
        || contains_query(&message.content, query)
        || contains_query(&message.pubkey, query)
}

fn contains_query(value: &str, query: &str) -> bool {
    value.to_ascii_lowercase().contains(query)
}

fn normalized_pubkey(pubkey: &str) -> String {
    pubkey.trim().to_ascii_lowercase()
}

fn profile_label(profile: &UserProfile) -> String {
    for label in [&profile.display_name, &profile.name, &profile.nip05] {
        if !label.trim().is_empty() {
            return label.clone();
        }
    }
    short_result_id(&profile.pubkey)
}

fn compact_result_text(value: &str, max_chars: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut compact = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    compact.push('…');
    compact
}

fn short_result_id(id: &str) -> String {
    id.chars().take(8).collect()
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
    fn usability_commands_have_plain_language_aliases() {
        let detail = COMMANDS
            .iter()
            .find(|command| command.id == CommandId::ToggleDetailPanel)
            .unwrap();
        let identity = COMMANDS
            .iter()
            .find(|command| command.id == CommandId::ExportIdentity)
            .unwrap();

        assert!(match_score(detail, "show details").is_some());
        assert!(match_score(identity, "backup key").is_some());
    }
}
