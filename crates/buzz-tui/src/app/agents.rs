use super::{
    clamp_index, nostr_pubkey_uri, parse_agent_allowlist, short_id, AgentCreateField, App, Focus,
    TimelineMode,
};
use crate::acp::{AgentRuntime, AgentStatus};
use crate::client::{CreateManagedAgentOptions, ManagedAgentInfo};

impl App {
    pub async fn toggle_selected_agent_autostart(&mut self) {
        let Some((id, managed, enabled)) = self.acp.agent_at(self.selected_agent).map(|agent| {
            (
                agent.runtime.id.clone(),
                agent.runtime.managed,
                agent.runtime.start_on_launch,
            )
        }) else {
            return;
        };
        if !managed {
            self.status = "Only managed agents can start on launch".to_string();
            return;
        }

        match self.managed_agent_store.set_start_on_launch(
            &self.managed_agent_store_path,
            &id,
            !enabled,
        ) {
            Ok(Some(agent)) => {
                self.sync_managed_agent(&agent);
                self.status = format!(
                    "{} start-on-launch {}",
                    agent.name,
                    if agent.start_on_launch {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
                self.refresh_selected_agent_log().await;
            }
            Ok(None) => self.status = "agent: managed agent not found".to_string(),
            Err(error) => self.status = format!("agent: {error}"),
        }
    }

    pub async fn delete_selected_managed_agent(&mut self) {
        let Some((id, label, managed, status)) =
            self.acp.agent_at(self.selected_agent).map(|agent| {
                (
                    agent.runtime.id.clone(),
                    agent.runtime.label.clone(),
                    agent.runtime.managed,
                    agent.status,
                )
            })
        else {
            return;
        };
        if !managed {
            self.status = "Only managed agents can be deleted".to_string();
            return;
        }

        if status == AgentStatus::Running {
            self.acp.stop(&id);
        }

        match self
            .managed_agent_store
            .remove(&self.managed_agent_store_path, &id)
        {
            Ok(true) => {
                self.acp.remove_runtime(&id);
                let agent_count = self.agent_count();
                clamp_index(&mut self.selected_agent, agent_count);
                self.refresh_selected_agent_log().await;
                self.status = format!("Deleted managed agent {label}");
            }
            Ok(false) => self.status = format!("agent delete: {label} was not deleted"),
            Err(error) => self.status = format!("agent delete: {error}"),
        }
    }

    pub async fn add_selected_agent_to_channel(&mut self) {
        let Some((pubkey, label, managed)) = self.acp.agent_at(self.selected_agent).map(|agent| {
            (
                agent.runtime.id.clone(),
                agent.runtime.label.clone(),
                agent.runtime.managed,
            )
        }) else {
            return;
        };
        if !managed {
            self.status = "Only managed agents can be added to channels".to_string();
            return;
        }

        let Some(channel) = self.selected_sidebar_channel_for_management() else {
            return;
        };

        match self
            .add_channel_member_native(&channel.id, &pubkey, Some("bot"))
            .await
        {
            Ok(_) => {
                self.refresh_selected_channel_details().await;
                self.status = format!("Added {label} to #{} as bot", channel.name);
            }
            Err(error) => self.status = format!("add agent: {error}"),
        }
    }

    pub(super) async fn create_managed_agent(&mut self) {
        let name = self.new_agent_name.trim().to_string();
        let Some(runtime_id) = self.new_agent_runtime_id.clone() else {
            self.status = "No runtime selected".to_string();
            self.focus = Focus::Agents;
            return;
        };
        if name.is_empty() {
            self.status = "Agent name is empty".to_string();
            return;
        }

        let options = CreateManagedAgentOptions {
            name,
            runtime: runtime_id,
            model: self.new_agent_model.trim().to_string(),
            system_prompt: self.new_agent_system_prompt.trim().to_string(),
            respond_to: self.new_agent_respond_to.clone(),
            respond_to_allowlist: parse_agent_allowlist(&self.new_agent_allowlist),
            reply_placement: self.new_agent_reply_placement.clone(),
            start_on_launch: self.new_agent_start_on_launch,
        };

        let runtimes = self.available_agent_runtimes();
        let relay_url = self.cli.relay_url().to_string();
        let acp_binary = self.acp_binary.clone();
        match self.managed_agent_store.create_agent(
            &self.managed_agent_store_path,
            &options,
            &runtimes,
            &relay_url,
            &acp_binary,
            None,
        ) {
            Ok(agent) => {
                let pubkey = agent.pubkey.clone();
                let label = agent.name.clone();
                self.sync_managed_agent(&agent);
                if let Some(index) = self.acp.position_of(&pubkey) {
                    self.selected_agent = index;
                }
                self.clear_new_agent_inputs();
                self.focus = Focus::Agents;
                self.refresh_selected_agent_log().await;
                self.status = format!("Created managed agent {label}");
            }
            Err(error) => self.status = format!("agent create: {error}"),
        }
    }

    pub async fn focus_agents(&mut self) {
        self.focus = Focus::Agents;
        self.refresh_agent_statuses().await;
        self.refresh_selected_agent_log().await;
    }

    pub fn focus_create_agent(&mut self) {
        let Some(agent) = self.acp.agent_at(self.selected_agent) else {
            self.status = "No runtime selected".to_string();
            return;
        };
        if agent.runtime.managed {
            self.status = "Select a runtime template to create a managed agent".to_string();
            return;
        }

        self.new_agent_runtime_id = Some(agent.runtime.id.clone());
        self.new_agent_name = agent.runtime.label.clone();
        self.new_agent_model.clear();
        self.new_agent_system_prompt.clear();
        self.new_agent_respond_to = "owner-only".to_string();
        self.new_agent_allowlist.clear();
        self.new_agent_reply_placement = "thread-direct-mentions".to_string();
        self.new_agent_start_on_launch = false;
        self.new_agent_field = AgentCreateField::Name;
        self.focus = Focus::CreateAgent;
        self.status = format!("Creating managed {} agent", agent.runtime.label);
    }

    pub fn insert_selected_agent_mention(&mut self) {
        let Some((pubkey, label)) = self.selected_managed_agent_identity() else {
            self.status = "Select a managed agent to mention".to_string();
            return;
        };
        if self.timeline_mode == TimelineMode::Pulse {
            self.status = "Agent mentions are for channel messages".to_string();
            return;
        }
        if self.active_channel().is_none() {
            self.status = "No channel selected".to_string();
            return;
        }
        let Some(uri) = nostr_pubkey_uri(&pubkey) else {
            self.status = format!("agent {} has an invalid pubkey", short_id(&pubkey));
            return;
        };
        if !self.composer.is_empty() && !self.composer.ends_with(char::is_whitespace) {
            self.composer.push(' ');
        }
        self.composer.push_str(&format!("@{} {uri} ", label.trim()));
        self.composer_cursor = self.composer.len();
        self.save_active_channel_draft();
        self.focus = Focus::Composer;
        self.status = format!("Mentioned agent {label}");
    }

    pub fn new_agent_name_push(&mut self, ch: char) {
        if self.new_agent_field == AgentCreateField::RespondTo {
            return;
        }
        if ch != '\n' && ch != '\r' {
            self.selected_new_agent_input_mut().push(ch);
        }
    }

    pub fn new_agent_name_pop(&mut self) {
        if self.new_agent_field == AgentCreateField::RespondTo {
            return;
        }
        self.selected_new_agent_input_mut().pop();
    }

    pub fn next_agent_create_field(&mut self) {
        self.new_agent_field = match self.new_agent_field {
            AgentCreateField::Name => AgentCreateField::Model,
            AgentCreateField::Model => AgentCreateField::SystemPrompt,
            AgentCreateField::SystemPrompt => AgentCreateField::RespondTo,
            AgentCreateField::RespondTo => AgentCreateField::Allowlist,
            AgentCreateField::Allowlist => AgentCreateField::Name,
        };
    }

    pub fn previous_agent_create_field(&mut self) {
        self.new_agent_field = match self.new_agent_field {
            AgentCreateField::Name => AgentCreateField::Allowlist,
            AgentCreateField::Model => AgentCreateField::Name,
            AgentCreateField::SystemPrompt => AgentCreateField::Model,
            AgentCreateField::RespondTo => AgentCreateField::SystemPrompt,
            AgentCreateField::Allowlist => AgentCreateField::RespondTo,
        };
    }

    pub fn toggle_new_agent_start_on_launch(&mut self) {
        self.new_agent_start_on_launch = !self.new_agent_start_on_launch;
        self.status = format!(
            "New agent start-on-launch {}",
            if self.new_agent_start_on_launch {
                "enabled"
            } else {
                "disabled"
            }
        );
    }

    pub fn cycle_new_agent_respond_to(&mut self) {
        self.new_agent_respond_to = match self.new_agent_respond_to.as_str() {
            "owner-only" => "allowlist",
            "allowlist" => "anyone",
            _ => "owner-only",
        }
        .to_string();
        self.status = format!("New agent responds to {}", self.new_agent_respond_to);
    }

    pub fn toggle_new_agent_reply_placement(&mut self) {
        self.new_agent_reply_placement = if self.new_agent_reply_placement == "top-level" {
            "thread-direct-mentions".to_string()
        } else {
            "top-level".to_string()
        };
        self.status = format!(
            "New agent reply placement {}",
            self.new_agent_reply_placement
        );
    }

    pub fn reap_agents(&mut self) {
        self.acp.reap();
    }

    pub async fn shutdown_all_agents(&mut self) {
        self.acp.shutdown_all().await;
    }

    pub(super) async fn toggle_selected_agent(&mut self) {
        let Some((id, label, managed, status)) =
            self.acp.agent_at(self.selected_agent).map(|agent| {
                (
                    agent.runtime.id.clone(),
                    agent.runtime.label.clone(),
                    agent.runtime.managed,
                    agent.status,
                )
            })
        else {
            return;
        };

        if managed {
            if status == AgentStatus::Running {
                self.acp.stop(&id);
                self.status = format!("{label} stopped");
            } else {
                match self.acp.start(&id) {
                    Ok(()) => self.status = format!("{label} running"),
                    Err(error) => self.status = format!("agent: {error}"),
                }
            }
            self.refresh_selected_agent_log().await;
            return;
        }

        match self.acp.toggle(&id) {
            Ok(()) => self.status = format!("Toggled {label} ACP harness"),
            Err(error) => self.status = format!("agent: {error}"),
        }
    }

    pub(super) fn agent_count(&self) -> usize {
        self.acp.agents().count()
    }

    pub(super) fn selected_managed_agent_identity(&self) -> Option<(String, String)> {
        self.acp.agent_at(self.selected_agent).and_then(|agent| {
            agent
                .runtime
                .managed
                .then(|| (agent.runtime.id.clone(), agent.runtime.label.clone()))
        })
    }

    pub async fn refresh_selected_agent_log(&mut self) {
        let Some((id, label, managed, install_hint, command, args, status, last_exit)) =
            self.acp.agent_at(self.selected_agent).map(|agent| {
                (
                    agent.runtime.id.clone(),
                    agent.runtime.label.clone(),
                    agent.runtime.managed,
                    agent.runtime.install_hint.clone(),
                    agent.runtime.command.clone(),
                    agent.runtime.args.clone(),
                    agent.status,
                    agent.last_exit.clone(),
                )
            })
        else {
            self.agent_log.clear();
            self.agent_log_path.clear();
            return;
        };

        if !managed {
            self.agent_log_path.clear();
            let mut text = format!(
                "Runtime: {label}\nCommand: {} {}\nStatus: {:?}",
                command,
                args.join(" "),
                status
            );
            if let Some(last_exit) = last_exit {
                if !last_exit.trim().is_empty() {
                    text.push_str(&format!("\nLast exit: {last_exit}"));
                }
            }
            if !install_hint.trim().is_empty() {
                text.push_str(&format!("\n{install_hint}"));
            }
            self.agent_log = text;
            return;
        }

        match self.managed_agent_store.log(&id, 120) {
            Some(log) => {
                self.agent_log_path = log.log_path;
                self.agent_log = if log.content.trim().is_empty() {
                    "No log output yet.".to_string()
                } else {
                    log.content
                };
            }
            None => {
                self.agent_log_path.clear();
                self.agent_log = "No stored log for this managed agent.".to_string();
            }
        }
    }

    pub(super) async fn refresh_agent_statuses(&mut self) {
        for agent in self.managed_agent_store.infos() {
            self.sync_managed_agent(&agent);
        }
    }

    fn sync_managed_agent(&mut self, agent: &ManagedAgentInfo) {
        self.acp.upsert_runtime(
            managed_agent_runtime(agent),
            agent.private_key_nsec.clone(),
            agent.auth_tag.clone(),
        );
    }

    fn available_agent_runtimes(&self) -> Vec<AgentRuntime> {
        self.acp
            .agents()
            .filter(|agent| !agent.runtime.managed)
            .map(|agent| agent.runtime.clone())
            .collect()
    }

    fn selected_new_agent_input_mut(&mut self) -> &mut String {
        match self.new_agent_field {
            AgentCreateField::Name => &mut self.new_agent_name,
            AgentCreateField::Model => &mut self.new_agent_model,
            AgentCreateField::SystemPrompt => &mut self.new_agent_system_prompt,
            AgentCreateField::RespondTo => &mut self.new_agent_respond_to,
            AgentCreateField::Allowlist => &mut self.new_agent_allowlist,
        }
    }

    pub(super) fn clear_new_agent_inputs(&mut self) {
        self.new_agent_name.clear();
        self.new_agent_model.clear();
        self.new_agent_system_prompt.clear();
        self.new_agent_respond_to = "owner-only".to_string();
        self.new_agent_allowlist.clear();
        self.new_agent_reply_placement = "thread-direct-mentions".to_string();
        self.new_agent_start_on_launch = false;
        self.new_agent_field = AgentCreateField::Name;
        self.new_agent_runtime_id = None;
    }
}

fn agent_status(status: &str) -> AgentStatus {
    match status {
        "running" => AgentStatus::Running,
        "exited" => AgentStatus::Exited,
        _ => AgentStatus::Stopped,
    }
}

fn managed_agent_runtime(agent: &ManagedAgentInfo) -> AgentRuntime {
    AgentRuntime {
        id: agent.pubkey.clone(),
        label: agent.name.clone(),
        relay_url: Some(crate::client::relay_http_to_ws_url(&agent.relay_url)),
        acp_command: Some(agent.acp_command.clone()),
        command: agent.agent_command.clone(),
        args: agent.agent_args.clone(),
        model: agent.model.clone().filter(|model| !model.trim().is_empty()),
        mcp_command: (!agent.mcp_command.trim().is_empty()).then_some(agent.mcp_command.clone()),
        turn_timeout_seconds: agent.turn_timeout_seconds,
        system_prompt: agent.system_prompt.clone(),
        respond_to: agent.respond_to.clone(),
        respond_to_allowlist: agent.respond_to_allowlist.clone(),
        reply_placement: agent.reply_placement.clone(),
        managed: true,
        start_on_launch: agent.start_on_launch,
        initial_status: agent_status(&agent.status),
        available: true,
        install_hint: "Managed by buzz-tui".to_string(),
        last_error: agent.last_error.clone(),
        log_path: agent.log_path.clone(),
    }
}
