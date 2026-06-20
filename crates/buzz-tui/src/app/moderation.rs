use super::{clamp_index, short_id, App, Focus};

impl App {
    pub fn focus_message_report(&mut self) {
        if self.selected_timeline_message().is_none() {
            self.status = "No message selected".to_string();
            return;
        }
        self.message_report_input = "other | ".to_string();
        self.focus = Focus::MessageReport;
        self.status = "Report as 'type | optional note'".to_string();
    }

    pub fn message_report_push(&mut self, ch: char) {
        if ch != '\n' && ch != '\r' {
            self.message_report_input.push(ch);
        }
    }

    pub fn message_report_pop(&mut self) {
        self.message_report_input.pop();
    }

    pub(super) async fn submit_message_report(&mut self) {
        let Some(message) = self.selected_timeline_message() else {
            self.status = "No message selected".to_string();
            return;
        };
        let (report_type, note) = self
            .message_report_input
            .split_once('|')
            .map(|(kind, note)| (kind.trim(), note.trim()))
            .unwrap_or((self.message_report_input.trim(), ""));
        match self
            .report_message_native(&message.pubkey, &message.id, report_type, note)
            .await
        {
            Ok(()) => {
                self.message_report_input.clear();
                self.focus = Focus::Timeline;
                self.status = format!("Reported message {}", short_id(&message.id));
            }
            Err(error) => self.status = format!("report: {error}"),
        }
    }

    pub async fn focus_moderation(&mut self) {
        self.focus = Focus::Moderation;
        self.refresh_moderation_reports().await;
    }

    pub async fn refresh_moderation_reports(&mut self) {
        match self.moderation_reports_native().await {
            Ok(reports) => {
                self.moderation_reports = reports;
                clamp_index(
                    &mut self.selected_moderation_report,
                    self.moderation_reports.len(),
                );
                self.status = format!(
                    "Loaded {} open moderation report{}",
                    self.moderation_reports.len(),
                    if self.moderation_reports.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
            }
            Err(error) => self.status = format!("moderation: {error}"),
        }
    }

    pub async fn dismiss_selected_moderation_report(&mut self) {
        let Some(report) = self
            .moderation_reports
            .get(self.selected_moderation_report)
            .cloned()
        else {
            self.status = "No moderation report selected".to_string();
            return;
        };
        match self
            .dismiss_moderation_report_native(&report.report_event_id)
            .await
        {
            Ok(()) => {
                self.refresh_moderation_reports().await;
                self.status = format!("Dismissed report {}", short_id(&report.report_event_id));
            }
            Err(error) => self.status = format!("dismiss report: {error}"),
        }
    }

    pub async fn mint_community_invite(&mut self) {
        match self.mint_invite_native(None).await {
            Ok(invite) => match crate::clipboard::copy_text(&invite.url) {
                Ok(()) => {
                    self.status = format!(
                        "Invite copied (expires {}): {}",
                        invite.expires_at, invite.url
                    );
                }
                Err(error) => {
                    self.status = format!("Invite minted: {}; copy failed: {error}", invite.url);
                }
            },
            Err(error) => self.status = format!("invite: {error}"),
        }
    }
}
