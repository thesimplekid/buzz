use super::{clamp_index, short_id, App, Focus};
use std::collections::BTreeMap;

/// One saved composer draft in the active workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftEntry {
    pub key: String,
    pub channel_id: String,
    pub thread_root: Option<String>,
    pub content: String,
    pub mention_pubkeys_by_name: BTreeMap<String, String>,
}

impl App {
    /// Drafts belonging to the active workspace, in stable key order.
    pub fn workspace_drafts(&self) -> Vec<DraftEntry> {
        let prefix = format!("{}::", self.active_workspace_id());
        self.channel_drafts
            .iter()
            .filter(|(_, content)| !content.trim().is_empty())
            .filter_map(|(key, content)| {
                let rest = key.strip_prefix(&prefix)?;
                let (channel_id, thread) = rest.split_once("::")?;
                Some(DraftEntry {
                    key: key.clone(),
                    channel_id: channel_id.to_string(),
                    thread_root: (thread != "channel").then(|| thread.to_string()),
                    content: content.clone(),
                    mention_pubkeys_by_name: self
                        .channel_draft_mentions
                        .get(key)
                        .cloned()
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    pub fn draft_channel_name(&self, channel_id: &str) -> String {
        self.channels
            .iter()
            .find(|channel| channel.id == channel_id)
            .map(|channel| channel.name.clone())
            .unwrap_or_else(|| short_id(channel_id).to_string())
    }

    pub fn focus_drafts(&mut self) {
        let count = self.workspace_drafts().len();
        clamp_index(&mut self.selected_draft, count);
        self.focus = Focus::Drafts;
        self.status = if count == 0 {
            "No drafts; unsent composer text lands here".to_string()
        } else {
            "Drafts: Enter opens, S sends, D discards".to_string()
        };
    }

    pub fn selected_draft_entry(&self) -> Option<DraftEntry> {
        self.workspace_drafts().get(self.selected_draft).cloned()
    }

    /// Persist in-memory drafts into the workspace config file when changed.
    pub fn flush_drafts(&mut self) {
        if !self.drafts_dirty {
            return;
        }
        self.workspace_config.drafts = self.channel_drafts.clone();
        self.workspace_config.draft_mentions = self.channel_draft_mentions.clone();
        self.save_workspace_config("drafts");
        self.drafts_dirty = false;
    }

    /// Open the selected draft's channel (and thread) with the draft restored
    /// into the composer.
    pub async fn open_selected_draft(&mut self) {
        let Some(entry) = self.selected_draft_entry() else {
            self.status = "No draft selected".to_string();
            return;
        };
        let Some(index) = self
            .channels
            .iter()
            .position(|channel| channel.id == entry.channel_id)
        else {
            self.status = "Draft channel is not loaded; refresh channels with r".to_string();
            return;
        };
        self.selected_channel = index;
        self.load_selected_channel().await;
        if let Some(root) = entry.thread_root.clone() {
            match self.get_thread_messages(&entry.channel_id, &root).await {
                Ok(messages) => {
                    self.remember_message_author_profiles(&messages).await;
                    self.begin_unread_session(&entry.channel_id, Some(&root));
                    self.thread_root = Some(root);
                    self.messages = messages;
                    self.selected_message = self.messages.len().saturating_sub(1);
                    self.reset_message_detail_scroll();
                }
                Err(error) => {
                    self.status = format!("draft thread: {error}");
                    return;
                }
            }
        }
        self.focus_composer();
        self.status = format!(
            "Opened draft in #{}",
            self.draft_channel_name(&entry.channel_id)
        );
    }

    /// Send the selected draft as-is (confirmed via the confirm overlay).
    pub async fn send_selected_draft(&mut self) {
        let Some(entry) = self.selected_draft_entry() else {
            self.status = "No draft selected".to_string();
            return;
        };
        let content = entry.content.trim().to_string();
        if content.is_empty() {
            self.discard_draft_entry(&entry);
            self.status = "Draft was empty; discarded".to_string();
            return;
        }
        let mention_pubkeys =
            super::composer::mention_refs_in_content(&content, &entry.mention_pubkeys_by_name)
                .into_values()
                .collect::<Vec<_>>();
        match self
            .send_message_native(
                &entry.channel_id,
                &content,
                &mention_pubkeys,
                entry.thread_root.as_deref(),
            )
            .await
        {
            Ok(_) => {
                self.discard_draft_entry(&entry);
                self.status = format!(
                    "Sent draft to #{}",
                    self.draft_channel_name(&entry.channel_id)
                );
            }
            Err(error) => self.status = format!("send draft: {error}"),
        }
    }

    /// Discard the selected draft (confirmed via the confirm overlay).
    pub fn delete_selected_draft(&mut self) {
        let Some(entry) = self.selected_draft_entry() else {
            self.status = "No draft selected".to_string();
            return;
        };
        self.discard_draft_entry(&entry);
        self.status = format!(
            "Discarded draft for #{}",
            self.draft_channel_name(&entry.channel_id)
        );
    }

    fn discard_draft_entry(&mut self, entry: &DraftEntry) {
        if self.channel_drafts.remove(&entry.key).is_some() {
            self.drafts_dirty = true;
        }
        if self.channel_draft_mentions.remove(&entry.key).is_some() {
            self.drafts_dirty = true;
        }
        // Clear the live composer too when it holds this draft, so the text
        // isn't immediately re-saved on the next keystroke.
        let is_active_context = self.active_channel_id.as_deref() == Some(&entry.channel_id)
            && self.thread_root == entry.thread_root;
        if is_active_context && self.composer == entry.content {
            self.composer.clear();
            self.composer_mentions.clear();
            self.composer_cursor = 0;
        }
        let count = self.workspace_drafts().len();
        clamp_index(&mut self.selected_draft, count);
        self.flush_drafts();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_app;

    #[test]
    fn workspace_drafts_parse_channel_and_thread_keys() {
        let mut app = test_app();
        let workspace = app.active_workspace_id().to_string();
        app.channel_drafts
            .insert(format!("{workspace}::chan-1::channel"), "hello".to_string());
        app.channel_drafts.insert(
            format!("{workspace}::chan-2::a1b2c3"),
            "thread reply".to_string(),
        );
        app.channel_drafts
            .insert(format!("{workspace}::chan-3::channel"), "  ".to_string());
        app.channel_drafts
            .insert("other-ws::chan-4::channel".to_string(), "nope".to_string());

        let drafts = app.workspace_drafts();
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].channel_id, "chan-1");
        assert_eq!(drafts[0].thread_root, None);
        assert_eq!(drafts[1].channel_id, "chan-2");
        assert_eq!(drafts[1].thread_root, Some("a1b2c3".to_string()));
    }

    #[test]
    fn flush_drafts_persists_only_when_dirty() {
        let mut app = test_app();
        let workspace = app.active_workspace_id().to_string();
        app.channel_drafts
            .insert(format!("{workspace}::chan-1::channel"), "hello".to_string());

        app.flush_drafts();
        assert!(app.workspace_config.drafts.is_empty());

        app.drafts_dirty = true;
        app.flush_drafts();
        assert_eq!(app.workspace_config.drafts, app.channel_drafts);
        assert!(!app.drafts_dirty);
    }
}
