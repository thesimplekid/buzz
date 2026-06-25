use thiserror::Error;

use crate::acp::AcpSupervisor;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum MemoryWriterError {
    #[error("Select a managed agent before editing memory")]
    NoManagedAgent,
    #[error("Selected agent has no stored private key")]
    MissingPrivateKey,
    #[error("Selected agent has no owner auth tag")]
    MissingAuthTag,
}

pub fn selected_agent_memory_identity(
    acp: &AcpSupervisor,
    selected_agent: usize,
) -> Result<(String, String), MemoryWriterError> {
    let agent = acp
        .agent_at(selected_agent)
        .filter(|agent| agent.runtime.managed)
        .ok_or(MemoryWriterError::NoManagedAgent)?;
    let (private_key, auth_tag) = acp.credentials_for(&agent.runtime.id);
    let private_key = private_key.ok_or(MemoryWriterError::MissingPrivateKey)?;
    let auth_tag = auth_tag.ok_or(MemoryWriterError::MissingAuthTag)?;
    Ok((private_key, auth_tag))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::acp::{AcpSupervisorConfig, AgentRuntime, AgentStatus};

    #[test]
    fn memory_writer_uses_selected_managed_agent_credentials() {
        let mut private_keys = BTreeMap::new();
        private_keys.insert("agent".to_string(), "nsec1agent".to_string());
        let mut auth_tags = BTreeMap::new();
        auth_tags.insert("agent".to_string(), "[\"auth\"]".to_string());
        let acp = AcpSupervisor::new(AcpSupervisorConfig {
            acp_binary: "buzz-acp".to_string(),
            relay_url: "ws://localhost:3000".to_string(),
            runtimes: vec![AgentRuntime {
                id: "agent".to_string(),
                label: "Agent".to_string(),
                relay_url: None,
                acp_command: None,
                command: "agent".to_string(),
                args: Vec::new(),
                model: None,
                mcp_command: None,
                turn_timeout_seconds: None,
                system_prompt: None,
                respond_to: "owner-only".to_string(),
                respond_to_allowlist: Vec::new(),
                reply_placement: "thread-direct-mentions".to_string(),
                managed: true,
                start_on_launch: false,
                initial_status: AgentStatus::Stopped,
                available: true,
                install_hint: String::new(),
                last_error: None,
                log_path: None,
            }],
            default_private_key: None,
            default_auth_tag: None,
            default_agent_owner: None,
            runtime_private_keys: private_keys,
            runtime_auth_tags: auth_tags,
            mcp_command: String::new(),
        });
        let (private_key, auth_tag) = selected_agent_memory_identity(&acp, 0).unwrap();

        assert_eq!(private_key, "nsec1agent");
        assert_eq!(auth_tag, "[\"auth\"]");
    }
}
