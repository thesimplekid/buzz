use buzz_pairing_client::{PairingEvent, PairingUri};
use qrcode::render::unicode::Dense1x2;
use qrcode::{EcLevel, QrCode};
use zeroize::Zeroize;

use super::{App, Focus};

/// Request handed from pure TUI state to the async pairing runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingRequest {
    Start,
    ConfirmSas,
    Cancel,
}

/// User-visible step in the terminal pairing overlay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingUiStep {
    Starting,
    Qr,
    Sas,
    Transferring,
    Done,
    Error,
}

/// State rendered by the pairing overlay.
#[derive(Debug)]
pub struct PairingUi {
    pub step: PairingUiStep,
    pub code: Option<PairingUri>,
    pub qr_lines: Vec<String>,
    pub sas: Option<String>,
    pub error: Option<String>,
    pub previous_focus: Focus,
}

impl PairingUi {
    fn starting(previous_focus: Focus) -> Self {
        Self {
            step: PairingUiStep::Starting,
            code: None,
            qr_lines: Vec::new(),
            sas: None,
            error: None,
            previous_focus,
        }
    }

    fn clear_session_material(&mut self) {
        self.code = None;
        for line in &mut self.qr_lines {
            line.zeroize();
        }
        self.qr_lines.clear();
        if let Some(sas) = self.sas.as_mut() {
            sas.zeroize();
        }
        self.sas = None;
    }
}

impl App {
    pub fn focus_mobile_pairing(&mut self) {
        if self.cli.private_key().is_none() {
            self.status = "Pairing requires an active identity".to_string();
            return;
        }
        let previous_focus = self.focus;
        self.pairing = Some(PairingUi::starting(previous_focus));
        self.pending_pairing_request = Some(PairingRequest::Start);
        self.focus = Focus::Pairing;
        self.status = "Preparing secure mobile pairing…".to_string();
    }

    pub fn apply_pairing_event(&mut self, event: PairingEvent) {
        let Some(pairing) = self.pairing.as_mut() else {
            return;
        };
        match event {
            PairingEvent::Ready { uri } => match render_qr(uri.as_str()) {
                Ok(lines) => {
                    pairing.step = PairingUiStep::Qr;
                    pairing.code = Some(uri);
                    pairing.qr_lines = lines;
                    pairing.error = None;
                    self.status = "Scan the pairing code with Buzz mobile".to_string();
                }
                Err(error) => {
                    pairing.step = PairingUiStep::Error;
                    pairing.error = Some(error);
                    pairing.clear_session_material();
                    self.pending_pairing_request = Some(PairingRequest::Cancel);
                }
            },
            PairingEvent::SasReceived { code } => {
                pairing.clear_session_material();
                pairing.step = PairingUiStep::Sas;
                pairing.sas = Some(code);
                pairing.error = None;
                self.status = "Verify the security code on both devices".to_string();
            }
            PairingEvent::Complete => {
                pairing.clear_session_material();
                pairing.step = PairingUiStep::Done;
                pairing.error = None;
                self.status = "Mobile pairing complete".to_string();
            }
            PairingEvent::Aborted { reason } => {
                pairing.clear_session_material();
                pairing.step = PairingUiStep::Error;
                pairing.error = Some(format!("Pairing aborted: {reason}"));
                self.status = "Mobile pairing aborted".to_string();
            }
            PairingEvent::Failed { message } => {
                pairing.clear_session_material();
                pairing.step = PairingUiStep::Error;
                pairing.error = Some(message);
                self.status = "Mobile pairing failed".to_string();
            }
        }
    }

    pub fn confirm_mobile_pairing_sas(&mut self) {
        let Some(pairing) = self.pairing.as_mut() else {
            return;
        };
        if pairing.step != PairingUiStep::Sas {
            return;
        }
        pairing.step = PairingUiStep::Transferring;
        self.pending_pairing_request = Some(PairingRequest::ConfirmSas);
        self.status = "Sending identity to mobile…".to_string();
    }

    pub fn copy_mobile_pairing_code(&mut self) {
        let Some(code) = self
            .pairing
            .as_ref()
            .and_then(|pairing| pairing.code.as_ref())
        else {
            return;
        };
        self.status = match crate::clipboard::copy_text(code.as_str()) {
            Ok(()) => "Pairing code copied to clipboard".to_string(),
            Err(error) => format!("pairing code copy: {error}"),
        };
    }

    pub fn close_mobile_pairing(&mut self) {
        let Some(mut pairing) = self.pairing.take() else {
            return;
        };
        let should_cancel = !matches!(pairing.step, PairingUiStep::Done | PairingUiStep::Error);
        pairing.clear_session_material();
        self.focus = pairing.previous_focus;
        if should_cancel {
            self.pending_pairing_request = Some(PairingRequest::Cancel);
            self.status = "Mobile pairing cancelled".to_string();
        }
    }

    pub fn take_pairing_request(&mut self) -> Option<PairingRequest> {
        self.pending_pairing_request.take()
    }
}

fn render_qr(uri: &str) -> Result<Vec<String>, String> {
    let code = QrCode::with_error_correction_level(uri.as_bytes(), EcLevel::M)
        .map_err(|error| format!("Could not render pairing QR code: {error}"))?;
    Ok(code
        .render::<Dense1x2>()
        .quiet_zone(true)
        .build()
        .lines()
        .map(ToString::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::ToBech32;

    #[test]
    fn terminal_qr_has_quiet_zone_and_multiple_rows() {
        let lines = render_qr(
            "nostrpair://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
             ?secret=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\
             &relay=wss%3A%2F%2Frelay.example&v=1",
        )
        .expect("render QR");
        assert!(lines.len() > 10);
        assert!(lines.first().is_some_and(|line| line.trim().is_empty()));
        assert!(lines.last().is_some_and(|line| line.trim().is_empty()));
    }

    #[test]
    fn pairing_flow_queues_runtime_actions_and_restores_focus() {
        let mut app = crate::app::tests::test_app();
        let keys = nostr::Keys::generate();
        let private_key = keys.secret_key().to_bech32().expect("encode nsec");
        app.cli.set_identity(private_key, None);
        app.focus = Focus::Timeline;

        app.focus_mobile_pairing();
        assert_eq!(app.focus, Focus::Pairing);
        assert_eq!(app.take_pairing_request(), Some(PairingRequest::Start));
        assert_eq!(
            app.pairing.as_ref().map(|pairing| pairing.step),
            Some(PairingUiStep::Starting)
        );

        app.apply_pairing_event(PairingEvent::SasReceived {
            code: "123456".to_string(),
        });
        assert_eq!(
            app.pairing.as_ref().map(|pairing| pairing.step),
            Some(PairingUiStep::Sas)
        );
        app.confirm_mobile_pairing_sas();
        assert_eq!(app.take_pairing_request(), Some(PairingRequest::ConfirmSas));
        assert_eq!(
            app.pairing.as_ref().map(|pairing| pairing.step),
            Some(PairingUiStep::Transferring)
        );

        app.close_mobile_pairing();
        assert_eq!(app.focus, Focus::Timeline);
        assert_eq!(app.take_pairing_request(), Some(PairingRequest::Cancel));
        assert!(app.pairing.is_none());
    }

    #[test]
    fn completed_pairing_closes_without_cancelling_runtime() {
        let mut app = crate::app::tests::test_app();
        let keys = nostr::Keys::generate();
        let private_key = keys.secret_key().to_bech32().expect("encode nsec");
        app.cli.set_identity(private_key, None);

        app.focus_mobile_pairing();
        let _ = app.take_pairing_request();
        app.apply_pairing_event(PairingEvent::Complete);
        app.close_mobile_pairing();

        assert_eq!(app.take_pairing_request(), None);
        assert!(app.pairing.is_none());
    }
}
