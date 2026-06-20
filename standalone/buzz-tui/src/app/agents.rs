use super::{
    clamp_index, move_index, nostr_pubkey_uri, parse_agent_allowlist, short_id, AgentCreateField,
    AgentDisplayEntry, AgentListEntry, App, Focus, TimelineMode,
};
use crate::acp::{AgentRuntime, AgentStatus};
use crate::client::{CreateManagedAgentOptions, ManagedAgentInfo, RelayAgentInfo};
use crate::refresh::AgentRefreshResult;
use std::collections::BTreeSet;

pub(super) fn relay_agent_is_available_to_user(
    agent: &RelayAgentInfo,
    own_pubkey: Option<&str>,
    joined_channel_ids: &BTreeSet<String>,
) -> bool {
    let normalize = |pubkey: &str| pubkey.trim().to_ascii_lowercase();
    let own_pubkey = own_pubkey.map(normalize);

    if own_pubkey.as_deref().is_some_and(|own| {
        agent
            .owner_pubkey
            .as_deref()
            .is_some_and(|owner| normalize(owner) == own)
    }) {
        return true;
    }

    if agent.respond_to.as_deref() == Some("allowlist") {
        return own_pubkey.as_deref().is_some_and(|own| {
            agent
                .respond_to_allowlist
                .iter()
                .any(|pubkey| normalize(pubkey) == own)
        });
    }

    agent.respond_to.as_deref() == Some("anyone")
        && agent
            .channel_ids
            .iter()
            .any(|channel_id| joined_channel_ids.contains(channel_id))
}

impl App {
    /// The label of an agent authored by `pubkey` that the current user owns:
    /// either a locally managed agent, or a relay directory agent whose
    /// verified NIP-OA owner is the current user.
    pub(super) fn owned_agent_label(&self, pubkey: &str) -> Option<String> {
        if let Some(agent) = self
            .acp
            .agents()
            .find(|agent| agent.runtime.managed && agent.runtime.id.eq_ignore_ascii_case(pubkey))
        {
            return Some(agent.runtime.label.clone());
        }
        let own_pubkey = self
            .native_relay_client()
            .ok()
            .map(|client| client.public_key_hex())?;
        self.relay_agents
            .iter()
            .find(|agent| {
                agent.pubkey.eq_ignore_ascii_case(pubkey)
                    && agent.owner_pubkey.as_deref() == Some(own_pubkey.as_str())
            })
            .map(|agent| agent.name.clone())
    }

    pub async fn toggle_selected_agent_autostart(&mut self) {
        let Some(AgentListEntry::Local(agent)) = self.selected_agent_entry() else {
            self.status = "Start-on-launch is only available for local managed agents".to_string();
            return;
        };
        let id = agent.runtime.id.clone();
        let managed = agent.runtime.managed;
        let enabled = agent.runtime.start_on_launch;
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
                self.refresh_selected_agent_log();
            }
            Ok(None) => self.status = "agent: managed agent not found".to_string(),
            Err(error) => self.status = format!("agent: {error}"),
        }
    }

    pub async fn delete_selected_managed_agent(&mut self) {
        let Some(AgentListEntry::Local(agent)) = self.selected_agent_entry() else {
            self.status = "Only local managed agents can be deleted".to_string();
            return;
        };
        let id = agent.runtime.id.clone();
        let label = agent.runtime.label.clone();
        let managed = agent.runtime.managed;
        let status = agent.status;
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
                self.refresh_selected_agent_log();
                self.status = format!("Deleted managed agent {label}");
            }
            Ok(false) => self.status = format!("agent delete: {label} was not deleted"),
            Err(error) => self.status = format!("agent delete: {error}"),
        }
    }

    pub async fn add_selected_agent_to_channel(&mut self) {
        let Some((pubkey, label)) = self.selected_agent_entry().and_then(|entry| match entry {
            AgentListEntry::Local(agent) if agent.runtime.managed => {
                Some((agent.runtime.id.clone(), agent.runtime.label.clone()))
            }
            AgentListEntry::Local(_) => None,
            AgentListEntry::Relay(agent) => Some((agent.pubkey.clone(), agent.name.clone())),
        }) else {
            self.status = "Select a managed or relay agent to add to a channel".to_string();
            return;
        };
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
            thinking_effort: (!self.new_agent_thinking_effort.trim().is_empty())
                .then(|| self.new_agent_thinking_effort.trim().to_string()),
            max_output_tokens: self.new_agent_max_tokens.trim().parse::<u64>().ok(),
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
                self.refresh_selected_agent_log();
                self.status = format!("Created managed agent {label}");
            }
            Err(error) => self.status = format!("agent create: {error}"),
        }
    }

    pub fn focus_agents(&mut self) {
        self.focus = Focus::Agents;
        self.relay_agents_refresh_requested = true;
    }

    /// Consume the one-shot relay-directory refresh request.
    pub fn take_relay_agents_refresh_request(&mut self) -> bool {
        let requested = std::mem::take(&mut self.relay_agents_refresh_requested);
        if requested {
            for agent in self.managed_agent_store.infos() {
                self.sync_managed_agent(&agent);
            }
        }
        requested
    }

    pub fn apply_agent_refresh_result(&mut self, result: AgentRefreshResult) {
        match result.agents {
            Ok(mut agents) => {
                agents.sort_by(|left, right| left.name.cmp(&right.name));
                self.relay_agents = agents;
                let agent_count = self.agent_count();
                clamp_index(&mut self.selected_agent, agent_count);
            }
            Err(error) => {
                self.status = format!("relay agents: {error}");
            }
        }
        if let Ok(metrics) = result.metrics {
            self.agent_turn_metrics = metrics;
        }
        if self.focus == Focus::Agents {
            self.refresh_selected_agent_log();
        }
    }

    pub fn focus_create_agent(&mut self) {
        let Some((runtime_id, runtime_label, managed)) =
            self.selected_agent_entry().and_then(|entry| match entry {
                AgentListEntry::Local(agent) => Some((
                    agent.runtime.id.clone(),
                    agent.runtime.label.clone(),
                    agent.runtime.managed,
                )),
                AgentListEntry::Relay(_) => None,
            })
        else {
            self.status = "No runtime selected".to_string();
            return;
        };
        if managed {
            self.status = "Select a runtime template to create a managed agent".to_string();
            return;
        }

        self.new_agent_runtime_id = Some(runtime_id);
        self.new_agent_name = runtime_label.clone();
        self.new_agent_model.clear();
        self.new_agent_system_prompt.clear();
        self.new_agent_respond_to = "owner-only".to_string();
        self.new_agent_allowlist.clear();
        self.new_agent_reply_placement = "thread-direct-mentions".to_string();
        self.new_agent_start_on_launch = false;
        self.new_agent_field = AgentCreateField::Name;
        self.focus = Focus::CreateAgent;
        self.status = format!("Creating managed {runtime_label} agent");
    }

    pub fn insert_selected_agent_mention(&mut self) {
        if self.selected_agent_entry().is_some_and(|entry| {
            matches!(
                entry,
                AgentListEntry::Relay(agent) if !self.relay_agent_is_available(agent)
            )
        }) {
            self.status = "This relay agent is not available to the current identity".to_string();
            return;
        }
        let Some((pubkey, label)) = self.selected_agent_mention_identity() else {
            self.status = "Select an agent to mention".to_string();
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
        if nostr_pubkey_uri(&pubkey).is_none() {
            self.status = format!("agent {} has an invalid pubkey", short_id(&pubkey));
            return;
        }
        self.append_composer_mention(&label, &pubkey);
        self.focus = Focus::Composer;
        self.status = format!("Mentioned agent {label}");
    }

    pub fn new_agent_name_push(&mut self, ch: char) {
        if self.new_agent_field == AgentCreateField::RespondTo {
            return;
        }
        if self.new_agent_field == AgentCreateField::MaxTokens && !ch.is_ascii_digit() {
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
            AgentCreateField::SystemPrompt => AgentCreateField::MaxTokens,
            AgentCreateField::MaxTokens => AgentCreateField::RespondTo,
            AgentCreateField::RespondTo => AgentCreateField::Allowlist,
            AgentCreateField::Allowlist => AgentCreateField::Name,
        };
    }

    pub fn previous_agent_create_field(&mut self) {
        self.new_agent_field = match self.new_agent_field {
            AgentCreateField::Name => AgentCreateField::Allowlist,
            AgentCreateField::Model => AgentCreateField::Name,
            AgentCreateField::SystemPrompt => AgentCreateField::Model,
            AgentCreateField::MaxTokens => AgentCreateField::SystemPrompt,
            AgentCreateField::RespondTo => AgentCreateField::MaxTokens,
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

    /// Cycle the new agent's thinking effort through the normalized ladder;
    /// empty means the provider default.
    pub fn cycle_new_agent_thinking_effort(&mut self) {
        const EFFORTS: [&str; 8] = [
            "", "none", "minimal", "low", "medium", "high", "xhigh", "max",
        ];
        let current = EFFORTS
            .iter()
            .position(|effort| *effort == self.new_agent_thinking_effort)
            .unwrap_or(0);
        self.new_agent_thinking_effort = EFFORTS[(current + 1) % EFFORTS.len()].to_string();
        self.status = format!(
            "New agent thinking effort {}",
            if self.new_agent_thinking_effort.is_empty() {
                "provider default"
            } else {
                self.new_agent_thinking_effort.as_str()
            }
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
            self.selected_agent_entry().map(|entry| match entry {
                AgentListEntry::Local(agent) => (
                    agent.runtime.id.clone(),
                    agent.runtime.label.clone(),
                    agent.runtime.managed,
                    agent.status,
                ),
                AgentListEntry::Relay(agent) => (
                    agent.pubkey.clone(),
                    agent.name.clone(),
                    false,
                    AgentStatus::Stopped,
                ),
            })
        else {
            return;
        };

        if self.acp.position_of(&id).is_none() {
            self.status = format!("{label} is a remote agent; local controls are disabled");
            return;
        }

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
            self.refresh_selected_agent_log();
            return;
        }

        match self.acp.toggle(&id) {
            Ok(()) => self.status = format!("Toggled {label} ACP harness"),
            Err(error) => self.status = format!("agent: {error}"),
        }
    }

    pub fn agent_count(&self) -> usize {
        self.agent_entries().len()
    }

    pub fn move_agent_selection(&mut self, delta: isize) {
        let agent_count = self.agent_count();
        move_index(&mut self.selected_agent, agent_count, delta);
        self.refresh_selected_agent_log();
    }

    pub fn agent_entries(&self) -> Vec<AgentListEntry<'_>> {
        let mut entries = self
            .acp
            .agents()
            .map(AgentListEntry::Local)
            .collect::<Vec<_>>();
        let (available, unavailable) = self.partitioned_relay_agents();
        entries.extend(available.into_iter().map(AgentListEntry::Relay));
        entries.extend(unavailable.into_iter().map(AgentListEntry::Relay));
        entries
    }

    pub fn agent_display_entries(&self) -> Vec<AgentDisplayEntry<'_>> {
        let local_agents = self.acp.agents().collect::<Vec<_>>();
        let (available, unavailable) = self.partitioned_relay_agents();
        let mut entries =
            Vec::with_capacity(local_agents.len() + available.len() + unavailable.len() + 6);

        entries.push(AgentDisplayEntry::Header("Local ACP agents"));
        if local_agents.is_empty() {
            entries.push(AgentDisplayEntry::Empty("No local runtimes configured"));
        } else {
            entries.extend(
                local_agents
                    .into_iter()
                    .map(|agent| AgentDisplayEntry::Entry(AgentListEntry::Local(agent))),
            );
        }

        entries.push(AgentDisplayEntry::Header("Available relay agents"));
        if available.is_empty() {
            entries.push(AgentDisplayEntry::Empty(
                "No relay agents available to this identity",
            ));
        } else {
            entries.extend(
                available
                    .into_iter()
                    .map(|agent| AgentDisplayEntry::Entry(AgentListEntry::Relay(agent))),
            );
        }

        entries.push(AgentDisplayEntry::Header("Other announced agents"));
        if unavailable.is_empty() {
            entries.push(AgentDisplayEntry::Empty("No unavailable relay agents"));
        } else {
            entries.extend(
                unavailable
                    .into_iter()
                    .map(|agent| AgentDisplayEntry::Entry(AgentListEntry::Relay(agent))),
            );
        }

        entries
    }

    pub fn selected_agent_display_index(&self) -> Option<usize> {
        let mut selectable_index = 0;
        for (display_index, entry) in self.agent_display_entries().into_iter().enumerate() {
            if matches!(entry, AgentDisplayEntry::Entry(_)) {
                if selectable_index == self.selected_agent {
                    return Some(display_index);
                }
                selectable_index += 1;
            }
        }
        None
    }

    pub fn selected_agent_entry(&self) -> Option<AgentListEntry<'_>> {
        self.agent_entries().into_iter().nth(self.selected_agent)
    }

    pub(crate) fn selected_managed_agent_identity(&self) -> Option<(String, String)> {
        match self.selected_agent_entry()? {
            AgentListEntry::Local(agent) if agent.runtime.managed => {
                Some((agent.runtime.id.clone(), agent.runtime.label.clone()))
            }
            _ => None,
        }
    }

    pub(crate) fn selected_agent_mention_identity(&self) -> Option<(String, String)> {
        match self.selected_agent_entry()? {
            AgentListEntry::Local(agent) if agent.runtime.managed => {
                Some((agent.runtime.id.clone(), agent.runtime.label.clone()))
            }
            AgentListEntry::Relay(agent) if self.relay_agent_is_available(agent) => {
                Some((agent.pubkey.clone(), agent.name.clone()))
            }
            _ => None,
        }
    }

    pub fn refresh_selected_agent_log(&mut self) {
        let Some((id, label, managed, install_hint, command, args, status, last_exit)) =
            self.selected_agent_entry().map(|entry| match entry {
                AgentListEntry::Local(agent) => (
                    agent.runtime.id.clone(),
                    agent.runtime.label.clone(),
                    agent.runtime.managed,
                    agent.runtime.install_hint.clone(),
                    agent.runtime.command.clone(),
                    agent.runtime.args.clone(),
                    agent.status,
                    agent.last_exit.clone(),
                ),
                AgentListEntry::Relay(agent) => (
                    agent.pubkey.clone(),
                    agent.name.clone(),
                    false,
                    String::new(),
                    "relay directory".to_string(),
                    Vec::new(),
                    AgentStatus::Stopped,
                    None,
                ),
            })
        else {
            self.agent_log.clear();
            self.agent_log_path.clear();
            return;
        };

        if !managed {
            self.agent_log_path.clear();
            let mut text = if let Some(relay_agent) = self
                .relay_agents
                .iter()
                .find(|agent| agent.pubkey == id)
                .cloned()
            {
                relay_agent_log_text(&relay_agent, self.relay_agent_is_available(&relay_agent))
            } else {
                format!(
                    "Runtime: {label}\nCommand: {} {}\nStatus: {:?}",
                    command,
                    args.join(" "),
                    status
                )
            };
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

    pub(super) fn available_relay_agents(&self) -> impl Iterator<Item = &RelayAgentInfo> {
        self.listed_relay_agents()
            .filter(|agent| self.relay_agent_is_available(agent))
    }

    fn partitioned_relay_agents(&self) -> (Vec<&RelayAgentInfo>, Vec<&RelayAgentInfo>) {
        let own_pubkey = self.own_pubkey_hex();
        self.listed_relay_agents().partition(|agent| {
            relay_agent_is_available_to_user(agent, own_pubkey.as_deref(), &self.joined_channel_ids)
        })
    }

    fn listed_relay_agents(&self) -> impl Iterator<Item = &RelayAgentInfo> {
        let local_pubkeys = self
            .acp
            .agents()
            .map(|agent| agent.runtime.id.trim().to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        self.relay_agents
            .iter()
            .filter(move |agent| !local_pubkeys.contains(&agent.pubkey.trim().to_ascii_lowercase()))
    }

    pub(crate) fn relay_agent_is_available(&self, agent: &RelayAgentInfo) -> bool {
        let own_pubkey = self.own_pubkey_hex();
        relay_agent_is_available_to_user(agent, own_pubkey.as_deref(), &self.joined_channel_ids)
    }

    fn sync_managed_agent(&mut self, agent: &ManagedAgentInfo) {
        self.acp.upsert_runtime(
            managed_agent_runtime(agent, self.active_workspace_repos_dir()),
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
            AgentCreateField::MaxTokens => &mut self.new_agent_max_tokens,
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
        self.new_agent_thinking_effort.clear();
        self.new_agent_max_tokens.clear();
        self.new_agent_field = AgentCreateField::Name;
        self.new_agent_runtime_id = None;
    }
}

fn relay_agent_log_text(agent: &RelayAgentInfo, available: bool) -> String {
    let mut text = format!(
        "Relay agent: {}\nkey: {}\nstatus: {}\ntype: {}",
        agent.name,
        short_id(&agent.pubkey),
        agent.status,
        agent.agent_type
    );
    if let Some(respond_to) = agent.respond_to.as_deref() {
        text.push_str(&format!("\nresponds: {respond_to}"));
    }
    if !agent.channels.is_empty() {
        text.push_str(&format!("\nchannels: {}", agent.channels.join(", ")));
    }
    if !agent.capabilities.is_empty() {
        text.push_str(&format!(
            "\ncapabilities: {}",
            agent.capabilities.join(", ")
        ));
    }
    if available {
        text.push_str(
            "\n\nThis relay agent is available to this identity and can be mentioned here.",
        );
    } else {
        text.push_str("\n\nThis relay agent was announced, but is not available to this identity.");
    }
    text.push_str(" Local runtime, log, autostart, delete, and memory actions are disabled.");
    text
}

fn agent_status(status: &str) -> AgentStatus {
    match status {
        "running" => AgentStatus::Running,
        "exited" => AgentStatus::Exited,
        _ => AgentStatus::Stopped,
    }
}

fn managed_agent_runtime(agent: &ManagedAgentInfo, repos_dir: Option<String>) -> AgentRuntime {
    AgentRuntime {
        id: agent.pubkey.clone(),
        label: agent.name.clone(),
        relay_url: Some(crate::client::relay_http_to_ws_url(&agent.relay_url)),
        acp_command: Some(agent.acp_command.clone()),
        command: agent.agent_command.clone(),
        args: agent.agent_args.clone(),
        model: agent.model.clone().filter(|model| !model.trim().is_empty()),
        thinking_effort: agent
            .thinking_effort
            .clone()
            .filter(|value| !value.trim().is_empty()),
        max_output_tokens: agent.max_output_tokens,
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
        repos_dir,
    }
}
