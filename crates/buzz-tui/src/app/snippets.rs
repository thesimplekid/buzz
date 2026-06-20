use super::{clamp_index, App, Focus, SavedSnippet, SnippetEditField};

impl App {
    pub fn focus_snippets(&mut self) {
        clamp_index(
            &mut self.selected_snippet,
            self.workspace_config.snippets.len(),
        );
        self.clear_snippet_editor();
        self.focus = Focus::Snippets;
        self.status = if self.workspace_config.snippets.is_empty() {
            "No saved snippets; A creates one".to_string()
        } else {
            "Snippets: Enter inserts, A creates, E edits, D deletes".to_string()
        };
    }

    pub fn focus_create_snippet(&mut self) {
        self.clear_snippet_editor();
        self.focus = Focus::SnippetEdit;
        self.status = "Creating snippet; Tab switches fields, Enter saves".to_string();
    }

    pub fn focus_edit_snippet(&mut self) {
        let Some(snippet) = self
            .workspace_config
            .snippets
            .get(self.selected_snippet)
            .cloned()
        else {
            self.status = "No snippet selected".to_string();
            return;
        };
        self.snippet_edit_id = Some(snippet.id);
        self.snippet_name = snippet.name;
        self.snippet_content = snippet.content;
        self.snippet_edit_field = SnippetEditField::Name;
        self.focus = Focus::SnippetEdit;
        self.status = "Editing snippet; Tab switches fields, Enter saves".to_string();
    }

    pub fn save_snippet(&mut self) {
        let name = self.snippet_name.trim().to_string();
        if name.is_empty() {
            self.status = "Snippet name is empty".to_string();
            return;
        }
        if self.snippet_content.trim().is_empty() {
            self.status = "Snippet content is empty".to_string();
            return;
        }
        let content = self.snippet_content.clone();

        if let Some(id) = self.snippet_edit_id.clone() {
            let Some(index) = self
                .workspace_config
                .snippets
                .iter()
                .position(|snippet| snippet.id == id)
            else {
                self.status = "Snippet no longer exists".to_string();
                return;
            };
            self.workspace_config.snippets[index].name = name.clone();
            self.workspace_config.snippets[index].content = content;
            self.selected_snippet = index;
            self.status = format!("Saved snippet {name}");
        } else {
            let id = self.workspace_config.next_snippet_id(&name);
            self.workspace_config.snippets.push(SavedSnippet {
                id,
                name: name.clone(),
                content,
            });
            self.selected_snippet = self.workspace_config.snippets.len().saturating_sub(1);
            self.status = format!("Created snippet {name}");
        }

        self.save_workspace_config("snippets");
        self.clear_snippet_editor();
        self.focus = Focus::Snippets;
    }

    pub fn delete_selected_snippet(&mut self) {
        if self.selected_snippet >= self.workspace_config.snippets.len() {
            self.status = "No snippet selected".to_string();
            return;
        }
        let removed = self.workspace_config.snippets.remove(self.selected_snippet);
        clamp_index(
            &mut self.selected_snippet,
            self.workspace_config.snippets.len(),
        );
        self.save_workspace_config("snippets");
        self.status = format!("Deleted snippet {}", removed.name);
    }

    pub fn insert_selected_snippet(&mut self) {
        let Some(snippet) = self
            .workspace_config
            .snippets
            .get(self.selected_snippet)
            .cloned()
        else {
            self.status = "No snippet selected".to_string();
            return;
        };
        let cursor = normalized_cursor(&self.composer, self.composer_cursor);
        self.composer.insert_str(cursor, &snippet.content);
        self.composer_cursor = cursor + snippet.content.len();
        self.save_active_channel_draft();
        self.focus = Focus::Composer;
        self.status = format!("Inserted snippet {}", snippet.name);
    }

    pub fn next_snippet_edit_field(&mut self) {
        self.snippet_edit_field = match self.snippet_edit_field {
            SnippetEditField::Name => SnippetEditField::Content,
            SnippetEditField::Content => SnippetEditField::Name,
        };
    }

    pub fn previous_snippet_edit_field(&mut self) {
        self.next_snippet_edit_field();
    }

    pub fn snippet_input_push(&mut self, ch: char) {
        if matches!(ch, '\n' | '\r') {
            return;
        }
        self.selected_snippet_input_mut().push(ch);
    }

    pub fn snippet_input_pop(&mut self) {
        self.selected_snippet_input_mut().pop();
    }

    pub fn snippet_content_newline(&mut self) {
        if self.snippet_edit_field == SnippetEditField::Content {
            self.snippet_content.push('\n');
        }
    }

    pub fn clear_snippet_editor(&mut self) {
        self.snippet_edit_id = None;
        self.snippet_name.clear();
        self.snippet_content.clear();
        self.snippet_edit_field = SnippetEditField::Name;
    }

    fn selected_snippet_input_mut(&mut self) -> &mut String {
        match self.snippet_edit_field {
            SnippetEditField::Name => &mut self.snippet_name,
            SnippetEditField::Content => &mut self.snippet_content,
        }
    }
}

fn normalized_cursor(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len());
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}
