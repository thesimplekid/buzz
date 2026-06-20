use super::{App, Focus};

/// A destructive or hard-to-undo action that must be confirmed before it runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmAction {
    DeleteMessage,
    DeleteWorkflow,
    DeleteManagedAgent,
    DeleteMemory,
    RemoveEmoji,
    RemoveWorkspace,
    LeaveChannel,
    HideDm,
    ArchiveChannel,
    DeleteChannel,
    RemoveContact,
    DeleteNote,
    RemoveMember(String),
}

/// Pending confirmation overlay state.
#[derive(Clone, Debug)]
pub struct ConfirmState {
    pub title: String,
    pub body: String,
    pub confirm_label: String,
    pub cancel_label: String,
    pub action: ConfirmAction,
    pub previous_focus: Focus,
}

impl App {
    /// Open a confirmation overlay for a destructive action. Returns without
    /// changing focus (and sets a status message) when the action has no valid
    /// target — so the user gets feedback instead of a silent no-op.
    pub fn request_confirm(&mut self, action: ConfirmAction) {
        let Some((title, body)) = self.confirm_description(&action) else {
            return;
        };
        let previous_focus = self.focus;
        self.confirm = Some(ConfirmState {
            title,
            body,
            confirm_label: "Enter confirm".to_string(),
            cancel_label: "Esc cancel".to_string(),
            action,
            previous_focus,
        });
        self.focus = Focus::Confirm;
    }

    /// Cancel a pending confirmation without side effects, restoring focus.
    pub fn cancel_confirm(&mut self) {
        if let Some(state) = self.confirm.take() {
            self.focus = state.previous_focus;
            self.status = "Cancelled".to_string();
        }
    }

    /// Execute the pending confirmation's action.
    pub async fn confirm_pending(&mut self) {
        let Some(state) = self.confirm.take() else {
            return;
        };
        // Restore the originating focus so the underlying action methods read
        // and mutate the same selection/state they would from a direct key.
        self.focus = state.previous_focus;
        match state.action {
            ConfirmAction::DeleteMessage => self.delete_selected_message().await,
            ConfirmAction::DeleteWorkflow => self.delete_selected_workflow().await,
            ConfirmAction::DeleteManagedAgent => self.delete_selected_managed_agent().await,
            ConfirmAction::DeleteMemory => self.delete_selected_memory().await,
            ConfirmAction::RemoveEmoji => self.remove_selected_emoji().await,
            ConfirmAction::RemoveWorkspace => self.remove_selected_workspace(),
            ConfirmAction::LeaveChannel => self.leave_selected_channel().await,
            ConfirmAction::HideDm => self.hide_selected_dm().await,
            ConfirmAction::ArchiveChannel => self.archive_selected_channel().await,
            ConfirmAction::DeleteChannel => self.delete_selected_channel().await,
            ConfirmAction::RemoveContact => self.delete_selected_contact().await,
            ConfirmAction::DeleteNote => self.delete_selected_note().await,
            ConfirmAction::RemoveMember(pubkey) => self.remove_member_confirmed(pubkey).await,
        }
    }

    /// Build a `(title, body)` for a confirmation, or `None` (with a status
    /// message) when the action cannot apply to the current selection.
    fn confirm_description(&mut self, action: &ConfirmAction) -> Option<(String, String)> {
        match action {
            ConfirmAction::DeleteMessage => {
                let message = self.selected_timeline_message()?;
                Some((
                    "Delete message".to_string(),
                    format!("Delete this message?\n{}", preview(&message.content)),
                ))
            }
            ConfirmAction::DeleteWorkflow => {
                let workflow = self.workflows.get(self.selected_workflow)?;
                Some((
                    "Delete workflow".to_string(),
                    format!("Delete workflow {}?", workflow.workflow_id),
                ))
            }
            ConfirmAction::DeleteManagedAgent => {
                let agent = self.acp.agent_at(self.selected_agent)?;
                Some((
                    "Delete agent".to_string(),
                    format!("Delete managed agent {}?", agent.runtime.label),
                ))
            }
            ConfirmAction::DeleteMemory => {
                let memory = self.memories.get(self.selected_memory)?;
                Some((
                    "Delete memory".to_string(),
                    format!("Delete memory {}?", memory.slug),
                ))
            }
            ConfirmAction::RemoveEmoji => {
                let emoji = self.selected_emoji_entry()?;
                Some((
                    "Remove emoji".to_string(),
                    format!("Remove custom emoji :{}:?", emoji.shortcode),
                ))
            }
            ConfirmAction::RemoveWorkspace => {
                let workspace = self.selected_workspace()?;
                Some((
                    "Remove workspace".to_string(),
                    format!("Remove workspace {}?", workspace.name),
                ))
            }
            ConfirmAction::LeaveChannel => {
                let channel = self.active_channel()?;
                Some((
                    "Leave channel".to_string(),
                    format!("Leave #{}?", channel.name),
                ))
            }
            ConfirmAction::HideDm => {
                let channel = self.active_channel()?;
                Some(("Hide DM".to_string(), format!("Hide DM {}?", channel.name)))
            }
            ConfirmAction::ArchiveChannel => {
                let channel = self.active_channel()?;
                Some((
                    "Archive channel".to_string(),
                    format!("Archive #{}?", channel.name),
                ))
            }
            ConfirmAction::DeleteChannel => {
                let channel = self.selected_sidebar_channel_for_management()?;
                Some((
                    "Delete channel".to_string(),
                    format!(
                        "Delete #{}?\nThis removes the channel for everyone and cannot be undone.",
                        channel.name
                    ),
                ))
            }
            ConfirmAction::RemoveContact => {
                let contact = self.contacts.get(self.selected_contact)?;
                let label = if contact.petname.is_empty() {
                    super::short_id(&contact.pubkey).to_string()
                } else {
                    contact.petname.clone()
                };
                Some((
                    "Remove contact".to_string(),
                    format!("Remove contact {label}?"),
                ))
            }
            ConfirmAction::DeleteNote => {
                let note = self.notes.get(self.selected_note)?;
                let label = if note.title.is_empty() {
                    note.slug.clone()
                } else {
                    note.title.clone()
                };
                Some((
                    "Delete note".to_string(),
                    format!("Delete long-form note {label}?"),
                ))
            }
            ConfirmAction::RemoveMember(pubkey) => Some((
                "Remove member".to_string(),
                format!(
                    "Remove member {} from this channel?",
                    super::short_id(pubkey)
                ),
            )),
        }
    }
}

fn preview(content: &str) -> String {
    let trimmed = content.trim().replace('\n', " ");
    if trimmed.chars().count() > 80 {
        let truncated: String = trimmed.chars().take(77).collect();
        format!("{truncated}...")
    } else {
        trimmed
    }
}
