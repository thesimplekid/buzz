use crate::acp::AcpSupervisor;
use crate::agent_store::ManagedAgentStore;
use crate::cli::BuzzCli;
use crate::client::{
    app_data::{
        is_msg_context_key, msg_context_key, ChannelPreferenceEntry, ChannelPreferenceStoreKind,
    },
    Channel, ChannelMember, ChannelPreferenceKind, ChannelSection, ChannelSections, Contact,
    ConversationKind, CreateChannelOptions, CreateIssueOptions, CreatePatchOptions,
    CreateRepoOptions, CustomEmojiEntry, GitIssue, GitPatch, ListNotesOptions, LongFormNoteOptions,
    MemoryEntry, Message, Note, NoteAuthor, PresenceInfo, PresenceStatus, ProfileField, Reaction,
    ReadState, RelayMember, Reminder, ReminderTarget, RepoProject, TuiMessageView, TuiRelayClient,
    UserProfile, Workflow, WorkflowDetail, WorkflowRun,
};
use crate::live::LiveChannelTarget;
use crate::memory::selected_agent_memory_identity;
use crate::refresh::{HydrateResult, HydrateTarget, RefreshResult, RefreshTarget, SidebarData};
use crate::workspace::WorkspaceConfig;
use buzz_core::kind::{KIND_STREAM_MESSAGE, KIND_STREAM_MESSAGE_EDIT, KIND_STREAM_MESSAGE_V2};
use nostr::{PublicKey, ToBech32};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

mod agents;
mod channels;
mod composer;
mod confirm;
mod hints;
mod navigation;
mod palette;
mod profile_social;
mod reminders;
mod timeline;
mod workflows;
mod workspaces;

pub use composer::{CompletionKind, CompletionState};
pub use confirm::{ConfirmAction, ConfirmState};
pub use navigation::NavigationEntry;

#[derive(Clone, Debug)]
pub struct PendingMessageSend {
    pub client: TuiRelayClient,
    pub event: nostr::Event,
    pub channel_name: String,
    pub reply_to: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ComposerSubmit {
    Queued(PendingMessageSend),
    Inline,
    Done,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Sidebar,
    Timeline,
    Composer,
    Attachment,
    Diff,
    Agents,
    Search,
    ChannelSearch,
    CreateChannel,
    DirectMessage,
    ChannelName,
    ChannelDescription,
    ChannelTopic,
    ChannelPurpose,
    ChannelSectionAssign,
    AddMember,
    RemoveMember,
    RelayMembers,
    AddRelayMember,
    RemoveRelayMember,
    ChangeRelayMemberRole,
    Canvas,
    CanvasEdit,
    Workflows,
    WorkflowEdit,
    WorkflowInputs,
    WorkflowApproval,
    Notes,
    NoteEdit,
    CreateAgent,
    Profile,
    ProfileEdit,
    ProfileAvatarUpload,
    Contacts,
    ContactAdd,
    UserLookup,
    UserProfile,
    Repos,
    RepoCreate,
    RepoIssueCreate,
    RepoPatchCreate,
    Memory,
    MemoryEdit,
    MemoryPatch,
    Emoji,
    EmojiEdit,
    EmojiImport,
    Feed,
    Pulse,
    Reminders,
    ReminderCreate,
    Help,
    Workspaces,
    WorkspaceAdd,
    CommandPalette,
    Confirm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelScope {
    Conversations,
    OpenChannels,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateChannelField {
    Name,
    Type,
    Visibility,
    Expiry,
    Description,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkflowApprovalField {
    Token,
    Note,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NotesSource {
    #[default]
    Mine,
    All,
}

impl NotesSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mine => "mine",
            Self::All => "all",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Mine => Self::All,
            Self::All => Self::Mine,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ChannelAddPolicy {
    Anyone,
    #[default]
    OwnerOnly,
    Nobody,
}

impl ChannelAddPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anyone => "anyone",
            Self::OwnerOnly => "owner_only",
            Self::Nobody => "nobody",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Anyone => "anyone",
            Self::OwnerOnly => "owner only",
            Self::Nobody => "nobody",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Anyone => Self::OwnerOnly,
            Self::OwnerOnly => Self::Nobody,
            Self::Nobody => Self::Anyone,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryEditField {
    Slug,
    Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryPatchField {
    BaseHash,
    Patch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewChannelType {
    Stream,
    Forum,
}

impl NewChannelType {
    pub fn label(self) -> &'static str {
        match self {
            NewChannelType::Stream => "stream",
            NewChannelType::Forum => "forum",
        }
    }

    fn next(self) -> Self {
        match self {
            NewChannelType::Stream => NewChannelType::Forum,
            NewChannelType::Forum => NewChannelType::Stream,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewChannelVisibility {
    Open,
    Private,
}

impl NewChannelVisibility {
    pub fn label(self) -> &'static str {
        match self {
            NewChannelVisibility::Open => "open",
            NewChannelVisibility::Private => "private",
        }
    }

    fn next(self) -> Self {
        match self {
            NewChannelVisibility::Open => NewChannelVisibility::Private,
            NewChannelVisibility::Private => NewChannelVisibility::Open,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewChannelExpiry {
    Permanent,
    OneHour,
    OneDay,
    SevenDays,
}

impl NewChannelExpiry {
    pub fn label(self) -> &'static str {
        match self {
            NewChannelExpiry::Permanent => "permanent",
            NewChannelExpiry::OneHour => "1 hour",
            NewChannelExpiry::OneDay => "1 day",
            NewChannelExpiry::SevenDays => "7 days",
        }
    }

    pub fn ttl_seconds(self) -> Option<i32> {
        match self {
            NewChannelExpiry::Permanent => None,
            NewChannelExpiry::OneHour => Some(60 * 60),
            NewChannelExpiry::OneDay => Some(24 * 60 * 60),
            NewChannelExpiry::SevenDays => Some(7 * 24 * 60 * 60),
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Permanent => Self::SevenDays,
            Self::SevenDays => Self::OneDay,
            Self::OneDay => Self::OneHour,
            Self::OneHour => Self::Permanent,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimelineMode {
    Channel,
    Search,
    Feed,
    Pulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedFilter {
    All,
    Mentions,
    NeedsAction,
    Activity,
    AgentActivity,
}

impl FeedFilter {
    pub fn label(self) -> &'static str {
        match self {
            FeedFilter::All => "all",
            FeedFilter::Mentions => "mentions",
            FeedFilter::NeedsAction => "needs action",
            FeedFilter::Activity => "activity",
            FeedFilter::AgentActivity => "agent activity",
        }
    }

    pub fn as_cli_type(self) -> Option<&'static str> {
        match self {
            FeedFilter::All => None,
            FeedFilter::Mentions => Some("mentions"),
            FeedFilter::NeedsAction => Some("needs_action"),
            FeedFilter::Activity => Some("activity"),
            FeedFilter::AgentActivity => Some("agent_activity"),
        }
    }

    fn next(self) -> Self {
        match self {
            FeedFilter::All => FeedFilter::Mentions,
            FeedFilter::Mentions => FeedFilter::NeedsAction,
            FeedFilter::NeedsAction => FeedFilter::Activity,
            FeedFilter::Activity => FeedFilter::AgentActivity,
            FeedFilter::AgentActivity => FeedFilter::All,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PulseSource {
    People,
    Mine,
    Agents,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReminderDraftMode {
    Create,
    Snooze(String),
}

impl PulseSource {
    pub fn label(self) -> &'static str {
        match self {
            PulseSource::People => "people",
            PulseSource::Mine => "mine",
            PulseSource::Agents => "agents",
        }
    }

    fn next(self) -> Self {
        match self {
            PulseSource::People => PulseSource::Mine,
            PulseSource::Mine => PulseSource::Agents,
            PulseSource::Agents => PulseSource::People,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadContext {
    pub channel_id: String,
    pub channel_name: String,
    pub return_mode: TimelineMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChannelInputKind {
    Name,
    Description,
    Topic,
    Purpose,
    AssignSection,
    AddMember,
    RemoveMember,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RelayMemberInputKind {
    Add,
    ChangeRole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentCreateField {
    Name,
    Model,
    SystemPrompt,
    RespondTo,
    Allowlist,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoteEditField {
    Name,
    Title,
    Summary,
    Tags,
    Content,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoCreateField {
    Id,
    Name,
    Description,
    CloneUrls,
    WebUrl,
    Relays,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoIssueField {
    Title,
    Labels,
    Content,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoPatchField {
    Commit,
    ParentCommit,
    Content,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmojiEditField {
    Shortcode,
    Url,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffField {
    Repo,
    Commit,
    File,
    Description,
    Diff,
}

pub const DEFAULT_SIDEBAR_WIDTH: u16 = 30;
pub const DEFAULT_DETAIL_WIDTH: u16 = 80;
pub const DEFAULT_AGENT_PANEL_HEIGHT: u16 = 9;
pub const MIN_SIDEBAR_WIDTH: u16 = 20;
pub const MIN_DETAIL_WIDTH: u16 = 24;
pub const MIN_AGENT_PANEL_HEIGHT: u16 = 3;
pub const MIN_MESSAGE_DETAIL_HEIGHT: u16 = 5;
pub const MAX_SIDEBAR_WIDTH: u16 = 56;
pub const MAX_DETAIL_WIDTH: u16 = 120;
pub const MAX_AGENT_PANEL_HEIGHT: u16 = 18;
const PANEL_RESIZE_STEP: u16 = 2;

#[derive(Debug)]
pub struct App {
    pub cli: BuzzCli,
    pub acp: AcpSupervisor,
    pub acp_binary: String,
    pub managed_agent_store: ManagedAgentStore,
    pub managed_agent_store_path: PathBuf,
    pub workspace_config: WorkspaceConfig,
    pub workspace_store_path: PathBuf,
    pub selected_workspace: usize,
    pub workspace_input: String,
    pub channels: Vec<Channel>,
    pub active_channel_id: Option<String>,
    pub messages: Vec<Message>,
    pub feed: Vec<Message>,
    pub feed_filter: FeedFilter,
    pub pulse: Vec<Message>,
    pub pulse_source: PulseSource,
    pub search_results: Vec<Message>,
    pub channel_search_results: Vec<Channel>,
    pub selected_reactions: Vec<Reaction>,
    pub selected_channel_detail: Option<Channel>,
    pub channel_members: Vec<ChannelMember>,
    pub relay_members: Vec<RelayMember>,
    pub selected_relay_member: usize,
    pub relay_member_input: String,
    pub canvas_channel_id: String,
    pub canvas_content: String,
    pub canvas_draft: String,
    pub workflow_channel_id: String,
    pub workflows: Vec<Workflow>,
    pub workflow_runs: Vec<WorkflowRun>,
    pub selected_workflow_detail: Option<WorkflowDetail>,
    pub selected_workflow: usize,
    pub workflow_edit_existing: bool,
    pub workflow_edit_id: Option<String>,
    pub workflow_yaml: String,
    pub workflow_yaml_cursor: usize,
    pub workflow_inputs: String,
    pub workflow_approval_approved: bool,
    pub workflow_approval_field: WorkflowApprovalField,
    pub workflow_approval_token: String,
    pub workflow_approval_note: String,
    pub notes: Vec<Note>,
    pub notes_source: NotesSource,
    pub selected_note: usize,
    pub note_edit_field: NoteEditField,
    pub note_edit_existing: bool,
    pub note_name: String,
    pub note_title: String,
    pub note_summary: String,
    pub note_tags: String,
    pub note_content: String,
    pub reminders: Vec<Reminder>,
    pub selected_reminder: usize,
    pub reminder_target: Option<ReminderTarget>,
    pub reminder_note: String,
    pub reminder_preset: usize,
    pub reminder_draft_mode: ReminderDraftMode,
    pub profile: Option<UserProfile>,
    pub selected_profile_field: ProfileField,
    pub profile_input: String,
    pub profile_upload_path: String,
    pub presence: Option<PresenceInfo>,
    pub last_presence_status: Option<PresenceStatus>,
    pub contacts: Vec<Contact>,
    pub author_profiles: BTreeMap<String, UserProfile>,
    pub selected_contact: usize,
    pub contact_input: String,
    pub viewed_profile: Option<UserProfile>,
    pub user_lookup_input: String,
    pub repos: Vec<RepoProject>,
    pub selected_repo: usize,
    pub repo_create_field: RepoCreateField,
    pub repo_id: String,
    pub repo_name: String,
    pub repo_description: String,
    pub repo_clone_urls: String,
    pub repo_web_url: String,
    pub repo_relays: String,
    pub repo_issues: Vec<GitIssue>,
    pub repo_patches: Vec<GitPatch>,
    pub selected_repo_issue: usize,
    pub selected_repo_patch: usize,
    pub repo_issue_field: RepoIssueField,
    pub repo_issue_title: String,
    pub repo_issue_labels: String,
    pub repo_issue_content: String,
    pub repo_patch_field: RepoPatchField,
    pub repo_patch_commit: String,
    pub repo_patch_parent_commit: String,
    pub repo_patch_content: String,
    pub memory_agent_pubkey: String,
    pub memory_agent_name: String,
    pub memories: Vec<MemoryEntry>,
    pub selected_memory: usize,
    pub memory_edit_existing: bool,
    pub memory_edit_field: MemoryEditField,
    pub memory_slug: String,
    pub memory_value: String,
    pub memory_patch_field: MemoryPatchField,
    pub memory_patch_base_hash: String,
    pub memory_patch_content: String,
    pub workspace_emoji: Vec<CustomEmojiEntry>,
    pub own_emoji: Vec<CustomEmojiEntry>,
    pub selected_emoji: usize,
    pub emoji_edit_field: EmojiEditField,
    pub emoji_shortcode: String,
    pub emoji_url: String,
    pub emoji_import_path: String,
    pub emoji_import_replace: bool,
    pub selected_channel: usize,
    pub selected_message: usize,
    pub selected_search_result: usize,
    pub selected_channel_search: usize,
    pub selected_feed: usize,
    pub selected_pulse: usize,
    pub message_detail_scroll: u16,
    pub sidebar_width: u16,
    pub detail_width: u16,
    pub agent_panel_height: u16,
    pub selected_agent: usize,
    pub channel_scope: ChannelScope,
    pub agent_log: String,
    pub agent_log_path: String,
    pub composer: String,
    pub channel_drafts: BTreeMap<String, String>,
    pub channel_latest_seen: BTreeMap<String, u64>,
    pub starred_channel_ids: BTreeSet<String>,
    pub muted_channel_ids: BTreeSet<String>,
    pub channel_sections: Vec<ChannelSection>,
    pub channel_section_assignments: BTreeMap<String, String>,
    pub attachment_input: String,
    pub diff_field: DiffField,
    pub diff_repo: String,
    pub diff_commit: String,
    pub diff_file: String,
    pub diff_description: String,
    pub diff_content: String,
    pub edit_target: Option<String>,
    pub pulse_reply_target: Option<String>,
    pub search_query: String,
    pub channel_search_query: String,
    pub channel_search_last_query: String,
    pub new_channel_name: String,
    pub new_channel_description: String,
    pub new_channel_type: NewChannelType,
    pub new_channel_visibility: NewChannelVisibility,
    pub new_channel_expiry: NewChannelExpiry,
    pub new_channel_field: CreateChannelField,
    pub dm_pubkey: String,
    pub new_agent_name: String,
    pub new_agent_model: String,
    pub new_agent_system_prompt: String,
    pub new_agent_respond_to: String,
    pub new_agent_allowlist: String,
    pub new_agent_reply_placement: String,
    pub new_agent_start_on_launch: bool,
    pub new_agent_field: AgentCreateField,
    pub new_agent_runtime_id: Option<String>,
    pub channel_action_input: String,
    pub channel_add_policy: ChannelAddPolicy,
    pub thread_root: Option<String>,
    pub thread_context: Option<ThreadContext>,
    pub timeline_mode: TimelineMode,
    pub focus: Focus,
    pub composer_cursor: usize,
    pub composer_completion: Option<CompletionState>,
    pub confirm: Option<ConfirmState>,
    pub palette_query: String,
    pub palette_selected: usize,
    pub palette_return_focus: Focus,
    pub nav_back: Vec<NavigationEntry>,
    pub nav_forward: Vec<NavigationEntry>,
    pub nav_current: NavigationEntry,
    pub nav_restoring: bool,
    pub status: String,
    startup_notice: Option<String>,
    pub should_quit: bool,
}

pub struct AppConfig {
    pub cli: BuzzCli,
    pub acp: AcpSupervisor,
    pub acp_binary: String,
    pub startup_notice: Option<String>,
    pub managed_agent_store: ManagedAgentStore,
    pub managed_agent_store_path: PathBuf,
    pub workspace_config: WorkspaceConfig,
    pub workspace_store_path: PathBuf,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let AppConfig {
            cli,
            acp,
            acp_binary,
            startup_notice,
            managed_agent_store,
            managed_agent_store_path,
            workspace_config,
            workspace_store_path,
        } = config;
        let selected_workspace = workspace_config.active_index();
        Self {
            cli,
            acp,
            acp_binary,
            managed_agent_store,
            managed_agent_store_path,
            workspace_config,
            workspace_store_path,
            selected_workspace,
            workspace_input: String::new(),
            channels: Vec::new(),
            active_channel_id: None,
            messages: Vec::new(),
            feed: Vec::new(),
            feed_filter: FeedFilter::All,
            pulse: Vec::new(),
            pulse_source: PulseSource::People,
            search_results: Vec::new(),
            channel_search_results: Vec::new(),
            selected_reactions: Vec::new(),
            selected_channel_detail: None,
            channel_members: Vec::new(),
            relay_members: Vec::new(),
            selected_relay_member: 0,
            relay_member_input: String::new(),
            canvas_channel_id: String::new(),
            canvas_content: String::new(),
            canvas_draft: String::new(),
            workflow_channel_id: String::new(),
            workflows: Vec::new(),
            workflow_runs: Vec::new(),
            selected_workflow_detail: None,
            selected_workflow: 0,
            workflow_edit_existing: false,
            workflow_edit_id: None,
            workflow_yaml: String::new(),
            workflow_yaml_cursor: 0,
            workflow_inputs: String::new(),
            workflow_approval_approved: true,
            workflow_approval_field: WorkflowApprovalField::Token,
            workflow_approval_token: String::new(),
            workflow_approval_note: String::new(),
            notes: Vec::new(),
            notes_source: NotesSource::Mine,
            selected_note: 0,
            note_edit_field: NoteEditField::Name,
            note_edit_existing: false,
            note_name: String::new(),
            note_title: String::new(),
            note_summary: String::new(),
            note_tags: String::new(),
            note_content: String::new(),
            reminders: Vec::new(),
            selected_reminder: 0,
            reminder_target: None,
            reminder_note: String::new(),
            reminder_preset: 0,
            reminder_draft_mode: ReminderDraftMode::Create,
            profile: None,
            selected_profile_field: ProfileField::DisplayName,
            profile_input: String::new(),
            profile_upload_path: String::new(),
            presence: None,
            last_presence_status: None,
            contacts: Vec::new(),
            author_profiles: BTreeMap::new(),
            selected_contact: 0,
            contact_input: String::new(),
            viewed_profile: None,
            user_lookup_input: String::new(),
            repos: Vec::new(),
            selected_repo: 0,
            repo_create_field: RepoCreateField::Id,
            repo_id: String::new(),
            repo_name: String::new(),
            repo_description: String::new(),
            repo_clone_urls: String::new(),
            repo_web_url: String::new(),
            repo_relays: String::new(),
            repo_issues: Vec::new(),
            repo_patches: Vec::new(),
            selected_repo_issue: 0,
            selected_repo_patch: 0,
            repo_issue_field: RepoIssueField::Title,
            repo_issue_title: String::new(),
            repo_issue_labels: String::new(),
            repo_issue_content: String::new(),
            repo_patch_field: RepoPatchField::Content,
            repo_patch_commit: String::new(),
            repo_patch_parent_commit: String::new(),
            repo_patch_content: String::new(),
            memory_agent_pubkey: String::new(),
            memory_agent_name: String::new(),
            memories: Vec::new(),
            selected_memory: 0,
            memory_edit_existing: false,
            memory_edit_field: MemoryEditField::Slug,
            memory_slug: String::new(),
            memory_value: String::new(),
            memory_patch_field: MemoryPatchField::BaseHash,
            memory_patch_base_hash: String::new(),
            memory_patch_content: String::new(),
            workspace_emoji: Vec::new(),
            own_emoji: Vec::new(),
            selected_emoji: 0,
            emoji_edit_field: EmojiEditField::Shortcode,
            emoji_shortcode: String::new(),
            emoji_url: String::new(),
            emoji_import_path: String::new(),
            emoji_import_replace: false,
            selected_channel: 0,
            selected_message: 0,
            selected_search_result: 0,
            selected_channel_search: 0,
            selected_feed: 0,
            selected_pulse: 0,
            message_detail_scroll: 0,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            detail_width: DEFAULT_DETAIL_WIDTH,
            agent_panel_height: DEFAULT_AGENT_PANEL_HEIGHT,
            selected_agent: 0,
            channel_scope: ChannelScope::Conversations,
            agent_log: String::new(),
            agent_log_path: String::new(),
            composer: String::new(),
            channel_drafts: BTreeMap::new(),
            channel_latest_seen: BTreeMap::new(),
            starred_channel_ids: BTreeSet::new(),
            muted_channel_ids: BTreeSet::new(),
            channel_sections: Vec::new(),
            channel_section_assignments: BTreeMap::new(),
            attachment_input: String::new(),
            diff_field: DiffField::Repo,
            diff_repo: String::new(),
            diff_commit: String::new(),
            diff_file: String::new(),
            diff_description: String::new(),
            diff_content: String::new(),
            edit_target: None,
            pulse_reply_target: None,
            search_query: String::new(),
            channel_search_query: String::new(),
            channel_search_last_query: String::new(),
            new_channel_name: String::new(),
            new_channel_description: String::new(),
            new_channel_type: NewChannelType::Stream,
            new_channel_visibility: NewChannelVisibility::Open,
            new_channel_expiry: NewChannelExpiry::Permanent,
            new_channel_field: CreateChannelField::Name,
            dm_pubkey: String::new(),
            new_agent_name: String::new(),
            new_agent_model: String::new(),
            new_agent_system_prompt: String::new(),
            new_agent_respond_to: "owner-only".to_string(),
            new_agent_allowlist: String::new(),
            new_agent_reply_placement: "thread-direct-mentions".to_string(),
            new_agent_start_on_launch: false,
            new_agent_field: AgentCreateField::Name,
            new_agent_runtime_id: None,
            channel_action_input: String::new(),
            channel_add_policy: ChannelAddPolicy::OwnerOnly,
            thread_root: None,
            thread_context: None,
            timeline_mode: TimelineMode::Channel,
            focus: Focus::Sidebar,
            composer_cursor: 0,
            composer_completion: None,
            confirm: None,
            palette_query: String::new(),
            palette_selected: 0,
            palette_return_focus: Focus::Sidebar,
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            nav_current: NavigationEntry {
                focus: Focus::Sidebar,
                timeline_mode: TimelineMode::Channel,
                channel_scope: ChannelScope::Conversations,
                active_channel_id: None,
                selected_channel: 0,
                selected_message: 0,
                selected_search_result: 0,
                selected_feed: 0,
                selected_pulse: 0,
                thread_root: None,
                thread_context: None,
            },
            nav_restoring: false,
            status: "Loading Buzz workspace...".to_string(),
            startup_notice,
            should_quit: false,
        }
    }

    pub async fn refresh(&mut self) {
        match self.load_sidebar_channels().await {
            Ok((channels, warning)) => {
                self.channels = channels;
                clamp_index(&mut self.selected_channel, self.channels.len());
                self.sync_remote_read_state().await;
                self.sync_channel_preferences().await;
                self.sync_channel_sections().await;
                self.set_status_once(warning.unwrap_or_else(|| self.loaded_status()));
                if let Some(channel) = self.active_channel() {
                    let thread_root = self.thread_root.clone();
                    let messages = if let Some(thread_root) = thread_root.as_deref() {
                        self.get_thread_messages(&channel.id, thread_root).await
                    } else {
                        self.get_channel_messages(&channel.id).await
                    };
                    match messages {
                        Ok(messages) => {
                            let was_at_end =
                                self.selected_message >= self.messages.len().saturating_sub(1);
                            let selected_id = self.selected_channel_message_id();
                            self.remember_message_author_profiles(&messages).await;
                            self.messages = messages;
                            self.remember_latest_message_for(&channel.id);
                            if thread_root.is_none() {
                                self.mark_channel_read_at(
                                    &channel.id,
                                    self.latest_active_message_at(),
                                );
                            } else {
                                self.mark_visible_thread_messages_read().await;
                            }
                            if was_at_end {
                                self.selected_message = self.messages.len().saturating_sub(1);
                            } else {
                                self.restore_selected_channel_message(selected_id.as_deref());
                            }
                            self.reset_message_detail_scroll_if_channel_message_changed(
                                selected_id.as_deref(),
                            );
                            if self.focus == Focus::Timeline {
                                self.refresh_selected_message_reactions().await;
                            }
                        }
                        Err(error) => {
                            self.messages.clear();
                            self.selected_reactions.clear();
                            self.status = format!("messages: {error}");
                        }
                    }
                }
            }
            Err(error) => {
                self.channels.clear();
                self.messages.clear();
                self.selected_reactions.clear();
                self.set_status_once(format!("channels: {error}"));
            }
        }

        if let Ok(feed) = self.get_feed_messages().await {
            self.remember_message_author_profiles(&feed).await;
            self.feed = feed;
            clamp_index(&mut self.selected_feed, self.feed.len());
        }

        if matches!(self.focus, Focus::Reminders | Focus::ReminderCreate) {
            self.refresh_reminders().await;
        }

        if self.focus == Focus::Sidebar {
            self.refresh_selected_channel_details().await;
        }

        if self.focus == Focus::Agents {
            self.refresh_selected_agent_log().await;
        }

        if self.focus == Focus::Pulse {
            self.focus_pulse().await;
        }
    }

    async fn load_sidebar_channels(&self) -> Result<(Vec<Channel>, Option<String>), String> {
        match self.channel_scope {
            ChannelScope::Conversations => {
                let mut channels = self.list_channels(true).await?;
                let mut warning = None;
                match self.list_direct_messages().await {
                    Ok(mut dms) => channels.append(&mut dms),
                    Err(error) => warning = Some(format!("dms: {error}")),
                }
                Ok((channels, warning))
            }
            ChannelScope::OpenChannels => self
                .list_channels(false)
                .await
                .map(|channels| (channels, None)),
        }
    }

    fn loaded_status(&self) -> String {
        match self.channel_scope {
            ChannelScope::Conversations => {
                format!("Loaded {} conversations", self.channels.len())
            }
            ChannelScope::OpenChannels => {
                format!("Loaded {} open channels", self.channels.len())
            }
        }
    }

    fn selected_channel_message_id(&self) -> Option<String> {
        self.messages
            .get(self.selected_message)
            .filter(|message| !message.id.is_empty())
            .map(|message| message.id.clone())
    }

    pub fn selected_timeline_message_id(&self) -> Option<String> {
        self.selected_timeline_message()
            .filter(|message| !message.id.is_empty())
            .map(|message| message.id)
    }

    fn restore_selected_channel_message(&mut self, selected_id: Option<&str>) -> bool {
        let before = self.selected_message;
        if let Some(id) = selected_id {
            if let Some(position) = self.messages.iter().position(|message| message.id == id) {
                self.selected_message = position;
                return before != self.selected_message;
            }
        }

        clamp_index(&mut self.selected_message, self.messages.len());
        before != self.selected_message
    }

    fn reset_message_detail_scroll_if_channel_message_changed(
        &mut self,
        previous_selected_id: Option<&str>,
    ) {
        let current_selected_id = self.selected_channel_message_id();
        if previous_selected_id != current_selected_id.as_deref() {
            self.reset_message_detail_scroll();
        }
    }

    pub async fn refresh_active(&mut self) {
        let Some(channel) = self.active_channel() else {
            return;
        };
        let messages = if let Some(thread_root) = &self.thread_root {
            self.get_thread_messages(&channel.id, thread_root).await
        } else {
            self.get_channel_messages(&channel.id).await
        };
        match messages {
            Ok(messages) => {
                let was_at_end = self.selected_message >= self.messages.len().saturating_sub(1);
                let selected_id = self.selected_channel_message_id();
                self.remember_message_author_profiles(&messages).await;
                self.messages = messages;
                self.remember_latest_message_for(&channel.id);
                if was_at_end {
                    self.selected_message = self.messages.len().saturating_sub(1);
                } else {
                    self.restore_selected_channel_message(selected_id.as_deref());
                }
                self.reset_message_detail_scroll_if_channel_message_changed(selected_id.as_deref());
                if self.focus == Focus::Timeline {
                    self.refresh_selected_message_reactions().await;
                }
            }
            Err(error) => {
                self.status = format!("messages: {error}");
            }
        }

        match self.get_feed_messages().await {
            Ok(feed) => {
                self.remember_message_author_profiles(&feed).await;
                self.feed = feed;
                clamp_index(&mut self.selected_feed, self.feed.len());
            }
            Err(error) => {
                self.status = format!("feed: {error}");
            }
        }

        if self.focus == Focus::Agents {
            self.refresh_agent_statuses().await;
            self.refresh_selected_agent_log().await;
        }

        if self.focus == Focus::Pulse {
            self.focus_pulse().await;
        }
    }

    pub fn refresh_target(&self) -> RefreshTarget {
        RefreshTarget {
            relay_url: self.cli.relay_url().to_string(),
            private_key: self.cli.private_key(),
            auth_tag: self.cli.auth_tag(),
            channel_scope: self.channel_scope,
            selected_channel: self.selected_channel,
            active_channel_id: self.active_channel().map(|channel| channel.id.clone()),
            thread_root: self.thread_root.clone(),
            timeline_mode: self.timeline_mode,
            focus: self.focus,
            feed_type: self.feed_filter.as_cli_type(),
            selected_message_id: self.selected_timeline_message_id(),
            known_author_pubkeys: self.author_profiles.keys().cloned().collect(),
        }
    }

    pub fn hydrate_target(&self, author_pubkeys: BTreeSet<String>) -> HydrateTarget {
        HydrateTarget {
            relay_url: self.cli.relay_url().to_string(),
            private_key: self.cli.private_key(),
            auth_tag: self.cli.auth_tag(),
            selected_message_id: self.selected_timeline_message_id(),
            author_pubkeys,
            known_author_pubkeys: self.author_profiles.keys().cloned().collect(),
        }
    }

    pub fn apply_refresh_result(&mut self, target: &RefreshTarget, result: RefreshResult) {
        if target.relay_url != self.cli.relay_url() {
            return;
        }

        if let Some(sidebar) = result.sidebar {
            match sidebar {
                Ok(sidebar) => self.apply_sidebar_refresh(sidebar),
                Err(error) => {
                    self.channels.clear();
                    self.messages.clear();
                    self.selected_reactions.clear();
                    self.set_status_once(format!("channels: {error}"));
                    return;
                }
            }
        }

        if let Some(read_state) = result.read_state {
            if self.merge_remote_read_state(read_state.contexts) {
                self.save_workspace_config("read state");
            }
        }
        if let Some(ids) = result.starred_channel_ids {
            self.starred_channel_ids = ids;
        }
        if let Some(ids) = result.muted_channel_ids {
            self.muted_channel_ids = ids;
        }
        if let Some(mut store) = result.channel_sections {
            store.sections.sort_by_key(|section| section.order);
            let section_ids = store
                .sections
                .iter()
                .map(|section| section.id.as_str())
                .collect::<BTreeSet<_>>();
            store
                .assignments
                .retain(|_, section_id| section_ids.contains(section_id.as_str()));
            self.channel_sections = store.sections;
            self.channel_section_assignments = store.assignments;
        }
        self.apply_channel_detail_refresh(
            result.channel_detail_id.as_deref(),
            result.channel_detail,
            result.channel_members,
        );

        if let Some(messages) = result.messages {
            match messages {
                Ok(messages) => self.apply_refreshed_messages(
                    target,
                    result.message_channel_id.as_deref(),
                    messages,
                ),
                Err(error) => {
                    self.messages.clear();
                    self.selected_reactions.clear();
                    self.status = format!("messages: {error}");
                }
            }
        }

        if let Some(feed) = result.feed {
            match feed {
                Ok(feed) => {
                    self.feed = feed;
                    clamp_index(&mut self.selected_feed, self.feed.len());
                }
                Err(error) => {
                    self.status = format!("feed: {error}");
                }
            }
        }

        self.apply_profiles(result.profiles);
        if let Some(reactions) = result.reactions {
            self.apply_reactions_if_current(
                result
                    .reaction_event_id
                    .as_deref()
                    .or(target.selected_message_id.as_deref()),
                reactions,
            );
        }
    }

    pub fn apply_hydrate_result(&mut self, target: &HydrateTarget, result: HydrateResult) {
        if target.relay_url != self.cli.relay_url() {
            return;
        }
        self.apply_profiles(result.profiles);
        if let Some(reactions) = result.reactions {
            self.apply_reactions_if_current(target.selected_message_id.as_deref(), reactions);
        }
    }

    fn apply_sidebar_refresh(&mut self, sidebar: SidebarData) {
        self.channels = sidebar.channels;
        clamp_index(&mut self.selected_channel, self.channels.len());
        let active_channel_missing = match self.active_channel_id.as_ref() {
            Some(active_id) => !self.channels.iter().any(|channel| channel.id == *active_id),
            None => true,
        };
        if active_channel_missing {
            self.active_channel_id = self
                .channels
                .get(self.selected_channel)
                .map(|channel| channel.id.clone());
        }
        self.set_status_once(sidebar.warning.unwrap_or_else(|| self.loaded_status()));
    }

    fn apply_channel_detail_refresh(
        &mut self,
        channel_id: Option<&str>,
        detail: Option<Result<Option<Channel>, String>>,
        members: Option<Result<Vec<ChannelMember>, String>>,
    ) {
        if channel_id
            != self
                .active_channel()
                .as_ref()
                .map(|channel| channel.id.as_str())
        {
            return;
        }
        if let Some(detail) = detail {
            match detail {
                Ok(detail) => {
                    self.selected_channel_detail = detail.or_else(|| self.active_channel());
                }
                Err(error) => {
                    self.selected_channel_detail = self.active_channel();
                    self.status = format!("channel: {error}");
                }
            }
        }
        if let Some(members) = members {
            match members {
                Ok(members) => self.channel_members = members,
                Err(error) => {
                    self.channel_members.clear();
                    self.status = format!("members: {error}");
                }
            }
        }
    }

    fn apply_refreshed_messages(
        &mut self,
        target: &RefreshTarget,
        message_channel_id: Option<&str>,
        messages: Vec<Message>,
    ) {
        if target.timeline_mode != self.timeline_mode || target.thread_root != self.thread_root {
            return;
        }
        let current_channel_id = self.active_channel().map(|channel| channel.id.clone());
        if message_channel_id != current_channel_id.as_deref() {
            return;
        }

        let was_at_end = self.selected_message >= self.messages.len().saturating_sub(1);
        let selected_id = self.selected_channel_message_id();
        let active_channel_id = current_channel_id;
        if self.active_channel_id.is_none() {
            self.active_channel_id = message_channel_id.map(str::to_string);
        }
        self.messages = messages;
        if let Some(channel_id) = active_channel_id {
            self.remember_latest_message_for(&channel_id);
            if target.thread_root.is_none() {
                self.mark_channel_read_at(&channel_id, self.latest_active_message_at());
            } else {
                let messages = self.messages.clone();
                self.mark_messages_read(&messages);
            }
        }
        if was_at_end {
            self.selected_message = self.messages.len().saturating_sub(1);
        } else {
            self.restore_selected_channel_message(selected_id.as_deref());
        }
        self.reset_message_detail_scroll_if_channel_message_changed(selected_id.as_deref());
    }

    fn apply_profiles(&mut self, profiles: Vec<UserProfile>) {
        for profile in profiles {
            self.author_profiles.insert(profile.pubkey.clone(), profile);
        }
    }

    fn apply_reactions_if_current(
        &mut self,
        event_id: Option<&str>,
        reactions: Result<Vec<Reaction>, String>,
    ) {
        if self.selected_timeline_message_id().as_deref() != event_id {
            return;
        }
        match reactions {
            Ok(reactions) => self.selected_reactions = reactions,
            Err(error) => {
                self.selected_reactions.clear();
                self.status = format!("reactions: {error}");
            }
        }
    }

    pub fn active_live_channel_target(&self) -> Option<LiveChannelTarget> {
        let channel = self.active_channel()?;
        let workspace = self
            .workspace_config
            .workspaces
            .get(self.workspace_config.active_index())?;
        Some(LiveChannelTarget {
            relay_url: workspace.relay.clone(),
            presence_pubkeys: self.live_presence_pubkeys(),
            since: self
                .channel_latest_seen
                .get(&channel.id)
                .copied()
                .map(|seen| seen.saturating_sub(1)),
            channel_id: channel.id,
        })
    }

    fn live_presence_pubkeys(&self) -> Vec<String> {
        let mut pubkeys = self
            .contacts
            .iter()
            .map(|contact| contact.pubkey.clone())
            .chain(
                self.channel_members
                    .iter()
                    .map(|member| member.pubkey.clone()),
            )
            .chain(self.profile.iter().map(|profile| profile.pubkey.clone()))
            .chain(
                self.viewed_profile
                    .iter()
                    .map(|profile| profile.pubkey.clone()),
            )
            .chain(self.presence.iter().map(|presence| presence.pubkey.clone()))
            .filter(|pubkey| !pubkey.is_empty())
            .collect::<Vec<_>>();
        pubkeys.sort();
        pubkeys.dedup();
        pubkeys
    }

    pub fn apply_live_message(&mut self, message: TuiMessageView) -> bool {
        if !is_timeline_message_kind(message.kind) {
            return false;
        }
        let Some(channel) = self.active_channel() else {
            return false;
        };
        let channel_id = channel.id;
        if message.channel_id != channel_id || self.timeline_mode != TimelineMode::Channel {
            return false;
        }
        if self
            .thread_root
            .as_deref()
            .is_some_and(|root| message.thread_root_id.as_deref() != Some(root))
            && !self
                .messages
                .iter()
                .any(|existing| existing.id == message.id)
        {
            return false;
        }

        let was_at_end = self.selected_message >= self.messages.len().saturating_sub(1);
        let selected_id = self.selected_channel_message_id();
        let changed = merge_timeline_messages(
            &mut self.messages,
            [Message {
                id: message.id,
                pubkey: message.pubkey,
                kind: message.kind,
                content: message.content,
                created_at: message.created_at,
                channel_id: message.channel_id,
                thread_root_id: message.thread_root_id,
            }],
        );
        if !changed {
            return false;
        }

        self.remember_latest_message_for(&channel_id);
        if was_at_end {
            self.selected_message = self.messages.len().saturating_sub(1);
        } else {
            self.restore_selected_channel_message(selected_id.as_deref());
        }
        self.reset_message_detail_scroll_if_channel_message_changed(selected_id.as_deref());
        if self.focus == Focus::Timeline {
            if self.thread_root.is_some() {
                let messages = self.messages.clone();
                self.mark_messages_read(&messages);
            } else {
                self.mark_channel_read_at(&channel_id, self.latest_active_message_at());
            }
        }
        true
    }

    pub fn remove_timeline_message(&mut self, event_id: &str) -> bool {
        let Some(position) = self
            .messages
            .iter()
            .position(|message| message.id == event_id)
        else {
            return false;
        };
        self.messages.remove(position);
        clamp_index(&mut self.selected_message, self.messages.len());
        self.reset_message_detail_scroll();
        true
    }

    pub async fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Focus::Sidebar => {
                move_index(&mut self.selected_channel, self.channels.len(), delta);
                self.selected_channel_detail = self.channels.get(self.selected_channel).cloned();
                self.channel_members.clear();
                if let Some(channel) = self.channels.get(self.selected_channel) {
                    self.status = format!("Selected #{}; Enter opens", channel.name);
                }
            }
            Focus::Timeline => {
                self.move_timeline_selection(delta);
            }
            Focus::Agents => {
                let agent_count = self.agent_count();
                move_index(&mut self.selected_agent, agent_count, delta);
                self.refresh_selected_agent_log().await;
            }
            Focus::ChannelSearch => {
                move_index(
                    &mut self.selected_channel_search,
                    self.channel_search_results.len(),
                    delta,
                );
            }
            Focus::Workflows => {
                move_index(&mut self.selected_workflow, self.workflows.len(), delta);
                self.refresh_selected_workflow_runs().await;
                self.refresh_selected_workflow_detail().await;
            }
            Focus::WorkflowEdit | Focus::WorkflowInputs | Focus::WorkflowApproval => {}
            Focus::Notes => {
                move_index(&mut self.selected_note, self.notes.len(), delta);
            }
            Focus::Reminders => {
                let len = self.visible_reminders().len();
                move_index(&mut self.selected_reminder, len, delta);
            }
            Focus::Profile => self.move_profile_field(delta),
            Focus::Contacts => move_index(&mut self.selected_contact, self.contacts.len(), delta),
            Focus::UserProfile => {}
            Focus::RelayMembers => move_index(
                &mut self.selected_relay_member,
                self.relay_members.len(),
                delta,
            ),
            Focus::Repos => {
                let before = self.selected_repo;
                move_index(&mut self.selected_repo, self.repos.len(), delta);
                if self.selected_repo != before {
                    self.refresh_selected_repo_collaboration().await;
                }
            }
            Focus::Memory => {
                move_index(&mut self.selected_memory, self.memories.len(), delta);
                self.refresh_selected_memory_value().await;
            }
            Focus::MemoryEdit | Focus::MemoryPatch => {}
            Focus::Workspaces => {
                move_index(
                    &mut self.selected_workspace,
                    self.workspace_config.workspaces.len(),
                    delta,
                );
            }
            Focus::Emoji => {
                let emoji_count = self.emoji_count();
                move_index(&mut self.selected_emoji, emoji_count, delta);
            }
            Focus::CommandPalette => self.palette_move(delta),
            Focus::EmojiImport => {}
            Focus::Feed => {
                move_index(&mut self.selected_feed, self.feed.len(), delta);
                self.refresh_selected_message_reactions().await;
            }
            Focus::Pulse => {
                move_index(&mut self.selected_pulse, self.pulse.len(), delta);
                self.refresh_selected_message_reactions().await;
            }
            Focus::Composer
            | Focus::Attachment
            | Focus::Diff
            | Focus::Search
            | Focus::CreateChannel
            | Focus::DirectMessage
            | Focus::ChannelName
            | Focus::ChannelDescription
            | Focus::ChannelTopic
            | Focus::ChannelPurpose
            | Focus::ChannelSectionAssign
            | Focus::AddMember
            | Focus::RemoveMember
            | Focus::Canvas
            | Focus::CanvasEdit
            | Focus::CreateAgent
            | Focus::ProfileEdit
            | Focus::ProfileAvatarUpload
            | Focus::ContactAdd
            | Focus::UserLookup
            | Focus::AddRelayMember
            | Focus::RemoveRelayMember
            | Focus::ChangeRelayMemberRole
            | Focus::RepoCreate
            | Focus::RepoIssueCreate
            | Focus::RepoPatchCreate
            | Focus::EmojiEdit
            | Focus::NoteEdit
            | Focus::ReminderCreate
            | Focus::WorkspaceAdd
            | Focus::Confirm
            | Focus::Help => {}
        }
    }

    pub fn move_timeline_selection(&mut self, delta: isize) -> bool {
        let before = self.selected_timeline_index();
        match self.timeline_mode {
            TimelineMode::Channel => {
                move_index(&mut self.selected_message, self.messages.len(), delta);
            }
            TimelineMode::Search => {
                move_index(
                    &mut self.selected_search_result,
                    self.search_results.len(),
                    delta,
                );
            }
            TimelineMode::Feed => {
                move_index(&mut self.selected_feed, self.feed.len(), delta);
            }
            TimelineMode::Pulse => {
                move_index(&mut self.selected_pulse, self.pulse.len(), delta);
            }
        }
        let changed = self.selected_timeline_index() != before;
        if changed {
            self.reset_message_detail_scroll();
            self.selected_reactions.clear();
        }
        changed
    }

    fn selected_timeline_index(&self) -> usize {
        match self.timeline_mode {
            TimelineMode::Channel => self.selected_message,
            TimelineMode::Search => self.selected_search_result,
            TimelineMode::Feed => self.selected_feed,
            TimelineMode::Pulse => self.selected_pulse,
        }
    }

    pub fn resize_sidebar(&mut self, delta: isize) {
        self.sidebar_width = resized_panel_width(
            self.sidebar_width,
            delta,
            MIN_SIDEBAR_WIDTH,
            MAX_SIDEBAR_WIDTH,
        );
        self.status = format!("Sidebar width {}", self.sidebar_width);
    }

    pub fn resize_detail_panel(&mut self, delta: isize) {
        self.detail_width =
            resized_panel_width(self.detail_width, delta, MIN_DETAIL_WIDTH, MAX_DETAIL_WIDTH);
        self.status = format!("Detail panel width {}", self.detail_width);
    }

    pub fn resize_message_detail_height(&mut self, delta: isize) {
        self.agent_panel_height = resized_panel_width(
            self.agent_panel_height,
            -delta,
            MIN_AGENT_PANEL_HEIGHT,
            MAX_AGENT_PANEL_HEIGHT,
        );
        self.status = if delta > 0 {
            "ACP Agents section shorter".to_string()
        } else {
            "ACP Agents section taller".to_string()
        };
    }

    pub fn reset_panel_sizes(&mut self) {
        self.sidebar_width = DEFAULT_SIDEBAR_WIDTH;
        self.detail_width = DEFAULT_DETAIL_WIDTH;
        self.agent_panel_height = DEFAULT_AGENT_PANEL_HEIGHT;
        self.status = "Panel sizes reset".to_string();
    }

    pub async fn activate(&mut self) {
        match self.focus {
            Focus::Sidebar => self.load_selected_channel().await,
            Focus::Composer => self.send_composer().await,
            Focus::Attachment => self.send_attachment().await,
            Focus::Diff => self.send_diff().await,
            Focus::Agents => self.toggle_selected_agent().await,
            Focus::Timeline => self.open_selected_thread().await,
            Focus::Search => self.run_search().await,
            Focus::ChannelSearch => self.activate_channel_search().await,
            Focus::CreateChannel => self.create_channel().await,
            Focus::DirectMessage => self.open_dm().await,
            Focus::ChannelName => self.apply_channel_input(ChannelInputKind::Name).await,
            Focus::ChannelDescription => {
                self.apply_channel_input(ChannelInputKind::Description)
                    .await
            }
            Focus::ChannelTopic => self.apply_channel_input(ChannelInputKind::Topic).await,
            Focus::ChannelPurpose => self.apply_channel_input(ChannelInputKind::Purpose).await,
            Focus::ChannelSectionAssign => {
                self.apply_channel_input(ChannelInputKind::AssignSection)
                    .await
            }
            Focus::AddMember => self.apply_channel_input(ChannelInputKind::AddMember).await,
            Focus::RemoveMember => self.request_remove_member_confirm(),
            Focus::Canvas => self.edit_canvas(),
            Focus::CanvasEdit => self.save_canvas().await,
            Focus::Workflows => self.trigger_selected_workflow().await,
            Focus::WorkflowEdit => self.save_workflow().await,
            Focus::WorkflowInputs => self.trigger_selected_workflow_with_inputs().await,
            Focus::WorkflowApproval => self.submit_workflow_approval().await,
            Focus::Notes => {}
            Focus::Reminders => self.open_selected_reminder_thread().await,
            Focus::ReminderCreate => self.save_reminder_draft().await,
            Focus::NoteEdit => self.save_note().await,
            Focus::CreateAgent => self.create_managed_agent().await,
            Focus::Profile => self.edit_profile_field(),
            Focus::ProfileEdit => self.save_profile_field().await,
            Focus::ProfileAvatarUpload => self.upload_profile_avatar().await,
            Focus::Contacts => self.open_selected_contact_dm().await,
            Focus::ContactAdd => self.add_contact().await,
            Focus::UserLookup => self.run_user_lookup().await,
            Focus::UserProfile => self.open_viewed_profile_dm().await,
            Focus::RelayMembers => {
                self.status = "A adds  E changes role  X removes  R refreshes".to_string();
            }
            Focus::AddRelayMember => {
                self.apply_relay_member_input(RelayMemberInputKind::Add)
                    .await
            }
            Focus::RemoveRelayMember => self.request_remove_relay_member_confirm(),
            Focus::ChangeRelayMemberRole => {
                self.apply_relay_member_input(RelayMemberInputKind::ChangeRole)
                    .await
            }
            Focus::Repos => {}
            Focus::RepoCreate => self.save_repo().await,
            Focus::RepoIssueCreate => self.save_repo_issue().await,
            Focus::RepoPatchCreate => self.save_repo_patch().await,
            Focus::Memory => self.refresh_selected_memory_value().await,
            Focus::MemoryEdit => self.save_memory().await,
            Focus::MemoryPatch => self.apply_memory_patch().await,
            Focus::Workspaces => {
                self.status = "Enter switches to the selected workspace".to_string();
            }
            Focus::WorkspaceAdd => self.add_workspace_from_input(),
            Focus::Emoji => self.add_selected_emoji_reaction().await,
            Focus::EmojiEdit => self.save_emoji().await,
            Focus::EmojiImport => self.import_emoji().await,
            Focus::Feed => self.open_selected_feed_thread().await,
            Focus::Pulse => {
                self.status = "Pulse notes do not have channel threads".to_string();
            }
            Focus::CommandPalette => self.run_selected_palette_command().await,
            Focus::Confirm => self.confirm_pending().await,
            Focus::Help => {}
        }
    }

    pub async fn toggle_channel_scope(&mut self) {
        self.channel_scope = match self.channel_scope {
            ChannelScope::Conversations => ChannelScope::OpenChannels,
            ChannelScope::OpenChannels => ChannelScope::Conversations,
        };
        self.selected_channel = 0;
        self.selected_message = 0;
        self.reset_message_detail_scroll();
        self.thread_root = None;
        self.thread_context = None;
        self.edit_target = None;
        self.pulse_reply_target = None;
        self.selected_reactions.clear();
        self.active_channel_id = None;
        self.messages.clear();
        self.refresh().await;
    }

    pub async fn join_selected_channel(&mut self) {
        if self.focus != Focus::Sidebar {
            return;
        }
        let Some(channel) = self.channels.get(self.selected_channel).cloned() else {
            return;
        };
        if channel.kind != ConversationKind::Channel {
            self.status = "Direct messages cannot be joined".to_string();
            return;
        }

        match self.join_channel_native(&channel.id).await {
            Ok(_) => {
                let joined_name = channel.name;
                self.channel_scope = ChannelScope::Conversations;
                self.selected_channel = 0;
                self.thread_root = None;
                self.thread_context = None;
                self.edit_target = None;
                self.selected_reactions.clear();
                self.refresh().await;
                self.status = format!("Joined #{joined_name}");
            }
            Err(error) => self.status = format!("join: {error}"),
        }
    }

    pub async fn leave_selected_channel(&mut self) {
        if self.focus != Focus::Sidebar {
            return;
        }
        if self.channel_scope != ChannelScope::Conversations {
            self.status = "Switch to conversations before leaving a channel".to_string();
            return;
        }
        let Some(channel) = self.channels.get(self.selected_channel).cloned() else {
            return;
        };
        if channel.kind != ConversationKind::Channel {
            self.status = "Direct messages cannot be left from channels".to_string();
            return;
        }

        match self.leave_channel_native(&channel.id).await {
            Ok(_) => {
                let left_name = channel.name;
                self.selected_channel = self.selected_channel.saturating_sub(1);
                self.selected_message = 0;
                self.reset_message_detail_scroll();
                self.thread_root = None;
                self.thread_context = None;
                self.timeline_mode = TimelineMode::Channel;
                self.edit_target = None;
                self.selected_reactions.clear();
                self.active_channel_id = None;
                self.messages.clear();
                self.refresh().await;
                self.status = format!("Left #{left_name}");
            }
            Err(error) => self.status = format!("leave: {error}"),
        }
    }

    pub async fn hide_selected_dm(&mut self) {
        if self.focus != Focus::Sidebar {
            return;
        }
        if self.channel_scope != ChannelScope::Conversations {
            self.status = "Switch to conversations before hiding a DM".to_string();
            return;
        }
        let Some(channel) = self.channels.get(self.selected_channel).cloned() else {
            return;
        };
        if channel.kind != ConversationKind::DirectMessage {
            self.status = "Only direct messages can be hidden".to_string();
            return;
        }

        match self.hide_dm_native(&channel.id).await {
            Ok(_) => {
                let hidden_name = channel.name;
                self.selected_channel = self.selected_channel.saturating_sub(1);
                self.selected_message = 0;
                self.reset_message_detail_scroll();
                self.thread_root = None;
                self.thread_context = None;
                self.timeline_mode = TimelineMode::Channel;
                self.edit_target = None;
                self.selected_reactions.clear();
                self.active_channel_id = None;
                self.messages.clear();
                self.refresh().await;
                self.status = format!("Hid @{hidden_name}");
            }
            Err(error) => self.status = format!("hide dm: {error}"),
        }
    }

    pub fn focus_channel_topic(&mut self) {
        self.focus_channel_input(ChannelInputKind::Topic);
    }

    pub fn focus_channel_name(&mut self) {
        self.focus_channel_input(ChannelInputKind::Name);
    }

    pub fn focus_channel_description(&mut self) {
        self.focus_channel_input(ChannelInputKind::Description);
    }

    pub fn focus_channel_purpose(&mut self) {
        self.focus_channel_input(ChannelInputKind::Purpose);
    }

    pub fn focus_channel_section_assignment(&mut self) {
        self.focus_channel_input(ChannelInputKind::AssignSection);
    }

    pub fn focus_add_member(&mut self) {
        if let Some(channel) = self.channels.get(self.selected_channel).cloned() {
            if channel.kind == ConversationKind::DirectMessage {
                self.channel_action_input.clear();
                self.focus = Focus::AddMember;
                self.status = "Type pubkey to add to DM".to_string();
                return;
            }
        }
        self.focus_channel_input(ChannelInputKind::AddMember);
    }

    pub fn focus_remove_member(&mut self) {
        self.focus_channel_input(ChannelInputKind::RemoveMember);
    }

    pub async fn archive_selected_channel(&mut self) {
        let Some(channel) = self.selected_sidebar_channel_for_management() else {
            return;
        };
        match self.archive_channel_native(&channel.id).await {
            Ok(_) => {
                self.status = format!("Archived #{}", channel.name);
                self.refresh_selected_channel_details().await;
            }
            Err(error) => self.status = format!("archive: {error}"),
        }
    }

    pub async fn unarchive_selected_channel(&mut self) {
        let Some(channel) = self.selected_sidebar_channel_for_management() else {
            return;
        };
        match self.unarchive_channel_native(&channel.id).await {
            Ok(_) => {
                self.status = format!("Unarchived #{}", channel.name);
                self.refresh_selected_channel_details().await;
            }
            Err(error) => self.status = format!("unarchive: {error}"),
        }
    }

    pub async fn delete_selected_channel(&mut self) {
        let Some(channel) = self.selected_sidebar_channel_for_management() else {
            return;
        };
        match self.delete_channel_native(&channel.id).await {
            Ok(_) => {
                let deleted_name = channel.name;
                self.selected_channel = self.selected_channel.saturating_sub(1);
                self.selected_message = 0;
                self.reset_message_detail_scroll();
                self.thread_root = None;
                self.thread_context = None;
                self.timeline_mode = TimelineMode::Channel;
                self.edit_target = None;
                self.selected_reactions.clear();
                self.active_channel_id = None;
                self.messages.clear();
                self.refresh().await;
                self.status = format!("Deleted #{deleted_name}");
            }
            Err(error) => self.status = format!("delete channel: {error}"),
        }
    }

    pub async fn unassign_selected_channel_section(&mut self) {
        let Some(channel) = self.selected_sidebar_channel_for_management() else {
            return;
        };
        match self.unassign_channel_section_native(&channel.id).await {
            Ok(_) => {
                self.channel_section_assignments.remove(&channel.id);
                self.sync_channel_sections().await;
                self.status = format!("Removed #{} from section", channel.name);
            }
            Err(error) => self.status = format!("channel sections: {error}"),
        }
    }

    pub async fn cycle_channel_add_policy(&mut self) {
        self.channel_add_policy = self.channel_add_policy.next();
        let policy = self.channel_add_policy.as_str();
        match self.set_channel_add_policy_native(policy).await {
            Ok(()) => {
                self.status = format!("Channel add policy: {}", self.channel_add_policy.label());
            }
            Err(error) => self.status = format!("channel add policy: {error}"),
        }
    }

    pub fn channel_action_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.channel_action_input.push(ch);
        }
    }

    pub fn channel_action_pop(&mut self) {
        self.channel_action_input.pop();
    }

    pub async fn focus_canvas(&mut self) {
        let Some(channel) = self.selected_channel_for_canvas() else {
            return;
        };
        match self.get_canvas_native(&channel.id).await {
            Ok(document) => {
                self.canvas_channel_id = document.channel_id;
                self.canvas_content = document.content.unwrap_or_default();
                self.canvas_draft = self.canvas_content.clone();
                self.focus = Focus::Canvas;
                self.status = format!("Loaded #{} canvas", channel.name);
            }
            Err(error) => self.status = format!("canvas: {error}"),
        }
    }

    pub fn edit_canvas(&mut self) {
        if self.canvas_channel_id.is_empty() {
            self.status = "No canvas loaded".to_string();
            return;
        }
        self.canvas_draft = self.canvas_content.clone();
        self.focus = Focus::CanvasEdit;
        self.status = "Editing canvas".to_string();
    }

    pub async fn save_canvas(&mut self) {
        if self.canvas_channel_id.is_empty() {
            self.status = "No canvas loaded".to_string();
            return;
        }
        match self
            .set_canvas_native(&self.canvas_channel_id, &self.canvas_draft)
            .await
        {
            Ok(_) => {
                self.canvas_content = self.canvas_draft.clone();
                self.focus = Focus::Canvas;
                self.status = "Saved canvas".to_string();
            }
            Err(error) => self.status = format!("canvas save: {error}"),
        }
    }

    pub fn canvas_draft_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.canvas_draft.push(ch);
        }
    }

    pub fn canvas_draft_pop(&mut self) {
        self.canvas_draft.pop();
    }

    pub async fn focus_notes(&mut self) {
        match self.list_notes_native(self.notes_source).await {
            Ok(notes) => {
                self.notes = notes;
                clamp_index(&mut self.selected_note, self.notes.len());
                self.focus = Focus::Notes;
                self.status = format!(
                    "Loaded {} {} note{}",
                    self.notes.len(),
                    self.notes_source.label(),
                    if self.notes.len() == 1 { "" } else { "s" }
                );
            }
            Err(error) => self.status = format!("notes: {error}"),
        }
    }

    pub async fn cycle_notes_source(&mut self) {
        self.notes_source = self.notes_source.next();
        self.focus_notes().await;
    }

    pub fn focus_create_note(&mut self) {
        self.clear_note_inputs();
        self.note_edit_existing = false;
        self.focus = Focus::NoteEdit;
        self.status = "Creating long-form note".to_string();
    }

    pub fn focus_edit_note(&mut self) {
        let Some(note) = self.notes.get(self.selected_note).cloned() else {
            self.status = "No note selected".to_string();
            return;
        };
        self.note_edit_existing = true;
        self.note_edit_field = NoteEditField::Title;
        self.note_name = note.slug;
        self.note_title = note.title;
        self.note_summary = note.summary.unwrap_or_default();
        self.note_tags = note.tags.join(", ");
        self.note_content = note.content;
        self.focus = Focus::NoteEdit;
        self.status = "Editing long-form note".to_string();
    }

    pub async fn save_note(&mut self) {
        let name = self.note_name.trim().to_string();
        let title = self.note_title.trim().to_string();
        let content = self.note_content.trim().to_string();
        if name.is_empty() {
            self.status = "Note slug is empty".to_string();
            self.note_edit_field = NoteEditField::Name;
            return;
        }
        if title.is_empty() {
            self.status = "Note title is empty".to_string();
            self.note_edit_field = NoteEditField::Title;
            return;
        }
        if content.is_empty() {
            self.status = "Note body is empty".to_string();
            self.note_edit_field = NoteEditField::Content;
            return;
        }

        let options = LongFormNoteOptions {
            name: name.clone(),
            title,
            summary: self.note_summary.trim().to_string(),
            tags: parse_note_tags(&self.note_tags),
            content,
        };
        match self.set_note_native(&options).await {
            Ok(_) => {
                self.clear_note_inputs();
                self.focus_notes().await;
                if let Some(index) = self.notes.iter().position(|note| note.slug == name) {
                    self.selected_note = index;
                }
                self.status = format!("Saved note {name}");
            }
            Err(error) => self.status = format!("note save: {error}"),
        }
    }

    pub async fn delete_selected_note(&mut self) {
        let Some(note) = self.notes.get(self.selected_note).cloned() else {
            self.status = "No note selected".to_string();
            return;
        };
        match self.delete_note_native(&note.slug).await {
            Ok(_) => {
                self.notes.remove(self.selected_note);
                clamp_index(&mut self.selected_note, self.notes.len());
                self.status = format!("Deleted note {}", note.slug);
            }
            Err(error) => self.status = format!("note delete: {error}"),
        }
    }

    pub fn note_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.selected_note_input_mut().push(ch);
        }
    }

    pub fn note_input_pop(&mut self) {
        self.selected_note_input_mut().pop();
    }

    pub fn next_note_edit_field(&mut self) {
        self.note_edit_field = match self.note_edit_field {
            NoteEditField::Name => NoteEditField::Title,
            NoteEditField::Title => NoteEditField::Summary,
            NoteEditField::Summary => NoteEditField::Tags,
            NoteEditField::Tags => NoteEditField::Content,
            NoteEditField::Content => NoteEditField::Name,
        };
    }

    pub fn previous_note_edit_field(&mut self) {
        self.note_edit_field = match self.note_edit_field {
            NoteEditField::Name => NoteEditField::Content,
            NoteEditField::Title => NoteEditField::Name,
            NoteEditField::Summary => NoteEditField::Title,
            NoteEditField::Tags => NoteEditField::Summary,
            NoteEditField::Content => NoteEditField::Tags,
        };
    }

    pub async fn focus_feed(&mut self) {
        match self.get_feed_messages().await {
            Ok(feed) => {
                self.remember_message_author_profiles(&feed).await;
                self.feed = feed;
                clamp_index(&mut self.selected_feed, self.feed.len());
                self.timeline_mode = TimelineMode::Feed;
                self.focus = Focus::Feed;
                self.refresh_selected_message_reactions().await;
                self.status = format!(
                    "Loaded {} {} feed item{}",
                    self.feed.len(),
                    self.feed_filter.label(),
                    if self.feed.len() == 1 { "" } else { "s" }
                );
            }
            Err(error) => self.status = format!("feed: {error}"),
        }
    }

    pub async fn cycle_feed_filter(&mut self) {
        self.feed_filter = self.feed_filter.next();
        self.focus_feed().await;
    }

    pub async fn focus_repos(&mut self) {
        match self.list_repos_native().await {
            Ok(repos) => {
                self.repos = repos;
                clamp_index(&mut self.selected_repo, self.repos.len());
                self.refresh_selected_repo_collaboration().await;
                self.focus = Focus::Repos;
                self.status = format!(
                    "Loaded {} repo announcement{}",
                    self.repos.len(),
                    if self.repos.len() == 1 { "" } else { "s" }
                );
            }
            Err(error) => self.status = format!("repos: {error}"),
        }
    }

    pub fn focus_create_repo(&mut self) {
        self.clear_repo_inputs();
        self.focus = Focus::RepoCreate;
        self.status = "Creating repo announcement".to_string();
    }

    pub fn focus_create_repo_issue(&mut self) {
        if self.repos.get(self.selected_repo).is_none() {
            self.status = "Select a repo before creating an issue".to_string();
            return;
        }
        self.clear_repo_issue_inputs();
        self.focus = Focus::RepoIssueCreate;
        self.status = "Creating repo issue".to_string();
    }

    pub fn focus_create_repo_patch(&mut self) {
        if self.repos.get(self.selected_repo).is_none() {
            self.status = "Select a repo before creating a patch".to_string();
            return;
        }
        self.clear_repo_patch_inputs();
        self.focus = Focus::RepoPatchCreate;
        self.status = "Creating repo patch".to_string();
    }

    pub async fn save_repo(&mut self) {
        let id = self.repo_id.trim().to_string();
        if id.is_empty() {
            self.status = "Repo id is empty".to_string();
            self.repo_create_field = RepoCreateField::Id;
            return;
        }

        let options = CreateRepoOptions {
            id: id.clone(),
            name: self.repo_name.trim().to_string(),
            description: self.repo_description.trim().to_string(),
            clone_urls: parse_repo_values(&self.repo_clone_urls),
            web_url: self.repo_web_url.trim().to_string(),
            relays: parse_repo_values(&self.repo_relays),
        };

        match self.create_repo_native(&options).await {
            Ok(_) => {
                self.clear_repo_inputs();
                self.focus_repos().await;
                if let Some(index) = self.repos.iter().position(|repo| repo.dtag == id) {
                    self.selected_repo = index;
                }
                self.refresh_selected_repo_collaboration().await;
                self.status = format!("Saved repo announcement {id}");
            }
            Err(error) => self.status = format!("repo save: {error}"),
        }
    }

    pub async fn save_repo_issue(&mut self) {
        let Some(repo) = self.repos.get(self.selected_repo).cloned() else {
            self.status = "No repo selected".to_string();
            return;
        };
        let title = self.repo_issue_title.trim().to_string();
        if title.is_empty() {
            self.status = "Issue title is empty".to_string();
            self.repo_issue_field = RepoIssueField::Title;
            return;
        }
        let content = self.repo_issue_content.trim().to_string();
        if content.is_empty() {
            self.status = "Issue body is empty".to_string();
            self.repo_issue_field = RepoIssueField::Content;
            return;
        }
        let options = CreateIssueOptions {
            repo_owner: repo.owner.clone(),
            repo_id: repo.dtag.clone(),
            title: title.clone(),
            content,
            labels: parse_repo_values(&self.repo_issue_labels),
            recipients: Vec::new(),
        };
        match self.create_issue_native(&options).await {
            Ok(()) => {
                self.clear_repo_issue_inputs();
                self.focus = Focus::Repos;
                self.refresh_selected_repo_collaboration().await;
                self.status = format!("Created issue {title}");
            }
            Err(error) => self.status = format!("issue create: {error}"),
        }
    }

    pub async fn save_repo_patch(&mut self) {
        let Some(repo) = self.repos.get(self.selected_repo).cloned() else {
            self.status = "No repo selected".to_string();
            return;
        };
        let content = self.repo_patch_content.trim().to_string();
        if content.is_empty() {
            self.status = "Patch content is empty".to_string();
            self.repo_patch_field = RepoPatchField::Content;
            return;
        }
        let options = CreatePatchOptions {
            repo_owner: repo.owner.clone(),
            repo_id: repo.dtag.clone(),
            content,
            commit: self.repo_patch_commit.trim().to_string(),
            parent_commit: self.repo_patch_parent_commit.trim().to_string(),
            root: false,
            root_revision: false,
            recipients: Vec::new(),
        };
        match self.create_patch_native(&options).await {
            Ok(()) => {
                self.clear_repo_patch_inputs();
                self.focus = Focus::Repos;
                self.refresh_selected_repo_collaboration().await;
                self.status = "Created repo patch".to_string();
            }
            Err(error) => self.status = format!("patch create: {error}"),
        }
    }

    pub fn repo_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.selected_repo_input_mut().push(ch);
        }
    }

    pub fn repo_input_pop(&mut self) {
        self.selected_repo_input_mut().pop();
    }

    pub fn repo_issue_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.selected_repo_issue_input_mut().push(ch);
        }
    }

    pub fn repo_issue_input_pop(&mut self) {
        self.selected_repo_issue_input_mut().pop();
    }

    pub fn repo_patch_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.selected_repo_patch_input_mut().push(ch);
        }
    }

    pub fn repo_patch_input_pop(&mut self) {
        self.selected_repo_patch_input_mut().pop();
    }

    pub fn next_repo_create_field(&mut self) {
        self.repo_create_field = match self.repo_create_field {
            RepoCreateField::Id => RepoCreateField::Name,
            RepoCreateField::Name => RepoCreateField::Description,
            RepoCreateField::Description => RepoCreateField::CloneUrls,
            RepoCreateField::CloneUrls => RepoCreateField::WebUrl,
            RepoCreateField::WebUrl => RepoCreateField::Relays,
            RepoCreateField::Relays => RepoCreateField::Id,
        };
    }

    pub fn previous_repo_create_field(&mut self) {
        self.repo_create_field = match self.repo_create_field {
            RepoCreateField::Id => RepoCreateField::Relays,
            RepoCreateField::Name => RepoCreateField::Id,
            RepoCreateField::Description => RepoCreateField::Name,
            RepoCreateField::CloneUrls => RepoCreateField::Description,
            RepoCreateField::WebUrl => RepoCreateField::CloneUrls,
            RepoCreateField::Relays => RepoCreateField::WebUrl,
        };
    }

    pub fn next_repo_issue_field(&mut self) {
        self.repo_issue_field = match self.repo_issue_field {
            RepoIssueField::Title => RepoIssueField::Labels,
            RepoIssueField::Labels => RepoIssueField::Content,
            RepoIssueField::Content => RepoIssueField::Title,
        };
    }

    pub fn previous_repo_issue_field(&mut self) {
        self.repo_issue_field = match self.repo_issue_field {
            RepoIssueField::Title => RepoIssueField::Content,
            RepoIssueField::Labels => RepoIssueField::Title,
            RepoIssueField::Content => RepoIssueField::Labels,
        };
    }

    pub fn next_repo_patch_field(&mut self) {
        self.repo_patch_field = match self.repo_patch_field {
            RepoPatchField::Commit => RepoPatchField::ParentCommit,
            RepoPatchField::ParentCommit => RepoPatchField::Content,
            RepoPatchField::Content => RepoPatchField::Commit,
        };
    }

    pub fn previous_repo_patch_field(&mut self) {
        self.repo_patch_field = match self.repo_patch_field {
            RepoPatchField::Commit => RepoPatchField::Content,
            RepoPatchField::ParentCommit => RepoPatchField::Commit,
            RepoPatchField::Content => RepoPatchField::ParentCommit,
        };
    }

    pub async fn refresh_selected_repo_collaboration(&mut self) {
        let Some(repo) = self.repos.get(self.selected_repo).cloned() else {
            self.repo_issues.clear();
            self.repo_patches.clear();
            self.selected_repo_issue = 0;
            self.selected_repo_patch = 0;
            return;
        };
        match self.list_repo_issues_native(&repo.owner, &repo.dtag).await {
            Ok(issues) => {
                self.repo_issues = issues;
                clamp_index(&mut self.selected_repo_issue, self.repo_issues.len());
            }
            Err(error) => {
                self.repo_issues.clear();
                self.status = format!("repo issues: {error}");
            }
        }
        match self.list_repo_patches_native(&repo.owner, &repo.dtag).await {
            Ok(patches) => {
                self.repo_patches = patches;
                clamp_index(&mut self.selected_repo_patch, self.repo_patches.len());
            }
            Err(error) => {
                self.repo_patches.clear();
                self.status = format!("repo patches: {error}");
            }
        }
    }

    pub async fn refresh_selected_workflow_detail(&mut self) {
        let Some(workflow) = self.workflows.get(self.selected_workflow).cloned() else {
            self.status = "No workflow selected".to_string();
            return;
        };
        match self.get_workflow_native(&workflow.workflow_id).await {
            Ok(Some(detail)) => {
                self.selected_workflow_detail = Some(detail);
                self.status = format!("Loaded workflow {}", short_id(&workflow.workflow_id));
            }
            Ok(None) => self.status = "Workflow not found".to_string(),
            Err(error) => self.status = format!("workflow get: {error}"),
        }
    }

    pub async fn focus_memory(&mut self) {
        let Some((pubkey, name)) = self.selected_managed_agent_identity() else {
            self.status = "Select a managed agent before opening memory".to_string();
            return;
        };

        match self.list_memory_native(&pubkey).await {
            Ok(mut memories) => {
                self.memory_agent_pubkey = pubkey.clone();
                self.memory_agent_name = name;
                self.memories.clear();
                if let Ok(value) = self.get_memory_native(&pubkey, "core").await {
                    self.memories.push(MemoryEntry {
                        slug: "core".to_string(),
                        event_id: String::new(),
                        created_at: 0,
                        value,
                    });
                }
                memories.sort_by(|a, b| a.slug.cmp(&b.slug));
                self.memories.extend(memories);
                clamp_index(&mut self.selected_memory, self.memories.len());
                self.focus = Focus::Memory;
                self.refresh_selected_memory_value().await;
                self.status = format!(
                    "Loaded {} memor{} for {}",
                    self.memories.len(),
                    if self.memories.len() == 1 { "y" } else { "ies" },
                    self.memory_agent_name
                );
            }
            Err(error) => self.status = format!("memory: {error}"),
        }
    }

    pub async fn refresh_selected_memory_value(&mut self) {
        let Some(memory) = self.memories.get(self.selected_memory).cloned() else {
            return;
        };
        if !memory.value.is_empty() {
            return;
        }
        if self.memory_agent_pubkey.trim().is_empty() {
            return;
        }
        match self
            .get_memory_native(&self.memory_agent_pubkey, &memory.slug)
            .await
        {
            Ok(value) => {
                if let Some(entry) = self.memories.get_mut(self.selected_memory) {
                    if entry.slug == memory.slug {
                        entry.value = value;
                    }
                }
            }
            Err(error) => self.status = format!("memory get: {error}"),
        }
    }

    pub fn focus_create_memory(&mut self) {
        if self.memory_agent_pubkey.trim().is_empty() {
            self.status = "Open a managed agent memory panel first".to_string();
            return;
        }
        self.clear_memory_editor();
        self.memory_edit_existing = false;
        self.focus = Focus::MemoryEdit;
        self.status = "Creating memory".to_string();
    }

    pub fn focus_edit_memory(&mut self) {
        let Some(memory) = self.memories.get(self.selected_memory).cloned() else {
            self.status = "No memory selected".to_string();
            return;
        };
        self.memory_edit_existing = true;
        self.memory_edit_field = MemoryEditField::Value;
        self.memory_slug = memory.slug;
        self.memory_value = memory.value;
        self.focus = Focus::MemoryEdit;
        self.status = "Editing memory".to_string();
    }

    pub fn focus_patch_memory(&mut self) {
        let Some(memory) = self.memories.get(self.selected_memory).cloned() else {
            self.status = "No memory selected".to_string();
            return;
        };
        self.clear_memory_patch_inputs();
        self.memory_slug = memory.slug;
        self.focus = Focus::MemoryPatch;
        self.status = "Patching memory; use H first to get the current base hash".to_string();
    }

    pub async fn save_memory(&mut self) {
        let slug = self.memory_slug.trim().to_string();
        if slug.is_empty() {
            self.status = "Memory slug is empty".to_string();
            self.memory_edit_field = MemoryEditField::Slug;
            return;
        }
        if self.memory_value.is_empty() {
            self.status = "Memory value is empty".to_string();
            self.memory_edit_field = MemoryEditField::Value;
            return;
        }
        let (private_key, auth_tag) =
            match selected_agent_memory_identity(&self.acp, self.selected_agent) {
                Ok(identity) => identity,
                Err(error) => {
                    self.status = error.to_string();
                    return;
                }
            };
        match self
            .set_memory_native(Some(private_key), Some(auth_tag), &slug, &self.memory_value)
            .await
        {
            Ok(()) => {
                self.clear_memory_editor();
                self.focus_memory().await;
                if let Some(index) = self.memories.iter().position(|memory| memory.slug == slug) {
                    self.selected_memory = index;
                    self.refresh_selected_memory_value().await;
                }
                self.status = format!("Saved memory {slug}");
            }
            Err(error) => {
                self.status = format!("memory save: {error}");
            }
        }
    }

    pub async fn delete_selected_memory(&mut self) {
        let Some(memory) = self.memories.get(self.selected_memory).cloned() else {
            self.status = "No memory selected".to_string();
            return;
        };
        if memory.slug == "core" {
            self.status = "Core memory cannot be tombstoned".to_string();
            return;
        }
        let (private_key, auth_tag) =
            match selected_agent_memory_identity(&self.acp, self.selected_agent) {
                Ok(identity) => identity,
                Err(error) => {
                    self.status = error.to_string();
                    return;
                }
            };
        match self
            .remove_memory_native(Some(private_key), Some(auth_tag), &memory.slug)
            .await
        {
            Ok(()) => {
                self.memories.remove(self.selected_memory);
                clamp_index(&mut self.selected_memory, self.memories.len());
                self.status = format!("Removed memory {}", memory.slug);
            }
            Err(error) => {
                self.status = format!("memory remove: {error}");
            }
        }
    }

    pub async fn show_selected_memory_hash(&mut self) {
        let Some(memory) = self.memories.get(self.selected_memory).cloned() else {
            self.status = "No memory selected".to_string();
            return;
        };
        if self.memory_agent_pubkey.trim().is_empty() {
            self.status = "Open a managed agent memory panel first".to_string();
            return;
        }
        match self
            .memory_hash_native(&self.memory_agent_pubkey, &memory.slug)
            .await
        {
            Ok(hash) => self.status = format!("sha256({}) = {hash}", memory.slug),
            Err(error) => self.status = format!("memory hash: {error}"),
        }
    }

    pub async fn apply_memory_patch(&mut self) {
        let slug = self.memory_slug.trim().to_string();
        if slug.is_empty() {
            self.status = "Memory slug is empty".to_string();
            return;
        }
        let base_hash = self.memory_patch_base_hash.trim().to_string();
        if base_hash.is_empty() {
            self.status = "Base hash is empty".to_string();
            self.memory_patch_field = MemoryPatchField::BaseHash;
            return;
        }
        let patch = self.memory_patch_content.trim().to_string();
        if patch.is_empty() {
            self.status = "Patch is empty".to_string();
            self.memory_patch_field = MemoryPatchField::Patch;
            return;
        }
        let (private_key, auth_tag) =
            match selected_agent_memory_identity(&self.acp, self.selected_agent) {
                Ok(identity) => identity,
                Err(error) => {
                    self.status = error.to_string();
                    return;
                }
            };
        match self
            .patch_memory_native(Some(private_key), Some(auth_tag), &slug, &patch, &base_hash)
            .await
        {
            Ok(new_hash) => {
                self.clear_memory_patch_inputs();
                self.focus_memory().await;
                if let Some(index) = self.memories.iter().position(|memory| memory.slug == slug) {
                    self.selected_memory = index;
                    self.refresh_selected_memory_value().await;
                }
                self.status = format!("Patched memory {slug}; new sha256 {new_hash}");
            }
            Err(error) => self.status = format!("memory patch: {error}"),
        }
    }

    pub fn memory_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.selected_memory_input_mut().push(ch);
        }
    }

    pub fn memory_input_pop(&mut self) {
        self.selected_memory_input_mut().pop();
    }

    pub fn memory_patch_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.selected_memory_patch_input_mut().push(ch);
        }
    }

    pub fn memory_patch_input_pop(&mut self) {
        self.selected_memory_patch_input_mut().pop();
    }

    pub fn next_memory_edit_field(&mut self) {
        self.memory_edit_field = match self.memory_edit_field {
            MemoryEditField::Slug => MemoryEditField::Value,
            MemoryEditField::Value => MemoryEditField::Slug,
        };
    }

    pub fn previous_memory_edit_field(&mut self) {
        self.next_memory_edit_field();
    }

    pub fn next_memory_patch_field(&mut self) {
        self.memory_patch_field = match self.memory_patch_field {
            MemoryPatchField::BaseHash => MemoryPatchField::Patch,
            MemoryPatchField::Patch => MemoryPatchField::BaseHash,
        };
    }

    pub fn previous_memory_patch_field(&mut self) {
        self.next_memory_patch_field();
    }

    pub async fn focus_emoji(&mut self) {
        match (
            self.workspace_emoji_native().await,
            self.own_emoji_native().await,
        ) {
            (Ok(workspace), Ok(own)) => {
                self.workspace_emoji = workspace;
                self.own_emoji = own;
                let emoji_count = self.emoji_count();
                clamp_index(&mut self.selected_emoji, emoji_count);
                self.focus = Focus::Emoji;
                self.status = format!(
                    "Loaded {} workspace emoji, {} own emoji",
                    self.workspace_emoji.len(),
                    self.own_emoji.len()
                );
            }
            (Err(error), _) => self.status = format!("emoji palette: {error}"),
            (_, Err(error)) => self.status = format!("own emoji: {error}"),
        }
    }

    pub fn focus_add_emoji(&mut self) {
        self.emoji_shortcode.clear();
        self.emoji_url.clear();
        self.emoji_edit_field = EmojiEditField::Shortcode;
        self.focus = Focus::EmojiEdit;
        self.status = "Adding custom emoji".to_string();
    }

    pub fn focus_import_emoji(&mut self) {
        self.clear_emoji_import();
        self.focus = Focus::EmojiImport;
        self.status = "Type an emoji JSON file path, Enter imports it".to_string();
    }

    pub async fn import_emoji(&mut self) {
        let file = self.emoji_import_path.trim().to_string();
        if file.is_empty() {
            self.status = "Emoji import path is empty".to_string();
            return;
        }

        let replace = self.emoji_import_replace;
        match self.import_emoji_native(&file, replace).await {
            Ok(_) => {
                self.clear_emoji_import();
                self.focus_emoji().await;
                self.status = format!("Imported emoji from {file}");
            }
            Err(error) => self.status = format!("emoji import: {error}"),
        }
    }

    pub fn toggle_emoji_import_replace(&mut self) {
        self.emoji_import_replace = !self.emoji_import_replace;
        let mode = if self.emoji_import_replace {
            "replace"
        } else {
            "merge"
        };
        self.status = format!("Emoji import mode: {mode}");
    }

    pub async fn save_emoji(&mut self) {
        let shortcode = self.emoji_shortcode.trim().trim_matches(':').to_string();
        let url = self.emoji_url.trim().to_string();
        if shortcode.is_empty() {
            self.status = "Emoji shortcode is empty".to_string();
            self.emoji_edit_field = EmojiEditField::Shortcode;
            return;
        }
        if url.is_empty() {
            self.status = "Emoji URL is empty".to_string();
            self.emoji_edit_field = EmojiEditField::Url;
            return;
        }

        match self.set_emoji_native(&shortcode, &url).await {
            Ok(_) => {
                self.clear_emoji_inputs();
                self.focus_emoji().await;
                if let Some(index) = self
                    .own_emoji
                    .iter()
                    .position(|emoji| emoji.shortcode == shortcode)
                {
                    self.selected_emoji = index;
                }
                self.status = format!("Saved :{shortcode}:");
            }
            Err(error) => self.status = format!("emoji save: {error}"),
        }
    }

    pub async fn remove_selected_emoji(&mut self) {
        let Some(shortcode) = self.selected_own_emoji_shortcode() else {
            self.status = "Only your own emoji can be removed".to_string();
            return;
        };
        match self.remove_emoji_native(&shortcode).await {
            Ok(_) => {
                self.focus_emoji().await;
                self.status = format!("Removed :{shortcode}:");
            }
            Err(error) => self.status = format!("emoji remove: {error}"),
        }
    }

    pub async fn export_workspace_emoji(&mut self) {
        let path = std::env::temp_dir().join("buzz-tui-emoji-workspace.json");
        match self.export_emoji_json_native().await {
            Ok(json) => match std::fs::write(&path, json) {
                Ok(()) => self.status = format!("Exported workspace emoji to {}", path.display()),
                Err(error) => self.status = format!("emoji export write: {error}"),
            },
            Err(error) => self.status = format!("emoji export: {error}"),
        }
    }

    pub async fn add_selected_emoji_reaction(&mut self) {
        let Some(emoji) = self.selected_emoji_entry() else {
            self.status = "No emoji selected".to_string();
            return;
        };
        let Some(message) = self.selected_timeline_message() else {
            self.status = "No message selected".to_string();
            return;
        };
        if message.id.is_empty() {
            self.status = "Selected message has no event id".to_string();
            return;
        }

        match self
            .add_reaction_to_event(&message.id, &emoji.shortcode, Some(&emoji.url))
            .await
        {
            Ok(_) => {
                let shortcode = emoji.shortcode;
                self.refresh_selected_message_reactions().await;
                self.status = format!("Reacted :{shortcode}: on {}", short_id(&message.id));
            }
            Err(error) => self.status = format!("custom reaction: {error}"),
        }
    }

    pub fn emoji_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.selected_emoji_input_mut().push(ch);
        }
    }

    pub fn emoji_input_pop(&mut self) {
        self.selected_emoji_input_mut().pop();
    }

    pub fn emoji_import_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.emoji_import_path.push(ch);
        }
    }

    pub fn emoji_import_pop(&mut self) {
        self.emoji_import_path.pop();
    }

    pub fn next_emoji_edit_field(&mut self) {
        self.emoji_edit_field = match self.emoji_edit_field {
            EmojiEditField::Shortcode => EmojiEditField::Url,
            EmojiEditField::Url => EmojiEditField::Shortcode,
        };
    }

    pub fn previous_emoji_edit_field(&mut self) {
        self.next_emoji_edit_field();
    }

    async fn run_search(&mut self) {
        let query = self.search_query.trim().to_string();
        if query.is_empty() {
            self.status = "Search query is empty".to_string();
            return;
        }

        match self.search_messages(&query).await {
            Ok(results) => {
                self.remember_message_author_profiles(&results).await;
                self.search_results = results;
                self.selected_search_result = 0;
                self.reset_message_detail_scroll();
                self.timeline_mode = TimelineMode::Search;
                self.focus = Focus::Timeline;
                self.refresh_selected_message_reactions().await;
                self.status = format!(
                    "Found {} message{} for {query:?}",
                    self.search_results.len(),
                    if self.search_results.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
            }
            Err(error) => self.status = format!("search: {error}"),
        }
    }

    pub fn focus_channel_search(&mut self) {
        self.focus = Focus::ChannelSearch;
        self.status = "Channel search".to_string();
    }

    pub fn channel_search_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.channel_search_query.push(ch);
        }
    }

    pub fn channel_search_pop(&mut self) {
        self.channel_search_query.pop();
    }

    async fn activate_channel_search(&mut self) {
        let query = self.channel_search_query.trim().to_string();
        if query.is_empty() {
            self.status = "Channel search query is empty".to_string();
            return;
        }
        if !self.channel_search_results.is_empty() && query == self.channel_search_last_query {
            self.open_selected_channel_search_result().await;
            return;
        }

        match self.search_channels_native(&query).await {
            Ok(results) => {
                self.channel_search_results = results;
                self.selected_channel_search = 0;
                self.channel_search_last_query = query.clone();
                self.status = format!(
                    "Found {} channel{} for {query:?}",
                    self.channel_search_results.len(),
                    if self.channel_search_results.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
            }
            Err(error) => self.status = format!("channel search: {error}"),
        }
    }

    pub fn next_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Timeline,
            Focus::Timeline => Focus::Composer,
            Focus::Composer => Focus::Agents,
            Focus::Attachment => Focus::Timeline,
            Focus::Diff => Focus::Timeline,
            Focus::Agents => Focus::Sidebar,
            Focus::Search => Focus::Timeline,
            Focus::ChannelSearch => Focus::Sidebar,
            Focus::CreateChannel => Focus::Sidebar,
            Focus::DirectMessage => Focus::Sidebar,
            Focus::ChannelName
            | Focus::ChannelDescription
            | Focus::ChannelTopic
            | Focus::ChannelPurpose
            | Focus::ChannelSectionAssign
            | Focus::AddMember
            | Focus::RemoveMember
            | Focus::Canvas
            | Focus::CanvasEdit
            | Focus::Workflows
            | Focus::WorkflowEdit
            | Focus::WorkflowInputs
            | Focus::WorkflowApproval
            | Focus::Notes
            | Focus::NoteEdit
            | Focus::Reminders
            | Focus::ReminderCreate
            | Focus::CreateAgent
            | Focus::Profile
            | Focus::ProfileEdit
            | Focus::ProfileAvatarUpload
            | Focus::Contacts
            | Focus::ContactAdd
            | Focus::UserLookup
            | Focus::UserProfile
            | Focus::RelayMembers
            | Focus::AddRelayMember
            | Focus::RemoveRelayMember
            | Focus::ChangeRelayMemberRole
            | Focus::Repos
            | Focus::RepoCreate
            | Focus::RepoIssueCreate
            | Focus::RepoPatchCreate
            | Focus::Memory
            | Focus::MemoryEdit
            | Focus::MemoryPatch
            | Focus::Workspaces
            | Focus::WorkspaceAdd
            | Focus::Emoji
            | Focus::EmojiEdit
            | Focus::EmojiImport
            | Focus::Feed
            | Focus::Pulse
            | Focus::CommandPalette
            | Focus::Confirm
            | Focus::Help => Focus::Sidebar,
        };
    }

    pub fn previous_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Agents,
            Focus::Timeline => Focus::Sidebar,
            Focus::Composer => Focus::Timeline,
            Focus::Attachment => Focus::Timeline,
            Focus::Diff => Focus::Timeline,
            Focus::Agents => Focus::Composer,
            Focus::Search => Focus::Timeline,
            Focus::ChannelSearch => Focus::Sidebar,
            Focus::CreateChannel => Focus::Sidebar,
            Focus::DirectMessage => Focus::Sidebar,
            Focus::ChannelName
            | Focus::ChannelDescription
            | Focus::ChannelTopic
            | Focus::ChannelPurpose
            | Focus::ChannelSectionAssign
            | Focus::AddMember
            | Focus::RemoveMember
            | Focus::Canvas
            | Focus::CanvasEdit
            | Focus::Workflows
            | Focus::WorkflowEdit
            | Focus::WorkflowInputs
            | Focus::WorkflowApproval
            | Focus::Notes
            | Focus::NoteEdit
            | Focus::Reminders
            | Focus::ReminderCreate
            | Focus::CreateAgent
            | Focus::Profile
            | Focus::ProfileEdit
            | Focus::ProfileAvatarUpload
            | Focus::Contacts
            | Focus::ContactAdd
            | Focus::UserLookup
            | Focus::UserProfile
            | Focus::RelayMembers
            | Focus::AddRelayMember
            | Focus::RemoveRelayMember
            | Focus::ChangeRelayMemberRole
            | Focus::Repos
            | Focus::RepoCreate
            | Focus::RepoIssueCreate
            | Focus::RepoPatchCreate
            | Focus::Memory
            | Focus::MemoryEdit
            | Focus::MemoryPatch
            | Focus::Workspaces
            | Focus::WorkspaceAdd
            | Focus::Emoji
            | Focus::EmojiEdit
            | Focus::EmojiImport
            | Focus::Feed
            | Focus::Pulse
            | Focus::CommandPalette
            | Focus::Confirm
            | Focus::Help => Focus::Sidebar,
        };
    }

    pub fn focus_help(&mut self) {
        self.focus = Focus::Help;
        self.status = "Help shows the full key map; Esc returns".to_string();
    }

    pub fn focus_search(&mut self) {
        self.focus = Focus::Search;
        self.timeline_mode = TimelineMode::Search;
    }

    pub fn search_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.search_query.push(ch);
        }
    }

    pub fn search_pop(&mut self) {
        self.search_query.pop();
    }

    pub fn focus_create_channel(&mut self) {
        self.focus = Focus::CreateChannel;
        self.clear_new_channel_inputs();
    }

    pub fn new_channel_push(&mut self, ch: char) {
        if !matches!(
            self.new_channel_field,
            CreateChannelField::Name | CreateChannelField::Description
        ) {
            return;
        }
        if ch != '\n' && ch != '\r' {
            if let Some(input) = self.selected_new_channel_input_mut() {
                input.push(ch);
            }
        }
    }

    pub fn new_channel_pop(&mut self) {
        if !matches!(
            self.new_channel_field,
            CreateChannelField::Name | CreateChannelField::Description
        ) {
            return;
        }
        if let Some(input) = self.selected_new_channel_input_mut() {
            input.pop();
        }
    }

    pub fn next_create_channel_field(&mut self) {
        self.new_channel_field = match self.new_channel_field {
            CreateChannelField::Name => CreateChannelField::Type,
            CreateChannelField::Type => CreateChannelField::Visibility,
            CreateChannelField::Visibility => CreateChannelField::Expiry,
            CreateChannelField::Expiry => CreateChannelField::Description,
            CreateChannelField::Description => CreateChannelField::Name,
        };
    }

    pub fn previous_create_channel_field(&mut self) {
        self.new_channel_field = match self.new_channel_field {
            CreateChannelField::Name => CreateChannelField::Description,
            CreateChannelField::Type => CreateChannelField::Name,
            CreateChannelField::Visibility => CreateChannelField::Type,
            CreateChannelField::Expiry => CreateChannelField::Visibility,
            CreateChannelField::Description => CreateChannelField::Expiry,
        };
    }

    pub fn cycle_new_channel_type(&mut self) {
        self.new_channel_type = self.new_channel_type.next();
        self.status = format!("New channel type {}", self.new_channel_type.label());
    }

    pub fn cycle_new_channel_visibility(&mut self) {
        self.new_channel_visibility = self.new_channel_visibility.next();
        self.status = format!(
            "New channel visibility {}",
            self.new_channel_visibility.label()
        );
    }

    pub fn cycle_new_channel_expiry(&mut self) {
        self.new_channel_expiry = self.new_channel_expiry.next();
        self.status = format!("New channel expiry {}", self.new_channel_expiry.label());
    }

    pub fn focus_direct_message(&mut self) {
        self.focus = Focus::DirectMessage;
        self.dm_pubkey.clear();
    }

    pub fn dm_pubkey_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.dm_pubkey.push(ch);
        }
    }

    pub fn dm_pubkey_pop(&mut self) {
        self.dm_pubkey.pop();
    }

    pub async fn focus_relay_members(&mut self) {
        self.focus = Focus::RelayMembers;
        self.refresh_relay_members().await;
    }

    pub async fn refresh_relay_members(&mut self) {
        match self.list_relay_members_native().await {
            Ok(members) => {
                self.relay_members = members;
                clamp_index(&mut self.selected_relay_member, self.relay_members.len());
                self.status = if self.relay_members.is_empty() {
                    "No relay members found; open relays may not publish NIP-43 membership"
                        .to_string()
                } else {
                    format!("Loaded {} relay members", self.relay_members.len())
                };
            }
            Err(error) => self.status = format!("relay members: {error}"),
        }
    }

    pub fn focus_add_relay_member(&mut self) {
        self.relay_member_input.clear();
        self.focus = Focus::AddRelayMember;
        self.status = "Type pubkey [member|admin]".to_string();
    }

    pub fn focus_change_relay_member_role(&mut self) {
        let Some(member) = self.relay_members.get(self.selected_relay_member) else {
            self.status = "No relay member selected".to_string();
            return;
        };
        self.relay_member_input = format!("{} {}", member.pubkey, member.role);
        self.focus = Focus::ChangeRelayMemberRole;
        self.status = "Edit to 'pubkey member' or 'pubkey admin'".to_string();
    }

    pub fn focus_remove_relay_member(&mut self) {
        let Some(member) = self.relay_members.get(self.selected_relay_member) else {
            self.status = "No relay member selected".to_string();
            return;
        };
        self.relay_member_input = member.pubkey.clone();
        self.focus = Focus::RemoveRelayMember;
        self.status = "Enter confirms selected relay member removal".to_string();
    }

    pub fn relay_member_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.relay_member_input.push(ch);
        }
    }

    pub fn relay_member_input_pop(&mut self) {
        self.relay_member_input.pop();
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub async fn escape(&mut self) {
        if self.focus == Focus::CommandPalette {
            self.close_palette();
            return;
        }
        if self.focus == Focus::Confirm {
            self.cancel_confirm();
            return;
        }
        self.composer_completion = None;
        if self.focus == Focus::Attachment {
            self.attachment_input.clear();
            self.focus = Focus::Timeline;
            return;
        }
        if self.focus == Focus::Diff {
            self.clear_diff_inputs();
            self.focus = Focus::Timeline;
            return;
        }
        if self.focus == Focus::Search {
            self.focus = Focus::Timeline;
            return;
        }
        if self.focus == Focus::ChannelSearch {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::CreateChannel {
            self.clear_new_channel_inputs();
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::DirectMessage {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::CreateAgent {
            self.clear_new_agent_inputs();
            self.focus = Focus::Agents;
            return;
        }
        if self.focus == Focus::ProfileEdit {
            self.profile_input.clear();
            self.focus = Focus::Profile;
            return;
        }
        if self.focus == Focus::ProfileAvatarUpload {
            self.profile_upload_path.clear();
            self.focus = Focus::Profile;
            return;
        }
        if self.focus == Focus::Profile {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::Contacts {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::ContactAdd {
            self.contact_input.clear();
            self.focus = Focus::Contacts;
            return;
        }
        if self.focus == Focus::UserLookup {
            self.user_lookup_input.clear();
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::UserProfile {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::RelayMembers {
            self.focus = Focus::Sidebar;
            return;
        }
        if matches!(
            self.focus,
            Focus::AddRelayMember | Focus::RemoveRelayMember | Focus::ChangeRelayMemberRole
        ) {
            self.relay_member_input.clear();
            self.focus = Focus::RelayMembers;
            return;
        }
        if self.focus == Focus::Repos {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::RepoCreate {
            self.clear_repo_inputs();
            self.focus = Focus::Repos;
            return;
        }
        if self.focus == Focus::RepoIssueCreate {
            self.clear_repo_issue_inputs();
            self.focus = Focus::Repos;
            return;
        }
        if self.focus == Focus::RepoPatchCreate {
            self.clear_repo_patch_inputs();
            self.focus = Focus::Repos;
            return;
        }
        if self.focus == Focus::Memory {
            self.focus = Focus::Agents;
            return;
        }
        if self.focus == Focus::MemoryEdit {
            self.clear_memory_editor();
            self.focus = Focus::Memory;
            return;
        }
        if self.focus == Focus::MemoryPatch {
            self.clear_memory_patch_inputs();
            self.focus = Focus::Memory;
            return;
        }
        if self.focus == Focus::Workspaces {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::WorkspaceAdd {
            self.workspace_input.clear();
            self.focus = Focus::Workspaces;
            return;
        }
        if self.focus == Focus::Emoji {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::EmojiEdit {
            self.clear_emoji_inputs();
            self.focus = Focus::Emoji;
            return;
        }
        if self.focus == Focus::EmojiImport {
            self.clear_emoji_import();
            self.focus = Focus::Emoji;
            return;
        }
        if self.focus == Focus::Help {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::Feed {
            self.timeline_mode = TimelineMode::Channel;
            self.focus = Focus::Sidebar;
            self.refresh_selected_message_reactions().await;
            return;
        }
        if self.focus == Focus::Pulse {
            self.timeline_mode = TimelineMode::Channel;
            self.focus = Focus::Sidebar;
            self.refresh_selected_message_reactions().await;
            return;
        }
        if matches!(
            self.focus,
            Focus::ChannelName
                | Focus::ChannelDescription
                | Focus::ChannelTopic
                | Focus::ChannelPurpose
                | Focus::ChannelSectionAssign
                | Focus::AddMember
                | Focus::RemoveMember
        ) {
            self.channel_action_input.clear();
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::CanvasEdit {
            self.canvas_draft = self.canvas_content.clone();
            self.focus = Focus::Canvas;
            return;
        }
        if self.focus == Focus::Canvas {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::Workflows {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::WorkflowEdit {
            self.clear_workflow_editor();
            self.focus = Focus::Workflows;
            return;
        }
        if self.focus == Focus::WorkflowInputs {
            self.workflow_inputs.clear();
            self.workflow_edit_id = None;
            self.focus = Focus::Workflows;
            return;
        }
        if self.focus == Focus::WorkflowApproval {
            self.clear_workflow_approval();
            self.focus = Focus::Workflows;
            return;
        }
        if self.focus == Focus::Notes {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::Reminders {
            self.focus = Focus::Sidebar;
            return;
        }
        if self.focus == Focus::ReminderCreate {
            self.clear_reminder_draft();
            self.focus = Focus::Reminders;
            return;
        }
        if self.focus == Focus::NoteEdit {
            self.clear_note_inputs();
            self.focus = Focus::Notes;
            return;
        }
        if self.focus == Focus::Composer && self.edit_target.take().is_some() {
            self.composer.clear();
            self.composer_cursor = 0;
            self.focus = Focus::Timeline;
            return;
        }
        if self.focus == Focus::Composer && self.timeline_mode == TimelineMode::Pulse {
            self.composer.clear();
            self.composer_cursor = 0;
            self.pulse_reply_target = None;
            self.focus = Focus::Pulse;
            return;
        }
        if self.timeline_mode == TimelineMode::Search {
            self.timeline_mode = TimelineMode::Channel;
            self.focus = Focus::Timeline;
            self.refresh_selected_message_reactions().await;
            return;
        }
        if self.timeline_mode == TimelineMode::Feed {
            self.timeline_mode = TimelineMode::Channel;
            self.focus = Focus::Timeline;
            self.refresh_selected_message_reactions().await;
            return;
        }
        if self.timeline_mode == TimelineMode::Pulse {
            self.timeline_mode = TimelineMode::Channel;
            self.focus = Focus::Timeline;
            self.refresh_selected_message_reactions().await;
            return;
        }
        if self.thread_root.is_some() && self.thread_context.is_some() {
            let return_mode = self
                .thread_context
                .as_ref()
                .map(|context| context.return_mode)
                .unwrap_or(TimelineMode::Search);
            self.thread_root = None;
            self.thread_context = None;
            self.timeline_mode = return_mode;
            self.focus = match return_mode {
                TimelineMode::Feed => Focus::Feed,
                TimelineMode::Pulse => Focus::Pulse,
                TimelineMode::Channel | TimelineMode::Search => Focus::Timeline,
            };
            return;
        }
        if self.thread_root.take().is_some() {
            self.thread_context = None;
            self.load_selected_channel().await;
        } else {
            self.focus = Focus::Sidebar;
        }
    }

    pub fn active_channel(&self) -> Option<Channel> {
        self.thread_context
            .as_ref()
            .map(|context| Channel {
                id: context.channel_id.clone(),
                name: context.channel_name.clone(),
                description: String::new(),
                channel_type: String::new(),
                visibility: String::new(),
                archived: false,
                topic: String::new(),
                purpose: String::new(),
                owner_pubkey: String::new(),
                created_at: 0,
                kind: ConversationKind::Channel,
            })
            .or_else(|| {
                self.active_channel_id.as_ref().and_then(|active_id| {
                    self.channels
                        .iter()
                        .find(|channel| channel.id == *active_id)
                        .cloned()
                })
            })
            .or_else(|| self.channels.get(self.selected_channel).cloned())
    }

    fn mark_channel_read_at(&mut self, channel_id: &str, latest: u64) -> bool {
        let workspace_id = self.active_workspace_id().to_string();
        let mut changed = false;
        let mut remove_empty_manual_set = false;
        if let Some(channels) = self.workspace_config.manual_unread.get_mut(&workspace_id) {
            changed |= channels.remove(channel_id);
            if channels.is_empty() {
                remove_empty_manual_set = true;
            }
        }
        if remove_empty_manual_set {
            self.workspace_config.manual_unread.remove(&workspace_id);
        }

        let frontiers = self
            .workspace_config
            .read_frontiers
            .entry(workspace_id)
            .or_default();
        if frontiers.get(channel_id).copied().unwrap_or_default() < latest {
            frontiers.insert(channel_id.to_string(), latest);
            changed = true;
        }

        if changed {
            self.save_workspace_config("read state");
        }

        changed
    }

    fn mark_messages_read(&mut self, messages: &[Message]) -> bool {
        let workspace_id = self.active_workspace_id().to_string();
        let contexts = self
            .workspace_config
            .read_frontiers
            .entry(workspace_id)
            .or_default();
        let mut changed = false;

        for message in messages {
            if message.id.is_empty() || message.created_at == 0 {
                continue;
            }
            let key = msg_context_key(&message.id);
            if contexts.get(&key).copied().unwrap_or_default() < message.created_at {
                contexts.insert(key, message.created_at);
                changed = true;
            }
        }

        if changed {
            self.save_workspace_config("read state");
        }

        changed
    }

    async fn mark_visible_thread_messages_read(&mut self) {
        let messages = self.messages.clone();
        if !self.mark_messages_read(&messages) {
            return;
        }
        let latest = messages
            .iter()
            .map(|message| message.created_at)
            .max()
            .unwrap_or_default();
        self.publish_workspace_read_state(latest).await;
    }

    async fn sync_remote_read_state(&mut self) {
        let Ok(read_state) = self.read_state().await else {
            return;
        };
        if self.merge_remote_read_state(read_state.contexts) {
            self.save_workspace_config("read state");
        }
    }

    fn merge_remote_read_state(&mut self, contexts: BTreeMap<String, u64>) -> bool {
        let workspace_id = self.active_workspace_id().to_string();
        let channel_ids = self
            .channels
            .iter()
            .map(|channel| channel.id.as_str())
            .collect::<BTreeSet<_>>();
        let mut changed = false;

        {
            let frontiers = self
                .workspace_config
                .read_frontiers
                .entry(workspace_id.clone())
                .or_default();
            for (context, timestamp) in contexts {
                if !channel_ids.contains(context.as_str()) && !is_msg_context_key(&context) {
                    continue;
                }
                if frontiers.get(&context).copied().unwrap_or_default() < timestamp {
                    frontiers.insert(context, timestamp);
                    changed = true;
                }
            }
        }

        if changed {
            let frontiers = self
                .workspace_config
                .read_frontiers
                .get(&workspace_id)
                .cloned()
                .unwrap_or_default();
            let mut remove_empty_manual_set = false;
            if let Some(channels) = self.workspace_config.manual_unread.get_mut(&workspace_id) {
                channels.retain(|channel_id| {
                    let latest = self
                        .channel_latest_seen
                        .get(channel_id)
                        .copied()
                        .unwrap_or_default();
                    let frontier = frontiers.get(channel_id).copied().unwrap_or_default();
                    latest > frontier
                });
                remove_empty_manual_set = channels.is_empty();
            }
            if remove_empty_manual_set {
                self.workspace_config.manual_unread.remove(&workspace_id);
            }
        }

        changed
    }

    async fn sync_channel_preferences(&mut self) {
        if let Ok(ids) = self
            .channel_preference_ids(ChannelPreferenceKind::Stars)
            .await
        {
            self.starred_channel_ids = ids;
        }
        if let Ok(ids) = self
            .channel_preference_ids(ChannelPreferenceKind::Mutes)
            .await
        {
            self.muted_channel_ids = ids;
        }
    }

    async fn sync_channel_sections(&mut self) {
        let Ok(mut store) = self.channel_sections_store().await else {
            return;
        };
        store.sections.sort_by_key(|section| section.order);
        let section_ids = store
            .sections
            .iter()
            .map(|section| section.id.as_str())
            .collect::<BTreeSet<_>>();
        store
            .assignments
            .retain(|_, section_id| section_ids.contains(section_id.as_str()));
        self.channel_sections = store.sections;
        self.channel_section_assignments = store.assignments;
    }

    async fn publish_channel_read_at(&mut self, channel_id: &str, latest: u64) -> bool {
        if latest == 0 {
            return true;
        }
        let _ = channel_id;
        self.publish_workspace_read_state(latest).await
    }

    async fn publish_workspace_read_state(&mut self, created_at: u64) -> bool {
        let contexts = self
            .workspace_config
            .read_frontiers
            .get(self.active_workspace_id())
            .cloned()
            .unwrap_or_default();
        if contexts.is_empty() {
            return true;
        }
        match self
            .publish_read_state_native(contexts, Some(created_at))
            .await
        {
            Ok(()) => true,
            Err(error) => {
                self.status = format!("read-state sync: {error}");
                false
            }
        }
    }

    async fn publish_read_state_native(
        &self,
        contexts: BTreeMap<String, u64>,
        created_at: Option<u64>,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        let event = client
            .build_read_state_event(contexts, created_at)
            .map_err(|error| error.to_string())?;
        client
            .submit_event(&event)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(super) async fn publish_channel_preference_native(
        &self,
        kind: ChannelPreferenceKind,
        channel_id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let store_kind = match kind {
            ChannelPreferenceKind::Stars => ChannelPreferenceStoreKind::Stars,
            ChannelPreferenceKind::Mutes => ChannelPreferenceStoreKind::Mutes,
        };
        let now = now_seconds();
        let entries = BTreeMap::from([(
            channel_id.to_string(),
            ChannelPreferenceEntry {
                enabled,
                updated_at: now,
            },
        )]);
        let client = self.native_relay_client()?;
        let event = client
            .build_channel_preference_event(store_kind, entries, Some(now))
            .map_err(|error| error.to_string())?;
        client
            .submit_event(&event)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Send a channel message through the native relay client.
    pub(super) async fn send_message_native(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .send_channel_message(channel_id, content, reply_to)
            .await
            .map_err(|error| error.to_string())
    }

    async fn edit_message(&self, event_id: &str, content: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .edit_message(event_id, content)
            .await
            .map_err(|error| error.to_string())
    }

    async fn delete_message(&self, event_id: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .delete_message(event_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn add_reaction_to_event(
        &self,
        event_id: &str,
        emoji: &str,
        emoji_url: Option<&str>,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .add_reaction(event_id, emoji, emoji_url)
            .await
            .map_err(|error| error.to_string())
    }

    async fn remove_reaction_from_event(&self, event_id: &str, emoji: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .remove_reaction(event_id, emoji)
            .await
            .map_err(|error| error.to_string())
    }

    async fn get_message_reactions(&self, event_id: &str) -> Result<Vec<Reaction>, String> {
        let client = self.native_relay_client()?;
        client
            .query_reactions(event_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn remember_message_author_profiles(&mut self, messages: &[Message]) {
        let pubkeys: Vec<String> = messages
            .iter()
            .filter_map(|message| {
                let pubkey = message.pubkey.trim();
                (!pubkey.is_empty()
                    && self.author_profile_label(pubkey).is_none()
                    && !self.author_profiles.contains_key(pubkey))
                .then(|| pubkey.to_string())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if pubkeys.is_empty() {
            return;
        }

        let Ok(client) = self.native_relay_client() else {
            return;
        };
        if let Ok(profiles) = client.user_profiles(&pubkeys).await {
            for profile in profiles {
                self.author_profiles.insert(profile.pubkey.clone(), profile);
            }
        }
    }

    pub fn author_label(&self, pubkey: &str) -> String {
        self.author_profile_label(pubkey)
            .or_else(|| {
                self.author_profiles
                    .get(pubkey)
                    .and_then(profile_display_label)
            })
            .unwrap_or_else(|| short_id(pubkey).to_string())
    }

    fn author_profile_label(&self, pubkey: &str) -> Option<String> {
        if pubkey.trim().is_empty() {
            return Some("unknown".to_string());
        }

        if let Some(contact) = self.contacts.iter().find(|contact| {
            contact.pubkey.eq_ignore_ascii_case(pubkey) && !contact.petname.trim().is_empty()
        }) {
            return Some(contact.petname.clone());
        }

        if let Some(agent) = self.acp.agents().find(|agent| {
            agent.runtime.id.eq_ignore_ascii_case(pubkey) && !agent.runtime.label.trim().is_empty()
        }) {
            return Some(agent.runtime.label.clone());
        }

        [&self.profile, &self.viewed_profile]
            .into_iter()
            .flatten()
            .find(|profile| profile.pubkey.eq_ignore_ascii_case(pubkey))
            .and_then(profile_display_label)
    }

    async fn current_profile(&self) -> Result<Option<UserProfile>, String> {
        let client = self.native_relay_client()?;
        client
            .current_profile()
            .await
            .map_err(|error| error.to_string())
    }

    async fn user_profile(&self, pubkey: &str) -> Result<Option<UserProfile>, String> {
        let client = self.native_relay_client()?;
        client
            .user_profile(pubkey)
            .await
            .map_err(|error| error.to_string())
    }

    async fn search_user_profiles(&self, query: &str) -> Result<Vec<UserProfile>, String> {
        let client = self.native_relay_client()?;
        client
            .search_user_profiles(query)
            .await
            .map_err(|error| error.to_string())
    }

    async fn set_profile_field(&self, field: ProfileField, value: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .set_profile_field(field, value)
            .await
            .map_err(|error| error.to_string())
    }

    async fn presence(&self, pubkey: &str) -> Result<Option<PresenceInfo>, String> {
        let client = self.native_relay_client()?;
        client
            .presence(pubkey)
            .await
            .map_err(|error| error.to_string())
    }

    async fn set_presence(&self, status: PresenceStatus) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .set_presence(status)
            .await
            .map_err(|error| error.to_string())
    }

    async fn list_channels(&self, joined_only: bool) -> Result<Vec<Channel>, String> {
        let client = self.native_relay_client()?;
        client
            .list_channels(joined_only)
            .await
            .map_err(|error| error.to_string())
    }

    async fn channel_detail(&self, channel_id: &str) -> Result<Option<Channel>, String> {
        let client = self.native_relay_client()?;
        client
            .channel(channel_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn channel_members(&self, channel_id: &str) -> Result<Vec<ChannelMember>, String> {
        let client = self.native_relay_client()?;
        client
            .channel_members(channel_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn list_relay_members_native(&self) -> Result<Vec<RelayMember>, String> {
        let client = self.native_relay_client()?;
        client
            .list_relay_members()
            .await
            .map_err(|error| error.to_string())
    }

    async fn add_relay_member_native(&self, pubkey: &str, role: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .add_relay_member(pubkey, role)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn remove_relay_member_native(&self, pubkey: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .remove_relay_member(pubkey)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn change_relay_member_role_native(
        &self,
        pubkey: &str,
        role: &str,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .change_relay_member_role(pubkey, role)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn read_state(&self) -> Result<ReadState, String> {
        let client = self.native_relay_client()?;
        client.read_state().await.map_err(|error| error.to_string())
    }

    async fn channel_preference_ids(
        &self,
        kind: ChannelPreferenceKind,
    ) -> Result<BTreeSet<String>, String> {
        let client = self.native_relay_client()?;
        client
            .channel_preference_ids(kind)
            .await
            .map_err(|error| error.to_string())
    }

    async fn channel_sections_store(&self) -> Result<ChannelSections, String> {
        let client = self.native_relay_client()?;
        client
            .channel_sections()
            .await
            .map_err(|error| error.to_string())
    }

    async fn get_channel_messages_native(
        &self,
        channel_id: &str,
        limit: u32,
    ) -> Result<Vec<Message>, String> {
        let client = self.native_relay_client()?;
        client
            .query_messages(&[TuiRelayClient::channel_history_filter(channel_id, limit)])
            .await
            .map(|messages| messages.into_iter().map(Message::from).collect())
            .map_err(|error| error.to_string())
    }

    async fn get_channel_messages(&self, channel_id: &str) -> Result<Vec<Message>, String> {
        self.get_channel_messages_with_limit(channel_id, 80).await
    }

    async fn get_channel_messages_with_limit(
        &self,
        channel_id: &str,
        limit: u32,
    ) -> Result<Vec<Message>, String> {
        match self.get_channel_messages_native(channel_id, limit).await {
            Ok(messages) => Ok(messages),
            Err(error) => Err(error),
        }
    }

    async fn get_thread_messages_native(
        &self,
        channel_id: &str,
        event_id: &str,
        limit: u32,
    ) -> Result<Vec<Message>, String> {
        let client = self.native_relay_client()?;
        client
            .query_messages(&[
                TuiRelayClient::thread_filter(channel_id, event_id, limit),
                TuiRelayClient::event_id_filter(event_id),
            ])
            .await
            .map(|messages| messages.into_iter().map(Message::from).collect())
            .map_err(|error| error.to_string())
    }

    async fn get_thread_messages(
        &self,
        channel_id: &str,
        event_id: &str,
    ) -> Result<Vec<Message>, String> {
        self.get_thread_messages_with_limit(channel_id, event_id, 120)
            .await
    }

    async fn get_thread_messages_with_limit(
        &self,
        channel_id: &str,
        event_id: &str,
        limit: u32,
    ) -> Result<Vec<Message>, String> {
        match self
            .get_thread_messages_native(channel_id, event_id, limit)
            .await
        {
            Ok(messages) => Ok(messages),
            Err(error) => Err(error),
        }
    }

    async fn search_messages_native(&self, query: &str) -> Result<Vec<Message>, String> {
        let client = self.native_relay_client()?;
        client
            .query_messages(&[TuiRelayClient::search_filter(query, 50)])
            .await
            .map(|messages| messages.into_iter().map(Message::from).collect())
            .map_err(|error| error.to_string())
    }

    async fn search_messages(&self, query: &str) -> Result<Vec<Message>, String> {
        match self.search_messages_native(query).await {
            Ok(messages) => Ok(messages),
            Err(error) => Err(error),
        }
    }

    async fn get_feed_messages_native(
        &self,
        feed_type: Option<&str>,
    ) -> Result<Vec<Message>, String> {
        let client = self.native_relay_client()?;
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

    async fn get_feed_messages(&self) -> Result<Vec<Message>, String> {
        let feed_type = self.feed_filter.as_cli_type();
        match self.get_feed_messages_native(feed_type).await {
            Ok(messages) => Ok(messages),
            Err(error) => Err(error),
        }
    }

    async fn list_direct_messages(&self) -> Result<Vec<Channel>, String> {
        let client = self.native_relay_client()?;
        client.list_dms(50).await.map_err(|error| error.to_string())
    }

    async fn search_channels_native(&self, query: &str) -> Result<Vec<Channel>, String> {
        let client = self.native_relay_client()?;
        client
            .search_channels(query)
            .await
            .map_err(|error| error.to_string())
    }

    async fn create_channel_native(&self, options: &CreateChannelOptions) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .create_channel(options)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn join_channel_native(&self, channel_id: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .join_channel(channel_id)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn leave_channel_native(&self, channel_id: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .leave_channel(channel_id)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn hide_dm_native(&self, channel_id: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .hide_dm(channel_id)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn open_dm_native(&self, pubkey: &str) -> Result<serde_json::Value, String> {
        let client = self.native_relay_client()?;
        client
            .open_dm(pubkey)
            .await
            .map_err(|error| error.to_string())
    }

    async fn add_dm_member_native(&self, channel_id: &str, pubkey: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .add_dm_member(channel_id, pubkey)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn archive_channel_native(&self, channel_id: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .archive_channel(channel_id)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn unarchive_channel_native(&self, channel_id: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .unarchive_channel(channel_id)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn delete_channel_native(&self, channel_id: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .delete_channel(channel_id)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn update_channel_name_native(&self, channel_id: &str, name: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .update_channel_name(channel_id, name)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn update_channel_description_native(
        &self,
        channel_id: &str,
        description: &str,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .update_channel_description(channel_id, description)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn set_channel_topic_native(&self, channel_id: &str, topic: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .set_channel_topic(channel_id, topic)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn set_channel_purpose_native(
        &self,
        channel_id: &str,
        purpose: &str,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .set_channel_purpose(channel_id, purpose)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn add_channel_member_native(
        &self,
        channel_id: &str,
        pubkey: &str,
        role: Option<&str>,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .add_channel_member(channel_id, pubkey, role)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn remove_channel_member_native(
        &self,
        channel_id: &str,
        pubkey: &str,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .remove_channel_member(channel_id, pubkey)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn set_channel_add_policy_native(&self, policy: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .set_channel_add_policy(policy)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn create_channel_section_native(&self, name: &str) -> Result<ChannelSection, String> {
        let client = self.native_relay_client()?;
        client
            .create_channel_section(name)
            .await
            .map_err(|error| error.to_string())
    }

    async fn assign_channel_section_native(
        &self,
        channel_id: &str,
        section_id: &str,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .assign_channel_section(channel_id, section_id)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn unassign_channel_section_native(&self, channel_id: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .unassign_channel_section(channel_id)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn get_canvas_native(
        &self,
        channel_id: &str,
    ) -> Result<crate::client::CanvasDocument, String> {
        let client = self.native_relay_client()?;
        client
            .get_canvas(channel_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn set_canvas_native(&self, channel_id: &str, content: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .set_canvas(channel_id, content)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn list_notes_native(&self, source: NotesSource) -> Result<Vec<Note>, String> {
        let client = self.native_relay_client()?;
        let options = ListNotesOptions {
            author: match source {
                NotesSource::Mine => NoteAuthor::Me,
                NotesSource::All => NoteAuthor::All,
            },
            tag: None,
            limit: 50,
        };
        client
            .list_notes_with(&options)
            .await
            .map_err(|error| error.to_string())
    }

    async fn set_note_native(&self, options: &LongFormNoteOptions) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .set_note(options)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn delete_note_native(&self, slug: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .delete_note(slug)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn list_repos_native(&self) -> Result<Vec<RepoProject>, String> {
        let client = self.native_relay_client()?;
        client.list_repos().await.map_err(|error| error.to_string())
    }

    async fn create_repo_native(&self, options: &CreateRepoOptions) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .create_repo(options)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn list_repo_issues_native(
        &self,
        repo_owner: &str,
        repo_id: &str,
    ) -> Result<Vec<GitIssue>, String> {
        let client = self.native_relay_client()?;
        client
            .list_repo_issues(repo_owner, repo_id, 50)
            .await
            .map_err(|error| error.to_string())
    }

    async fn list_repo_patches_native(
        &self,
        repo_owner: &str,
        repo_id: &str,
    ) -> Result<Vec<GitPatch>, String> {
        let client = self.native_relay_client()?;
        client
            .list_repo_patches(repo_owner, repo_id, 50)
            .await
            .map_err(|error| error.to_string())
    }

    async fn create_issue_native(&self, options: &CreateIssueOptions) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .create_issue(options)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn create_patch_native(&self, options: &CreatePatchOptions) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .create_patch(options)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn list_workflows_native(&self, channel_id: &str) -> Result<Vec<Workflow>, String> {
        let client = self.native_relay_client()?;
        client
            .list_workflows(channel_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn get_workflow_native(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowDetail>, String> {
        let client = self.native_relay_client()?;
        client
            .get_workflow(workflow_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn trigger_workflow_native(
        &self,
        workflow_id: &str,
        inputs: Option<&str>,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .trigger_workflow(workflow_id, inputs)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn create_workflow_native(
        &self,
        channel_id: &str,
        yaml: &str,
    ) -> Result<serde_json::Value, String> {
        let client = self.native_relay_client()?;
        client
            .create_workflow(channel_id, yaml)
            .await
            .map_err(|error| error.to_string())
    }

    async fn update_workflow_native(
        &self,
        channel_id: &str,
        workflow_id: &str,
        yaml: &str,
    ) -> Result<serde_json::Value, String> {
        let client = self.native_relay_client()?;
        client
            .update_workflow(channel_id, workflow_id, yaml)
            .await
            .map_err(|error| error.to_string())
    }

    async fn delete_workflow_native(&self, workflow_id: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .delete_workflow(workflow_id)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn approve_workflow_step_native(
        &self,
        token: &str,
        approved: bool,
        note: &str,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .approve_workflow_step(token, approved, note)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn get_workflow_runs_native(
        &self,
        workflow_id: &str,
    ) -> Result<Vec<WorkflowRun>, String> {
        let client = self.native_relay_client()?;
        client
            .get_workflow_runs(workflow_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn list_memory_native(&self, agent_pubkey: &str) -> Result<Vec<MemoryEntry>, String> {
        let client = self.native_relay_client()?;
        client
            .list_memory(agent_pubkey)
            .await
            .map_err(|error| error.to_string())
    }

    async fn get_memory_native(&self, agent_pubkey: &str, slug: &str) -> Result<String, String> {
        let client = self.native_relay_client()?;
        client
            .get_memory(agent_pubkey, slug)
            .await
            .map_err(|error| error.to_string())
    }

    async fn memory_hash_native(&self, agent_pubkey: &str, slug: &str) -> Result<String, String> {
        let client = self.native_relay_client()?;
        client
            .memory_hash(agent_pubkey, slug)
            .await
            .map_err(|error| error.to_string())
    }

    async fn set_memory_native(
        &self,
        private_key: Option<String>,
        auth_tag: Option<String>,
        slug: &str,
        value: &str,
    ) -> Result<(), String> {
        let client = self.native_relay_client_with_identity(private_key, auth_tag)?;
        client
            .set_memory(slug, value)
            .await
            .map_err(|error| error.to_string())
    }

    async fn remove_memory_native(
        &self,
        private_key: Option<String>,
        auth_tag: Option<String>,
        slug: &str,
    ) -> Result<(), String> {
        let client = self.native_relay_client_with_identity(private_key, auth_tag)?;
        client
            .remove_memory(slug)
            .await
            .map_err(|error| error.to_string())
    }

    async fn patch_memory_native(
        &self,
        private_key: Option<String>,
        auth_tag: Option<String>,
        slug: &str,
        patch: &str,
        base_hash: &str,
    ) -> Result<String, String> {
        let client = self.native_relay_client_with_identity(private_key, auth_tag)?;
        client
            .patch_memory(slug, patch, base_hash, false)
            .await
            .map_err(|error| error.to_string())
    }

    async fn workspace_emoji_native(&self) -> Result<Vec<CustomEmojiEntry>, String> {
        let client = self.native_relay_client()?;
        client
            .workspace_emoji()
            .await
            .map_err(|error| error.to_string())
    }

    async fn own_emoji_native(&self) -> Result<Vec<CustomEmojiEntry>, String> {
        let client = self.native_relay_client()?;
        client.own_emoji().await.map_err(|error| error.to_string())
    }

    async fn import_emoji_native(&self, file: &str, replace: bool) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .import_emoji(file, replace)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn export_emoji_json_native(&self) -> Result<String, String> {
        let client = self.native_relay_client()?;
        client
            .export_emoji_json()
            .await
            .map_err(|error| error.to_string())
    }

    async fn set_emoji_native(&self, shortcode: &str, url: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .set_emoji(shortcode, url)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn remove_emoji_native(&self, shortcode: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .remove_emoji(shortcode)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn send_message_with_files_native(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
        files: &[String],
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .send_channel_message_with_files(channel_id, content, reply_to, files)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn send_diff_native(
        &self,
        channel_id: &str,
        options: &crate::client::SendDiffOptions,
        reply_to: Option<&str>,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .send_diff(channel_id, options, reply_to)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn vote_message_native(&self, event_id: &str, direction: &str) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .vote_message(event_id, direction)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn upload_file_native(&self, path: &str) -> Result<crate::client::UploadedFile, String> {
        let client = self.native_relay_client()?;
        client
            .upload_file(path)
            .await
            .map_err(|error| error.to_string())
    }

    async fn contact_list_native(&self, pubkey: &str) -> Result<Vec<Contact>, String> {
        let client = self.native_relay_client()?;
        client
            .contact_list(pubkey)
            .await
            .map_err(|error| error.to_string())
    }

    async fn set_contact_list_native(&self, contacts: &[Contact]) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .set_contact_list(contacts)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn social_user_notes_native(
        &self,
        pubkey: &str,
        limit: u32,
    ) -> Result<Vec<Message>, String> {
        let client = self.native_relay_client()?;
        client
            .social_user_notes(pubkey, limit)
            .await
            .map_err(|error| error.to_string())
    }

    async fn publish_social_note_native(
        &self,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<(), String> {
        let client = self.native_relay_client()?;
        client
            .publish_social_note(content, reply_to)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn native_relay_client(&self) -> Result<TuiRelayClient, String> {
        self.native_relay_client_with_identity(self.cli.private_key(), self.cli.auth_tag())
    }

    fn native_relay_client_with_identity(
        &self,
        private_key: Option<String>,
        auth_tag: Option<String>,
    ) -> Result<TuiRelayClient, String> {
        let private_key = private_key
            .or_else(|| self.cli.private_key())
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| "BUZZ_PRIVATE_KEY is required".to_string())?;
        let auth_tag = auth_tag.or_else(|| self.cli.auth_tag());
        TuiRelayClient::new(self.cli.relay_url().to_string(), &private_key, auth_tag)
            .map_err(|error| error.to_string())
    }

    fn mark_channel_unread(&mut self, channel_id: &str) {
        let workspace_id = self.active_workspace_id().to_string();
        self.workspace_config
            .manual_unread
            .entry(workspace_id)
            .or_default()
            .insert(channel_id.to_string());
        self.save_workspace_config("read state");
    }

    fn save_workspace_config(&mut self, label: &str) {
        if let Err(error) = self.workspace_config.save(&self.workspace_store_path) {
            self.status = format!("{label}: {error}");
        }
    }

    fn set_status_once(&mut self, status: String) {
        self.status = if let Some(notice) = self.startup_notice.take() {
            format!("{notice}; {status}")
        } else {
            status
        };
    }

    async fn create_channel(&mut self) {
        let name = self.new_channel_name.trim().to_string();
        if name.is_empty() {
            self.status = "Channel name is empty".to_string();
            return;
        }

        let options = CreateChannelOptions {
            name: name.clone(),
            channel_type: self.new_channel_type.label().to_string(),
            visibility: self.new_channel_visibility.label().to_string(),
            description: self.new_channel_description.trim().to_string(),
            ttl: self.new_channel_expiry.ttl_seconds(),
        };

        match self.create_channel_native(&options).await {
            Ok(()) => {
                self.channel_scope = ChannelScope::Conversations;
                self.thread_root = None;
                self.thread_context = None;
                self.timeline_mode = TimelineMode::Channel;
                self.edit_target = None;
                self.clear_new_channel_inputs();
                self.focus = Focus::Sidebar;
                self.refresh().await;
                if let Some(index) = self
                    .channels
                    .iter()
                    .position(|channel| channel.name == name)
                {
                    self.selected_channel = index;
                    self.load_selected_channel().await;
                }
                self.status = format!(
                    "Created #{} ({}, {})",
                    name, options.channel_type, options.visibility
                );
            }
            Err(error) => self.status = format!("create channel: {error}"),
        }
    }

    async fn open_dm(&mut self) {
        let pubkey = self.dm_pubkey.trim().to_string();
        if pubkey.is_empty() {
            self.status = "DM pubkey is empty".to_string();
            return;
        }

        self.open_dm_pubkey(&pubkey).await;
        self.dm_pubkey.clear();
    }

    fn selected_new_channel_input_mut(&mut self) -> Option<&mut String> {
        match self.new_channel_field {
            CreateChannelField::Name => Some(&mut self.new_channel_name),
            CreateChannelField::Description => Some(&mut self.new_channel_description),
            CreateChannelField::Type
            | CreateChannelField::Visibility
            | CreateChannelField::Expiry => None,
        }
    }

    fn selected_note_input_mut(&mut self) -> &mut String {
        match self.note_edit_field {
            NoteEditField::Name => &mut self.note_name,
            NoteEditField::Title => &mut self.note_title,
            NoteEditField::Summary => &mut self.note_summary,
            NoteEditField::Tags => &mut self.note_tags,
            NoteEditField::Content => &mut self.note_content,
        }
    }

    fn selected_repo_input_mut(&mut self) -> &mut String {
        match self.repo_create_field {
            RepoCreateField::Id => &mut self.repo_id,
            RepoCreateField::Name => &mut self.repo_name,
            RepoCreateField::Description => &mut self.repo_description,
            RepoCreateField::CloneUrls => &mut self.repo_clone_urls,
            RepoCreateField::WebUrl => &mut self.repo_web_url,
            RepoCreateField::Relays => &mut self.repo_relays,
        }
    }

    fn selected_repo_issue_input_mut(&mut self) -> &mut String {
        match self.repo_issue_field {
            RepoIssueField::Title => &mut self.repo_issue_title,
            RepoIssueField::Labels => &mut self.repo_issue_labels,
            RepoIssueField::Content => &mut self.repo_issue_content,
        }
    }

    fn selected_repo_patch_input_mut(&mut self) -> &mut String {
        match self.repo_patch_field {
            RepoPatchField::Commit => &mut self.repo_patch_commit,
            RepoPatchField::ParentCommit => &mut self.repo_patch_parent_commit,
            RepoPatchField::Content => &mut self.repo_patch_content,
        }
    }

    fn selected_memory_input_mut(&mut self) -> &mut String {
        match self.memory_edit_field {
            MemoryEditField::Slug => &mut self.memory_slug,
            MemoryEditField::Value => &mut self.memory_value,
        }
    }

    fn selected_memory_patch_input_mut(&mut self) -> &mut String {
        match self.memory_patch_field {
            MemoryPatchField::BaseHash => &mut self.memory_patch_base_hash,
            MemoryPatchField::Patch => &mut self.memory_patch_content,
        }
    }

    fn selected_emoji_input_mut(&mut self) -> &mut String {
        match self.emoji_edit_field {
            EmojiEditField::Shortcode => &mut self.emoji_shortcode,
            EmojiEditField::Url => &mut self.emoji_url,
        }
    }

    fn clear_new_channel_inputs(&mut self) {
        self.new_channel_name.clear();
        self.new_channel_description.clear();
        self.new_channel_type = NewChannelType::Stream;
        self.new_channel_visibility = NewChannelVisibility::Open;
        self.new_channel_expiry = NewChannelExpiry::Permanent;
        self.new_channel_field = CreateChannelField::Name;
    }

    fn clear_note_inputs(&mut self) {
        self.note_edit_field = NoteEditField::Name;
        self.note_edit_existing = false;
        self.note_name.clear();
        self.note_title.clear();
        self.note_summary.clear();
        self.note_tags.clear();
        self.note_content.clear();
    }

    fn clear_repo_inputs(&mut self) {
        self.repo_create_field = RepoCreateField::Id;
        self.repo_id.clear();
        self.repo_name.clear();
        self.repo_description.clear();
        self.repo_clone_urls.clear();
        self.repo_web_url.clear();
        self.repo_relays.clear();
    }

    fn clear_repo_issue_inputs(&mut self) {
        self.repo_issue_field = RepoIssueField::Title;
        self.repo_issue_title.clear();
        self.repo_issue_labels.clear();
        self.repo_issue_content.clear();
    }

    fn clear_repo_patch_inputs(&mut self) {
        self.repo_patch_field = RepoPatchField::Content;
        self.repo_patch_commit.clear();
        self.repo_patch_parent_commit.clear();
        self.repo_patch_content.clear();
    }

    fn clear_memory_editor(&mut self) {
        self.memory_edit_existing = false;
        self.memory_edit_field = MemoryEditField::Slug;
        self.memory_slug.clear();
        self.memory_value.clear();
    }

    fn clear_memory_patch_inputs(&mut self) {
        self.memory_patch_field = MemoryPatchField::BaseHash;
        self.memory_patch_base_hash.clear();
        self.memory_patch_content.clear();
    }

    fn clear_emoji_inputs(&mut self) {
        self.emoji_edit_field = EmojiEditField::Shortcode;
        self.emoji_shortcode.clear();
        self.emoji_url.clear();
    }

    fn clear_emoji_import(&mut self) {
        self.emoji_import_path.clear();
        self.emoji_import_replace = false;
    }

    pub fn emoji_count(&self) -> usize {
        self.own_emoji.len() + self.workspace_other_emoji().len()
    }

    pub fn workspace_other_emoji(&self) -> Vec<CustomEmojiEntry> {
        self.workspace_emoji
            .iter()
            .filter(|emoji| {
                !self
                    .own_emoji
                    .iter()
                    .any(|own| own.shortcode == emoji.shortcode)
            })
            .cloned()
            .collect()
    }

    fn selected_own_emoji_shortcode(&self) -> Option<String> {
        self.own_emoji
            .get(self.selected_emoji)
            .map(|emoji| emoji.shortcode.clone())
    }

    fn selected_emoji_entry(&self) -> Option<CustomEmojiEntry> {
        if let Some(emoji) = self.own_emoji.get(self.selected_emoji) {
            return Some(emoji.clone());
        }
        let other_index = self.selected_emoji.checked_sub(self.own_emoji.len())?;
        self.workspace_other_emoji().get(other_index).cloned()
    }

    fn focus_channel_input(&mut self, kind: ChannelInputKind) {
        let Some(channel) = self.selected_sidebar_channel_for_management() else {
            return;
        };
        self.channel_action_input = match kind {
            ChannelInputKind::Name => channel.name,
            ChannelInputKind::Description => channel.description,
            ChannelInputKind::Topic => channel.topic,
            ChannelInputKind::Purpose => channel.purpose,
            ChannelInputKind::AssignSection => self
                .channel_section_name(&channel.id)
                .unwrap_or_default()
                .to_string(),
            ChannelInputKind::AddMember | ChannelInputKind::RemoveMember => String::new(),
        };
        self.focus = match kind {
            ChannelInputKind::Name => Focus::ChannelName,
            ChannelInputKind::Description => Focus::ChannelDescription,
            ChannelInputKind::Topic => Focus::ChannelTopic,
            ChannelInputKind::Purpose => Focus::ChannelPurpose,
            ChannelInputKind::AssignSection => Focus::ChannelSectionAssign,
            ChannelInputKind::AddMember => Focus::AddMember,
            ChannelInputKind::RemoveMember => Focus::RemoveMember,
        };
    }

    fn selected_sidebar_channel_for_management(&mut self) -> Option<Channel> {
        let Some(channel) = self.channels.get(self.selected_channel).cloned() else {
            self.status = "No channel selected".to_string();
            return None;
        };
        if channel.kind != ConversationKind::Channel {
            self.status = "Channel actions do not apply to DMs".to_string();
            return None;
        }
        Some(channel)
    }

    fn selected_channel_for_canvas(&mut self) -> Option<Channel> {
        let Some(channel) = self.active_channel() else {
            self.status = "No channel selected".to_string();
            return None;
        };
        if channel.kind != ConversationKind::Channel {
            self.status = "Canvas is only available for channels".to_string();
            return None;
        }
        Some(channel)
    }

    fn selected_channel_for_workflows(&mut self) -> Option<Channel> {
        let Some(channel) = self.active_channel() else {
            self.status = "No channel selected".to_string();
            return None;
        };
        if channel.kind != ConversationKind::Channel {
            self.status = "Workflows are only available for channels".to_string();
            return None;
        }
        Some(channel)
    }

    /// Validate the remove-member input, then open a confirmation overlay
    /// instead of removing immediately.
    pub fn request_remove_member_confirm(&mut self) {
        let input = self.channel_action_input.trim().to_string();
        if input.is_empty() {
            self.status = "Input is empty".to_string();
            return;
        }
        if self.selected_sidebar_channel_for_management().is_none() {
            return;
        }
        self.request_confirm(ConfirmAction::RemoveMember(input));
    }

    pub fn request_remove_relay_member_confirm(&mut self) {
        let input = self.relay_member_input.trim().to_string();
        if input.is_empty() {
            self.status = "Input is empty".to_string();
            return;
        }
        self.request_confirm(ConfirmAction::RemoveRelayMember(input));
    }

    /// Remove a channel member after the action has been confirmed.
    pub(super) async fn remove_member_confirmed(&mut self, pubkey: String) {
        let Some(channel) = self.selected_sidebar_channel_for_management() else {
            return;
        };
        match self
            .remove_channel_member_native(&channel.id, &pubkey)
            .await
        {
            Ok(_) => {
                self.channel_action_input.clear();
                self.focus = Focus::Sidebar;
                self.refresh_selected_channel_details().await;
                self.status = format!("Removed member from #{}", channel.name);
            }
            Err(error) => {
                self.status = format!("remove member: {error}");
            }
        }
    }

    pub(super) async fn remove_relay_member_confirmed(&mut self, pubkey: String) {
        match self.remove_relay_member_native(&pubkey).await {
            Ok(_) => {
                self.relay_member_input.clear();
                self.focus = Focus::RelayMembers;
                self.refresh_relay_members().await;
                self.status = format!("Removed relay member {}", short_id(&pubkey));
            }
            Err(error) => {
                self.status = format!("remove relay member: {error}");
            }
        }
    }

    async fn apply_relay_member_input(&mut self, kind: RelayMemberInputKind) {
        let input = self.relay_member_input.trim().to_string();
        if input.is_empty() {
            self.status = "Input is empty".to_string();
            return;
        }

        let result = match kind {
            RelayMemberInputKind::Add => {
                let (pubkey, role) = parse_relay_member_input(&input);
                self.add_relay_member_native(pubkey, role).await
            }
            RelayMemberInputKind::ChangeRole => {
                let (pubkey, role) = parse_relay_member_input(&input);
                self.change_relay_member_role_native(pubkey, role).await
            }
        };

        match result {
            Ok(_) => {
                let status = match kind {
                    RelayMemberInputKind::Add => "Added relay member",
                    RelayMemberInputKind::ChangeRole => "Updated relay member role",
                };
                self.relay_member_input.clear();
                self.focus = Focus::RelayMembers;
                self.refresh_relay_members().await;
                self.status = status.to_string();
            }
            Err(error) => {
                let action = match kind {
                    RelayMemberInputKind::Add => "add relay member",
                    RelayMemberInputKind::ChangeRole => "change relay member role",
                };
                self.status = format!("{action}: {error}");
            }
        }
    }

    async fn apply_channel_input(&mut self, kind: ChannelInputKind) {
        let Some(channel) = (if kind == ChannelInputKind::AddMember {
            self.channels
                .get(self.selected_channel)
                .cloned()
                .or_else(|| {
                    self.status = "No channel selected".to_string();
                    None
                })
        } else {
            self.selected_sidebar_channel_for_management()
        }) else {
            return;
        };
        let input = self.channel_action_input.trim().to_string();
        if input.is_empty() {
            self.status = "Input is empty".to_string();
            return;
        }

        if kind == ChannelInputKind::AssignSection {
            self.assign_channel_to_section(&channel, &input).await;
            return;
        }

        let result = match kind {
            ChannelInputKind::Name => self.update_channel_name_native(&channel.id, &input).await,
            ChannelInputKind::Description => {
                self.update_channel_description_native(&channel.id, &input)
                    .await
            }
            ChannelInputKind::Topic => self.set_channel_topic_native(&channel.id, &input).await,
            ChannelInputKind::Purpose => self.set_channel_purpose_native(&channel.id, &input).await,
            ChannelInputKind::AssignSection => unreachable!("handled above"),
            ChannelInputKind::AddMember => {
                let (pubkey, role) = parse_member_input(&input);
                if channel.kind == ConversationKind::DirectMessage {
                    self.add_dm_member_native(&channel.id, pubkey).await
                } else {
                    self.add_channel_member_native(&channel.id, pubkey, role)
                        .await
                }
            }
            ChannelInputKind::RemoveMember => {
                self.remove_channel_member_native(&channel.id, &input).await
            }
        };

        match result {
            Ok(_) => {
                let status = match kind {
                    ChannelInputKind::Name => format!("Renamed #{}", channel.name),
                    ChannelInputKind::Description => {
                        format!("Updated #{} description", channel.name)
                    }
                    ChannelInputKind::Topic => format!("Set #{} topic", channel.name),
                    ChannelInputKind::Purpose => format!("Set #{} purpose", channel.name),
                    ChannelInputKind::AssignSection => unreachable!("handled above"),
                    ChannelInputKind::AddMember
                        if channel.kind == ConversationKind::DirectMessage =>
                    {
                        format!("Added member to DM {}", channel.name)
                    }
                    ChannelInputKind::AddMember => format!("Added member to #{}", channel.name),
                    ChannelInputKind::RemoveMember => {
                        format!("Removed member from #{}", channel.name)
                    }
                };
                self.channel_action_input.clear();
                self.focus = Focus::Sidebar;
                self.refresh_selected_channel_details().await;
                self.status = status;
            }
            Err(error) => {
                let action = match kind {
                    ChannelInputKind::Name => "rename",
                    ChannelInputKind::Description => "description",
                    ChannelInputKind::Topic => "topic",
                    ChannelInputKind::Purpose => "purpose",
                    ChannelInputKind::AssignSection => "assign section",
                    ChannelInputKind::AddMember => "add member",
                    ChannelInputKind::RemoveMember => "remove member",
                };
                self.status = format!("{action}: {error}");
            }
        }
    }

    async fn assign_channel_to_section(&mut self, channel: &Channel, input: &str) {
        let (section_id, section_name) = match self.resolve_section_input(input) {
            Some(section) => (section.id, section.name),
            None => match self.create_channel_section_native(input).await {
                Ok(section) => {
                    self.channel_sections.push(section.clone());
                    self.channel_sections.sort_by_key(|section| section.order);
                    (section.id, section.name)
                }
                Err(error) => {
                    self.status = format!("create section: {error}");
                    return;
                }
            },
        };

        match self
            .assign_channel_section_native(&channel.id, &section_id)
            .await
        {
            Ok(_) => {
                self.channel_section_assignments
                    .insert(channel.id.clone(), section_id);
                self.channel_action_input.clear();
                self.focus = Focus::Sidebar;
                self.sync_channel_sections().await;
                self.status = format!("Moved #{} to {section_name}", channel.name);
            }
            Err(error) => self.status = format!("assign section: {error}"),
        }
    }

    fn resolve_section_input(&self, input: &str) -> Option<ChannelSection> {
        let input = input.trim();
        self.channel_sections
            .iter()
            .find(|section| section.id == input || section.name.eq_ignore_ascii_case(input))
            .cloned()
    }

    pub async fn refresh_selected_channel_details(&mut self) {
        let Some(channel) = self.channels.get(self.selected_channel).cloned() else {
            self.selected_channel_detail = None;
            self.channel_members.clear();
            return;
        };
        if channel.kind != ConversationKind::Channel {
            self.selected_channel_detail = None;
            self.channel_members.clear();
            return;
        }

        self.refresh_channel_details_for(&channel).await;
    }

    async fn refresh_channel_details_for(&mut self, channel: &Channel) {
        match self.channel_detail(&channel.id).await {
            Ok(detail) => self.selected_channel_detail = detail.or(Some(channel.clone())),
            Err(error) => {
                self.selected_channel_detail = Some(channel.clone());
                self.status = format!("channel: {error}");
            }
        }
        match self.channel_members(&channel.id).await {
            Ok(members) => self.channel_members = members,
            Err(error) => {
                self.channel_members.clear();
                self.status = format!("members: {error}");
            }
        }
    }
}

fn move_index(index: &mut usize, len: usize, delta: isize) {
    if len == 0 {
        *index = 0;
        return;
    }
    let next = (*index as isize + delta).clamp(0, len.saturating_sub(1) as isize);
    *index = next as usize;
}

fn resized_panel_width(current: u16, delta: isize, min: u16, max: u16) -> u16 {
    let step = PANEL_RESIZE_STEP as isize;
    let next = current as isize + delta.saturating_mul(step);
    next.clamp(min as isize, max as isize) as u16
}

fn clamp_index(index: &mut usize, len: usize) {
    if len == 0 {
        *index = 0;
    } else if *index >= len {
        *index = len - 1;
    }
}

fn is_timeline_message_kind(kind: u64) -> bool {
    kind == u64::from(KIND_STREAM_MESSAGE)
        || kind == u64::from(KIND_STREAM_MESSAGE_V2)
        || kind == u64::from(KIND_STREAM_MESSAGE_EDIT)
}

pub fn merge_timeline_messages(
    messages: &mut Vec<Message>,
    incoming: impl IntoIterator<Item = Message>,
) -> bool {
    let mut changed = false;
    let mut positions = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| !message.id.is_empty())
        .map(|(index, message)| (message.id.clone(), index))
        .collect::<BTreeMap<_, _>>();

    for message in incoming {
        if message.id.is_empty() {
            messages.push(message);
            changed = true;
            continue;
        }

        if let Some(index) = positions.get(&message.id).copied() {
            if messages[index] != message {
                messages[index] = message;
                changed = true;
            }
        } else {
            positions.insert(message.id.clone(), messages.len());
            messages.push(message);
            changed = true;
        }
    }

    if changed {
        messages.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
    }
    changed
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn parse_member_input(input: &str) -> (&str, Option<&str>) {
    let mut parts = input.split_whitespace();
    let pubkey = parts.next().unwrap_or("");
    let role = parts.next();
    (pubkey, role)
}

fn parse_relay_member_input(input: &str) -> (&str, &str) {
    let mut parts = input.split_whitespace();
    let pubkey = parts.next().unwrap_or("");
    let role = parts.next().unwrap_or("member");
    (pubkey, role)
}

fn parse_contact_input(input: &str) -> Option<Contact> {
    let mut parts = input.trim().splitn(3, char::is_whitespace);
    let pubkey = parts.next()?.trim();
    if pubkey.is_empty() {
        return None;
    }

    let second = parts.next().unwrap_or("").trim();
    let third = parts.next().unwrap_or("").trim();
    let second_is_relay =
        second.starts_with("ws://") || second.starts_with("wss://") || second.starts_with("http");
    let (relay_url, petname) = if second_is_relay {
        (second.to_string(), third.to_string())
    } else {
        let mut petname = second.to_string();
        if !third.is_empty() {
            if !petname.is_empty() {
                petname.push(' ');
            }
            petname.push_str(third);
        }
        (String::new(), petname)
    };

    Some(Contact {
        pubkey: pubkey.to_string(),
        relay_url,
        petname,
    })
}

fn parse_agent_allowlist(input: &str) -> Vec<String> {
    input
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_note_tags(input: &str) -> Vec<String> {
    input
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_attachment_paths(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_workflow_inputs(input: &str) -> Result<serde_json::Value, &'static str> {
    let parsed =
        serde_json::from_str::<serde_json::Value>(input).map_err(|_| "invalid workflow inputs")?;
    if parsed.is_object() {
        Ok(parsed)
    } else {
        Err("workflow inputs must be an object")
    }
}

fn is_hex64(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn parse_repo_values(input: &str) -> Vec<String> {
    input
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn profile_field_label(field: ProfileField) -> &'static str {
    match field {
        ProfileField::DisplayName => "display name",
        ProfileField::About => "about",
        ProfileField::Picture => "avatar URL",
        ProfileField::Nip05 => "NIP-05",
    }
}

fn profile_label(profile: &UserProfile) -> String {
    profile_display_label(profile).unwrap_or_else(|| short_id(&profile.pubkey).to_string())
}

fn profile_display_label(profile: &UserProfile) -> Option<String> {
    if !profile.display_name.trim().is_empty() {
        return Some(profile.display_name.clone());
    }
    if !profile.name.trim().is_empty() {
        return Some(profile.name.clone());
    }
    None
}

fn nostr_pubkey_uri(pubkey: &str) -> Option<String> {
    let npub = PublicKey::from_hex(pubkey).ok()?.to_bech32().ok()?;
    Some(format!("nostr:{npub}"))
}

pub fn note_edit_field_label(field: NoteEditField) -> &'static str {
    match field {
        NoteEditField::Name => "Slug",
        NoteEditField::Title => "Title",
        NoteEditField::Summary => "Summary",
        NoteEditField::Tags => "Tags",
        NoteEditField::Content => "Content",
    }
}

pub fn repo_create_field_label(field: RepoCreateField) -> &'static str {
    match field {
        RepoCreateField::Id => "ID",
        RepoCreateField::Name => "Name",
        RepoCreateField::Description => "Description",
        RepoCreateField::CloneUrls => "Clone URLs",
        RepoCreateField::WebUrl => "Web URL",
        RepoCreateField::Relays => "Relays",
    }
}

pub fn diff_field_label(field: DiffField) -> &'static str {
    match field {
        DiffField::Repo => "Repo URL",
        DiffField::Commit => "Commit SHA",
        DiffField::File => "File path",
        DiffField::Description => "Description",
        DiffField::Diff => "Diff",
    }
}

fn presence_status_from_info(presence: Option<&PresenceInfo>) -> Option<PresenceStatus> {
    match presence?.status.as_str() {
        "online" => Some(PresenceStatus::Online),
        "away" => Some(PresenceStatus::Away),
        "offline" => Some(PresenceStatus::Offline),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::AcpSupervisorConfig;
    use crate::acp::{AgentRuntime, AgentStatus};

    fn test_app() -> App {
        let workspace_config = WorkspaceConfig::with_default("http://localhost:3000");
        App::new(AppConfig {
            cli: BuzzCli::new("http://localhost:3000".to_string(), None, None),
            acp: AcpSupervisor::new(AcpSupervisorConfig {
                acp_binary: "buzz-acp".to_string(),
                relay_url: "ws://localhost:3000".to_string(),
                runtimes: Vec::new(),
                default_private_key: None,
                default_auth_tag: None,
                default_agent_owner: None,
                runtime_private_keys: BTreeMap::new(),
                runtime_auth_tags: BTreeMap::new(),
                mcp_command: String::new(),
            }),
            acp_binary: "buzz-acp".to_string(),
            startup_notice: None,
            managed_agent_store: ManagedAgentStore::default(),
            managed_agent_store_path: PathBuf::from("/tmp/buzz-tui-test-agents.json"),
            workspace_config,
            workspace_store_path: PathBuf::from("/tmp/buzz-tui-test-workspaces.json"),
        })
    }

    fn test_channel(id: &str, name: &str) -> Channel {
        Channel {
            id: id.to_string(),
            name: name.to_string(),
            kind: ConversationKind::Channel,
            ..Channel::default()
        }
    }

    fn test_reminder(id: &str, status: crate::client::ReminderStatus) -> Reminder {
        Reminder {
            id: id.to_string(),
            not_before: Some(now_seconds().saturating_sub(1)),
            content: crate::client::ReminderContent {
                target: Some(ReminderTarget {
                    event_id: format!("event-{id}"),
                    channel_id: "channel-1".to_string(),
                    preview: format!("preview {id}"),
                    author_pubkey: "author".to_string(),
                }),
                note: None,
                status,
            },
            created_at: 1,
            event_id: format!("relay-{id}"),
        }
    }

    fn test_managed_runtime(id: &str, label: &str) -> AgentRuntime {
        AgentRuntime {
            id: id.to_string(),
            label: label.to_string(),
            relay_url: Some("ws://localhost:3000".to_string()),
            acp_command: None,
            command: "echo".to_string(),
            args: Vec::new(),
            model: None,
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "owner-only".to_string(),
            respond_to_allowlist: Vec::new(),
            reply_placement: "thread-direct-mentions".to_string(),
            managed: true,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: String::new(),
            last_error: None,
            log_path: None,
        }
    }

    #[test]
    fn nostr_pubkey_uri_formats_nip27_uri() {
        assert_eq!(
            nostr_pubkey_uri("7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e")
                .as_deref(),
            Some("nostr:npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg")
        );
        assert_eq!(nostr_pubkey_uri("not-a-pubkey"), None);
    }

    #[test]
    fn insert_selected_agent_mention_adds_nip27_uri_to_composer() {
        const AGENT: &str = "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e";
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one")];
        app.acp
            .upsert_runtime(test_managed_runtime(AGENT, "Helper"), None, None);

        app.insert_selected_agent_mention();

        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(
            app.composer,
            "@Helper nostr:npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg "
        );
        assert!(app.has_channel_draft("one"));
    }

    #[test]
    fn parse_workflow_inputs_accepts_json_object_only() {
        assert!(parse_workflow_inputs(r#"{"branch":"main"}"#).is_ok());
        assert_eq!(
            parse_workflow_inputs("[]"),
            Err("workflow inputs must be an object")
        );
        assert_eq!(parse_workflow_inputs("{"), Err("invalid workflow inputs"));
    }

    #[test]
    fn create_workflow_starts_with_basic_template() {
        let mut app = test_app();
        app.channels = vec![test_channel("channel-1", "ops")];

        app.focus_create_workflow();

        assert_eq!(app.focus, Focus::WorkflowEdit);
        assert!(app.workflow_yaml.contains("name: \"New workflow\""));
        assert!(app.workflow_yaml.contains("on: message_posted"));
        assert!(app.workflow_yaml.contains("action: send_message"));
    }

    #[test]
    fn workflow_editor_can_load_digest_templates_and_newlines() {
        let mut app = test_app();
        app.channels = vec![test_channel("channel-1", "ops")];
        app.focus_create_workflow();

        app.use_scheduled_digest_workflow_template();
        assert!(app.workflow_yaml.contains("on: schedule"));
        assert!(app.workflow_yaml.contains("cron: \"0 9 * * 1-5\""));
        assert!(app.workflow_yaml.contains("\"repo\":\"cashubtc/cdk\""));

        app.use_webhook_digest_workflow_template();
        assert!(app.workflow_yaml.contains("on: webhook"));
        assert!(app.workflow_yaml.contains("{{trigger.summary}}"));

        app.workflow_yaml.clear();
        app.workflow_yaml_cursor = 0;
        app.workflow_yaml_push('a');
        app.workflow_yaml_newline();
        app.workflow_yaml_push('b');
        assert_eq!(app.workflow_yaml, "a\nb");

        app.workflow_yaml_left();
        app.workflow_yaml_push('!');
        assert_eq!(app.workflow_yaml, "a\n!b");
        app.workflow_yaml_delete();
        assert_eq!(app.workflow_yaml, "a\n!");
        app.workflow_yaml_home();
        app.workflow_yaml_push('#');
        assert_eq!(app.workflow_yaml, "a\n#!");
        app.workflow_yaml_end();
        app.workflow_yaml_push('?');
        assert_eq!(app.workflow_yaml, "a\n#!?");

        app.workflow_yaml = "alpha\nb\ncharlie".to_string();
        app.workflow_yaml_cursor = app.workflow_yaml.len();
        app.workflow_yaml_up();
        assert_eq!(app.workflow_yaml_cursor, "alpha\nb".len());
        app.workflow_yaml_up();
        assert_eq!(app.workflow_yaml_cursor, "a".len());
        app.workflow_yaml_down();
        assert_eq!(app.workflow_yaml_cursor, "alpha\nb".len());
        app.workflow_yaml_down();
        assert_eq!(app.workflow_yaml_cursor, "alpha\nb\nc".len());
    }

    #[test]
    fn channel_drafts_are_scoped_by_channel() {
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one"), test_channel("two", "two")];

        app.selected_channel = 0;
        app.composer = "draft one".to_string();
        app.save_active_channel_draft();
        app.selected_channel = 1;
        app.composer = "draft two".to_string();
        app.save_active_channel_draft();

        app.selected_channel = 0;
        app.composer.clear();
        app.restore_active_channel_draft();
        assert_eq!(app.composer, "draft one");

        app.selected_channel = 1;
        app.restore_active_channel_draft();
        assert_eq!(app.composer, "draft two");
    }

    #[test]
    fn channel_drafts_are_scoped_by_thread_root() {
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one")];

        app.composer = "top-level".to_string();
        app.save_active_channel_draft();
        app.thread_root = Some("root-1".to_string());
        app.composer = "thread reply".to_string();
        app.save_active_channel_draft();

        app.thread_root = None;
        app.restore_active_channel_draft();
        assert_eq!(app.composer, "top-level");

        app.thread_root = Some("root-1".to_string());
        app.restore_active_channel_draft();
        assert_eq!(app.composer, "thread reply");
    }

    #[tokio::test]
    async fn sidebar_movement_does_not_reload_active_conversation() {
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one"), test_channel("two", "two")];
        app.selected_channel = 0;
        app.active_channel_id = Some("one".to_string());
        app.messages = vec![Message {
            id: "message-one".to_string(),
            channel_id: "one".to_string(),
            content: "still loaded".to_string(),
            ..Message::default()
        }];
        app.nav_current = app.nav_snapshot();

        app.move_selection(1).await;
        app.track_navigation();

        assert_eq!(app.selected_channel, 1);
        assert_eq!(
            app.active_channel()
                .as_ref()
                .map(|channel| channel.id.as_str()),
            Some("one"),
        );
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].channel_id, "one");
        assert!(app.nav_back.is_empty());
        assert!(app.status.contains("Enter opens"));
    }

    #[test]
    fn composer_input_updates_active_channel_draft() {
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one")];

        app.composer_push('h');
        app.composer_push('i');
        app.composer.clear();
        app.restore_active_channel_draft();
        assert_eq!(app.composer, "hi");

        app.composer_pop();
        app.composer_pop();
        assert!(app.channel_drafts.is_empty());
    }

    #[test]
    fn composer_cursor_edits_inside_multiline_text() {
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one")];

        for ch in "helo".chars() {
            app.composer_push(ch);
        }
        app.composer_left();
        app.composer_push('l');
        assert_eq!(app.composer, "hello");
        assert_eq!(app.composer_cursor, 4);

        app.composer_delete();
        assert_eq!(app.composer, "hell");
        app.composer_newline();
        for ch in "world".chars() {
            app.composer_push(ch);
        }
        app.composer_home();
        app.composer_push('>');
        assert_eq!(app.composer, "hell\n>world");
        app.composer_end();
        assert_eq!(app.composer_cursor, app.composer.len());
    }

    #[test]
    fn composer_completion_inserts_at_cursor_and_saves_draft() {
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one"), test_channel("two", "random")];

        for ch in "see #ra".chars() {
            app.composer_push(ch);
        }
        assert_eq!(
            app.composer_completion.as_ref().map(|state| state.kind),
            Some(CompletionKind::Channel)
        );

        assert!(app.accept_completion());
        assert_eq!(app.composer, "see #random ");
        assert_eq!(app.composer_cursor, app.composer.len());
        assert!(app.has_channel_draft("one"));
    }

    #[test]
    fn author_label_prefers_human_labels_over_short_pubkey() {
        const CONTACT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const AGENT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        const PROFILE: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        const UNKNOWN: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        let mut app = test_app();
        app.contacts.push(Contact {
            pubkey: CONTACT.to_string(),
            relay_url: String::new(),
            petname: "Ada".to_string(),
        });
        app.acp
            .upsert_runtime(test_managed_runtime(AGENT, "cdk-review-agent"), None, None);
        app.author_profiles.insert(
            PROFILE.to_string(),
            UserProfile {
                pubkey: PROFILE.to_string(),
                display_name: "Profile Name".to_string(),
                ..UserProfile::default()
            },
        );

        assert_eq!(app.author_label(CONTACT), "Ada");
        assert_eq!(app.author_label(AGENT), "cdk-review-agent");
        assert_eq!(app.author_label(PROFILE), "Profile Name");
        assert_eq!(app.author_label(UNKNOWN), "dddddddd");
    }

    #[test]
    fn mention_completion_inserts_nip27_agent_uri() {
        let mut app = test_app();
        let pubkey = "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e";
        app.acp
            .upsert_runtime(test_managed_runtime(pubkey, "cdk-review-agent"), None, None);

        for ch in "ask @cdk".chars() {
            app.composer_push(ch);
        }
        assert_eq!(
            app.composer_completion.as_ref().map(|state| state.kind),
            Some(CompletionKind::Mention)
        );

        assert!(app.accept_completion());
        let uri = nostr_pubkey_uri(pubkey).expect("valid pubkey");
        assert_eq!(app.composer, format!("ask @cdk-review-agent {uri} "));
    }

    #[test]
    fn confirmation_overlay_restores_focus_without_side_effects() {
        let mut app = test_app();
        app.focus = Focus::Timeline;
        app.messages = vec![Message {
            id: "event-1".to_string(),
            content: "delete me".to_string(),
            ..Message::default()
        }];

        app.request_confirm(ConfirmAction::DeleteMessage);
        assert_eq!(app.focus, Focus::Confirm);
        assert!(app.confirm.is_some());

        app.cancel_confirm();
        assert_eq!(app.focus, Focus::Timeline);
        assert_eq!(app.messages.len(), 1);
        assert!(app.confirm.is_none());
    }

    #[test]
    fn palette_ranks_contextual_commands_and_explains_disabled_actions() {
        let mut app = test_app();
        app.focus = Focus::Timeline;
        app.open_palette();

        let commands = app.palette_filtered();
        assert_eq!(
            commands.first().map(|command| command.label),
            Some("Copy message")
        );
        let copy = commands
            .iter()
            .find(|command| command.label == "Copy message")
            .expect("copy command");
        assert_eq!(copy.disabled_reason, Some("select a message first"));
        let delete = commands
            .iter()
            .find(|command| command.label == "Delete message")
            .expect("delete command");
        assert_eq!(delete.disabled_reason, Some("select a message first"));
    }

    #[test]
    fn timeline_reminder_shortcut_populates_selected_message_target() {
        let mut app = test_app();
        app.channels = vec![test_channel("channel-1", "general")];
        app.messages = vec![Message {
            id: "event-1".to_string(),
            pubkey: "author-1".to_string(),
            content: "remember this\nwith details".to_string(),
            channel_id: "channel-1".to_string(),
            ..Message::default()
        }];
        app.focus = Focus::Timeline;

        app.start_reminder_for_selected_message();

        assert_eq!(app.focus, Focus::ReminderCreate);
        let target = app.reminder_target.expect("reminder target");
        assert_eq!(target.event_id, "event-1");
        assert_eq!(target.channel_id, "channel-1");
        assert_eq!(target.preview, "remember this");
        assert_eq!(target.author_pubkey, "author-1");
    }

    #[test]
    fn timeline_selection_moves_synchronously_and_clears_reactions() {
        let mut app = test_app();
        app.focus = Focus::Timeline;
        app.channels = vec![test_channel("channel-1", "general")];
        app.messages = vec![
            Message {
                id: "event-1".to_string(),
                content: "first".to_string(),
                channel_id: "channel-1".to_string(),
                ..Message::default()
            },
            Message {
                id: "event-2".to_string(),
                content: "second".to_string(),
                channel_id: "channel-1".to_string(),
                ..Message::default()
            },
        ];
        app.selected_reactions = vec![Reaction {
            emoji: "+".to_string(),
            count: 1,
            pubkeys: vec!["pubkey-1".to_string()],
        }];
        app.message_detail_scroll = 12;

        assert!(app.move_timeline_selection(1));

        assert_eq!(app.selected_message, 1);
        assert_eq!(
            app.selected_timeline_message_id().as_deref(),
            Some("event-2")
        );
        assert_eq!(app.message_detail_scroll, 0);
        assert!(app.selected_reactions.is_empty());

        app.search_results = app.messages.clone();
        app.timeline_mode = TimelineMode::Search;
        app.selected_search_result = 0;
        assert!(app.move_timeline_selection(1));
        assert_eq!(app.selected_search_result, 1);

        app.feed = app.messages.clone();
        app.timeline_mode = TimelineMode::Feed;
        app.selected_feed = 0;
        assert!(app.move_timeline_selection(1));
        assert_eq!(app.selected_feed, 1);

        app.pulse = app.messages.clone();
        app.timeline_mode = TimelineMode::Pulse;
        app.selected_pulse = 0;
        assert!(app.move_timeline_selection(1));
        assert_eq!(app.selected_pulse, 1);
    }

    #[test]
    fn stale_hydrate_reactions_do_not_apply_after_selection_changes() {
        let mut app = test_app();
        app.focus = Focus::Timeline;
        app.channels = vec![test_channel("channel-1", "general")];
        app.messages = vec![
            Message {
                id: "event-1".to_string(),
                channel_id: "channel-1".to_string(),
                ..Message::default()
            },
            Message {
                id: "event-2".to_string(),
                channel_id: "channel-1".to_string(),
                ..Message::default()
            },
        ];
        app.selected_message = 0;
        let stale_target = app.hydrate_target(BTreeSet::new());

        app.selected_message = 1;
        app.apply_hydrate_result(
            &stale_target,
            HydrateResult {
                profiles: Vec::new(),
                reactions: Some(Ok(vec![Reaction {
                    emoji: "+".to_string(),
                    count: 3,
                    pubkeys: vec!["pubkey-1".to_string()],
                }])),
            },
        );

        assert!(app.selected_reactions.is_empty());

        let current_target = app.hydrate_target(BTreeSet::new());
        app.apply_hydrate_result(
            &current_target,
            HydrateResult {
                profiles: Vec::new(),
                reactions: Some(Ok(vec![Reaction {
                    emoji: "heart".to_string(),
                    count: 1,
                    pubkeys: vec!["pubkey-2".to_string()],
                }])),
            },
        );

        assert_eq!(
            app.selected_reactions
                .iter()
                .map(|reaction| reaction.emoji.as_str())
                .collect::<Vec<_>>(),
            vec!["heart"]
        );
    }

    #[test]
    fn primary_refresh_reactions_apply_when_target_had_no_selected_message() {
        let mut app = test_app();
        app.focus = Focus::Timeline;
        app.channels = vec![test_channel("channel-1", "general")];
        let target = app.refresh_target();

        app.apply_refresh_result(
            &target,
            RefreshResult {
                sidebar: None,
                read_state: None,
                starred_channel_ids: None,
                muted_channel_ids: None,
                channel_sections: None,
                channel_detail_id: None,
                channel_detail: None,
                channel_members: None,
                message_channel_id: Some("channel-1".to_string()),
                messages: Some(Ok(vec![
                    Message {
                        id: "event-1".to_string(),
                        channel_id: "channel-1".to_string(),
                        created_at: 1,
                        ..Message::default()
                    },
                    Message {
                        id: "event-2".to_string(),
                        channel_id: "channel-1".to_string(),
                        created_at: 2,
                        ..Message::default()
                    },
                ])),
                feed: None,
                profiles: Vec::new(),
                reaction_event_id: Some("event-2".to_string()),
                reactions: Some(Ok(vec![Reaction {
                    emoji: "+".to_string(),
                    count: 2,
                    pubkeys: vec!["pubkey-1".to_string()],
                }])),
            },
        );

        assert_eq!(
            app.selected_timeline_message_id().as_deref(),
            Some("event-2")
        );
        assert_eq!(
            app.selected_reactions
                .iter()
                .map(|reaction| (reaction.emoji.as_str(), reaction.count))
                .collect::<Vec<_>>(),
            vec![("+", 2)]
        );
    }

    #[test]
    fn primary_refresh_applies_channel_members() {
        let mut app = test_app();
        app.channels = vec![test_channel("channel-1", "general")];
        app.selected_channel = 0;
        let target = app.refresh_target();

        app.apply_refresh_result(
            &target,
            RefreshResult {
                sidebar: None,
                read_state: None,
                starred_channel_ids: None,
                muted_channel_ids: None,
                channel_sections: None,
                channel_detail_id: Some("channel-1".to_string()),
                channel_detail: Some(Ok(Some(Channel {
                    id: "channel-1".to_string(),
                    name: "general".to_string(),
                    description: "General chat".to_string(),
                    ..Channel::default()
                }))),
                channel_members: Some(Ok(vec![ChannelMember {
                    pubkey: "pubkey-1".to_string(),
                    role: "admin".to_string(),
                }])),
                message_channel_id: None,
                messages: None,
                feed: None,
                profiles: Vec::new(),
                reaction_event_id: None,
                reactions: None,
            },
        );

        assert_eq!(
            app.selected_channel_detail
                .as_ref()
                .map(|channel| channel.description.as_str()),
            Some("General chat")
        );
        assert_eq!(
            app.channel_members
                .iter()
                .map(|member| (member.pubkey.as_str(), member.role.as_str()))
                .collect::<Vec<_>>(),
            vec![("pubkey-1", "admin")]
        );
    }

    #[test]
    fn visible_reminders_hide_cancelled_and_done() {
        let mut app = test_app();
        app.reminders = vec![
            test_reminder("pending", crate::client::ReminderStatus::Pending),
            test_reminder("done", crate::client::ReminderStatus::Done),
            test_reminder("cancelled", crate::client::ReminderStatus::Cancelled),
        ];

        assert_eq!(
            app.visible_reminders()
                .iter()
                .map(|reminder| reminder.id.as_str())
                .collect::<Vec<_>>(),
            vec!["pending"]
        );
    }

    #[test]
    fn navigation_tracking_ignores_transient_overlays() {
        let mut app = test_app();
        app.track_navigation();
        app.focus = Focus::Profile;
        app.track_navigation();
        assert_eq!(app.nav_back.len(), 1);
        assert_eq!(app.nav_current.focus, Focus::Profile);

        app.focus = Focus::CommandPalette;
        app.track_navigation();
        assert_eq!(app.nav_back.len(), 1);
        assert_eq!(app.nav_current.focus, Focus::Profile);
    }

    #[test]
    fn local_read_state_tracks_frontier_and_manual_unread() {
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one")];
        app.messages = vec![Message {
            id: "event-1".to_string(),
            created_at: 42,
            ..Message::default()
        }];

        app.remember_latest_message_for("one");
        assert!(app.channel_has_unread("one"));

        app.mark_channel_read_at("one", 42);
        assert!(!app.channel_has_unread("one"));

        app.mark_channel_unread("one");
        assert!(app.channel_has_unread("one"));

        app.mark_channel_read_at("one", 42);
        assert!(!app.channel_has_unread("one"));
    }

    #[test]
    fn create_channel_expiry_cycles_to_ttl_values() {
        let mut app = test_app();
        assert_eq!(app.new_channel_expiry.label(), "permanent");
        assert_eq!(app.new_channel_expiry.ttl_seconds(), None);

        app.cycle_new_channel_expiry();
        assert_eq!(app.new_channel_expiry.label(), "7 days");
        assert_eq!(app.new_channel_expiry.ttl_seconds(), Some(7 * 24 * 60 * 60));
    }

    #[test]
    fn remote_read_state_advances_frontiers_and_clears_covered_manual_unread() {
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one")];
        app.channel_latest_seen.insert("one".to_string(), 100);
        app.mark_channel_unread("one");
        assert!(app.channel_has_unread("one"));

        let mut contexts = BTreeMap::new();
        contexts.insert("one".to_string(), 100);
        contexts.insert("thread:event-1".to_string(), 200);
        assert!(app.merge_remote_read_state(contexts));

        let frontiers = app
            .workspace_config
            .read_frontiers
            .get(app.active_workspace_id())
            .cloned()
            .unwrap_or_default();
        assert_eq!(frontiers.get("one").copied(), Some(100));
        assert!(!frontiers.contains_key("thread:event-1"));
        assert!(!app.channel_has_unread("one"));

        let mut older_contexts = BTreeMap::new();
        older_contexts.insert("one".to_string(), 99);
        assert!(!app.merge_remote_read_state(older_contexts));
    }

    #[test]
    fn read_state_preserves_per_message_markers() {
        let mut app = test_app();
        app.channels = vec![test_channel("one", "one")];

        let mut contexts = BTreeMap::new();
        contexts.insert("msg:event-1".to_string(), 42);
        contexts.insert("thread:event-1".to_string(), 99);
        assert!(app.merge_remote_read_state(contexts));

        let frontiers = app
            .workspace_config
            .read_frontiers
            .get(app.active_workspace_id())
            .cloned()
            .unwrap_or_default();
        assert_eq!(frontiers.get("msg:event-1").copied(), Some(42));
        assert!(!frontiers.contains_key("thread:event-1"));

        let messages = vec![Message {
            id: "event-2".to_string(),
            created_at: 100,
            ..Message::default()
        }];
        assert!(app.mark_messages_read(&messages));
        let frontiers = app
            .workspace_config
            .read_frontiers
            .get(app.active_workspace_id())
            .cloned()
            .unwrap_or_default();
        assert_eq!(frontiers.get("msg:event-2").copied(), Some(100));
    }

    #[test]
    fn timeline_merge_dedupes_updates_and_sorts_by_created_at() {
        let mut messages = vec![
            Message {
                id: "later".to_string(),
                content: "later".to_string(),
                created_at: 20,
                ..Message::default()
            },
            Message {
                id: "edit-me".to_string(),
                content: "old".to_string(),
                created_at: 10,
                ..Message::default()
            },
        ];

        assert!(merge_timeline_messages(
            &mut messages,
            vec![
                Message {
                    id: "edit-me".to_string(),
                    content: "new".to_string(),
                    created_at: 10,
                    ..Message::default()
                },
                Message {
                    id: "middle".to_string(),
                    content: "middle".to_string(),
                    created_at: 15,
                    ..Message::default()
                },
            ],
        ));

        assert_eq!(
            messages
                .iter()
                .map(|message| (message.id.as_str(), message.content.as_str()))
                .collect::<Vec<_>>(),
            vec![("edit-me", "new"), ("middle", "middle"), ("later", "later")]
        );
        assert!(!merge_timeline_messages(
            &mut messages,
            Vec::<Message>::new()
        ));
    }

    #[test]
    fn active_live_channel_target_uses_workspace_relay_and_latest_seen() {
        let mut app = test_app();
        app.channels = vec![test_channel(
            "9ba26a41-91b9-4c57-83a9-08afd46330d2",
            "general",
        )];
        app.channel_latest_seen
            .insert(app.channels[0].id.clone(), 42);

        assert_eq!(
            app.active_live_channel_target(),
            Some(LiveChannelTarget {
                relay_url: "http://localhost:3000".to_string(),
                channel_id: "9ba26a41-91b9-4c57-83a9-08afd46330d2".to_string(),
                presence_pubkeys: Vec::new(),
                since: Some(41),
            })
        );
    }

    #[test]
    fn live_presence_pubkeys_include_visible_users_once() {
        let mut app = test_app();
        app.contacts = vec![
            Contact {
                pubkey: "b".to_string(),
                ..Contact::default()
            },
            Contact {
                pubkey: "a".to_string(),
                ..Contact::default()
            },
        ];
        app.channel_members = vec![ChannelMember {
            pubkey: "b".to_string(),
            role: "member".to_string(),
        }];
        app.viewed_profile = Some(UserProfile {
            pubkey: "c".to_string(),
            ..UserProfile::default()
        });

        assert_eq!(app.live_presence_pubkeys(), vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn live_message_merges_active_channel_timeline() {
        let mut app = test_app();
        app.channels = vec![test_channel(
            "9ba26a41-91b9-4c57-83a9-08afd46330d2",
            "general",
        )];
        app.messages = vec![Message {
            id: "older".to_string(),
            content: "older".to_string(),
            created_at: 10,
            channel_id: app.channels[0].id.clone(),
            ..Message::default()
        }];
        app.selected_message = 0;

        assert!(app.apply_live_message(TuiMessageView {
            id: "newer".to_string(),
            pubkey: "author".to_string(),
            kind: u64::from(KIND_STREAM_MESSAGE),
            content: "newer".to_string(),
            created_at: 20,
            channel_id: app.channels[0].id.clone(),
            thread_root_id: None,
        }));

        assert_eq!(
            app.messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["older", "newer"]
        );
        assert_eq!(app.selected_message, 1);
        assert_eq!(
            app.channel_latest_seen.get(&app.channels[0].id).copied(),
            Some(20)
        );
    }

    #[tokio::test]
    async fn live_message_preserves_selected_history_message() {
        let mut app = test_app();
        app.channels = vec![test_channel(
            "9ba26a41-91b9-4c57-83a9-08afd46330d2",
            "general",
        )];
        app.messages = vec![
            Message {
                id: "older".to_string(),
                content: "older".to_string(),
                created_at: 10,
                channel_id: app.channels[0].id.clone(),
                ..Message::default()
            },
            Message {
                id: "selected".to_string(),
                content: "selected".to_string(),
                created_at: 20,
                channel_id: app.channels[0].id.clone(),
                ..Message::default()
            },
            Message {
                id: "latest".to_string(),
                content: "latest".to_string(),
                created_at: 30,
                channel_id: app.channels[0].id.clone(),
                ..Message::default()
            },
        ];
        app.selected_message = 1;

        assert!(app.apply_live_message(TuiMessageView {
            id: "newer".to_string(),
            pubkey: "author".to_string(),
            kind: u64::from(KIND_STREAM_MESSAGE),
            content: "newer".to_string(),
            created_at: 40,
            channel_id: app.channels[0].id.clone(),
            thread_root_id: None,
        }));

        assert_eq!(
            app.messages
                .get(app.selected_message)
                .map(|message| message.id.as_str()),
            Some("selected")
        );
    }

    #[tokio::test]
    async fn live_message_update_preserves_detail_scroll_for_same_selected_message() {
        let mut app = test_app();
        app.channels = vec![test_channel(
            "9ba26a41-91b9-4c57-83a9-08afd46330d2",
            "general",
        )];
        app.messages = vec![Message {
            id: "selected".to_string(),
            content: "before".to_string(),
            created_at: 20,
            channel_id: app.channels[0].id.clone(),
            ..Message::default()
        }];
        app.selected_message = 0;
        app.message_detail_scroll = 24;

        assert!(app.apply_live_message(TuiMessageView {
            id: "selected".to_string(),
            pubkey: "author".to_string(),
            kind: u64::from(KIND_STREAM_MESSAGE),
            content: "after".to_string(),
            created_at: 20,
            channel_id: app.channels[0].id.clone(),
            thread_root_id: None,
        }));

        assert_eq!(app.selected_message, 0);
        assert_eq!(app.message_detail_scroll, 24);
    }

    #[tokio::test]
    async fn live_message_append_resets_detail_scroll_when_following_latest() {
        let mut app = test_app();
        app.channels = vec![test_channel(
            "9ba26a41-91b9-4c57-83a9-08afd46330d2",
            "general",
        )];
        app.messages = vec![Message {
            id: "older".to_string(),
            content: "older".to_string(),
            created_at: 10,
            channel_id: app.channels[0].id.clone(),
            ..Message::default()
        }];
        app.selected_message = 0;
        app.message_detail_scroll = 24;

        assert!(app.apply_live_message(TuiMessageView {
            id: "newer".to_string(),
            pubkey: "author".to_string(),
            kind: u64::from(KIND_STREAM_MESSAGE),
            content: "newer".to_string(),
            created_at: 20,
            channel_id: app.channels[0].id.clone(),
            thread_root_id: None,
        }));

        assert_eq!(
            app.messages
                .get(app.selected_message)
                .map(|message| message.id.as_str()),
            Some("newer")
        );
        assert_eq!(app.message_detail_scroll, 0);
    }

    #[tokio::test]
    async fn live_message_does_not_append_unrelated_channel_message_while_thread_open() {
        let mut app = test_app();
        app.channels = vec![test_channel(
            "9ba26a41-91b9-4c57-83a9-08afd46330d2",
            "general",
        )];
        app.thread_root = Some("root".to_string());
        app.messages = vec![
            Message {
                id: "root".to_string(),
                content: "root".to_string(),
                created_at: 10,
                channel_id: app.channels[0].id.clone(),
                ..Message::default()
            },
            Message {
                id: "reply".to_string(),
                content: "reply".to_string(),
                created_at: 20,
                channel_id: app.channels[0].id.clone(),
                ..Message::default()
            },
        ];

        assert!(!app.apply_live_message(TuiMessageView {
            id: "other-top-level".to_string(),
            pubkey: "author".to_string(),
            kind: u64::from(KIND_STREAM_MESSAGE),
            content: "other".to_string(),
            created_at: 30,
            channel_id: app.channels[0].id.clone(),
            thread_root_id: None,
        }));

        assert_eq!(
            app.messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "reply"]
        );
    }

    #[tokio::test]
    async fn live_message_updates_existing_message_while_thread_open() {
        let mut app = test_app();
        app.channels = vec![test_channel(
            "9ba26a41-91b9-4c57-83a9-08afd46330d2",
            "general",
        )];
        app.thread_root = Some("root".to_string());
        app.messages = vec![Message {
            id: "reply".to_string(),
            content: "before".to_string(),
            created_at: 20,
            channel_id: app.channels[0].id.clone(),
            ..Message::default()
        }];

        assert!(app.apply_live_message(TuiMessageView {
            id: "reply".to_string(),
            pubkey: "author".to_string(),
            kind: u64::from(KIND_STREAM_MESSAGE),
            content: "after".to_string(),
            created_at: 20,
            channel_id: app.channels[0].id.clone(),
            thread_root_id: Some("root".to_string()),
        }));

        assert_eq!(app.messages[0].content, "after");
    }

    #[tokio::test]
    async fn live_message_ignores_inactive_channel_and_non_timeline_kind() {
        let mut app = test_app();
        app.channels = vec![test_channel(
            "9ba26a41-91b9-4c57-83a9-08afd46330d2",
            "general",
        )];

        assert!(!app.apply_live_message(TuiMessageView {
            id: "other".to_string(),
            kind: u64::from(KIND_STREAM_MESSAGE),
            channel_id: "bc768704-0733-43c5-b9f6-ac1ff3731eb9".to_string(),
            ..TuiMessageView::default()
        }));
        assert!(!app.apply_live_message(TuiMessageView {
            id: "reaction".to_string(),
            kind: 7,
            channel_id: app.channels[0].id.clone(),
            ..TuiMessageView::default()
        }));
        assert!(app.messages.is_empty());
    }

    #[tokio::test]
    async fn opening_message_inside_thread_keeps_existing_thread_root() {
        let mut app = test_app();
        app.channels = vec![test_channel(
            "9ba26a41-91b9-4c57-83a9-08afd46330d2",
            "general",
        )];
        app.thread_root = Some("root".to_string());
        app.timeline_mode = TimelineMode::Channel;
        app.messages = vec![
            Message {
                id: "root".to_string(),
                content: "root".to_string(),
                created_at: 10,
                channel_id: app.channels[0].id.clone(),
                ..Message::default()
            },
            Message {
                id: "reply".to_string(),
                content: "reply".to_string(),
                created_at: 20,
                channel_id: app.channels[0].id.clone(),
                ..Message::default()
            },
        ];
        app.selected_message = 1;

        app.open_selected_thread().await;

        assert_eq!(app.thread_root.as_deref(), Some("root"));
        assert_eq!(
            app.messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "reply"]
        );
    }

    #[test]
    fn selected_reply_opens_its_thread_root() {
        let reply = Message {
            id: "reply".to_string(),
            thread_root_id: Some("root".to_string()),
            ..Message::default()
        };
        let root = Message {
            id: "root".to_string(),
            thread_root_id: None,
            ..Message::default()
        };

        assert_eq!(super::timeline::thread_root_for_message(&reply), "root");
        assert_eq!(super::timeline::thread_root_for_message(&root), "root");
    }

    #[test]
    fn local_channel_preferences_track_starred_and_muted_ids() {
        let mut app = test_app();

        app.set_local_channel_preference(ChannelPreferenceKind::Stars, "one", true);
        app.set_local_channel_preference(ChannelPreferenceKind::Mutes, "two", true);

        assert!(app.channel_is_starred("one"));
        assert!(!app.channel_is_starred("two"));
        assert!(app.channel_is_muted("two"));

        app.set_local_channel_preference(ChannelPreferenceKind::Stars, "one", false);
        app.set_local_channel_preference(ChannelPreferenceKind::Mutes, "two", false);

        assert!(!app.channel_is_starred("one"));
        assert!(!app.channel_is_muted("two"));
    }

    #[test]
    fn channel_section_name_uses_assignments_and_sections() {
        let mut app = test_app();
        app.channel_sections = vec![ChannelSection {
            id: "core".to_string(),
            name: "Core Work".to_string(),
            order: 0,
        }];
        app.channel_section_assignments =
            BTreeMap::from([("channel-1".to_string(), "core".to_string())]);

        assert_eq!(app.channel_section_name("channel-1"), Some("Core Work"));
        assert_eq!(app.channel_section_name("channel-2"), None);

        app.channel_section_assignments
            .insert("channel-3".to_string(), "missing".to_string());
        assert_eq!(app.channel_section_name("channel-3"), None);
    }

    #[test]
    fn workspace_view_reset_clears_workspace_scoped_state() {
        let mut app = test_app();
        app.channels = vec![test_channel("channel-1", "general")];
        app.messages = vec![Message {
            id: "event-1".to_string(),
            content: "hello".to_string(),
            created_at: 42,
            ..Message::default()
        }];
        app.feed = app.messages.clone();
        app.search_results = app.messages.clone();
        app.channel_latest_seen.insert("channel-1".into(), 42);
        app.starred_channel_ids.insert("channel-1".into());
        app.muted_channel_ids.insert("channel-1".into());
        app.channel_sections = vec![ChannelSection {
            id: "section-1".into(),
            name: "Core Work".into(),
            order: 0,
        }];
        app.channel_section_assignments
            .insert("channel-1".into(), "section-1".into());
        app.thread_root = Some("event-1".into());
        app.timeline_mode = TimelineMode::Search;
        app.composer = "draft".into();

        app.reset_workspace_view_state();

        assert!(app.channels.is_empty());
        assert!(app.messages.is_empty());
        assert!(app.feed.is_empty());
        assert!(app.search_results.is_empty());
        assert!(app.channel_latest_seen.is_empty());
        assert!(app.starred_channel_ids.is_empty());
        assert!(app.muted_channel_ids.is_empty());
        assert!(app.channel_sections.is_empty());
        assert!(app.channel_section_assignments.is_empty());
        assert!(app.thread_root.is_none());
        assert_eq!(app.timeline_mode, TimelineMode::Channel);
        assert!(app.composer.is_empty());
    }

    #[test]
    fn resolve_section_input_matches_id_or_name() {
        let mut app = test_app();
        app.channel_sections = vec![ChannelSection {
            id: "core".to_string(),
            name: "Core Work".to_string(),
            order: 0,
        }];

        assert_eq!(
            app.resolve_section_input("core").map(|section| section.id),
            Some("core".to_string())
        );
        assert_eq!(
            app.resolve_section_input("core work")
                .map(|section| section.name),
            Some("Core Work".to_string())
        );
        assert!(app.resolve_section_input("missing").is_none());
    }

    #[test]
    fn parse_contact_input_accepts_relay_and_petname() {
        assert_eq!(
            parse_contact_input("abc123 wss://relay.example Ada Lovelace"),
            Some(Contact {
                pubkey: "abc123".to_string(),
                relay_url: "wss://relay.example".to_string(),
                petname: "Ada Lovelace".to_string(),
            })
        );
    }

    #[test]
    fn parse_contact_input_accepts_petname_without_relay() {
        assert_eq!(
            parse_contact_input("abc123 Ada Lovelace"),
            Some(Contact {
                pubkey: "abc123".to_string(),
                relay_url: String::new(),
                petname: "Ada Lovelace".to_string(),
            })
        );
    }

    #[test]
    fn parse_agent_allowlist_splits_commas_and_whitespace() {
        assert_eq!(
            parse_agent_allowlist("abc, def\n ghi"),
            vec!["abc".to_string(), "def".to_string(), "ghi".to_string()]
        );
    }

    #[test]
    fn parse_note_tags_splits_commas_and_whitespace() {
        assert_eq!(
            parse_note_tags("release, changelog docs"),
            vec![
                "release".to_string(),
                "changelog".to_string(),
                "docs".to_string()
            ]
        );
    }

    #[test]
    fn parse_attachment_paths_splits_whitespace() {
        assert_eq!(
            parse_attachment_paths("/tmp/a.png\n/tmp/b.jpg"),
            vec!["/tmp/a.png".to_string(), "/tmp/b.jpg".to_string()]
        );
    }

    #[test]
    fn is_hex64_accepts_only_hex_pubkeys() {
        assert!(is_hex64(&"a".repeat(64)));
        assert!(!is_hex64(&"g".repeat(64)));
        assert!(!is_hex64(&"a".repeat(63)));
    }

    #[tokio::test]
    #[ignore = "requires BUZZ_TUI_LIVE_SMOKE=1, a running relay, and direct relay auth"]
    async fn live_relay_smoke_exercises_tui_paths() {
        if std::env::var("BUZZ_TUI_LIVE_SMOKE").ok().as_deref() != Some("1") {
            eprintln!("set BUZZ_TUI_LIVE_SMOKE=1 to run the live relay smoke test");
            return;
        }

        use crate::live::{LiveChannelTarget, LiveEvent, LiveRuntime};
        use crate::workspace::WorkspaceConfig;
        use nostr::{Keys, ToBech32};
        use tokio::sync::mpsc;
        use tokio::time::{timeout, Duration};

        let relay = std::env::var("BUZZ_TUI_SMOKE_RELAY")
            .unwrap_or_else(|_| "http://localhost:3030".to_string());
        let keys = Keys::generate();
        let private_key = keys.secret_key().to_bech32().unwrap();
        let cli = BuzzCli::new(relay.clone(), Some(private_key.clone()), None);
        let client =
            TuiRelayClient::new(relay.clone(), &private_key, None).expect("direct relay client");

        let suffix = format!("{}-{}", std::process::id(), now_seconds());
        let channel = client
            .create_channel(&CreateChannelOptions {
                name: format!("tui-smoke-{suffix}"),
                channel_type: "stream".to_string(),
                visibility: "open".to_string(),
                description: "buzz-tui live smoke".to_string(),
                ttl: None,
            })
            .await
            .expect("create smoke channel");
        let channel_id = channel
            .get("channel_id")
            .and_then(serde_json::Value::as_str)
            .expect("channel id")
            .to_string();

        let first = client
            .send_channel_message_with_files(&channel_id, "hello from tui live smoke", None, &[])
            .await
            .expect("send first smoke message");
        let first_event_id = first
            .get("event_id")
            .and_then(serde_json::Value::as_str)
            .expect("first event id")
            .to_string();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut live = LiveRuntime::new(Some(private_key.clone()), None, tx);
        live.sync_active_channel(Some(LiveChannelTarget {
            relay_url: relay.clone(),
            channel_id: channel_id.clone(),
            presence_pubkeys: Vec::new(),
            since: Some(now_seconds().saturating_sub(1)),
        }));
        tokio::time::sleep(Duration::from_millis(250)).await;

        let live_send = client
            .send_channel_message_with_files(
                &channel_id,
                "live delivery without refresh",
                None,
                &[],
            )
            .await
            .expect("send live smoke message");
        let live_event_id = live_send
            .get("event_id")
            .and_then(serde_json::Value::as_str)
            .expect("live event id")
            .to_string();

        let live_message = timeout(Duration::from_secs(10), async {
            loop {
                match rx.recv().await.expect("live event") {
                    LiveEvent::Message(message) if message.id == live_event_id => break message,
                    LiveEvent::Error(error) => panic!("live runtime error: {error}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("live message without manual refresh");
        assert_eq!(live_message.content, "live delivery without refresh");
        live.stop();

        let temp_dir = std::env::temp_dir().join(format!("buzz-tui-live-smoke-{suffix}"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let acp_script = temp_dir.join("sleep-acp.sh");
        std::fs::write(&acp_script, "#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&acp_script).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&acp_script, permissions).unwrap();
        }

        let managed_runtime = test_managed_runtime("managed-smoke", "Managed Smoke");
        let mut runtime_keys = BTreeMap::new();
        runtime_keys.insert("managed-smoke".to_string(), private_key.clone());
        let acp = AcpSupervisor::new(AcpSupervisorConfig {
            acp_binary: acp_script.display().to_string(),
            relay_url: crate::client::relay_http_to_ws_url(&relay),
            runtimes: vec![managed_runtime],
            default_private_key: Some(private_key.clone()),
            default_auth_tag: None,
            default_agent_owner: None,
            runtime_private_keys: runtime_keys,
            runtime_auth_tags: BTreeMap::new(),
            mcp_command: String::new(),
        });

        let workspace_store = temp_dir.join("workspaces.json");
        let mut app = App::new(AppConfig {
            cli: cli.clone(),
            acp,
            acp_binary: acp_script.display().to_string(),
            startup_notice: None,
            managed_agent_store: ManagedAgentStore::default(),
            managed_agent_store_path: temp_dir.join("agents.json"),
            workspace_config: WorkspaceConfig::with_default(&relay),
            workspace_store_path: workspace_store,
        });

        app.refresh().await;
        app.selected_channel = app
            .channels
            .iter()
            .position(|channel| channel.id == channel_id)
            .expect("created channel visible in sidebar");
        app.load_selected_channel().await;
        app.selected_message = app
            .messages
            .iter()
            .position(|message| message.id == first_event_id)
            .expect("first smoke message visible in timeline");

        app.open_selected_thread().await;
        assert_eq!(app.thread_root.as_deref(), Some(first_event_id.as_str()));

        app.focus = Focus::Sidebar;
        app.thread_root = None;
        app.thread_context = None;
        app.timeline_mode = TimelineMode::Channel;
        app.mark_channel_unread(&channel_id);
        assert!(app.channel_has_unread(&channel_id));
        app.toggle_selected_channel_read_marker().await;
        assert!(
            !app.channel_has_unread(&channel_id),
            "channel should be read"
        );

        app.toggle_selected_channel_star().await;
        assert!(app.channel_is_starred(&channel_id));
        app.toggle_selected_channel_mute().await;
        assert!(app.channel_is_muted(&channel_id));

        let new_workspace_id = app
            .workspace_config
            .add_workspace("smoke workspace", &relay)
            .expect("add smoke workspace");
        app.apply_workspace_session(
            new_workspace_id.clone(),
            cli.clone(),
            AcpSupervisor::new(AcpSupervisorConfig {
                acp_binary: acp_script.display().to_string(),
                relay_url: crate::client::relay_http_to_ws_url(&relay),
                runtimes: Vec::new(),
                default_private_key: Some(private_key.clone()),
                default_auth_tag: None,
                default_agent_owner: None,
                runtime_private_keys: BTreeMap::new(),
                runtime_auth_tags: BTreeMap::new(),
                mcp_command: String::new(),
            }),
            None,
        )
        .await;
        assert_eq!(app.workspace_config.active_id, new_workspace_id);

        let mut runtime_keys = BTreeMap::new();
        runtime_keys.insert("managed-smoke".to_string(), private_key.clone());
        app.acp = AcpSupervisor::new(AcpSupervisorConfig {
            acp_binary: acp_script.display().to_string(),
            relay_url: crate::client::relay_http_to_ws_url(&relay),
            runtimes: vec![test_managed_runtime("managed-smoke", "Managed Smoke")],
            default_private_key: Some(private_key),
            default_auth_tag: None,
            default_agent_owner: None,
            runtime_private_keys: runtime_keys,
            runtime_auth_tags: BTreeMap::new(),
            mcp_command: String::new(),
        });
        app.selected_agent = 0;
        app.toggle_selected_agent().await;
        assert_eq!(
            app.acp.agent_at(0).map(|agent| agent.status),
            Some(AgentStatus::Running)
        );
        app.toggle_selected_agent().await;
        assert_eq!(
            app.acp.agent_at(0).map(|agent| agent.status),
            Some(AgentStatus::Stopped)
        );
    }
}
