use super::{clamp_index, parse_workflow_inputs, short_id, App, Focus, WorkflowApprovalField};

const BASIC_WORKFLOW_TEMPLATE: &str = r#"name: "New workflow"
description: "Posts a message when someone posts in this channel."
trigger:
  on: message_posted
steps:
  - id: post_message
    action: send_message
    text: "Workflow saw: {{trigger.text | truncate(160)}}"
"#;

const SCHEDULED_DIGEST_WORKFLOW_TEMPLATE: &str = r#"name: "Daily maintainer digest"
description: "Calls a digest service each weekday morning and posts the response body."
trigger:
  on: schedule
  cron: "0 9 * * 1-5"
steps:
  - id: fetch_digest
    action: call_webhook
    url: "https://digest-service.example.invalid/buzz/digests/cdk"
    method: "POST"
    headers:
      content-type: "application/json"
    body: '{"repo":"cashubtc/cdk","window":"24h"}'
  - id: post_digest
    action: send_message
    text: "{{steps.fetch_digest.output.body}}"
"#;

const WEBHOOK_DIGEST_WORKFLOW_TEMPLATE: &str = r#"name: "Webhook digest"
description: "Receives a digest payload and posts its summary field."
trigger:
  on: webhook
steps:
  - id: post_digest
    action: send_message
    text: "{{trigger.summary}}"
"#;

impl App {
    pub async fn focus_workflows(&mut self) {
        let Some(channel) = self.selected_channel_for_workflows() else {
            return;
        };
        match self.list_workflows_native(&channel.id).await {
            Ok(workflows) => {
                self.workflow_channel_id = channel.id;
                self.workflows = workflows;
                clamp_index(&mut self.selected_workflow, self.workflows.len());
                self.selected_workflow_detail = None;
                self.refresh_selected_workflow_runs().await;
                self.refresh_selected_workflow_detail().await;
                self.focus = Focus::Workflows;
                self.status = format!(
                    "Loaded {} workflow{} for #{}",
                    self.workflows.len(),
                    if self.workflows.len() == 1 { "" } else { "s" },
                    channel.name
                );
            }
            Err(error) => self.status = format!("workflows: {error}"),
        }
    }

    pub async fn trigger_selected_workflow(&mut self) {
        let Some(workflow) = self.workflows.get(self.selected_workflow).cloned() else {
            self.status = "No workflow selected".to_string();
            return;
        };
        match self
            .trigger_workflow_native(&workflow.workflow_id, None)
            .await
        {
            Ok(_) => {
                self.refresh_selected_workflow_runs().await;
                self.status = format!("Triggered workflow {}", short_id(&workflow.workflow_id));
            }
            Err(error) => self.status = format!("workflow trigger: {error}"),
        }
    }

    pub fn focus_workflow_inputs(&mut self) {
        let Some(workflow) = self.workflows.get(self.selected_workflow).cloned() else {
            self.status = "No workflow selected".to_string();
            return;
        };
        self.workflow_edit_id = Some(workflow.workflow_id.clone());
        self.workflow_inputs = "{}".to_string();
        self.focus = Focus::WorkflowInputs;
        self.status = format!(
            "Trigger inputs for workflow {}",
            short_id(&workflow.workflow_id)
        );
    }

    pub async fn trigger_selected_workflow_with_inputs(&mut self) {
        let Some(workflow_id) = self.workflow_edit_id.clone() else {
            self.status = "No workflow selected for trigger".to_string();
            return;
        };
        let inputs = self.workflow_inputs.trim();
        if inputs.is_empty() {
            self.status = "Workflow inputs must be a JSON object".to_string();
            return;
        }
        if let Err(message) = parse_workflow_inputs(inputs) {
            self.status = message.to_string();
            return;
        }
        match self
            .trigger_workflow_native(&workflow_id, Some(inputs))
            .await
        {
            Ok(_) => {
                self.workflow_inputs.clear();
                self.workflow_edit_id = None;
                self.focus = Focus::Workflows;
                self.refresh_selected_workflow_runs().await;
                self.status = format!("Triggered workflow {}", short_id(&workflow_id));
            }
            Err(error) => self.status = format!("workflow trigger: {error}"),
        }
    }

    pub fn focus_create_workflow(&mut self) {
        let Some(channel) = self.selected_channel_for_workflows() else {
            return;
        };
        self.workflow_channel_id = channel.id;
        self.workflow_edit_existing = false;
        self.workflow_edit_id = None;
        self.workflow_yaml = BASIC_WORKFLOW_TEMPLATE.to_string();
        self.workflow_yaml_cursor = self.workflow_yaml.len();
        self.focus = Focus::WorkflowEdit;
        self.status = "Creating workflow YAML. F2 scheduled digest, F3 webhook digest".to_string();
    }

    pub fn focus_edit_workflow(&mut self) {
        let Some(workflow) = self.workflows.get(self.selected_workflow).cloned() else {
            self.status = "No workflow selected".to_string();
            return;
        };
        self.workflow_edit_existing = true;
        self.workflow_edit_id = Some(workflow.workflow_id.clone());
        self.workflow_yaml = workflow.content;
        self.workflow_yaml_cursor = self.workflow_yaml.len();
        self.focus = Focus::WorkflowEdit;
        self.status = format!("Editing workflow {}", short_id(&workflow.workflow_id));
    }

    pub async fn save_workflow(&mut self) {
        let yaml = self.workflow_yaml.trim().to_string();
        if yaml.is_empty() {
            self.status = "Workflow YAML is empty".to_string();
            return;
        }
        if self.workflow_channel_id.is_empty() {
            self.status = "No workflow channel loaded".to_string();
            return;
        }

        let result = if self.workflow_edit_existing {
            let Some(workflow_id) = self.workflow_edit_id.clone() else {
                self.status = "No workflow selected for update".to_string();
                return;
            };
            self.update_workflow_native(&self.workflow_channel_id, &workflow_id, &yaml)
                .await
                .map(|value| (value, Some(workflow_id)))
        } else {
            self.create_workflow_native(&self.workflow_channel_id, &yaml)
                .await
                .map(|value| {
                    let workflow_id = value
                        .get("workflow_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    (value, workflow_id)
                })
        };

        match result {
            Ok((_, saved_id)) => {
                self.clear_workflow_editor();
                self.focus_workflows().await;
                if let Some(saved_id) = saved_id {
                    if let Some(index) = self
                        .workflows
                        .iter()
                        .position(|workflow| workflow.workflow_id == saved_id)
                    {
                        self.selected_workflow = index;
                    }
                }
                self.status = "Saved workflow".to_string();
            }
            Err(error) => self.status = format!("workflow save: {error}"),
        }
    }

    pub async fn delete_selected_workflow(&mut self) {
        let Some(workflow) = self.workflows.get(self.selected_workflow).cloned() else {
            self.status = "No workflow selected".to_string();
            return;
        };
        match self.delete_workflow_native(&workflow.workflow_id).await {
            Ok(_) => {
                self.workflows.remove(self.selected_workflow);
                self.workflow_runs.clear();
                self.selected_workflow_detail = None;
                clamp_index(&mut self.selected_workflow, self.workflows.len());
                self.refresh_selected_workflow_runs().await;
                self.refresh_selected_workflow_detail().await;
                self.status = format!("Deleted workflow {}", short_id(&workflow.workflow_id));
            }
            Err(error) => self.status = format!("workflow delete: {error}"),
        }
    }

    pub fn focus_workflow_approval(&mut self, approved: bool) {
        self.clear_workflow_approval();
        self.workflow_approval_approved = approved;
        self.focus = Focus::WorkflowApproval;
        self.status = if approved {
            "Approve workflow step token".to_string()
        } else {
            "Deny workflow step token".to_string()
        };
    }

    pub async fn submit_workflow_approval(&mut self) {
        let token = self.workflow_approval_token.trim().to_string();
        if token.is_empty() {
            self.status = "Approval token is empty".to_string();
            self.workflow_approval_field = WorkflowApprovalField::Token;
            return;
        }
        let note = self.workflow_approval_note.trim().to_string();
        match self
            .approve_workflow_step_native(&token, self.workflow_approval_approved, &note)
            .await
        {
            Ok(_) => {
                let approved = self.workflow_approval_approved;
                self.clear_workflow_approval();
                self.focus = Focus::Workflows;
                self.refresh_selected_workflow_runs().await;
                self.status = format!(
                    "{} workflow approval {}",
                    if approved { "Granted" } else { "Denied" },
                    short_id(&token)
                );
            }
            Err(error) => self.status = format!("workflow approval: {error}"),
        }
    }

    pub fn workflow_yaml_push(&mut self, ch: char) {
        if ch != '\r' {
            let cursor = normalize_text_cursor(&self.workflow_yaml, self.workflow_yaml_cursor);
            self.workflow_yaml.insert(cursor, ch);
            self.workflow_yaml_cursor = cursor + ch.len_utf8();
        }
    }

    pub fn workflow_yaml_newline(&mut self) {
        let cursor = normalize_text_cursor(&self.workflow_yaml, self.workflow_yaml_cursor);
        self.workflow_yaml.insert(cursor, '\n');
        self.workflow_yaml_cursor = cursor + 1;
    }

    pub fn workflow_yaml_pop(&mut self) {
        let cursor = normalize_text_cursor(&self.workflow_yaml, self.workflow_yaml_cursor);
        if cursor == 0 {
            return;
        }
        let previous = previous_text_boundary(&self.workflow_yaml, cursor);
        self.workflow_yaml.replace_range(previous..cursor, "");
        self.workflow_yaml_cursor = previous;
    }

    pub fn workflow_yaml_delete(&mut self) {
        let cursor = normalize_text_cursor(&self.workflow_yaml, self.workflow_yaml_cursor);
        if cursor >= self.workflow_yaml.len() {
            return;
        }
        let next = next_text_boundary(&self.workflow_yaml, cursor);
        self.workflow_yaml.replace_range(cursor..next, "");
        self.workflow_yaml_cursor = cursor;
    }

    pub fn workflow_yaml_left(&mut self) {
        self.workflow_yaml_cursor = previous_text_boundary(
            &self.workflow_yaml,
            normalize_text_cursor(&self.workflow_yaml, self.workflow_yaml_cursor),
        );
    }

    pub fn workflow_yaml_right(&mut self) {
        self.workflow_yaml_cursor = next_text_boundary(
            &self.workflow_yaml,
            normalize_text_cursor(&self.workflow_yaml, self.workflow_yaml_cursor),
        );
    }

    pub fn workflow_yaml_up(&mut self) {
        self.workflow_yaml_cursor =
            move_text_cursor_vertical(&self.workflow_yaml, self.workflow_yaml_cursor, -1);
    }

    pub fn workflow_yaml_down(&mut self) {
        self.workflow_yaml_cursor =
            move_text_cursor_vertical(&self.workflow_yaml, self.workflow_yaml_cursor, 1);
    }

    pub fn workflow_yaml_home(&mut self) {
        let cursor = normalize_text_cursor(&self.workflow_yaml, self.workflow_yaml_cursor);
        self.workflow_yaml_cursor = self.workflow_yaml[..cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
    }

    pub fn workflow_yaml_end(&mut self) {
        let cursor = normalize_text_cursor(&self.workflow_yaml, self.workflow_yaml_cursor);
        self.workflow_yaml_cursor = self.workflow_yaml[cursor..]
            .find('\n')
            .map(|offset| cursor + offset)
            .unwrap_or(self.workflow_yaml.len());
    }

    pub fn use_basic_workflow_template(&mut self) {
        if self.focus != Focus::WorkflowEdit {
            return;
        }
        self.workflow_yaml = BASIC_WORKFLOW_TEMPLATE.to_string();
        self.workflow_yaml_cursor = self.workflow_yaml.len();
        self.status = "Loaded basic message workflow template".to_string();
    }

    pub fn use_scheduled_digest_workflow_template(&mut self) {
        if self.focus != Focus::WorkflowEdit {
            return;
        }
        self.workflow_yaml = SCHEDULED_DIGEST_WORKFLOW_TEMPLATE.to_string();
        self.workflow_yaml_cursor = self.workflow_yaml.len();
        self.status =
            "Loaded scheduled digest template. Replace the webhook URL before saving".to_string();
    }

    pub fn use_webhook_digest_workflow_template(&mut self) {
        if self.focus != Focus::WorkflowEdit {
            return;
        }
        self.workflow_yaml = WEBHOOK_DIGEST_WORKFLOW_TEMPLATE.to_string();
        self.workflow_yaml_cursor = self.workflow_yaml.len();
        self.status = "Loaded webhook digest template".to_string();
    }

    pub fn workflow_inputs_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.workflow_inputs.push(ch);
        }
    }

    pub fn workflow_inputs_pop(&mut self) {
        self.workflow_inputs.pop();
    }

    pub fn workflow_approval_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.selected_workflow_approval_input_mut().push(ch);
        }
    }

    pub fn workflow_approval_pop(&mut self) {
        self.selected_workflow_approval_input_mut().pop();
    }

    pub fn next_workflow_approval_field(&mut self) {
        self.workflow_approval_field = match self.workflow_approval_field {
            WorkflowApprovalField::Token => WorkflowApprovalField::Note,
            WorkflowApprovalField::Note => WorkflowApprovalField::Token,
        };
    }

    pub fn previous_workflow_approval_field(&mut self) {
        self.next_workflow_approval_field();
    }

    pub async fn refresh_selected_workflow_runs(&mut self) {
        let Some(workflow) = self.workflows.get(self.selected_workflow).cloned() else {
            self.workflow_runs.clear();
            return;
        };
        match self.get_workflow_runs_native(&workflow.workflow_id).await {
            Ok(runs) => self.workflow_runs = runs,
            Err(error) => {
                self.workflow_runs.clear();
                self.status = format!("workflow runs: {error}");
            }
        }
    }

    fn selected_workflow_approval_input_mut(&mut self) -> &mut String {
        match self.workflow_approval_field {
            WorkflowApprovalField::Token => &mut self.workflow_approval_token,
            WorkflowApprovalField::Note => &mut self.workflow_approval_note,
        }
    }

    pub(super) fn clear_workflow_editor(&mut self) {
        self.workflow_edit_existing = false;
        self.workflow_edit_id = None;
        self.workflow_yaml.clear();
        self.workflow_yaml_cursor = 0;
    }

    pub(super) fn clear_workflow_approval(&mut self) {
        self.workflow_approval_approved = true;
        self.workflow_approval_field = WorkflowApprovalField::Token;
        self.workflow_approval_token.clear();
        self.workflow_approval_note.clear();
    }
}

fn normalize_text_cursor(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len());
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

fn previous_text_boundary(text: &str, cursor: usize) -> usize {
    let cursor = normalize_text_cursor(text, cursor);
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_text_boundary(text: &str, cursor: usize) -> usize {
    let cursor = normalize_text_cursor(text, cursor);
    if cursor >= text.len() {
        return text.len();
    }
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

fn move_text_cursor_vertical(text: &str, cursor: usize, delta: isize) -> usize {
    let cursor = normalize_text_cursor(text, cursor);
    let (line_start, column) = line_start_and_column(text, cursor);
    if delta < 0 {
        if line_start == 0 {
            return cursor;
        }
        let previous_end = line_start.saturating_sub(1);
        let previous_start = text[..previous_end]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        return cursor_at_column(text, previous_start, previous_end, column);
    }

    let current_end = text[cursor..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(text.len());
    if current_end >= text.len() {
        return cursor;
    }
    let next_start = current_end + 1;
    let next_end = text[next_start..]
        .find('\n')
        .map(|offset| next_start + offset)
        .unwrap_or(text.len());
    cursor_at_column(text, next_start, next_end, column)
}

fn line_start_and_column(text: &str, cursor: usize) -> (usize, usize) {
    let cursor = normalize_text_cursor(text, cursor);
    let line_start = text[..cursor]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let column = text[line_start..cursor].chars().count();
    (line_start, column)
}

fn cursor_at_column(text: &str, line_start: usize, line_end: usize, column: usize) -> usize {
    text[line_start..line_end]
        .char_indices()
        .nth(column)
        .map(|(offset, _)| line_start + offset)
        .unwrap_or(line_end)
}
