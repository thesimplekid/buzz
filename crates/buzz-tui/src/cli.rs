#[derive(Clone, Debug)]
pub struct BuzzCli {
    relay: String,
    private_key: Option<String>,
    auth_tag: Option<String>,
}

#[allow(unused_imports)]
pub use buzz_tui_client::{
    default_reply_placement, CanvasDocument, Channel, ChannelMember, ChannelPreferenceKind,
    ChannelSection, ChannelSections, Contact, ConversationKind, CreateChannelOptions,
    CreateIssueOptions, CreateManagedAgentOptions, CreateManagedAgentResponse, CreatePatchOptions,
    CreateRepoOptions, CustomEmojiEntry, DeleteManagedAgentResponse, EmojiExportScope, GitIssue,
    GitPatch, ListNotesOptions, LongFormNoteOptions, ManagedAgentInfo,
    ManagedAgentLifecycleResponse, ManagedAgentLogInfo, ManagedAgentRestoreResponse, MemoryEntry,
    Message, Note, NoteAuthor, PresenceInfo, PresenceStatus, ProfileField, Reaction, ReadState,
    RepoProject, SendDiffOptions, UploadedFile, UserProfile, Workflow, WorkflowDetail, WorkflowRun,
};

impl BuzzCli {
    pub fn new(relay: String, private_key: Option<String>, auth_tag: Option<String>) -> Self {
        Self {
            relay,
            private_key,
            auth_tag,
        }
    }

    pub fn relay_url(&self) -> &str {
        &self.relay
    }

    pub fn private_key(&self) -> Option<String> {
        self.private_key.clone()
    }

    pub fn auth_tag(&self) -> Option<String> {
        self.auth_tag.clone()
    }
}
