use super::{
    nostr_pubkey_uri, parse_attachment_paths, short_id, App, DiffField, Focus, TimelineMode,
};
use crate::cli::SendDiffOptions;

/// What an autocomplete popup is currently completing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionKind {
    Mention,
    Channel,
    Emoji,
}

impl CompletionKind {
    fn trigger(self) -> char {
        match self {
            CompletionKind::Mention => '@',
            CompletionKind::Channel => '#',
            CompletionKind::Emoji => ':',
        }
    }
}

/// A single autocomplete suggestion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionItem {
    pub display: String,
    pub insert: String,
}

/// Active autocomplete state for the composer.
#[derive(Clone, Debug)]
pub struct CompletionState {
    pub kind: CompletionKind,
    pub token_start: usize,
    pub matches: Vec<CompletionItem>,
    pub selected: usize,
}

impl App {
    pub fn composer_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            let cursor = normalize_cursor(&self.composer, self.composer_cursor);
            self.composer.insert(cursor, ch);
            self.composer_cursor = cursor + ch.len_utf8();
            self.update_composer_completion();
            self.save_active_channel_draft();
        }
    }

    pub fn composer_pop(&mut self) {
        let cursor = normalize_cursor(&self.composer, self.composer_cursor);
        if cursor == 0 {
            return;
        }
        let previous = previous_boundary(&self.composer, cursor);
        self.composer.replace_range(previous..cursor, "");
        self.composer_cursor = previous;
        self.update_composer_completion();
        self.save_active_channel_draft();
    }

    pub fn composer_delete(&mut self) {
        let cursor = normalize_cursor(&self.composer, self.composer_cursor);
        if cursor >= self.composer.len() {
            return;
        }
        let next = next_boundary(&self.composer, cursor);
        self.composer.replace_range(cursor..next, "");
        self.composer_cursor = cursor;
        self.update_composer_completion();
        self.save_active_channel_draft();
    }

    /// Insert a newline so the composer supports multiline messages.
    pub fn composer_newline(&mut self) {
        let cursor = normalize_cursor(&self.composer, self.composer_cursor);
        self.composer.insert(cursor, '\n');
        self.composer_cursor = cursor + 1;
        self.composer_completion = None;
        self.save_active_channel_draft();
    }

    pub fn composer_left(&mut self) {
        self.composer_cursor = previous_boundary(
            &self.composer,
            normalize_cursor(&self.composer, self.composer_cursor),
        );
        self.update_composer_completion();
    }

    pub fn composer_right(&mut self) {
        self.composer_cursor = next_boundary(
            &self.composer,
            normalize_cursor(&self.composer, self.composer_cursor),
        );
        self.update_composer_completion();
    }

    pub fn composer_home(&mut self) {
        let cursor = normalize_cursor(&self.composer, self.composer_cursor);
        self.composer_cursor = self.composer[..cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        self.update_composer_completion();
    }

    pub fn composer_end(&mut self) {
        let cursor = normalize_cursor(&self.composer, self.composer_cursor);
        self.composer_cursor = self.composer[cursor..]
            .find('\n')
            .map(|offset| cursor + offset)
            .unwrap_or(self.composer.len());
        self.update_composer_completion();
    }

    /// Handle `Up` in the composer: navigate an open completion popup, or, when
    /// the composer is empty, edit the most recent own message in scope.
    pub fn composer_up(&mut self) {
        if self.composer_completion.is_some() {
            self.completion_move(-1);
            return;
        }
        if self.composer.is_empty() && self.edit_target.is_none() {
            self.edit_last_own_message();
        }
    }

    /// Handle `Down` in the composer: navigate an open completion popup.
    pub fn composer_down(&mut self) {
        if self.composer_completion.is_some() {
            self.completion_move(1);
        }
    }

    pub fn completion_move(&mut self, delta: isize) {
        if let Some(state) = &mut self.composer_completion {
            let len = state.matches.len();
            if len == 0 {
                return;
            }
            let next = (state.selected as isize + delta).rem_euclid(len as isize);
            state.selected = next as usize;
        }
    }

    /// Accept the selected completion, replacing the trailing token.
    pub fn accept_completion(&mut self) -> bool {
        let Some(state) = self.composer_completion.take() else {
            return false;
        };
        let Some(item) = state.matches.get(state.selected).cloned() else {
            return false;
        };
        let cursor = normalize_cursor(&self.composer, self.composer_cursor);
        let mut text = self.composer[..state.token_start].to_string();
        text.push(state.kind.trigger());
        text.push_str(&item.insert);
        text.push(' ');
        let new_cursor = text.len();
        text.push_str(&self.composer[cursor..]);
        self.composer = text;
        self.composer_cursor = new_cursor;
        self.save_active_channel_draft();
        true
    }

    /// Recompute the completion popup from the trailing token of the composer.
    fn update_composer_completion(&mut self) {
        self.composer_cursor = normalize_cursor(&self.composer, self.composer_cursor);
        let prefix = &self.composer[..self.composer_cursor];
        let Some((token_start, kind, query)) = detect_completion_token(prefix) else {
            self.composer_completion = None;
            return;
        };
        let matches = self.completion_matches(kind, &query);
        if matches.is_empty() {
            self.composer_completion = None;
        } else {
            self.composer_completion = Some(CompletionState {
                kind,
                token_start,
                matches,
                selected: 0,
            });
        }
    }

    fn completion_matches(&self, kind: CompletionKind, query: &str) -> Vec<CompletionItem> {
        let query = query.to_lowercase();
        let mut items: Vec<CompletionItem> = match kind {
            CompletionKind::Mention => {
                let mut people: Vec<CompletionItem> = Vec::new();
                for agent in self.acp.agents() {
                    let label = agent.runtime.label.clone();
                    people.push(CompletionItem {
                        display: format!("{label} (agent)"),
                        insert: mention_insert(&label, &agent.runtime.id),
                    });
                }
                for contact in &self.contacts {
                    let name = if contact.petname.is_empty() {
                        short_id(&contact.pubkey).to_string()
                    } else {
                        contact.petname.clone()
                    };
                    people.push(CompletionItem {
                        display: name.clone(),
                        insert: mention_insert(&name, &contact.pubkey),
                    });
                }
                for member in &self.channel_members {
                    let label = short_id(&member.pubkey).to_string();
                    people.push(CompletionItem {
                        display: format!("{label} ({})", member.role),
                        insert: mention_insert(&label, &member.pubkey),
                    });
                }
                people
            }
            CompletionKind::Channel => self
                .channels
                .iter()
                .map(|channel| CompletionItem {
                    display: channel.name.clone(),
                    insert: channel.name.clone(),
                })
                .collect(),
            CompletionKind::Emoji => self
                .workspace_emoji
                .iter()
                .chain(self.own_emoji.iter())
                .map(|emoji| CompletionItem {
                    display: format!(":{}:", emoji.shortcode),
                    insert: format!("{}:", emoji.shortcode),
                })
                .collect(),
        };
        items.retain(|item| item.display.to_lowercase().contains(&query));
        items.sort_by(|a, b| {
            let a_prefix = a.display.to_lowercase().starts_with(&query);
            let b_prefix = b.display.to_lowercase().starts_with(&query);
            b_prefix.cmp(&a_prefix).then(a.display.cmp(&b.display))
        });
        items.dedup_by(|a, b| a.display == b.display);
        items.truncate(8);
        items
    }

    /// Edit the most recent own message in the active channel timeline.
    fn edit_last_own_message(&mut self) {
        let own = self.own_pubkey_hex();
        let target = self.messages.iter().rev().find(|message| {
            !message.id.is_empty() && own.as_deref().is_none_or(|key| message.pubkey == key)
        });
        match target {
            Some(message) => {
                self.edit_target = Some(message.id.clone());
                self.composer = message.content.clone();
                self.composer_cursor = self.composer.len();
                self.status = format!("Editing {}", short_id(&message.id));
            }
            None => {
                self.status = "No own message to edit".to_string();
            }
        }
    }

    fn own_pubkey_hex(&self) -> Option<String> {
        self.native_relay_client()
            .ok()
            .map(|client| client.public_key_hex())
    }

    pub fn attachment_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.attachment_input.push(ch);
        }
    }

    pub fn attachment_pop(&mut self) {
        self.attachment_input.pop();
    }

    pub fn focus_composer(&mut self) {
        self.composer_completion = None;
        self.edit_target = None;
        if self.timeline_mode == TimelineMode::Pulse {
            self.pulse_reply_target = None;
            self.status = "Composing Pulse note".to_string();
        } else {
            self.restore_active_channel_draft();
        }
        self.composer_cursor = self.composer.len();
        self.focus = Focus::Composer;
    }

    pub fn focus_attachment(&mut self) {
        if self.timeline_mode == TimelineMode::Pulse {
            self.status = "Pulse attachments are not supported by buzz social publish".to_string();
            return;
        }
        if self.active_channel().is_none() {
            self.status = "No channel selected".to_string();
            return;
        }
        self.edit_target = None;
        self.attachment_input.clear();
        self.focus = Focus::Attachment;
        self.status = "Attaching files; composer text is used as caption".to_string();
    }

    pub fn focus_diff(&mut self) {
        if self.timeline_mode == TimelineMode::Pulse {
            self.status = "Pulse diffs are not supported by buzz social publish".to_string();
            return;
        }
        if self.active_channel().is_none() {
            self.status = "No channel selected".to_string();
            return;
        }
        self.edit_target = None;
        self.diff_field = DiffField::Repo;
        self.focus = Focus::Diff;
        self.status = "Composing code diff".to_string();
    }

    pub fn diff_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.selected_diff_input_mut().push(ch);
        }
    }

    pub fn diff_input_pop(&mut self) {
        self.selected_diff_input_mut().pop();
    }

    pub fn next_diff_field(&mut self) {
        self.diff_field = match self.diff_field {
            DiffField::Repo => DiffField::Commit,
            DiffField::Commit => DiffField::File,
            DiffField::File => DiffField::Description,
            DiffField::Description => DiffField::Diff,
            DiffField::Diff => DiffField::Repo,
        };
    }

    pub fn previous_diff_field(&mut self) {
        self.diff_field = match self.diff_field {
            DiffField::Repo => DiffField::Diff,
            DiffField::Commit => DiffField::Repo,
            DiffField::File => DiffField::Commit,
            DiffField::Description => DiffField::File,
            DiffField::Diff => DiffField::Description,
        };
    }

    pub fn focus_pulse_reply(&mut self) {
        let Some(message) = self.pulse.get(self.selected_pulse) else {
            self.status = "No Pulse note selected".to_string();
            return;
        };
        if message.id.is_empty() {
            self.status = "Selected Pulse note has no event id".to_string();
            return;
        }
        self.edit_target = None;
        self.pulse_reply_target = Some(message.id.clone());
        self.composer.clear();
        self.composer_cursor = 0;
        self.focus = Focus::Composer;
        self.status = format!("Replying to Pulse note {}", short_id(&message.id));
    }

    pub fn edit_selected_message(&mut self) {
        let Some(message) = self.selected_timeline_message() else {
            self.status = "No message selected".to_string();
            return;
        };
        if message.id.is_empty() {
            self.status = "Selected message has no event id".to_string();
            return;
        }
        self.edit_target = Some(message.id.clone());
        self.composer = message.content;
        self.composer_cursor = self.composer.len();
        self.focus = Focus::Composer;
        self.status = format!("Editing {}", short_id(&message.id));
    }

    pub(super) async fn send_composer(&mut self) {
        self.composer_completion = None;
        let content = self.composer.trim().to_string();
        if content.is_empty() {
            return;
        }
        if let Some(event_id) = self.edit_target.clone() {
            match self.edit_message(&event_id, &content).await {
                Ok(_) => {
                    self.composer.clear();
                    self.composer_cursor = 0;
                    self.edit_target = None;
                    self.update_selected_timeline_message_content(&event_id, &content);
                    self.focus = Focus::Timeline;
                    self.status = format!("Edited {}", short_id(&event_id));
                }
                Err(error) => {
                    self.status = format!("edit: {error}");
                }
            }
            return;
        }
        if self.timeline_mode == TimelineMode::Pulse {
            self.send_pulse_note(content).await;
            return;
        }
        let Some(channel) = self.active_channel() else {
            self.status = "No channel selected".to_string();
            return;
        };
        let reply_to = self.thread_root.clone();
        let result = self
            .send_message_native(&channel.id, &content, reply_to.as_deref())
            .await;
        match result {
            Ok(_) => {
                self.composer.clear();
                self.composer_cursor = 0;
                self.clear_channel_draft(&channel.id);
                if reply_to.is_some() {
                    self.status = format!("Replied in #{}", channel.name);
                    self.refresh_active().await;
                } else {
                    self.status = format!("Sent to #{}", channel.name);
                    self.load_selected_channel().await;
                }
            }
            Err(error) => {
                self.status = format!("send: {error}");
            }
        }
    }

    pub(super) async fn send_attachment(&mut self) {
        let files = parse_attachment_paths(&self.attachment_input);
        if files.is_empty() {
            self.status = "Type at least one file path".to_string();
            return;
        }
        let Some(channel) = self.active_channel() else {
            self.status = "No channel selected".to_string();
            return;
        };
        let content = self.composer.trim().to_string();
        let reply_to = self.thread_root.clone();
        match self
            .send_message_with_files_native(&channel.id, &content, reply_to.as_deref(), &files)
            .await
        {
            Ok(_) => {
                let count = files.len();
                self.composer.clear();
                self.composer_cursor = 0;
                self.clear_channel_draft(&channel.id);
                self.attachment_input.clear();
                self.focus = Focus::Timeline;
                self.refresh_active().await;
                self.status = format!(
                    "Sent {count} attachment{}",
                    if count == 1 { "" } else { "s" }
                );
            }
            Err(error) => {
                self.status = format!("attachment send: {error}");
            }
        }
    }

    pub(super) async fn send_diff(&mut self) {
        let Some(channel) = self.active_channel() else {
            self.status = "No channel selected".to_string();
            return;
        };
        if self.diff_repo.trim().is_empty() {
            self.status = "Diff repo URL is empty".to_string();
            self.diff_field = DiffField::Repo;
            return;
        }
        if self.diff_commit.trim().is_empty() {
            self.status = "Diff commit SHA is empty".to_string();
            self.diff_field = DiffField::Commit;
            return;
        }
        if self.diff_content.trim().is_empty() {
            self.status = "Diff body is empty".to_string();
            self.diff_field = DiffField::Diff;
            return;
        }

        let options = SendDiffOptions {
            repo_url: self.diff_repo.trim().to_string(),
            commit_sha: self.diff_commit.trim().to_string(),
            file_path: self.diff_file.trim().to_string(),
            description: self.diff_description.trim().to_string(),
            diff: self.diff_content.clone(),
        };
        let reply_to = self.thread_root.as_deref();
        match self.send_diff_native(&channel.id, &options, reply_to).await {
            Ok(_) => {
                self.clear_diff_inputs();
                self.focus = Focus::Timeline;
                self.refresh_active().await;
                self.status = format!("Sent diff to #{}", channel.name);
            }
            Err(error) => {
                self.status = format!("send diff: {error}");
            }
        }
    }

    async fn send_pulse_note(&mut self, content: String) {
        let reply_to = self.pulse_reply_target.clone();
        match self
            .publish_social_note_native(&content, reply_to.as_deref())
            .await
        {
            Ok(_) => {
                self.composer.clear();
                self.composer_cursor = 0;
                self.pulse_reply_target = None;
                self.focus = Focus::Pulse;
                let status = if let Some(reply_to) = reply_to {
                    format!("Replied to Pulse note {}", short_id(&reply_to))
                } else {
                    "Published Pulse note".to_string()
                };
                self.focus_pulse().await;
                self.status = status;
            }
            Err(error) => {
                self.status = format!("pulse publish: {error}");
            }
        }
    }

    fn selected_diff_input_mut(&mut self) -> &mut String {
        match self.diff_field {
            DiffField::Repo => &mut self.diff_repo,
            DiffField::Commit => &mut self.diff_commit,
            DiffField::File => &mut self.diff_file,
            DiffField::Description => &mut self.diff_description,
            DiffField::Diff => &mut self.diff_content,
        }
    }

    pub(super) fn clear_diff_inputs(&mut self) {
        self.diff_field = DiffField::Repo;
        self.diff_repo.clear();
        self.diff_commit.clear();
        self.diff_file.clear();
        self.diff_description.clear();
        self.diff_content.clear();
    }
}

/// Detect a trailing `@`/`#`/`:` completion token in `text`. Returns the byte
/// index of the trigger character, the completion kind, and the typed query.
fn detect_completion_token(text: &str) -> Option<(usize, CompletionKind, String)> {
    let start = text
        .rfind(|c: char| c.is_whitespace())
        .map(|index| index + 1)
        .unwrap_or(0);
    let token = &text[start..];
    let mut chars = token.chars();
    let trigger = chars.next()?;
    let kind = match trigger {
        '@' => CompletionKind::Mention,
        '#' => CompletionKind::Channel,
        ':' => CompletionKind::Emoji,
        _ => return None,
    };
    let query: String = chars.collect();
    if query
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        Some((start, kind, query))
    } else {
        None
    }
}

fn mention_insert(label: &str, pubkey: &str) -> String {
    if let Some(uri) = nostr_pubkey_uri(pubkey) {
        format!("{label} {uri}")
    } else {
        label.to_string()
    }
}

fn normalize_cursor(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len());
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

fn previous_boundary(text: &str, cursor: usize) -> usize {
    let cursor = normalize_cursor(text, cursor);
    if cursor == 0 {
        return 0;
    }
    text[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_boundary(text: &str, cursor: usize) -> usize {
    let cursor = normalize_cursor(text, cursor);
    if cursor >= text.len() {
        return text.len();
    }
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_mention_token() {
        let (start, kind, query) = detect_completion_token("hello @al").unwrap();
        assert_eq!(start, 6);
        assert_eq!(kind, CompletionKind::Mention);
        assert_eq!(query, "al");
    }

    #[test]
    fn detects_channel_and_emoji_tokens() {
        let (_, kind, query) = detect_completion_token("see #gen").unwrap();
        assert_eq!(kind, CompletionKind::Channel);
        assert_eq!(query, "gen");
        let (_, kind, query) = detect_completion_token("nice :smi").unwrap();
        assert_eq!(kind, CompletionKind::Emoji);
        assert_eq!(query, "smi");
    }

    #[test]
    fn ignores_non_trigger_and_closed_tokens() {
        assert!(detect_completion_token("plain text").is_none());
        // A completed `:emoji:` ends in a colon, breaking the wordy-query rule.
        assert!(detect_completion_token("done :smile:").is_none());
    }

    #[test]
    fn mention_insert_includes_nip27_pubkey_uri() {
        assert_eq!(
            mention_insert(
                "Helper",
                "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e",
            ),
            "Helper nostr:npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg"
        );
    }
}
