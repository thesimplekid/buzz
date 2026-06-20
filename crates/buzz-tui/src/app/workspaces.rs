use crate::acp::AcpSupervisor;
use crate::app::{
    clamp_index, App, Focus, TimelineMode, DEFAULT_AGENT_PANEL_HEIGHT, DEFAULT_DETAIL_WIDTH,
    DEFAULT_SIDEBAR_WIDTH,
};
use crate::cli::BuzzCli;
use crate::workspace::{parse_workspace_input, TuiWorkspace};

impl App {
    pub fn focus_workspaces(&mut self) {
        self.selected_workspace = self.workspace_config.active_index();
        self.focus = Focus::Workspaces;
        self.status = "Workspace switcher loaded".to_string();
    }

    pub fn focus_add_workspace(&mut self) {
        self.workspace_input.clear();
        self.focus = Focus::WorkspaceAdd;
        self.status = "Type 'name http://relay' or just a relay URL".to_string();
    }

    pub fn workspace_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.workspace_input.push(ch);
        }
    }

    pub fn workspace_input_pop(&mut self) {
        self.workspace_input.pop();
    }

    pub fn selected_workspace(&self) -> Option<TuiWorkspace> {
        self.workspace_config
            .workspaces
            .get(self.selected_workspace)
            .cloned()
    }

    pub fn add_workspace_from_input(&mut self) {
        let input = self.workspace_input.trim().to_string();
        let (name, relay) = match parse_workspace_input(&input) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.status = format!("workspace add: {error}");
                return;
            }
        };
        match self.workspace_config.add_workspace(&name, &relay) {
            Ok(id) => {
                self.selected_workspace = self
                    .workspace_config
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.id == id)
                    .unwrap_or_default();
                if let Err(error) = self.workspace_config.save(&self.workspace_store_path) {
                    self.status = format!("workspace save: {error}");
                    return;
                }
                self.workspace_input.clear();
                self.focus = Focus::Workspaces;
                self.status = format!("Added workspace {name}; press Enter to switch");
            }
            Err(error) => self.status = format!("workspace add: {error}"),
        }
    }

    pub fn remove_selected_workspace(&mut self) {
        let Some(workspace) = self.selected_workspace() else {
            self.status = "No workspace selected".to_string();
            return;
        };
        if workspace.id == self.workspace_config.active_id {
            self.status = "Cannot remove the active workspace".to_string();
            return;
        }
        match self
            .workspace_config
            .remove_workspace(self.selected_workspace)
        {
            Some(removed) => {
                clamp_index(
                    &mut self.selected_workspace,
                    self.workspace_config.workspaces.len(),
                );
                if let Err(error) = self.workspace_config.save(&self.workspace_store_path) {
                    self.status = format!("workspace save: {error}");
                    return;
                }
                self.status = format!("Removed workspace {}", removed.name);
            }
            None => self.status = "Cannot remove the final workspace".to_string(),
        }
    }

    pub async fn apply_workspace_session(
        &mut self,
        workspace_id: String,
        cli: BuzzCli,
        acp: AcpSupervisor,
        startup_notice: Option<String>,
    ) {
        self.save_active_channel_draft();
        self.cli = cli;
        self.acp = acp;
        self.workspace_config.set_active(&workspace_id);
        self.selected_workspace = self.workspace_config.active_index();
        if let Err(error) = self.workspace_config.save(&self.workspace_store_path) {
            self.status = format!("workspace save: {error}");
            return;
        }
        self.reset_workspace_view_state();
        self.startup_notice = startup_notice;
        self.focus = Focus::Sidebar;
        self.refresh().await;
    }

    pub(super) fn reset_workspace_view_state(&mut self) {
        self.channels.clear();
        self.messages.clear();
        self.active_channel_id = None;
        self.feed.clear();
        self.pulse.clear();
        self.search_results.clear();
        self.channel_search_results.clear();
        self.selected_reactions.clear();
        self.selected_channel_detail = None;
        self.channel_members.clear();
        self.relay_members.clear();
        self.selected_relay_member = 0;
        self.relay_member_input.clear();
        self.canvas_channel_id.clear();
        self.canvas_content.clear();
        self.canvas_draft.clear();
        self.workflow_channel_id.clear();
        self.workflows.clear();
        self.workflow_runs.clear();
        self.selected_workflow_detail = None;
        self.clear_workflow_editor();
        self.workflow_inputs.clear();
        self.clear_workflow_approval();
        self.notes.clear();
        self.profile = None;
        self.presence = None;
        self.last_presence_status = None;
        self.contacts.clear();
        self.author_profiles.clear();
        self.viewed_profile = None;
        self.repos.clear();
        self.repo_issues.clear();
        self.repo_patches.clear();
        self.clear_repo_issue_inputs();
        self.clear_repo_patch_inputs();
        self.memories.clear();
        self.clear_memory_editor();
        self.clear_memory_patch_inputs();
        self.workspace_emoji.clear();
        self.own_emoji.clear();
        self.clear_emoji_inputs();
        self.clear_emoji_import();
        self.agent_log.clear();
        self.agent_log_path.clear();
        self.composer.clear();
        self.composer_cursor = 0;
        self.channel_latest_seen.clear();
        self.starred_channel_ids.clear();
        self.muted_channel_ids.clear();
        self.channel_sections.clear();
        self.channel_section_assignments.clear();
        self.attachment_input.clear();
        self.clear_diff_inputs();
        self.edit_target = None;
        self.pulse_reply_target = None;
        self.search_query.clear();
        self.channel_search_query.clear();
        self.channel_search_last_query.clear();
        self.clear_new_channel_inputs();
        self.dm_pubkey.clear();
        self.clear_new_agent_inputs();
        self.channel_action_input.clear();
        self.thread_root = None;
        self.thread_context = None;
        self.timeline_mode = TimelineMode::Channel;
        self.selected_channel = 0;
        self.selected_message = 0;
        self.selected_search_result = 0;
        self.selected_channel_search = 0;
        self.selected_feed = 0;
        self.selected_pulse = 0;
        self.message_detail_scroll = 0;
        self.sidebar_width = DEFAULT_SIDEBAR_WIDTH;
        self.detail_width = DEFAULT_DETAIL_WIDTH;
        self.agent_panel_height = DEFAULT_AGENT_PANEL_HEIGHT;
        self.selected_agent = 0;
    }
}
