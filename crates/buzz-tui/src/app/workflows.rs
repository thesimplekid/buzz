use super::{clamp_index, parse_workflow_inputs, short_id, App, Focus, WorkflowApprovalField};

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
        self.workflow_yaml.clear();
        self.focus = Focus::WorkflowEdit;
        self.status = "Creating workflow YAML".to_string();
    }

    pub fn focus_edit_workflow(&mut self) {
        let Some(workflow) = self.workflows.get(self.selected_workflow).cloned() else {
            self.status = "No workflow selected".to_string();
            return;
        };
        self.workflow_edit_existing = true;
        self.workflow_edit_id = Some(workflow.workflow_id.clone());
        self.workflow_yaml = workflow.content;
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
        if ch != '\n' && ch != '\r' {
            self.workflow_yaml.push(ch);
        }
    }

    pub fn workflow_yaml_pop(&mut self) {
        self.workflow_yaml.pop();
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
    }

    pub(super) fn clear_workflow_approval(&mut self) {
        self.workflow_approval_approved = true;
        self.workflow_approval_field = WorkflowApprovalField::Token;
        self.workflow_approval_token.clear();
        self.workflow_approval_note.clear();
    }
}
