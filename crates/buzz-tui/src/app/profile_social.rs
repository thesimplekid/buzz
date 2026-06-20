use super::{
    clamp_index, is_hex64, parse_contact_input, presence_status_from_info, profile_field_label,
    profile_label, short_id, App, Focus, ProfileField, PulseSource, TimelineMode,
};
use crate::cli::{Message, PresenceStatus, UserProfile};
use std::cmp::Reverse;

impl App {
    pub async fn focus_pulse(&mut self) {
        if self.profile.is_none() {
            match self.current_profile().await {
                Ok(profile) => self.profile = profile,
                Err(error) => {
                    self.status = format!("pulse profile: {error}");
                    return;
                }
            }
        }
        let Some(pubkey) = self.profile.as_ref().map(|profile| profile.pubkey.clone()) else {
            self.status = "No profile found; cannot load Pulse".to_string();
            return;
        };

        let (mut notes, failures, empty_status) = match self.pulse_source {
            PulseSource::People => self.load_people_pulse_notes(&pubkey).await,
            PulseSource::Mine => self.load_single_author_pulse_notes(&pubkey, 50).await,
            PulseSource::Agents => self.load_agent_pulse_notes().await,
        };
        notes.sort_by_key(|note| Reverse(note.created_at));
        notes.truncate(50);
        self.remember_message_author_profiles(&notes).await;
        self.pulse = notes;
        clamp_index(&mut self.selected_pulse, self.pulse.len());
        self.timeline_mode = TimelineMode::Pulse;
        self.focus = Focus::Pulse;
        self.refresh_selected_message_reactions().await;
        self.status = if self.pulse.is_empty() {
            empty_status
        } else if failures == 0 {
            format!(
                "Loaded {} {} Pulse note{}",
                self.pulse.len(),
                self.pulse_source.label(),
                if self.pulse.len() == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "Loaded {} {} Pulse note{} ({} source error{})",
                self.pulse.len(),
                self.pulse_source.label(),
                if self.pulse.len() == 1 { "" } else { "s" },
                failures,
                if failures == 1 { "" } else { "s" }
            )
        };
    }

    pub async fn cycle_pulse_source(&mut self) {
        self.pulse_source = self.pulse_source.next();
        self.focus_pulse().await;
    }

    async fn load_people_pulse_notes(&mut self, pubkey: &str) -> (Vec<Message>, usize, String) {
        if self.contacts.is_empty() {
            match self.contact_list_native(pubkey).await {
                Ok(contacts) => self.contacts = contacts,
                Err(error) => {
                    return (Vec::new(), 1, format!("pulse contacts: {error}"));
                }
            }
        }
        if self.contacts.is_empty() {
            return (Vec::new(), 0, "Pulse has no contacts to load".to_string());
        }

        let mut notes = Vec::new();
        let mut failures = 0usize;
        for contact in self.contacts.iter().take(20) {
            match self.social_user_notes_native(&contact.pubkey, 10).await {
                Ok(mut contact_notes) => notes.append(&mut contact_notes),
                Err(_) => failures += 1,
            }
        }
        (notes, failures, "No people Pulse notes found".to_string())
    }

    async fn load_single_author_pulse_notes(
        &self,
        pubkey: &str,
        limit: u32,
    ) -> (Vec<Message>, usize, String) {
        match self.social_user_notes_native(pubkey, limit).await {
            Ok(notes) => (notes, 0, "No personal Pulse notes found".to_string()),
            Err(error) => (Vec::new(), 1, format!("pulse notes: {error}")),
        }
    }

    async fn load_agent_pulse_notes(&self) -> (Vec<Message>, usize, String) {
        let agent_pubkeys = self
            .acp
            .agents()
            .filter(|agent| agent.runtime.managed && is_hex64(&agent.runtime.id))
            .map(|agent| agent.runtime.id.clone())
            .collect::<Vec<_>>();
        if agent_pubkeys.is_empty() {
            return (Vec::new(), 0, "Pulse has no managed agents".to_string());
        }

        let mut notes = Vec::new();
        let mut failures = 0usize;
        for pubkey in agent_pubkeys.iter().take(20) {
            match self.social_user_notes_native(pubkey, 10).await {
                Ok(mut agent_notes) => notes.append(&mut agent_notes),
                Err(_) => failures += 1,
            }
        }
        (
            notes,
            failures,
            "No managed-agent Pulse notes found".to_string(),
        )
    }

    pub async fn focus_profile(&mut self) {
        match self.current_profile().await {
            Ok(profile) => {
                self.profile = profile;
                self.refresh_profile_presence().await;
                self.focus = Focus::Profile;
                self.status = if self.profile.is_some() {
                    "Loaded profile".to_string()
                } else {
                    "No profile found; press Enter to set display name".to_string()
                };
            }
            Err(error) => self.status = format!("profile: {error}"),
        }
    }

    pub async fn focus_contacts(&mut self) {
        if self.profile.is_none() {
            match self.current_profile().await {
                Ok(profile) => self.profile = profile,
                Err(error) => {
                    self.status = format!("contacts profile: {error}");
                    return;
                }
            }
        }
        let Some(pubkey) = self.profile.as_ref().map(|profile| profile.pubkey.clone()) else {
            self.status = "No profile found; cannot load contacts".to_string();
            return;
        };
        if pubkey.trim().is_empty() {
            self.status = "Profile has no pubkey; cannot load contacts".to_string();
            return;
        }

        match self.contact_list_native(&pubkey).await {
            Ok(contacts) => {
                self.contacts = contacts;
                clamp_index(&mut self.selected_contact, self.contacts.len());
                self.focus = Focus::Contacts;
                self.status = format!(
                    "Loaded {} contact{}",
                    self.contacts.len(),
                    if self.contacts.len() == 1 { "" } else { "s" }
                );
            }
            Err(error) => self.status = format!("contacts: {error}"),
        }
    }

    pub fn focus_add_contact(&mut self) {
        self.contact_input.clear();
        self.focus = Focus::ContactAdd;
        self.status = "Adding contact".to_string();
    }

    pub fn contact_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.contact_input.push(ch);
        }
    }

    pub fn contact_input_pop(&mut self) {
        self.contact_input.pop();
    }

    pub async fn add_contact(&mut self) {
        let Some(contact) = parse_contact_input(&self.contact_input) else {
            self.status = "Type pubkey [relay_url] [petname]".to_string();
            return;
        };
        let mut contacts = self.contacts.clone();
        if let Some(existing) = contacts
            .iter_mut()
            .find(|entry| entry.pubkey == contact.pubkey)
        {
            *existing = contact.clone();
        } else {
            contacts.push(contact.clone());
        }

        match self.set_contact_list_native(&contacts).await {
            Ok(_) => {
                self.contacts = contacts;
                self.selected_contact = self
                    .contacts
                    .iter()
                    .position(|entry| entry.pubkey == contact.pubkey)
                    .unwrap_or_else(|| self.contacts.len().saturating_sub(1));
                self.contact_input.clear();
                self.focus = Focus::Contacts;
                self.status = format!("Saved contact {}", short_id(&contact.pubkey));
            }
            Err(error) => self.status = format!("contacts save: {error}"),
        }
    }

    pub async fn delete_selected_contact(&mut self) {
        let Some(contact) = self.contacts.get(self.selected_contact).cloned() else {
            self.status = "No contact selected".to_string();
            return;
        };
        let mut contacts = self.contacts.clone();
        contacts.retain(|entry| entry.pubkey != contact.pubkey);
        match self.set_contact_list_native(&contacts).await {
            Ok(_) => {
                self.contacts = contacts;
                clamp_index(&mut self.selected_contact, self.contacts.len());
                self.status = format!("Removed contact {}", short_id(&contact.pubkey));
            }
            Err(error) => self.status = format!("contacts save: {error}"),
        }
    }

    pub fn focus_user_lookup(&mut self) {
        self.user_lookup_input.clear();
        self.viewed_profile = None;
        self.focus = Focus::UserLookup;
        self.status = "User lookup".to_string();
    }

    pub async fn focus_selected_user_profile(&mut self) {
        let pubkey = match self.focus {
            Focus::Contacts => self
                .contacts
                .get(self.selected_contact)
                .map(|contact| contact.pubkey.clone()),
            Focus::Timeline | Focus::Feed | Focus::Pulse => self
                .selected_timeline_message()
                .and_then(|message| (!message.pubkey.trim().is_empty()).then_some(message.pubkey)),
            _ => None,
        };
        let Some(pubkey) = pubkey else {
            self.focus_user_lookup();
            return;
        };
        self.load_user_profile(&pubkey).await;
    }

    pub async fn run_user_lookup(&mut self) {
        let query = self.user_lookup_input.trim().to_string();
        if query.is_empty() {
            self.status = "Type a pubkey or display name".to_string();
            return;
        }
        if query.len() == 64 && query.chars().all(|ch| ch.is_ascii_hexdigit()) {
            self.load_user_profile(&query).await;
            return;
        }

        match self.search_user_profiles(&query).await {
            Ok(profiles) => {
                if let Some(profile) = profiles.into_iter().next() {
                    let label = profile_label(&profile);
                    self.viewed_profile = Some(profile);
                    self.focus = Focus::UserProfile;
                    self.status = format!("Loaded profile for {label}");
                } else {
                    self.viewed_profile = None;
                    self.status = format!("No user profile matched {query:?}");
                }
            }
            Err(error) => self.status = format!("user lookup: {error}"),
        }
    }

    pub fn user_lookup_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.user_lookup_input.push(ch);
        }
    }

    pub fn user_lookup_pop(&mut self) {
        self.user_lookup_input.pop();
    }

    pub fn profile_input_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.profile_input.push(ch);
        }
    }

    pub fn profile_input_pop(&mut self) {
        self.profile_input.pop();
    }

    pub(super) fn edit_profile_field(&mut self) {
        self.profile_input = self.profile_field_value().to_string();
        self.focus = Focus::ProfileEdit;
        self.status = format!(
            "Editing {}",
            profile_field_label(self.selected_profile_field)
        );
    }

    pub fn focus_profile_avatar_upload(&mut self) {
        self.profile_upload_path.clear();
        self.focus = Focus::ProfileAvatarUpload;
        self.status = "Type an avatar image path, Enter uploads and saves it".to_string();
    }

    pub fn profile_upload_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.profile_upload_path.push(ch);
        }
    }

    pub fn profile_upload_pop(&mut self) {
        self.profile_upload_path.pop();
    }

    pub(super) async fn upload_profile_avatar(&mut self) {
        let path = self.profile_upload_path.trim().to_string();
        if path.is_empty() {
            self.status = "Avatar path cannot be empty".to_string();
            return;
        }

        let upload = match self.upload_file_native(&path).await {
            Ok(upload) => upload,
            Err(error) => {
                self.status = format!("avatar upload: {error}");
                return;
            }
        };
        if upload.url.trim().is_empty() {
            self.status = "avatar upload response did not include a URL".to_string();
            return;
        }

        match self
            .set_profile_field(ProfileField::Picture, &upload.url)
            .await
        {
            Ok(_) => {
                match self.current_profile().await {
                    Ok(profile) => self.profile = profile,
                    Err(error) => self.status = format!("profile refresh: {error}"),
                }
                self.profile_upload_path.clear();
                self.selected_profile_field = ProfileField::Picture;
                self.focus = Focus::Profile;
                self.status = "Uploaded and saved avatar".to_string();
            }
            Err(error) => self.status = format!("avatar save: {error}"),
        }
    }

    pub(super) async fn save_profile_field(&mut self) {
        let value = self.profile_input.trim().to_string();
        if value.is_empty() {
            self.status = format!(
                "{} cannot be empty",
                profile_field_label(self.selected_profile_field)
            );
            return;
        }

        match self
            .set_profile_field(self.selected_profile_field, &value)
            .await
        {
            Ok(_) => {
                match self.current_profile().await {
                    Ok(profile) => self.profile = profile,
                    Err(error) => self.status = format!("profile refresh: {error}"),
                }
                self.profile_input.clear();
                self.focus = Focus::Profile;
                self.status = format!("Saved {}", profile_field_label(self.selected_profile_field));
            }
            Err(error) => self.status = format!("profile save: {error}"),
        }
    }

    pub async fn cycle_presence(&mut self) {
        let next = self
            .last_presence_status
            .map(PresenceStatus::next)
            .or_else(|| presence_status_from_info(self.presence.as_ref()).map(PresenceStatus::next))
            .unwrap_or(PresenceStatus::Online);
        match self.set_presence(next).await {
            Ok(_) => {
                self.last_presence_status = Some(next);
                self.refresh_profile_presence().await;
                self.status = format!("Presence set to {}", next.as_str());
            }
            Err(error) => self.status = format!("presence: {error}"),
        }
    }

    async fn refresh_profile_presence(&mut self) {
        let Some(pubkey) = self.profile.as_ref().map(|profile| profile.pubkey.clone()) else {
            self.presence = None;
            return;
        };
        if pubkey.trim().is_empty() {
            self.presence = None;
            return;
        }
        match self.presence(&pubkey).await {
            Ok(presence) => {
                self.last_presence_status =
                    presence_status_from_info(presence.as_ref()).or(self.last_presence_status);
                self.presence = presence;
            }
            Err(error) => self.status = format!("presence read: {error}"),
        }
    }

    pub(super) async fn open_selected_contact_dm(&mut self) {
        let Some(pubkey) = self
            .contacts
            .get(self.selected_contact)
            .map(|contact| contact.pubkey.clone())
        else {
            self.status = "No contact selected".to_string();
            return;
        };
        self.open_dm_pubkey(&pubkey).await;
    }

    pub(super) async fn open_viewed_profile_dm(&mut self) {
        let Some(pubkey) = self
            .viewed_profile
            .as_ref()
            .map(|profile| profile.pubkey.clone())
            .filter(|pubkey| !pubkey.trim().is_empty())
        else {
            self.status = "No viewed profile pubkey".to_string();
            return;
        };
        self.open_dm_pubkey(&pubkey).await;
    }

    pub(super) fn move_profile_field(&mut self, delta: isize) {
        const FIELDS: [ProfileField; 4] = [
            ProfileField::DisplayName,
            ProfileField::About,
            ProfileField::Picture,
            ProfileField::Nip05,
        ];
        let current = FIELDS
            .iter()
            .position(|field| *field == self.selected_profile_field)
            .unwrap_or(0);
        let mut next = current;
        super::move_index(&mut next, FIELDS.len(), delta);
        self.selected_profile_field = FIELDS[next];
    }

    pub fn profile_field_value(&self) -> &str {
        let Some(profile) = self.profile.as_ref() else {
            return "";
        };
        match self.selected_profile_field {
            ProfileField::DisplayName => {
                if profile.display_name.trim().is_empty() {
                    &profile.name
                } else {
                    &profile.display_name
                }
            }
            ProfileField::About => &profile.about,
            ProfileField::Picture => &profile.picture,
            ProfileField::Nip05 => &profile.nip05,
        }
    }

    pub(super) async fn load_user_profile(&mut self, pubkey: &str) {
        match self.user_profile(pubkey).await {
            Ok(Some(profile)) => {
                let label = profile_label(&profile);
                self.viewed_profile = Some(profile);
                self.focus = Focus::UserProfile;
                self.status = format!("Loaded profile for {label}");
            }
            Ok(None) => {
                self.viewed_profile = Some(UserProfile {
                    pubkey: pubkey.to_string(),
                    ..UserProfile::default()
                });
                self.focus = Focus::UserProfile;
                self.status = format!("No profile metadata for {}", short_id(pubkey));
            }
            Err(error) => self.status = format!("profile lookup: {error}"),
        }
    }
}
