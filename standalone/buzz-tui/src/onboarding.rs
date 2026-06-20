use std::io::Stdout;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use nostr::{Keys, ToBech32};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::client::TuiRelayClient;
use crate::clipboard;
use crate::identity::{IdentityStorage, IdentityStore};
use crate::workspace::{normalize_workspace_relay, WorkspaceConfig};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    Relay,
    IdentityChoice,
    Import,
    Recovery,
    Authorization,
}

pub struct OnboardingResult {
    pub private_key: String,
    pub warning: Option<String>,
}

struct Onboarding {
    step: Step,
    relay: String,
    input: String,
    private_key: Option<String>,
    public_key: Option<String>,
    status: String,
}

impl Onboarding {
    fn new(relay: &str) -> Self {
        Self {
            step: Step::Relay,
            relay: relay.to_string(),
            input: relay.to_string(),
            private_key: None,
            public_key: None,
            status: "Enter the relay used by this workspace".to_string(),
        }
    }
}

pub async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    relay: &str,
    store: &IdentityStore,
    workspace_config: &mut WorkspaceConfig,
    workspace_store_path: &std::path::Path,
) -> anyhow::Result<OnboardingResult> {
    let mut onboarding = Onboarding::new(relay);
    loop {
        terminal.draw(|frame| draw(frame, &onboarding))?;
        if !event::poll(Duration::from_millis(150))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            anyhow::bail!("onboarding cancelled");
        }
        match onboarding.step {
            Step::Relay => match key.code {
                KeyCode::Enter => match normalize_workspace_relay(&onboarding.input) {
                    Ok(relay) => {
                        onboarding.relay = relay;
                        onboarding.input.clear();
                        onboarding.step = Step::IdentityChoice;
                        onboarding.status = "Press g to generate or i to import an identity".into();
                    }
                    Err(error) => onboarding.status = error.to_string(),
                },
                KeyCode::Backspace => {
                    onboarding.input.pop();
                }
                KeyCode::Char(ch) => onboarding.input.push(ch),
                _ => {}
            },
            Step::IdentityChoice => match key.code {
                KeyCode::Char('g') | KeyCode::Char('G') => {
                    let keys = Keys::generate();
                    let secret = keys.secret_key().to_bech32()?;
                    onboarding.public_key = Some(keys.public_key().to_bech32()?);
                    onboarding.private_key = Some(secret);
                    onboarding.input.clear();
                    onboarding.step = Step::Recovery;
                    onboarding.status =
                        "Copy the recovery key, type saved, then press Enter".into();
                }
                KeyCode::Char('i') | KeyCode::Char('I') => {
                    onboarding.input.clear();
                    onboarding.step = Step::Import;
                    onboarding.status = "Paste an nsec or hex private key".into();
                }
                KeyCode::Esc => {
                    onboarding.step = Step::Relay;
                    onboarding.input = onboarding.relay.clone();
                }
                _ => {}
            },
            Step::Import => match key.code {
                KeyCode::Enter => match Keys::parse(onboarding.input.trim()) {
                    Ok(keys) => {
                        onboarding.private_key = Some(keys.secret_key().to_bech32()?);
                        onboarding.public_key = Some(keys.public_key().to_bech32()?);
                        onboarding.input.clear();
                        onboarding.step = Step::Authorization;
                        onboarding.status =
                            "Optional: paste a workspace auth tag, or press Enter".into();
                    }
                    Err(_) => onboarding.status = "That private key is not valid".into(),
                },
                KeyCode::Backspace => {
                    onboarding.input.pop();
                }
                KeyCode::Esc => {
                    onboarding.input.clear();
                    onboarding.step = Step::IdentityChoice;
                }
                KeyCode::Char(ch) => onboarding.input.push(ch),
                _ => {}
            },
            Step::Recovery => match key.code {
                KeyCode::Char('y') if onboarding.input.is_empty() => {
                    if let Some(secret) = onboarding.private_key.as_deref() {
                        onboarding.status = match clipboard::copy_text(secret) {
                            Ok(()) => "Recovery key copied; type saved and press Enter".into(),
                            Err(error) => format!("Copy failed: {error}; select it manually"),
                        };
                    }
                }
                KeyCode::Enter if onboarding.input.eq_ignore_ascii_case("saved") => {
                    onboarding.input.clear();
                    onboarding.step = Step::Authorization;
                    onboarding.status =
                        "Optional: paste a workspace auth tag, or press Enter".into();
                }
                KeyCode::Enter => onboarding.status = "Type saved before continuing".into(),
                KeyCode::Backspace => {
                    onboarding.input.pop();
                }
                KeyCode::Char(ch) => onboarding.input.push(ch),
                KeyCode::Esc => {
                    onboarding.private_key = None;
                    onboarding.public_key = None;
                    onboarding.input.clear();
                    onboarding.step = Step::IdentityChoice;
                }
                _ => {}
            },
            Step::Authorization => match key.code {
                KeyCode::Enter => {
                    let private_key = onboarding.private_key.clone().ok_or_else(|| {
                        anyhow::anyhow!("onboarding identity disappeared before validation")
                    })?;
                    let auth_tag = (!onboarding.input.trim().is_empty())
                        .then(|| onboarding.input.trim().to_string());
                    onboarding.status = "Connecting to relay...".into();
                    terminal.draw(|frame| draw(frame, &onboarding))?;
                    let client = TuiRelayClient::new(
                        onboarding.relay.clone(),
                        &private_key,
                        auth_tag.clone(),
                    )?;
                    match client.list_channels(false).await {
                        Ok(_) => {
                            let storage = store.store(&private_key)?;
                            let active_index = workspace_config.active_index();
                            if let Some(workspace) =
                                workspace_config.workspaces.get_mut(active_index)
                            {
                                workspace.relay = onboarding.relay.clone();
                                workspace.auth_tag = auth_tag;
                            }
                            workspace_config.save(workspace_store_path)?;
                            let warning = (storage == IdentityStorage::File).then(|| {
                                format!(
                                    "OS keyring unavailable; identity stored in {}",
                                    store.path().display()
                                )
                            });
                            return Ok(OnboardingResult {
                                private_key,
                                warning,
                            });
                        }
                        Err(error) => {
                            onboarding.status = format!("Could not connect: {error}");
                        }
                    }
                }
                KeyCode::Backspace => {
                    onboarding.input.pop();
                }
                KeyCode::Esc => {
                    onboarding.input.clear();
                    onboarding.step = Step::IdentityChoice;
                }
                KeyCode::Char(ch) => onboarding.input.push(ch),
                _ => {}
            },
        }
    }
}

fn draw(frame: &mut Frame<'_>, onboarding: &Onboarding) {
    let area = centered_rect(72, 22, frame.area());
    frame.render_widget(Clear, area);
    let title = format!(" Buzz setup · {} ", step_label(onboarding.step));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(title);
    let mut lines = vec![
        Line::from(Span::styled(
            "Welcome to Buzz",
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    match onboarding.step {
        Step::Relay => {
            lines.push(Line::from("Relay URL"));
            lines.push(Line::from(format!("> {}", onboarding.input)));
        }
        Step::IdentityChoice => {
            lines.push(Line::from("Your Nostr key is your Buzz identity."));
            lines.push(Line::from(""));
            lines.push(Line::from("[g] Generate a new identity"));
            lines.push(Line::from("[i] Import an existing nsec or hex key"));
        }
        Step::Import => {
            lines.push(Line::from("Private key"));
            lines.push(Line::from(format!(
                "> {}",
                "•".repeat(onboarding.input.chars().count())
            )));
        }
        Step::Recovery => {
            lines.push(Line::from(
                "Save this recovery key. It cannot be recovered later:",
            ));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                onboarding.private_key.as_deref().unwrap_or_default(),
                Style::new().fg(Color::Yellow),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from("Press y to copy, then type saved:"));
            lines.push(Line::from(format!("> {}", onboarding.input)));
        }
        Step::Authorization => {
            lines.push(Line::from(format!("Relay: {}", onboarding.relay)));
            lines.push(Line::from(format!(
                "Identity: {}",
                onboarding.public_key.as_deref().unwrap_or_default()
            )));
            lines.push(Line::from(""));
            lines.push(Line::from("Optional workspace auth tag"));
            lines.push(Line::from(format!("> {}", onboarding.input)));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        &onboarding.status,
        Style::new().fg(Color::Green),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from("Enter continue · Esc back · Ctrl-C quit"));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Left),
        area,
    );
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(height) / 2),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(area.width.saturating_sub(width) / 2),
            Constraint::Length(width.min(area.width)),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}

const fn step_label(step: Step) -> &'static str {
    match step {
        Step::Relay => "relay",
        Step::IdentityChoice => "identity",
        Step::Import => "import",
        Step::Recovery => "backup",
        Step::Authorization => "connect",
    }
}
