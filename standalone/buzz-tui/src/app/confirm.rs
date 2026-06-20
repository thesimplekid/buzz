use super::{App, Focus};

/// A destructive or hard-to-undo action that must be confirmed before it runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmAction {
    DeleteMessage,
    DeleteWorkflow,
    DeleteManagedAgent,
    DeleteMemory,
    RemoveEmoji,
    DeleteSnippet,
    SendDraft,
    DeleteDraft,
    RemoveWorkspace,
    LeaveChannel,
    HideDm,
    ArchiveChannel,
    DeleteChannel,
    RemoveContact,
    DeleteNote,
    RemoveMember(String),
    RemoveRelayMember(String),
    ExportIdentity,
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
            ConfirmAction::DeleteSnippet => self.delete_selected_snippet(),
            ConfirmAction::SendDraft => self.send_selected_draft().await,
            ConfirmAction::DeleteDraft => self.delete_selected_draft(),
            ConfirmAction::RemoveWorkspace => self.remove_selected_workspace(),
            ConfirmAction::LeaveChannel => self.leave_selected_channel().await,
            ConfirmAction::HideDm => self.hide_selected_dm().await,
            ConfirmAction::ArchiveChannel => self.archive_selected_channel().await,
            ConfirmAction::DeleteChannel => self.delete_selected_channel().await,
            ConfirmAction::RemoveContact => self.delete_selected_contact().await,
            ConfirmAction::DeleteNote => self.delete_selected_note().await,
            ConfirmAction::RemoveMember(pubkey) => self.remove_member_confirmed(pubkey).await,
            ConfirmAction::RemoveRelayMember(pubkey) => {
                self.remove_relay_member_confirmed(pubkey).await
            }
            ConfirmAction::ExportIdentity => self.reveal_identity_secret(),
        }
    }

    /// Build a `(title, body)` for a confirmation, or `None` (with a status
    /// message) when the action cannot apply to the current selection.
    fn confirm_description(&mut self, action: &ConfirmAction) -> Option<(String, String)> {
        match action {
            ConfirmAction::DeleteMessage => {
                let message = self.selected_timeline_message()?;
                let own_pubkey = self
                    .native_relay_client()
                    .ok()
                    .map(|client| client.public_key_hex());
                let is_own = own_pubkey.as_deref() == Some(message.pubkey.as_str());
                let body = if is_own || own_pubkey.is_none() {
                    format!("Delete this message?\n{}", preview(&message.content))
                } else if let Some(agent_label) = self.owned_agent_label(&message.pubkey) {
                    format!(
                        "Delete your agent {agent_label}'s message?\n{}",
                        preview(&message.content)
                    )
                } else {
                    format!(
                        "Delete this message by {}? The relay only accepts deletes from the author or their agent owner.\n{}",
                        self.author_label(&message.pubkey),
                        preview(&message.content)
                    )
                };
                Some(("Delete message".to_string(), body))
            }
            ConfirmAction::DeleteWorkflow => {
                let workflow = self.workflows.get(self.selected_workflow)?;
                Some((
                    "Delete workflow".to_string(),
                    format!("Delete workflow {}?", workflow.workflow_id),
                ))
            }
            ConfirmAction::DeleteManagedAgent => {
                let (_, label) = self.selected_managed_agent_identity()?;
                Some((
                    "Delete agent".to_string(),
                    format!("Delete managed agent {label}?"),
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
            ConfirmAction::DeleteSnippet => {
                let snippet = self.workspace_config.snippets.get(self.selected_snippet)?;
                Some((
                    "Delete snippet".to_string(),
                    format!("Delete snippet {}?", snippet.name),
                ))
            }
            ConfirmAction::SendDraft => {
                let draft = self.selected_draft_entry()?;
                Some((
                    "Send draft".to_string(),
                    format!(
                        "Send this draft to #{}?\n{}",
                        self.draft_channel_name(&draft.channel_id),
                        preview(&draft.content)
                    ),
                ))
            }
            ConfirmAction::DeleteDraft => {
                let draft = self.selected_draft_entry()?;
                Some((
                    "Discard draft".to_string(),
                    format!(
                        "Discard the draft for #{}?\n{}",
                        self.draft_channel_name(&draft.channel_id),
                        preview(&draft.content)
                    ),
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
            ConfirmAction::RemoveRelayMember(pubkey) => Some((
                "Remove relay member".to_string(),
                format!(
                    "Remove relay member {} from this relay?",
                    super::short_id(pubkey)
                ),
            )),
            ConfirmAction::ExportIdentity => Some((
                "Export private key".to_string(),
                "Reveal your private identity key? Anyone with this key can act as you."
                    .to_string(),
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
