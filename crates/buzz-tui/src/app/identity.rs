use nostr::{Keys, ToBech32};

use super::{App, Focus};
use crate::identity::IdentityStorage;

impl App {
    pub fn focus_identity(&mut self) {
        self.identity_secret_visible = false;
        self.identity_input.clear();
        self.focus = Focus::Identity;
        self.status = "Identity loaded; use the palette to export or replace it".to_string();
    }

    pub fn reveal_identity_secret(&mut self) {
        if self.identity_from_env {
            self.status = "Identity is controlled by BUZZ_PRIVATE_KEY for this session".to_string();
            return;
        }
        match self.identity_store.load() {
            Ok(Some(_)) => {
                self.identity_secret_visible = true;
                self.focus = Focus::Identity;
                self.status = "Private key revealed; press y to copy and Esc to hide".to_string();
            }
            Ok(None) => self.status = "No persisted identity found".to_string(),
            Err(error) => self.status = format!("identity export: {error}"),
        }
    }

    pub fn copy_identity_secret(&mut self) {
        if !self.identity_secret_visible {
            self.status = "Export identity from the palette first".to_string();
            return;
        }
        match self.identity_store.load() {
            Ok(Some(secret)) => {
                self.status = match crate::clipboard::copy_text(&secret) {
                    Ok(()) => "Private key copied".to_string(),
                    Err(error) => format!("identity copy: {error}"),
                };
            }
            Ok(None) => self.status = "No persisted identity found".to_string(),
            Err(error) => self.status = format!("identity copy: {error}"),
        }
    }

    pub fn focus_identity_replace(&mut self) {
        if self.identity_from_env {
            self.status = "Identity is controlled by BUZZ_PRIVATE_KEY for this session".to_string();
            return;
        }
        self.identity_input.clear();
        self.identity_candidate = None;
        self.focus = Focus::IdentityReplace;
        self.status = "Paste an nsec/hex key, or press g to generate a replacement".to_string();
    }

    pub fn generate_replacement_identity(&mut self) {
        let keys = Keys::generate();
        match keys.secret_key().to_bech32() {
            Ok(secret) => {
                self.identity_candidate = Some(secret);
                self.identity_input.clear();
                self.focus = Focus::IdentityRecovery;
                self.status = "Save the recovery key, type saved, then press Enter".to_string();
            }
            Err(error) => self.status = format!("identity generation: {error}"),
        }
    }

    pub async fn submit_identity_replacement(&mut self) {
        let raw = self.identity_input.trim();
        let keys = match Keys::parse(raw) {
            Ok(keys) => keys,
            Err(_) => {
                self.status = "That private key is not valid".to_string();
                return;
            }
        };
        match keys.secret_key().to_bech32() {
            Ok(secret) => self.apply_replacement_identity(secret).await,
            Err(error) => self.status = format!("identity replacement: {error}"),
        }
    }

    pub async fn confirm_generated_replacement(&mut self) {
        if !self.identity_input.eq_ignore_ascii_case("saved") {
            self.status = "Type saved before continuing".to_string();
            return;
        }
        let Some(secret) = self.identity_candidate.take() else {
            self.status = "No generated identity to save".to_string();
            return;
        };
        self.apply_replacement_identity(secret).await;
    }

    pub fn identity_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.identity_input.push(ch);
        }
    }

    pub fn identity_input_pop(&mut self) {
        self.identity_input.pop();
    }

    pub fn copy_candidate_identity(&mut self) {
        let Some(secret) = self.identity_candidate.as_deref() else {
            return;
        };
        self.status = match crate::clipboard::copy_text(secret) {
            Ok(()) => "Recovery key copied; type saved and press Enter".to_string(),
            Err(error) => format!("identity copy: {error}"),
        };
    }

    pub fn focus_workspace_authorization(&mut self) {
        self.identity_input = self
            .workspace_config
            .workspaces
            .get(self.workspace_config.active_index())
            .and_then(|workspace| workspace.auth_tag.clone())
            .unwrap_or_default();
        self.focus = Focus::WorkspaceAuthorization;
        self.status = "Enter an auth tag, or leave blank to clear it".to_string();
    }

    pub async fn save_workspace_authorization(&mut self) {
        let auth_tag = (!self.identity_input.trim().is_empty())
            .then(|| self.identity_input.trim().to_string());
        let active_index = self.workspace_config.active_index();
        if let Some(workspace) = self.workspace_config.workspaces.get_mut(active_index) {
            workspace.auth_tag = auth_tag.clone();
        }
        if let Err(error) = self.workspace_config.save(&self.workspace_store_path) {
            self.status = format!("workspace authorization: {error}");
            return;
        }
        if let Some(private_key) = self.cli.private_key() {
            self.cli.set_identity(private_key.clone(), auth_tag.clone());
            self.acp
                .update_default_credentials(Some(private_key), auth_tag);
        }
        self.identity_input.clear();
        self.focus = Focus::Sidebar;
        self.refresh().await;
    }

    async fn apply_replacement_identity(&mut self, secret: String) {
        let storage = match self.identity_store.store(&secret) {
            Ok(storage) => storage,
            Err(error) => {
                self.status = format!("identity replacement: {error}");
                return;
            }
        };
        self.shutdown_all_agents().await;
        for workspace in &mut self.workspace_config.workspaces {
            workspace.auth_tag = None;
        }
        if let Err(error) = self.workspace_config.save(&self.workspace_store_path) {
            self.status = format!("identity replacement: {error}");
            return;
        }
        self.cli.set_identity(secret.clone(), None);
        self.acp.update_default_credentials(Some(secret), None);
        self.identity_input.clear();
        self.identity_candidate = None;
        self.identity_secret_visible = false;
        self.reset_workspace_view_state();
        self.focus = Focus::Sidebar;
        self.status = match storage {
            IdentityStorage::Keyring => "Identity replaced".to_string(),
            IdentityStorage::File => format!(
                "Identity replaced; OS keyring unavailable, stored in {}",
                self.identity_store.path().display()
            ),
        };
        self.refresh().await;
    }
}
