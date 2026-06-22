use super::{clamp_index, short_id, App, Focus, ReminderDraftMode, ThreadContext, TimelineMode};
use crate::client::{
    count_due_reminders, group_reminders, Message, Reminder, ReminderStatus, ReminderTarget,
};
use chrono::{Datelike, Duration, Local, TimeZone};

const REMINDER_PRESETS: &[(&str, ReminderPreset)] = &[
    ("In 30 minutes", ReminderPreset::Minutes(30)),
    ("In 1 hour", ReminderPreset::Minutes(60)),
    ("In 3 hours", ReminderPreset::Minutes(180)),
    ("Tomorrow at 9am", ReminderPreset::DayAt9(1)),
    ("Next Monday at 9am", ReminderPreset::NextMondayAt9),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReminderPreset {
    Minutes(u64),
    DayAt9(u64),
    NextMondayAt9,
}

impl App {
    pub async fn focus_reminders(&mut self) {
        self.focus = Focus::Reminders;
        self.refresh_reminders().await;
    }

    pub async fn refresh_reminders(&mut self) {
        match self.fetch_reminders_native().await {
            Ok(reminders) => {
                self.reminders = reminders;
                self.remember_reminder_author_profiles().await;
                let len = self.visible_reminders().len();
                clamp_index(&mut self.selected_reminder, len);
                let due = count_due_reminders(&self.reminders, now_seconds());
                self.status = format!(
                    "Loaded {} reminder{} ({} due)",
                    self.visible_reminders().len(),
                    if self.visible_reminders().len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    due
                );
            }
            Err(error) => self.status = format!("reminders: {error}"),
        }
    }

    pub fn start_reminder_for_selected_message(&mut self) {
        let Some(message) = self.selected_timeline_message() else {
            self.status = "No message selected".to_string();
            return;
        };
        let Some(target) = self.reminder_target_for_message(&message) else {
            self.status = "Selected message has no reminder target".to_string();
            return;
        };
        self.reminder_target = Some(target);
        self.reminder_note.clear();
        self.reminder_preset = 0;
        self.reminder_draft_mode = ReminderDraftMode::Create;
        self.focus = Focus::ReminderCreate;
        self.status =
            "Reminder draft: choose preset with Tab, optional note, Enter saves".to_string();
    }

    pub fn focus_snooze_selected_reminder(&mut self) {
        let Some(reminder) = self.selected_visible_reminder() else {
            self.status = "No reminder selected".to_string();
            return;
        };
        if reminder.content.status != ReminderStatus::Pending {
            self.status = "Only pending reminders can be snoozed".to_string();
            return;
        }
        self.reminder_target = reminder.content.target.clone();
        self.reminder_note = reminder.content.note.clone().unwrap_or_default();
        self.reminder_preset = 0;
        self.reminder_draft_mode = ReminderDraftMode::Snooze(reminder.id.clone());
        self.focus = Focus::ReminderCreate;
        self.status = "Snooze: choose preset with Tab, Enter saves".to_string();
    }

    pub async fn save_reminder_draft(&mut self) {
        let not_before = self.selected_reminder_preset_timestamp();
        match self.reminder_draft_mode.clone() {
            ReminderDraftMode::Create => {
                let Some(target) = self.reminder_target.clone() else {
                    self.status = "Reminder target missing".to_string();
                    return;
                };
                match self
                    .create_reminder_native(
                        target,
                        not_before,
                        Some(self.reminder_note.trim().to_string()),
                    )
                    .await
                {
                    Ok(reminder) => {
                        self.upsert_reminder(reminder);
                        self.clear_reminder_draft();
                        self.focus = Focus::Reminders;
                        self.status = "Reminder created".to_string();
                    }
                    Err(error) => self.status = format!("create reminder: {error}"),
                }
            }
            ReminderDraftMode::Snooze(reminder_id) => {
                let Some(reminder) = self.reminder_by_id(&reminder_id) else {
                    self.status = "Reminder no longer exists".to_string();
                    return;
                };
                match self.snooze_reminder_native(&reminder, not_before).await {
                    Ok(updated) => {
                        self.upsert_reminder(updated);
                        self.clear_reminder_draft();
                        self.focus = Focus::Reminders;
                        self.status = "Reminder snoozed".to_string();
                    }
                    Err(error) => self.status = format!("snooze reminder: {error}"),
                }
            }
        }
    }

    pub async fn complete_selected_reminder(&mut self) {
        let Some(reminder) = self.selected_visible_reminder() else {
            self.status = "No reminder selected".to_string();
            return;
        };
        match self.complete_reminder_native(&reminder).await {
            Ok(updated) => {
                self.upsert_reminder(updated);
                let len = self.visible_reminders().len();
                clamp_index(&mut self.selected_reminder, len);
                self.status = "Reminder completed".to_string();
            }
            Err(error) => self.status = format!("complete reminder: {error}"),
        }
    }

    pub async fn cancel_selected_reminder(&mut self) {
        let Some(reminder) = self.selected_visible_reminder() else {
            self.status = "No reminder selected".to_string();
            return;
        };
        match self.cancel_reminder_native(&reminder).await {
            Ok(updated) => {
                self.upsert_reminder(updated);
                let len = self.visible_reminders().len();
                clamp_index(&mut self.selected_reminder, len);
                self.status = "Reminder cancelled".to_string();
            }
            Err(error) => self.status = format!("cancel reminder: {error}"),
        }
    }

    pub async fn open_selected_reminder_thread(&mut self) {
        let Some(reminder) = self.selected_visible_reminder() else {
            self.status = "No reminder selected".to_string();
            return;
        };
        let Some(target) = reminder.content.target else {
            self.status = "Reminder has no target thread".to_string();
            return;
        };
        match self
            .get_thread_messages(&target.channel_id, &target.event_id)
            .await
        {
            Ok(messages) => {
                let channel_name = self
                    .channels
                    .iter()
                    .find(|channel| channel.id == target.channel_id)
                    .map(|channel| channel.name.clone())
                    .unwrap_or_else(|| short_id(&target.channel_id).to_string());
                self.remember_message_author_profiles(&messages).await;
                self.thread_root = Some(target.event_id.clone());
                self.thread_context = Some(ThreadContext {
                    channel_id: target.channel_id,
                    channel_name,
                    return_mode: TimelineMode::Channel,
                });
                self.edit_target = None;
                self.messages = messages;
                self.selected_message = self.messages.len().saturating_sub(1);
                self.reset_message_detail_scroll();
                self.timeline_mode = TimelineMode::Channel;
                self.focus = Focus::Timeline;
                self.refresh_selected_message_reactions().await;
                self.mark_visible_thread_messages_read().await;
                self.status = format!("Opened reminder {}", short_id(&target.event_id));
            }
            Err(error) => self.status = format!("reminder thread: {error}"),
        }
    }

    pub fn reminder_note_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.reminder_note.push(ch);
        }
    }

    pub fn reminder_note_pop(&mut self) {
        self.reminder_note.pop();
    }

    pub fn next_reminder_preset(&mut self) {
        self.reminder_preset = (self.reminder_preset + 1) % REMINDER_PRESETS.len();
        self.status = format!("Reminder preset {}", self.selected_reminder_preset_label());
    }

    pub fn previous_reminder_preset(&mut self) {
        self.reminder_preset =
            (self.reminder_preset + REMINDER_PRESETS.len() - 1) % REMINDER_PRESETS.len();
        self.status = format!("Reminder preset {}", self.selected_reminder_preset_label());
    }

    pub fn clear_reminder_draft(&mut self) {
        self.reminder_target = None;
        self.reminder_note.clear();
        self.reminder_preset = 0;
        self.reminder_draft_mode = ReminderDraftMode::Create;
    }

    pub fn selected_reminder_preset_label(&self) -> &'static str {
        REMINDER_PRESETS
            .get(self.reminder_preset)
            .map(|(label, _)| *label)
            .unwrap_or(REMINDER_PRESETS[0].0)
    }

    pub fn selected_reminder_preset_timestamp(&self) -> u64 {
        match REMINDER_PRESETS
            .get(self.reminder_preset)
            .map(|(_, preset)| *preset)
            .unwrap_or(REMINDER_PRESETS[0].1)
        {
            ReminderPreset::Minutes(minutes) => now_seconds() + minutes * 60,
            ReminderPreset::DayAt9(offset) => next_day_at_9(offset),
            ReminderPreset::NextMondayAt9 => {
                let day = current_local_day_of_week();
                let days_until_monday = (8 - day) % 7;
                next_day_at_9(if days_until_monday == 0 {
                    7
                } else {
                    days_until_monday
                })
            }
        }
    }

    pub fn visible_reminders(&self) -> Vec<Reminder> {
        group_reminders(&self.reminders, now_seconds())
            .into_iter()
            .flat_map(|group| group.reminders)
            .collect()
    }

    pub fn selected_visible_reminder(&self) -> Option<Reminder> {
        self.visible_reminders()
            .get(self.selected_reminder)
            .cloned()
    }

    fn reminder_by_id(&self, id: &str) -> Option<Reminder> {
        self.reminders
            .iter()
            .find(|reminder| reminder.id == id)
            .cloned()
    }

    fn upsert_reminder(&mut self, reminder: Reminder) {
        if let Some(existing) = self
            .reminders
            .iter_mut()
            .find(|existing| existing.id == reminder.id)
        {
            *existing = reminder;
        } else {
            self.reminders.push(reminder);
        }
    }

    async fn remember_reminder_author_profiles(&mut self) {
        let pubkeys: Vec<String> = self
            .reminders
            .iter()
            .filter_map(|reminder| reminder.content.target.as_ref())
            .map(|target| target.author_pubkey.trim())
            .filter(|pubkey| !pubkey.is_empty())
            .filter(|pubkey| {
                self.author_profile_label(pubkey).is_none()
                    && !self.author_profiles.contains_key(*pubkey)
            })
            .map(str::to_string)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if pubkeys.is_empty() {
            return;
        }

        let Ok(client) = self.native_relay_client() else {
            return;
        };
        if let Ok(profiles) = client.user_profiles(&pubkeys).await {
            for profile in profiles {
                self.author_profiles.insert(profile.pubkey.clone(), profile);
            }
        }
    }

    fn reminder_target_for_message(&self, message: &Message) -> Option<ReminderTarget> {
        if message.id.is_empty() {
            return None;
        }
        let channel_id = if message.channel_id.is_empty() {
            self.active_channel()?.id
        } else {
            message.channel_id.clone()
        };
        Some(ReminderTarget {
            event_id: message.id.clone(),
            channel_id,
            preview: compact_preview(&message.content, 180),
            author_pubkey: message.pubkey.clone(),
        })
    }

    async fn fetch_reminders_native(&self) -> Result<Vec<Reminder>, String> {
        let client = self.native_relay_client()?;
        client
            .fetch_reminders()
            .await
            .map_err(|error| error.to_string())
    }

    async fn create_reminder_native(
        &self,
        target: ReminderTarget,
        not_before: u64,
        note: Option<String>,
    ) -> Result<Reminder, String> {
        let client = self.native_relay_client()?;
        client
            .create_reminder(target, not_before, note)
            .await
            .map_err(|error| error.to_string())
    }

    async fn complete_reminder_native(&self, reminder: &Reminder) -> Result<Reminder, String> {
        let client = self.native_relay_client()?;
        client
            .complete_reminder(reminder)
            .await
            .map_err(|error| error.to_string())
    }

    async fn cancel_reminder_native(&self, reminder: &Reminder) -> Result<Reminder, String> {
        let client = self.native_relay_client()?;
        client
            .cancel_reminder(reminder)
            .await
            .map_err(|error| error.to_string())
    }

    async fn snooze_reminder_native(
        &self,
        reminder: &Reminder,
        not_before: u64,
    ) -> Result<Reminder, String> {
        let client = self.native_relay_client()?;
        client
            .snooze_reminder(reminder, not_before)
            .await
            .map_err(|error| error.to_string())
    }
}

fn compact_preview(value: &str, max_chars: usize) -> String {
    let mut preview = value
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(value)
        .trim()
        .to_string();
    if preview.chars().count() > max_chars {
        preview = preview
            .chars()
            .take(max_chars.saturating_sub(3))
            .collect::<String>();
        preview.push_str("...");
    }
    preview
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn next_day_at_9(day_offset: u64) -> u64 {
    let now = Local::now();
    let mut date = now.date_naive() + Duration::days(day_offset as i64);
    let Some(mut target) = date
        .and_hms_opt(9, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
    else {
        return now_seconds() + 86_400;
    };
    if target <= now {
        date += Duration::days(1);
        if let Some(next) = date
            .and_hms_opt(9, 0, 0)
            .and_then(|naive| Local.from_local_datetime(&naive).single())
        {
            target = next;
        }
    }
    target.timestamp().max(0) as u64
}

fn current_local_day_of_week() -> u64 {
    u64::from(Local::now().weekday().num_days_from_sunday())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_preview_uses_first_non_empty_line() {
        assert_eq!(compact_preview("\n  hello\nworld", 20), "hello");
    }
}
