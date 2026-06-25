use std::collections::BTreeMap;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::process::Stdio;
use std::time::Duration;

use thiserror::Error;
use tokio::process::{Child, Command};

const AGENT_STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRuntime {
    pub id: String,
    pub label: String,
    pub relay_url: Option<String>,
    pub acp_command: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub model: Option<String>,
    pub mcp_command: Option<String>,
    pub turn_timeout_seconds: Option<u64>,
    pub system_prompt: Option<String>,
    pub respond_to: String,
    pub respond_to_allowlist: Vec<String>,
    pub reply_placement: String,
    pub managed: bool,
    pub start_on_launch: bool,
    pub initial_status: AgentStatus,
    pub available: bool,
    pub install_hint: String,
    pub last_error: Option<String>,
    pub log_path: Option<String>,
}

#[derive(Debug, Error)]
pub enum AcpError {
    #[error("BUZZ_PRIVATE_KEY is required to start an ACP harness")]
    MissingPrivateKey,
    #[error("{runtime} is not available: {hint}")]
    RuntimeUnavailable { runtime: String, hint: String },
    #[error("failed to prepare agent log: {0}")]
    Log(std::io::Error),
    #[error("failed to spawn buzz-acp: {0}")]
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub struct AgentProcess {
    pub runtime: AgentRuntime,
    pub status: AgentStatus,
    pub last_exit: Option<String>,
    child: Option<Child>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentStatus {
    Stopped,
    Running,
    Exited,
}

#[derive(Debug)]
pub struct AcpSupervisor {
    acp_binary: String,
    relay_url: String,
    default_private_key: Option<String>,
    default_auth_tag: Option<String>,
    default_agent_owner: Option<String>,
    runtime_private_keys: BTreeMap<String, String>,
    runtime_auth_tags: BTreeMap<String, String>,
    mcp_command: String,
    agents: Vec<AgentProcess>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AgentLaunch {
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct AcpSupervisorConfig {
    pub acp_binary: String,
    pub relay_url: String,
    pub runtimes: Vec<AgentRuntime>,
    pub default_private_key: Option<String>,
    pub default_auth_tag: Option<String>,
    pub default_agent_owner: Option<String>,
    pub runtime_private_keys: BTreeMap<String, String>,
    pub runtime_auth_tags: BTreeMap<String, String>,
    pub mcp_command: String,
}

impl AcpSupervisor {
    pub fn new(config: AcpSupervisorConfig) -> Self {
        let agents = config
            .runtimes
            .into_iter()
            .map(|runtime| AgentProcess {
                status: runtime.initial_status,
                runtime,
                last_exit: None,
                child: None,
            })
            .collect();
        Self {
            acp_binary: config.acp_binary,
            relay_url: config.relay_url,
            default_private_key: config.default_private_key,
            default_auth_tag: config.default_auth_tag,
            default_agent_owner: config.default_agent_owner,
            runtime_private_keys: config.runtime_private_keys,
            runtime_auth_tags: config.runtime_auth_tags,
            mcp_command: config.mcp_command,
            agents,
        }
    }

    pub fn agents(&self) -> impl Iterator<Item = &AgentProcess> {
        self.agents.iter()
    }

    pub fn agent_at(&self, index: usize) -> Option<&AgentProcess> {
        self.agents.get(index)
    }

    pub fn credentials_for(&self, id: &str) -> (Option<String>, Option<String>) {
        (self.private_key_for(id), self.auth_tag_for(id))
    }

    pub fn position_of(&self, id: &str) -> Option<usize> {
        self.agent_index(id)
    }

    pub fn upsert_runtime(
        &mut self,
        runtime: AgentRuntime,
        private_key: Option<String>,
        auth_tag: Option<String>,
    ) {
        if let Some(private_key) = private_key {
            self.runtime_private_keys
                .insert(runtime.id.clone(), private_key);
        }
        if let Some(auth_tag) = auth_tag {
            self.runtime_auth_tags.insert(runtime.id.clone(), auth_tag);
        }

        if let Some(index) = self.agent_index(&runtime.id) {
            let agent = &mut self.agents[index];
            agent.status = runtime.initial_status;
            agent.runtime = runtime;
            agent.last_exit = None;
            return;
        }

        self.agents.push(AgentProcess {
            status: runtime.initial_status,
            runtime,
            last_exit: None,
            child: None,
        });
    }

    pub fn remove_runtime(&mut self, id: &str) -> bool {
        let Some(index) = self.agent_index(id) else {
            return false;
        };
        let mut agent = self.agents.remove(index);
        stop_agent(&mut agent);
        self.runtime_private_keys.remove(id);
        self.runtime_auth_tags.remove(id);
        true
    }

    pub fn toggle(&mut self, id: &str) -> Result<(), AcpError> {
        let is_running = self
            .agents
            .iter()
            .find(|agent| agent.runtime.id == id)
            .map(|agent| agent.status == AgentStatus::Running)
            .unwrap_or(false);
        if is_running {
            self.stop(id);
            return Ok(());
        }
        self.start(id)
    }

    pub fn start(&mut self, id: &str) -> Result<(), AcpError> {
        let private_key = self
            .private_key_for(id)
            .ok_or(AcpError::MissingPrivateKey)?;
        let auth_tag = self.auth_tag_for(id);
        let agent_index = match self.agent_index(id) {
            Some(index) => index,
            None => return Ok(()),
        };
        let agent = &self.agents[agent_index];
        if agent.status == AgentStatus::Running {
            return Ok(());
        }
        if !agent.runtime.available {
            return Err(AcpError::RuntimeUnavailable {
                runtime: agent.runtime.label.clone(),
                hint: agent.runtime.install_hint.clone(),
            });
        }

        let runtime = agent.runtime.clone();
        let acp_binary = runtime
            .acp_command
            .clone()
            .unwrap_or_else(|| self.acp_binary.clone());
        let relay_url = runtime
            .relay_url
            .clone()
            .unwrap_or_else(|| self.relay_url.clone());
        let mcp_command = effective_mcp_command(&self.mcp_command, runtime.mcp_command.as_deref());
        let respond_to = if runtime.respond_to.trim().is_empty() {
            "owner-only".to_string()
        } else {
            runtime.respond_to.clone()
        };
        let agent_owner = self.agent_owner_for(auth_tag.as_deref(), &respond_to);
        let log_path = runtime.log_path.clone();

        let launch = build_agent_launch(
            acp_binary,
            relay_url,
            private_key,
            auth_tag,
            agent_owner,
            runtime,
            mcp_command,
            respond_to,
        );

        let mut command = Command::new(&launch.program);
        sanitize_agent_environment(&mut command);
        command.args(&launch.args);
        for (key, value) in &launch.env {
            command.env(key, value);
        }
        command.kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);

        command.stdin(Stdio::null());
        if let Some(log_path) = log_path.as_deref() {
            let log = open_agent_log(log_path).map_err(AcpError::Log)?;
            let stderr = log.try_clone().map_err(AcpError::Log)?;
            command.stdout(Stdio::from(log));
            command.stderr(Stdio::from(stderr));
        } else {
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }

        let child = command.spawn().map_err(AcpError::Spawn)?;
        let agent = &mut self.agents[agent_index];
        agent.child = Some(child);
        agent.status = AgentStatus::Running;
        agent.last_exit = None;
        Ok(())
    }

    pub fn start_on_launch_agents(&mut self) -> (Vec<String>, Vec<(String, String)>) {
        let ids = self
            .agents
            .iter()
            .filter(|agent| agent.runtime.managed && agent.runtime.start_on_launch)
            .map(|agent| (agent.runtime.id.clone(), agent.runtime.label.clone()))
            .collect::<Vec<_>>();
        let mut started = Vec::new();
        let mut failed = Vec::new();
        for (id, label) in ids {
            match self.start(&id) {
                Ok(()) => started.push(label),
                Err(error) => failed.push((label, error.to_string())),
            }
        }
        (started, failed)
    }

    pub fn stop(&mut self, id: &str) {
        if let Some(agent) = self.agent_mut(id) {
            stop_agent(agent);
        }
    }

    pub fn stop_all(&mut self) {
        for agent in &mut self.agents {
            stop_agent(agent);
        }
    }

    pub async fn shutdown_all(&mut self) {
        for agent in &mut self.agents {
            shutdown_agent(agent).await;
        }
    }

    pub fn reap(&mut self) {
        for agent in &mut self.agents {
            let Some(child) = agent.child.as_mut() else {
                continue;
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    agent.child = None;
                    agent.status = AgentStatus::Exited;
                    agent.last_exit = Some(describe_exit_status(status));
                }
                Ok(None) => {}
                Err(error) => {
                    agent.child = None;
                    agent.status = AgentStatus::Exited;
                    agent.last_exit = Some(format!("wait failed: {error}"));
                }
            }
        }
    }

    fn private_key_for(&self, id: &str) -> Option<String> {
        self.runtime_private_keys
            .get(id)
            .cloned()
            .or_else(|| self.default_private_key.clone())
    }

    fn auth_tag_for(&self, id: &str) -> Option<String> {
        self.runtime_auth_tags
            .get(id)
            .cloned()
            .or_else(|| self.default_auth_tag.clone())
    }

    fn agent_owner_for(&self, auth_tag: Option<&str>, respond_to: &str) -> Option<String> {
        if auth_tag.is_some() || !respond_to_needs_owner(respond_to) {
            return None;
        }
        self.default_agent_owner.clone()
    }

    fn agent_index(&self, id: &str) -> Option<usize> {
        self.agents.iter().position(|agent| agent.runtime.id == id)
    }

    fn agent_mut(&mut self, id: &str) -> Option<&mut AgentProcess> {
        self.agents.iter_mut().find(|agent| agent.runtime.id == id)
    }
}

impl Drop for AcpSupervisor {
    fn drop(&mut self) {
        self.stop_all();
    }
}

pub fn fallback_runtimes() -> Vec<AgentRuntime> {
    vec![
        AgentRuntime {
            id: "goose".to_string(),
            label: "Goose".to_string(),
            relay_url: None,
            acp_command: None,
            command: "goose".to_string(),
            args: vec!["acp".to_string()],
            model: None,
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "owner-only".to_string(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: false,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: "Install Goose from https://block.github.io/goose/".to_string(),
            last_error: None,
            log_path: None,
        },
        AgentRuntime {
            id: "codex".to_string(),
            label: "Codex".to_string(),
            relay_url: None,
            acp_command: None,
            command: "codex-acp".to_string(),
            args: Vec::new(),
            model: None,
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "owner-only".to_string(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: false,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: "Install Codex and the codex-acp adapter.".to_string(),
            last_error: None,
            log_path: None,
        },
        AgentRuntime {
            id: "claude".to_string(),
            label: "Claude Code".to_string(),
            relay_url: None,
            acp_command: None,
            command: "claude-agent-acp".to_string(),
            args: Vec::new(),
            model: None,
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "owner-only".to_string(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: false,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: "Install Claude Code and the claude-agent-acp adapter.".to_string(),
            last_error: None,
            log_path: None,
        },
    ]
}

fn effective_mcp_command(global: &str, runtime: Option<&str>) -> String {
    if !global.trim().is_empty() {
        global.to_string()
    } else {
        runtime.unwrap_or_default().to_string()
    }
}

fn build_agent_launch(
    acp_binary: String,
    relay_url: String,
    private_key: String,
    auth_tag: Option<String>,
    agent_owner: Option<String>,
    runtime: AgentRuntime,
    mcp_command: String,
    respond_to: String,
) -> AgentLaunch {
    let subscribe_mode = subscribe_mode_for_runtime(&runtime, &respond_to);
    let needs_owner = respond_to_needs_owner(&respond_to);
    let mut args = vec![
        "--relay-url".to_string(),
        relay_url,
        "--agent-command".to_string(),
        runtime.command,
        "--subscribe".to_string(),
        subscribe_mode.to_string(),
        "--respond-to".to_string(),
        respond_to,
    ];

    if !runtime.args.is_empty() {
        args.push("--agent-args".to_string());
        args.push(runtime.args.join(","));
    }
    if let Some(model) = runtime.model {
        if !model.trim().is_empty() {
            args.push("--model".to_string());
            args.push(model);
        }
    }
    if !mcp_command.trim().is_empty() {
        args.push("--mcp-command".to_string());
        args.push(mcp_command);
    }
    if let Some(timeout) = runtime.turn_timeout_seconds {
        args.push("--idle-timeout".to_string());
        args.push(timeout.to_string());
    }
    if let Some(system_prompt) = runtime.system_prompt {
        if !system_prompt.trim().is_empty() {
            args.push("--system-prompt".to_string());
            args.push(system_prompt);
        }
    }
    for pubkey in runtime.respond_to_allowlist {
        args.push("--respond-to-allowlist".to_string());
        args.push(pubkey);
    }
    if let Some(agent_owner) = agent_owner.filter(|_| needs_owner) {
        args.push("--agent-owner".to_string());
        args.push(agent_owner);
    }

    let mut env = BTreeMap::new();
    env.insert("BUZZ_PRIVATE_KEY".to_string(), private_key);
    env.insert(
        "BUZZ_ACP_REPLY_PLACEMENT".to_string(),
        if runtime.reply_placement.trim().is_empty() {
            default_reply_placement()
        } else {
            runtime.reply_placement
        },
    );
    if let Some(auth_tag) = auth_tag {
        env.insert("BUZZ_AUTH_TAG".to_string(), auth_tag);
    }

    AgentLaunch {
        program: acp_binary,
        args,
        env,
    }
}

fn sanitize_agent_environment(command: &mut Command) {
    for key in [
        "CODEX_CI",
        "CODEX_SANDBOX_NETWORK_DISABLED",
        "CODEX_THREAD_ID",
    ] {
        command.env_remove(key);
    }

    if let Some(path) = std::env::var_os("BUZZ_ORIGINAL_PATH") {
        command.env("PATH", path);
    }
}

fn subscribe_mode_for_respond_to(respond_to: &str) -> &'static str {
    if respond_to == "anyone" {
        "all"
    } else {
        "mentions"
    }
}

fn subscribe_mode_for_runtime(runtime: &AgentRuntime, respond_to: &str) -> &'static str {
    if runtime.managed {
        "mentions"
    } else {
        subscribe_mode_for_respond_to(respond_to)
    }
}

fn respond_to_needs_owner(respond_to: &str) -> bool {
    matches!(respond_to, "owner-only" | "allowlist")
}

fn default_reply_placement() -> String {
    "thread-direct-mentions".to_string()
}

fn stop_agent(agent: &mut AgentProcess) {
    if let Some(mut child) = agent.child.take() {
        kill_child_tree(&mut child);
        let _ = child.try_wait();
        agent.last_exit = Some("stopped by user".to_string());
    }
    agent.status = AgentStatus::Stopped;
}

async fn shutdown_agent(agent: &mut AgentProcess) {
    if let Some(mut child) = agent.child.take() {
        terminate_child_tree(&mut child);
        let stopped = match tokio::time::timeout(AGENT_STOP_TIMEOUT, child.wait()).await {
            Ok(Ok(_)) => true,
            Ok(Err(error)) => {
                agent.last_exit = Some(format!("stop wait failed: {error}"));
                true
            }
            Err(_) => false,
        };
        if stopped {
            agent
                .last_exit
                .get_or_insert_with(|| "stopped by user".to_string());
        } else {
            kill_child_tree(&mut child);
            match tokio::time::timeout(AGENT_STOP_TIMEOUT, child.wait()).await {
                Ok(Ok(_)) => {
                    agent.last_exit = Some("stopped by user".to_string());
                }
                Ok(Err(error)) => {
                    agent.last_exit = Some(format!("stop wait failed after kill: {error}"));
                }
                Err(_) => {
                    agent.last_exit = Some("stop wait timed out".to_string());
                }
            }
        }
    }
    agent.status = AgentStatus::Stopped;
}

fn terminate_child_tree(child: &mut Child) {
    match child.id() {
        Some(pid) if terminate_process_group(pid) => {}
        _ => {
            let _ = child.start_kill();
        }
    }
}

fn kill_child_tree(child: &mut Child) {
    match child.id() {
        Some(pid) if kill_process_group(pid) => {}
        _ => {
            let _ = child.start_kill();
        }
    }
}

#[cfg(unix)]
fn terminate_process_group(pid: u32) -> bool {
    signal_process_group(pid, nix::sys::signal::Signal::SIGTERM)
}

#[cfg(not(unix))]
fn terminate_process_group(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn kill_process_group(pid: u32) -> bool {
    signal_process_group(pid, nix::sys::signal::Signal::SIGKILL)
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: nix::sys::signal::Signal) -> bool {
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;

    killpg(Pid::from_raw(pid as i32), signal).is_ok()
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) -> bool {
    false
}

fn open_agent_log(path: &str) -> std::io::Result<std::fs::File> {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn describe_exit_status(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exited with code {code}");
    }

    #[cfg(unix)]
    if let Some(signal) = status.signal() {
        return format!("terminated by signal {signal}");
    }

    status.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_default_acp_runtimes() {
        let ids: Vec<_> = fallback_runtimes()
            .into_iter()
            .map(|runtime| runtime.id)
            .collect();
        assert_eq!(ids, ["goose", "codex", "claude"]);
    }

    #[test]
    fn refuses_to_start_without_private_key() {
        let mut supervisor = AcpSupervisor::new(AcpSupervisorConfig {
            acp_binary: "buzz-acp".into(),
            relay_url: "ws://localhost:3000".into(),
            runtimes: fallback_runtimes(),
            default_private_key: None,
            default_auth_tag: None,
            default_agent_owner: None,
            runtime_private_keys: BTreeMap::new(),
            runtime_auth_tags: BTreeMap::new(),
            mcp_command: String::new(),
        });
        assert!(matches!(
            supervisor.start("goose"),
            Err(AcpError::MissingPrivateKey)
        ));
    }

    #[test]
    fn runtime_private_key_overrides_default_key() {
        let mut keys = BTreeMap::new();
        keys.insert("goose".to_string(), "agent-key".to_string());
        let supervisor = AcpSupervisor::new(AcpSupervisorConfig {
            acp_binary: "buzz-acp".into(),
            relay_url: "ws://localhost:3000".into(),
            runtimes: fallback_runtimes(),
            default_private_key: Some("operator-key".into()),
            default_auth_tag: None,
            default_agent_owner: None,
            runtime_private_keys: keys,
            runtime_auth_tags: BTreeMap::new(),
            mcp_command: String::new(),
        });
        assert_eq!(
            supervisor.private_key_for("goose").as_deref(),
            Some("agent-key")
        );
        assert_eq!(
            supervisor.private_key_for("codex").as_deref(),
            Some("operator-key")
        );
    }

    #[test]
    fn runtime_mcp_command_is_used_when_global_is_empty() {
        assert_eq!(
            effective_mcp_command("", Some("buzz-dev-mcp")),
            "buzz-dev-mcp"
        );
    }

    #[test]
    fn global_mcp_command_overrides_runtime_default() {
        assert_eq!(
            effective_mcp_command("custom-mcp", Some("buzz-dev-mcp")),
            "custom-mcp"
        );
    }

    #[test]
    fn upsert_runtime_adds_private_keyed_agent() {
        let mut supervisor = AcpSupervisor::new(AcpSupervisorConfig {
            acp_binary: "buzz-acp".into(),
            relay_url: "ws://localhost:3000".into(),
            runtimes: fallback_runtimes(),
            default_private_key: None,
            default_auth_tag: None,
            default_agent_owner: None,
            runtime_private_keys: BTreeMap::new(),
            runtime_auth_tags: BTreeMap::new(),
            mcp_command: String::new(),
        });

        supervisor.upsert_runtime(
            AgentRuntime {
                id: "pubkey".into(),
                label: "Review Bot".into(),
                relay_url: Some("ws://localhost:3000".into()),
                acp_command: Some("buzz-acp".into()),
                command: "codex-acp".into(),
                args: Vec::new(),
                model: None,
                mcp_command: None,
                turn_timeout_seconds: Some(600),
                system_prompt: None,
                respond_to: "owner-only".into(),
                respond_to_allowlist: Vec::new(),
                reply_placement: default_reply_placement(),
                managed: true,
                start_on_launch: false,
                initial_status: AgentStatus::Stopped,
                available: true,
                install_hint: String::new(),
                last_error: None,
                log_path: None,
            },
            Some("nsec1agent".into()),
            None,
        );

        assert_eq!(
            supervisor.private_key_for("pubkey").as_deref(),
            Some("nsec1agent")
        );
        assert_eq!(
            supervisor.agent_at(3).map(|agent| agent.runtime.managed),
            Some(true)
        );
    }

    #[test]
    fn remove_runtime_removes_agent_and_key_overrides() {
        let mut keys = BTreeMap::new();
        keys.insert("goose".to_string(), "agent-key".to_string());
        let mut auth_tags = BTreeMap::new();
        auth_tags.insert("goose".to_string(), "[\"auth\"]".to_string());
        let mut supervisor = AcpSupervisor::new(AcpSupervisorConfig {
            acp_binary: "buzz-acp".into(),
            relay_url: "ws://localhost:3000".into(),
            runtimes: fallback_runtimes(),
            default_private_key: None,
            default_auth_tag: None,
            default_agent_owner: None,
            runtime_private_keys: keys,
            runtime_auth_tags: auth_tags,
            mcp_command: String::new(),
        });

        assert!(supervisor.remove_runtime("goose"));
        assert!(supervisor.position_of("goose").is_none());
        assert!(supervisor.private_key_for("goose").is_none());
        assert!(supervisor.auth_tag_for("goose").is_none());
    }

    #[test]
    fn agent_launch_passes_credentials_through_env_only() {
        let runtime = AgentRuntime {
            id: "goose".into(),
            label: "Goose".into(),
            relay_url: None,
            acp_command: None,
            command: "goose".into(),
            args: vec!["acp".into()],
            model: Some("gpt-5.5-codex".into()),
            mcp_command: None,
            turn_timeout_seconds: Some(60),
            system_prompt: Some("Be concise".into()),
            respond_to: "owner-only".into(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: false,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: String::new(),
            last_error: None,
            log_path: None,
        };

        let launch = build_agent_launch(
            "buzz-acp".into(),
            "ws://localhost:3000".into(),
            "nsec1secret".into(),
            Some("[\"auth\"]".into()),
            None,
            runtime,
            "buzz-dev-mcp".into(),
            "owner-only".into(),
        );

        assert_eq!(
            launch.env.get("BUZZ_PRIVATE_KEY").map(String::as_str),
            Some("nsec1secret")
        );
        assert_eq!(
            launch.env.get("BUZZ_AUTH_TAG").map(String::as_str),
            Some("[\"auth\"]")
        );
        assert_eq!(
            launch
                .env
                .get("BUZZ_ACP_REPLY_PLACEMENT")
                .map(String::as_str),
            Some("thread-direct-mentions")
        );
        assert!(!launch.args.iter().any(|arg| arg == "--private-key"));
        assert!(!launch.args.iter().any(|arg| arg == "nsec1secret"));
    }

    #[test]
    fn agent_launch_passes_owner_for_owner_scoped_modes() {
        let runtime = AgentRuntime {
            id: "codex".into(),
            label: "Codex".into(),
            relay_url: None,
            acp_command: None,
            command: "codex-acp".into(),
            args: Vec::new(),
            model: None,
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "owner-only".into(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: true,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: String::new(),
            last_error: None,
            log_path: None,
        };

        let launch = build_agent_launch(
            "buzz-acp".into(),
            "ws://localhost:3000".into(),
            "nsec1secret".into(),
            None,
            Some("owner-pubkey".into()),
            runtime,
            String::new(),
            "owner-only".into(),
        );

        let owner_flag = launch
            .args
            .windows(2)
            .any(|window| window == ["--agent-owner", "owner-pubkey"]);
        assert!(owner_flag);
    }

    #[test]
    fn agent_launch_does_not_pass_owner_when_responding_to_anyone() {
        let runtime = AgentRuntime {
            id: "codex".into(),
            label: "Codex".into(),
            relay_url: None,
            acp_command: None,
            command: "codex-acp".into(),
            args: Vec::new(),
            model: None,
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "anyone".into(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: true,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: String::new(),
            last_error: None,
            log_path: None,
        };

        let launch = build_agent_launch(
            "buzz-acp".into(),
            "ws://localhost:3000".into(),
            "nsec1secret".into(),
            None,
            Some("owner-pubkey".into()),
            runtime,
            String::new(),
            "anyone".into(),
        );

        assert!(!launch.args.iter().any(|arg| arg == "--agent-owner"));
    }

    #[test]
    fn agent_launch_passes_top_level_reply_placement() {
        let runtime = AgentRuntime {
            id: "codex".into(),
            label: "Codex".into(),
            relay_url: None,
            acp_command: None,
            command: "codex-acp".into(),
            args: Vec::new(),
            model: None,
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "owner-only".into(),
            respond_to_allowlist: Vec::new(),
            reply_placement: "top-level".into(),
            managed: true,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: String::new(),
            last_error: None,
            log_path: None,
        };

        let launch = build_agent_launch(
            "buzz-acp".into(),
            "ws://localhost:3000".into(),
            "nsec1secret".into(),
            None,
            None,
            runtime,
            String::new(),
            "owner-only".into(),
        );

        assert_eq!(
            launch
                .env
                .get("BUZZ_ACP_REPLY_PLACEMENT")
                .map(String::as_str),
            Some("top-level")
        );
    }

    #[test]
    fn agent_launch_passes_model_to_buzz_acp() {
        let runtime = AgentRuntime {
            id: "codex".into(),
            label: "Codex".into(),
            relay_url: None,
            acp_command: None,
            command: "codex-acp".into(),
            args: Vec::new(),
            model: Some("gpt-5.5-codex".into()),
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "owner-only".into(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: false,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: String::new(),
            last_error: None,
            log_path: None,
        };

        let launch = build_agent_launch(
            "buzz-acp".into(),
            "ws://localhost:3000".into(),
            "nsec1secret".into(),
            None,
            None,
            runtime,
            String::new(),
            "owner-only".into(),
        );

        let model_flag = launch
            .args
            .windows(2)
            .any(|window| window == ["--model", "gpt-5.5-codex"]);
        assert!(model_flag);
    }

    #[test]
    fn unmanaged_agent_launch_subscribes_to_all_when_responding_to_anyone() {
        let runtime = AgentRuntime {
            id: "codex".into(),
            label: "Codex".into(),
            relay_url: None,
            acp_command: None,
            command: "codex-acp".into(),
            args: Vec::new(),
            model: None,
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "anyone".into(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: false,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: String::new(),
            last_error: None,
            log_path: None,
        };

        let launch = build_agent_launch(
            "buzz-acp".into(),
            "ws://localhost:3000".into(),
            "nsec1secret".into(),
            None,
            None,
            runtime,
            String::new(),
            "anyone".into(),
        );

        let subscribe_all = launch
            .args
            .windows(2)
            .any(|window| window == ["--subscribe", "all"]);
        assert!(subscribe_all);
    }

    #[test]
    fn managed_agent_launch_subscribes_to_mentions() {
        let runtime = AgentRuntime {
            id: "agent-pubkey".into(),
            label: "Review Bot".into(),
            relay_url: None,
            acp_command: None,
            command: "codex-acp".into(),
            args: Vec::new(),
            model: None,
            mcp_command: None,
            turn_timeout_seconds: None,
            system_prompt: None,
            respond_to: "anyone".into(),
            respond_to_allowlist: Vec::new(),
            reply_placement: default_reply_placement(),
            managed: true,
            start_on_launch: false,
            initial_status: AgentStatus::Stopped,
            available: true,
            install_hint: String::new(),
            last_error: None,
            log_path: None,
        };

        let launch = build_agent_launch(
            "buzz-acp".into(),
            "ws://localhost:3000".into(),
            "nsec1secret".into(),
            None,
            None,
            runtime,
            String::new(),
            "anyone".into(),
        );

        let subscribe_mentions = launch
            .args
            .windows(2)
            .any(|window| window == ["--subscribe", "mentions"]);
        let respond_to_anyone = launch
            .args
            .windows(2)
            .any(|window| window == ["--respond-to", "anyone"]);
        assert!(subscribe_mentions);
        assert!(respond_to_anyone);
    }

    #[test]
    fn describe_exit_status_includes_exit_code() {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .status()
            .expect("run shell");

        assert_eq!(describe_exit_status(status), "exited with code 7");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kill_child_tree_kills_descendant_processes() {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 30 & echo $!; wait")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);

        let mut child = command.spawn().expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .await
            .expect("read descendant pid");
        let descendant_pid: i32 = line.trim().parse().expect("descendant pid");

        kill_child_tree(&mut child);
        tokio::time::timeout(AGENT_STOP_TIMEOUT, child.wait())
            .await
            .expect("shell exits after kill")
            .expect("wait shell");

        let deadline = tokio::time::Instant::now() + AGENT_STOP_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            if !process_is_running(descendant_pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        panic!("descendant process should not still be running");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_agent_allows_sigterm_cleanup_before_kill() {
        let pid_file = std::env::temp_dir().join(format!(
            "buzz-tui-shutdown-child-{}-{}.pid",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before unix epoch")
                .as_nanos()
        ));
        let script = r#"
            cleanup() {
                kill -KILL "-$child" 2>/dev/null || kill -KILL "$child" 2>/dev/null || true
                wait "$child" 2>/dev/null || true
                exit 0
            }
            trap cleanup TERM
            setsid sleep 30 &
            child=$!
            printf '%s\n' "$child" > "$PID_FILE"
            while :; do sleep 1; done
        "#;
        let mut command = Command::new("bash");
        command
            .arg("-c")
            .arg(script)
            .env("PID_FILE", &pid_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);

        let child = command.spawn().expect("spawn fake acp");
        let descendant_pid = read_pid_file(&pid_file).await;
        assert!(
            process_is_running(descendant_pid),
            "separate process-group child should be running before shutdown"
        );

        let mut agent = AgentProcess {
            runtime: fallback_runtimes()
                .into_iter()
                .next()
                .expect("fallback runtime"),
            status: AgentStatus::Running,
            last_exit: None,
            child: Some(child),
        };
        shutdown_agent(&mut agent).await;

        let deadline = tokio::time::Instant::now() + AGENT_STOP_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            if !process_is_running(descendant_pid) {
                let _ = std::fs::remove_file(&pid_file);
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        panic!("separate process-group child should not still be running");
    }

    #[cfg(unix)]
    async fn read_pid_file(path: &std::path::Path) -> i32 {
        let deadline = tokio::time::Instant::now() + AGENT_STOP_TIMEOUT;
        loop {
            if let Ok(contents) = std::fs::read_to_string(path) {
                if let Ok(pid) = contents.trim().parse() {
                    return pid;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for pid file {}", path.display());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[cfg(all(unix, target_os = "linux"))]
    fn process_is_running(pid: i32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        stat.rsplit_once(") ")
            .and_then(|(_, rest)| rest.chars().next())
            .is_some_and(|state| state != 'Z')
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn process_is_running(pid: i32) -> bool {
        use nix::sys::signal;
        use nix::unistd::Pid;

        signal::kill(Pid::from_raw(pid), None).is_ok()
    }
}
