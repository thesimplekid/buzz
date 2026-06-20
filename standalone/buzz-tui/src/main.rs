#![deny(unsafe_code)]

mod acp;
mod agent_store;
mod app;
mod cli;
mod client;
mod clipboard;
mod identity;
mod live;
mod memory;
mod onboarding;
mod refresh;
mod render;
mod ui;
mod workspace;

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use app::{App, AppConfig, ComposerSubmit, ConfirmAction, Focus, PendingMessageSend};
use buzz_pairing_client::{
    PairingCommand, PairingEvent, PairingSourceConfig, PairingSourceRuntime,
};
use clap::Parser;
use crossterm::cursor::{MoveTo, RestorePosition, SavePosition};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{
    Attribute, Color as CrosstermColor, Print, SetAttribute, SetForegroundColor,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use nostr::Keys;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::acp::{AcpSupervisor, AcpSupervisorConfig, AgentRuntime, AgentStatus};
use crate::agent_store::{managed_agent_store_path, ManagedAgentStore};
use crate::cli::BuzzCli;
use crate::client::{Message, TuiRelayClient, UserProfile};
use crate::live::{LiveEvent, LiveRuntime};
use crate::refresh::{
    AgentRefreshEvent, AgentRefreshRuntime, ReadStateSyncRuntime, RefreshEvent, RefreshKind,
    RefreshRuntime,
};
use crate::workspace::{workspace_store_path, TuiWorkspace, WorkspaceConfig};

const MAX_EVENTS_PER_FRAME: usize = 256;

#[derive(Debug)]
struct MessageSendResult {
    event_id: String,
    channel_name: String,
    reply_to: Option<String>,
    result: Result<(), String>,
}

#[derive(Debug)]
struct PaletteSearchResult {
    query: String,
    messages: Vec<Message>,
    profiles: Vec<UserProfile>,
}

fn drain_channel<T>(rx: &mut tokio::sync::mpsc::UnboundedReceiver<T>) -> Vec<T> {
    (0..MAX_EVENTS_PER_FRAME)
        .map_while(|_| rx.try_recv().ok())
        .collect()
}

#[derive(Debug, Parser)]
#[command(
    name = "buzz-tui",
    about = "Terminal UI for Buzz backed by direct relay access and buzz-acp agents"
)]
struct Args {
    /// Relay HTTP URL. Defaults to BUZZ_RELAY_URL or localhost.
    #[arg(long, env = "BUZZ_RELAY_URL", default_value = "http://localhost:3000")]
    relay: String,

    /// Nostr private key for direct relay access and buzz-acp. Defaults to BUZZ_PRIVATE_KEY.
    #[arg(long, env = "BUZZ_PRIVATE_KEY")]
    private_key: Option<String>,

    /// NIP-OA auth tag JSON forwarded to the relay client and buzz-acp.
    #[arg(long, env = "BUZZ_AUTH_TAG")]
    auth_tag: Option<String>,

    /// buzz-acp harness binary to execute for local agents.
    #[arg(long, env = "BUZZ_TUI_ACP_BIN", default_value = "buzz-acp")]
    acp_bin: String,

    /// Optional MCP server command exposed to ACP agents.
    #[arg(long, env = "BUZZ_ACP_MCP_COMMAND", default_value = "")]
    mcp_command: String,

    /// Per-agent or per-runtime ACP private key override, e.g. goose=nsec1... May be repeated.
    #[arg(long = "agent-key")]
    agent_keys: Vec<String>,

    /// Per-agent or per-runtime auth tag override, e.g. goose='["auth",...]'. May be repeated.
    #[arg(long = "agent-auth-tag")]
    agent_auth_tags: Vec<String>,

    /// Seconds between active-channel/feed refreshes. 0 disables polling.
    #[arg(long, env = "BUZZ_TUI_REFRESH_INTERVAL", default_value_t = 5)]
    refresh_interval: u64,

    /// Path to the local TUI workspace list.
    #[arg(long, env = "BUZZ_TUI_WORKSPACES")]
    workspace_store: Option<String>,

    /// Path to the local TUI managed-agent store.
    #[arg(long, env = "BUZZ_TUI_AGENTS")]
    agent_store: Option<String>,

    /// Path to the fallback file used when the OS keyring is unavailable.
    #[arg(long, env = "BUZZ_TUI_IDENTITY_FILE")]
    identity_file: Option<String>,
}

#[derive(Clone, Debug)]
struct SessionConfig {
    acp_bin: String,
    private_key: Option<String>,
    auth_tag: Option<String>,
    mcp_command: String,
    runtime_private_keys: BTreeMap<String, String>,
    runtime_auth_tags: BTreeMap<String, String>,
}

#[derive(Debug)]
struct Session {
    cli: BuzzCli,
    supervisor: AcpSupervisor,
    startup_notice: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let identity_from_env = args.private_key.is_some();
    let identity_store =
        identity::IdentityStore::new(identity::identity_file_path(args.identity_file.as_deref()));
    let private_key = match args.private_key {
        Some(private_key) => {
            Keys::parse(&private_key)
                .map_err(|error| anyhow::anyhow!("invalid BUZZ_PRIVATE_KEY: {error}"))?;
            Some(private_key)
        }
        None => identity_store
            .load()?
            .filter(|private_key| Keys::parse(private_key).is_ok()),
    };
    let mut session_config = SessionConfig {
        acp_bin: args.acp_bin,
        private_key,
        auth_tag: args.auth_tag,
        mcp_command: args.mcp_command,
        runtime_private_keys: parse_runtime_assignments(&args.agent_keys)?,
        runtime_auth_tags: parse_runtime_assignments(&args.agent_auth_tags)?,
    };
    let workspace_store_path = workspace_store_path(args.workspace_store.as_deref());
    let agent_store_path = managed_agent_store_path(args.agent_store.as_deref());
    let agent_store =
        ManagedAgentStore::load_or_default(&agent_store_path).unwrap_or_else(|error| {
            eprintln!("{error}");
            ManagedAgentStore::default()
        });
    let workspace_config = WorkspaceConfig::load_or_default(&workspace_store_path, &args.relay)
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            WorkspaceConfig::with_default(&args.relay)
        });
    let mut workspace_config = workspace_config;
    let onboarding_warning = if session_config.private_key.is_none() {
        install_terminal()?;
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let result = onboarding::run(
            &mut terminal,
            &args.relay,
            &identity_store,
            &mut workspace_config,
            &workspace_store_path,
        )
        .await;
        let _ = terminal.clear();
        restore_terminal()?;
        let result = result?;
        session_config.private_key = Some(result.private_key);
        result.warning
    } else {
        None
    };
    let active_workspace = workspace_config
        .workspaces
        .get(workspace_config.active_index())
        .cloned()
        .unwrap_or_else(|| TuiWorkspace {
            id: "default".to_string(),
            name: "default".to_string(),
            relay: args.relay.clone(),
            repos_dir: None,
            auth_tag: None,
        });
    let reaped_stale_agents = acp::reap_stale_agents();
    let mut session = build_session(&session_config, &active_workspace, &agent_store).await;
    if let Some(warning) = onboarding_warning {
        session.startup_notice = Some(match session.startup_notice.take() {
            Some(existing) => format!("{warning}; {existing}"),
            None => warning,
        });
    }
    if reaped_stale_agents > 0 {
        let notice = format!(
            "Reaped {reaped_stale_agents} stale agent harness group{}",
            if reaped_stale_agents == 1 { "" } else { "s" }
        );
        session.startup_notice = Some(match session.startup_notice.take() {
            Some(existing) => format!("{notice}; {existing}"),
            None => notice,
        });
    }
    let mut app = App::new(AppConfig {
        cli: session.cli,
        acp: session.supervisor,
        acp_binary: session_config.acp_bin.clone(),
        startup_notice: session.startup_notice,
        managed_agent_store: agent_store,
        managed_agent_store_path: agent_store_path,
        workspace_config,
        workspace_store_path,
        identity_store,
        identity_from_env,
    });
    let shutdown_rx = install_shutdown_signal_handler();

    install_terminal()?;
    let result = run_app(
        &mut app,
        &session_config,
        args.refresh_interval,
        shutdown_rx,
    )
    .await;
    app.shutdown_all_agents().await;
    restore_terminal()?;

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
    Ok(())
}

async fn build_session(
    config: &SessionConfig,
    workspace: &TuiWorkspace,
    agent_store: &ManagedAgentStore,
) -> Session {
    let relay = &workspace.relay;
    let cli = BuzzCli::new(
        relay.to_string(),
        config.private_key.clone(),
        config
            .auth_tag
            .clone()
            .or_else(|| workspace.auth_tag.clone()),
    );
    let mut runtime_private_keys = BTreeMap::new();
    let mut runtime_auth_tags = BTreeMap::new();
    let managed_agents = agent_store.infos();
    let mut runtimes = managed_agents
        .into_iter()
        .map(|agent| {
            managed_agent_runtime(
                &agent,
                workspace.repos_dir.clone(),
                &mut runtime_private_keys,
                &mut runtime_auth_tags,
            )
        })
        .collect::<Vec<_>>();
    let mut catalog_runtimes = acp::fallback_runtimes();
    runtimes.append(&mut catalog_runtimes);
    runtime_private_keys.extend(config.runtime_private_keys.clone());
    runtime_auth_tags.extend(config.runtime_auth_tags.clone());
    let mut supervisor = AcpSupervisor::new(AcpSupervisorConfig {
        acp_binary: config.acp_bin.clone(),
        relay_url: crate::client::relay_http_to_ws_url(relay),
        runtimes,
        default_private_key: config.private_key.clone(),
        default_auth_tag: config
            .auth_tag
            .clone()
            .or_else(|| workspace.auth_tag.clone()),
        default_agent_owner: owner_pubkey_from_private_key(config.private_key.as_deref()),
        runtime_private_keys,
        runtime_auth_tags,
        mcp_command: config.mcp_command.clone(),
    });
    let startup_notice = start_on_launch_notice(&mut supervisor);
    Session {
        cli,
        supervisor,
        startup_notice,
    }
}

fn agent_status(status: &str) -> AgentStatus {
    match status {
        "running" => AgentStatus::Running,
        "exited" => AgentStatus::Exited,
        _ => AgentStatus::Stopped,
    }
}

fn managed_agent_runtime(
    agent: &crate::client::ManagedAgentInfo,
    repos_dir: Option<String>,
    runtime_private_keys: &mut BTreeMap<String, String>,
    runtime_auth_tags: &mut BTreeMap<String, String>,
) -> AgentRuntime {
    if let Some(private_key) = agent.private_key_nsec.clone() {
        runtime_private_keys.insert(agent.pubkey.clone(), private_key);
    }
    if let Some(auth_tag) = agent.auth_tag.clone() {
        runtime_auth_tags.insert(agent.pubkey.clone(), auth_tag);
    }
    AgentRuntime {
        id: agent.pubkey.clone(),
        label: agent.name.clone(),
        relay_url: Some(crate::client::relay_http_to_ws_url(&agent.relay_url)),
        acp_command: Some(agent.acp_command.clone()),
        command: agent.agent_command.clone(),
        args: agent.agent_args.clone(),
        model: agent.model.clone().filter(|model| !model.trim().is_empty()),
        thinking_effort: None,
        max_output_tokens: None,
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

fn start_on_launch_notice(supervisor: &mut AcpSupervisor) -> Option<String> {
    let (started, failed) = supervisor.start_on_launch_agents();
    if started.is_empty() && failed.is_empty() {
        return None;
    }
    if failed.is_empty() {
        return Some(format!("Agent restore: {} started", started.len()));
    }
    let failed_count = failed.len();
    let failed = failed
        .iter()
        .take(3)
        .map(|(name, error)| format!("{name}: {error}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "Agent restore: {} started, {} failed ({failed})",
        started.len(),
        failed_count
    ))
}

fn parse_runtime_assignments(
    values: &[String],
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let mut assignments = std::collections::BTreeMap::new();
    for value in values {
        let Some((runtime, assigned)) = value.split_once('=') else {
            anyhow::bail!("expected runtime=value, got {value:?}");
        };
        let runtime = runtime.trim();
        let assigned = assigned.trim();
        if runtime.is_empty() || assigned.is_empty() {
            anyhow::bail!("expected non-empty runtime=value, got {value:?}");
        }
        assignments.insert(runtime.to_string(), assigned.to_string());
    }
    Ok(assignments)
}

fn owner_pubkey_from_private_key(private_key: Option<&str>) -> Option<String> {
    private_key
        .and_then(|key| Keys::parse(key).ok())
        .map(|keys| keys.public_key().to_hex().to_ascii_lowercase())
}

fn install_terminal() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    Ok(())
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
    Ok(())
}

fn paint_terminal_hyperlinks(
    writer: &mut impl Write,
    links: &[ui::TerminalHyperlink],
) -> io::Result<()> {
    if links.is_empty() {
        return Ok(());
    }

    queue!(writer, SavePosition)?;
    for link in links {
        let open = format!("\x1b]8;;{}\x1b\\", link.url);
        queue!(
            writer,
            MoveTo(link.x, link.y),
            SetForegroundColor(CrosstermColor::Blue),
            SetAttribute(Attribute::Underlined),
            Print(open),
            Print(&link.text),
            Print("\x1b]8;;\x1b\\"),
            SetForegroundColor(CrosstermColor::Reset),
            SetAttribute(Attribute::Reset),
        )?;
    }
    queue!(writer, RestorePosition)?;
    writer.flush()
}

fn terminal_hyperlink_at(
    links: &[ui::TerminalHyperlink],
    column: u16,
    row: u16,
) -> Option<&ui::TerminalHyperlink> {
    links.iter().find(|link| {
        row == link.y && column >= link.x && column < link.x.saturating_add(link.width)
    })
}

fn handle_terminal_hyperlink_click(
    app: &mut App,
    mouse: MouseEvent,
    links: &[ui::TerminalHyperlink],
) -> bool {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return false;
    }
    let Some(link) = terminal_hyperlink_at(links, mouse.column, mouse.row) else {
        return false;
    };

    app.status = match crate::clipboard::copy_text(&link.url) {
        Ok(()) => "Copied link — paste it into your local browser".to_string(),
        Err(error) => format!("copy attachment link: {error}"),
    };
    true
}

async fn run_app(
    app: &mut App,
    session_config: &SessionConfig,
    refresh_interval_secs: u64,
    mut shutdown_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
) -> anyhow::Result<()> {
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let (live_tx, mut live_rx) = tokio::sync::mpsc::unbounded_channel();
    let (refresh_tx, mut refresh_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent_refresh_tx, mut agent_refresh_rx) = tokio::sync::mpsc::unbounded_channel();
    let (read_state_tx, mut read_state_rx) = tokio::sync::mpsc::unbounded_channel();
    let (send_tx, mut send_rx) = tokio::sync::mpsc::unbounded_channel();
    let (palette_tx, mut palette_rx) =
        tokio::sync::mpsc::unbounded_channel::<PaletteSearchResult>();
    let (pairing_tx, mut pairing_rx) = tokio::sync::mpsc::unbounded_channel::<PairingEvent>();
    let mut live = LiveRuntime::new(app.cli.private_key(), app.cli.auth_tag(), live_tx);
    let mut refresh = RefreshRuntime::new(refresh_tx);
    let mut agent_refresh = AgentRefreshRuntime::new(agent_refresh_tx);
    let mut read_state = ReadStateSyncRuntime::new(read_state_tx);
    let mut pairing = PairingSourceRuntime::new();
    refresh.request_primary(RefreshKind::Full, app.refresh_target());
    let refresh_interval =
        (refresh_interval_secs > 0).then(|| Duration::from_secs(refresh_interval_secs));
    let mut last_refresh = Instant::now();
    let mut pending_reaction_hydrate_id: Option<String> = None;
    let mut completed_reaction_hydrate_id: Option<String> = None;
    let mut requested_reaction_actor_pubkeys: std::collections::BTreeSet<String> =
        Default::default();
    let mut palette_seen_query = String::new();
    let mut palette_requested_query = String::new();
    let mut palette_changed_at = Instant::now();

    while !app.should_quit {
        if shutdown_rx.try_recv().is_ok() {
            app.quit();
            continue;
        }

        live.update_credentials(app.cli.private_key(), app.cli.auth_tag());
        live.sync_active_channel(app.active_live_channel_target());
        let mut workspace_changed = false;
        for event in drain_channel(&mut live_rx) {
            if event == LiveEvent::WorkspaceChanged {
                workspace_changed = true;
            } else {
                apply_live_event(app, event, &mut refresh);
            }
        }
        if workspace_changed {
            apply_live_event(app, LiveEvent::WorkspaceChanged, &mut refresh);
        }
        app.prune_typing_indicators();
        for event in drain_channel(&mut refresh_rx) {
            let outcome = apply_refresh_event(app, &refresh, event);
            if let Some(event_id) = outcome.hydrated_event_id {
                if pending_reaction_hydrate_id.as_deref() == Some(event_id.as_str()) {
                    pending_reaction_hydrate_id = None;
                }
                completed_reaction_hydrate_id = Some(event_id);
            }
            if outcome.timeline_applied {
                completed_reaction_hydrate_id = None;
                request_active_timeline_hydrate(
                    app,
                    &mut refresh,
                    &mut pending_reaction_hydrate_id,
                );
            }
        }
        for event in drain_channel(&mut agent_refresh_rx) {
            let AgentRefreshEvent {
                generation,
                target,
                result,
            } = event;
            if agent_refresh.is_current(generation)
                && target.relay_url == app.cli.relay_url()
                && target.private_key == app.cli.private_key()
                && target.auth_tag == app.cli.auth_tag()
            {
                match result {
                    Ok(result) => app.apply_agent_refresh_result(result),
                    Err(error) => app.status = format!("relay agents: {error}"),
                }
            }
        }
        for event in drain_channel(&mut read_state_rx) {
            read_state.complete();
            if let Err(error) = event.result {
                if event.target.relay_url == app.cli.relay_url()
                    && event.target.slot_id == app.workspace_config.read_state_slot_id
                {
                    app.status = format!("read-state sync: {error}");
                }
            }
        }
        for result in drain_channel(&mut send_rx) {
            apply_message_send_result(app, result, &mut refresh);
        }
        for result in drain_channel(&mut palette_rx) {
            if app.focus == Focus::CommandPalette && app.palette_query.trim() == result.query {
                app.search_results = result.messages;
                app.apply_profiles(result.profiles);
            }
        }
        for event in drain_channel(&mut pairing_rx) {
            app.apply_pairing_event(event);
        }

        if let Some(request) = app.take_pairing_request() {
            match request {
                app::PairingRequest::Start => {
                    if let Some(private_key) = app.cli.private_key() {
                        pairing
                            .start(
                                PairingSourceConfig::new(
                                    app.cli.relay_url().to_string(),
                                    private_key,
                                ),
                                pairing_tx.clone(),
                            )
                            .await;
                    } else {
                        app.apply_pairing_event(PairingEvent::Failed {
                            message: "Pairing requires an active identity".to_string(),
                        });
                    }
                }
                app::PairingRequest::ConfirmSas => {
                    if let Err(error) = pairing.send(PairingCommand::ConfirmSas) {
                        app.apply_pairing_event(PairingEvent::Failed {
                            message: error.to_string(),
                        });
                    }
                }
                app::PairingRequest::Cancel => pairing.stop().await,
            }
        }

        if let Some(notice) = app.attention_notice.take() {
            app.status = notice;
            terminal.backend_mut().write_all(b"\x07")?;
            terminal.backend_mut().flush()?;
        }

        if app.focus == Focus::CommandPalette {
            let query = app.palette_query.trim().to_string();
            if query != palette_seen_query {
                palette_seen_query = query.clone();
                palette_changed_at = Instant::now();
            }
            if query.chars().count() >= 2
                && query != palette_requested_query
                && palette_changed_at.elapsed() >= Duration::from_millis(250)
            {
                palette_requested_query = query.clone();
                spawn_palette_search(app, query, palette_tx.clone());
            }
        } else {
            palette_seen_query.clear();
            palette_requested_query.clear();
        }

        // Channel activation requests its authoritative refresh here so the
        // input handler always returns before relay work starts.
        if app.take_pending_active_refresh() {
            refresh.request_primary(RefreshKind::Active, app.refresh_target());
        }
        if let Some(target) = app.take_read_state_sync_target() {
            read_state.request(target);
        }

        if is_message_detail_focus(app.focus) && app.selected_reactions.is_empty() {
            request_selected_reaction_hydrate(
                app,
                &mut refresh,
                &mut pending_reaction_hydrate_id,
                &completed_reaction_hydrate_id,
            );
        }

        // Resolve display names for reaction actors that arrived via the
        // background hydrate path, requesting each unknown pubkey once.
        let unknown_reaction_actors: std::collections::BTreeSet<String> = app
            .unknown_reaction_actor_pubkeys()
            .into_iter()
            .filter(|pubkey| !requested_reaction_actor_pubkeys.contains(pubkey))
            .collect();
        if !unknown_reaction_actors.is_empty() {
            requested_reaction_actor_pubkeys.extend(unknown_reaction_actors.iter().cloned());
            refresh.request_hydrate(app.hydrate_target(unknown_reaction_actors));
        }

        if app.take_relay_agents_refresh_request() {
            agent_refresh.request(app.agent_refresh_target());
        }
        let mut terminal_hyperlinks = Vec::new();
        terminal.draw(|frame| {
            terminal_hyperlinks = ui::draw(frame, app);
        })?;
        paint_terminal_hyperlinks(terminal.backend_mut(), &terminal_hyperlinks)?;
        app.reap_agents();

        if refresh_interval.is_some_and(|interval| last_refresh.elapsed() >= interval) {
            if app.channels.is_empty() {
                refresh.request_primary(RefreshKind::Full, app.refresh_target());
            } else {
                refresh.request_primary(RefreshKind::Active, app.refresh_target());
            }
            last_refresh = Instant::now();
        }

        if event::poll(Duration::from_millis(150))? {
            match event::read()? {
                Event::Key(key) => {
                    let focus_before = app.focus;
                    let selected_message_before = app.selected_timeline_message_id();
                    handle_key(app, session_config, key, &send_tx).await;
                    let focus_after = app.focus;
                    let selected_message_after = app.selected_timeline_message_id();
                    let entered_message_detail = !is_message_detail_focus(focus_before)
                        && is_message_detail_focus(focus_after);
                    if selected_message_after.is_some()
                        && (selected_message_after != selected_message_before
                            || entered_message_detail)
                    {
                        completed_reaction_hydrate_id = None;
                        request_selected_reaction_hydrate(
                            app,
                            &mut refresh,
                            &mut pending_reaction_hydrate_id,
                            &completed_reaction_hydrate_id,
                        );
                    }
                    app.track_navigation();
                    last_refresh = Instant::now();
                }
                Event::Mouse(mouse)
                    if handle_terminal_hyperlink_click(app, mouse, &terminal_hyperlinks) =>
                {
                    last_refresh = Instant::now();
                }
                _ => {}
            }
        }

        // Persist drafts once typing pauses (composer blurred) so unsent text
        // survives restarts without a disk write per keystroke.
        if app.focus != Focus::Composer {
            app.flush_drafts();
        }
    }

    app.flush_drafts();
    live.stop();
    refresh.stop();
    agent_refresh.stop();
    read_state.stop();
    pairing.stop().await;
    terminal.clear()?;
    Ok(())
}

fn install_shutdown_signal_handler() -> tokio::sync::mpsc::UnboundedReceiver<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    let ctrl_c_tx = tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = ctrl_c_tx.send(());
        }
    });

    #[cfg(unix)]
    {
        let terminate_tx = tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};

            if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                sigterm.recv().await;
                let _ = terminate_tx.send(());
            }
        });

        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};

            if let Ok(mut sighup) = signal(SignalKind::hangup()) {
                sighup.recv().await;
                let _ = tx.send(());
            }
        });
    }

    rx
}

fn apply_live_event(app: &mut App, event: LiveEvent, refresh: &mut RefreshRuntime) {
    match event {
        LiveEvent::Message(message) => {
            let profile_pubkeys = crate::client::timeline_profile_pubkeys(
                message.kind,
                &message.pubkey,
                &message.content,
            );
            if app.apply_live_message(message) {
                let author_pubkeys = profile_pubkeys.into_iter().collect();
                refresh.request_hydrate(app.hydrate_target(author_pubkeys));
            }
        }
        LiveEvent::Edit {
            target_id,
            pubkey,
            content,
            created_at,
            event_id,
        } => {
            app.apply_live_edit(&target_id, &pubkey, content, created_at, event_id);
        }
        LiveEvent::Delete { target_ids } => {
            app.apply_live_deletions(&target_ids);
            // A deleted edit must reveal the previous revision, which requires
            // rebuilding the auxiliary overlay from relay history.
            refresh.request_primary(RefreshKind::Active, app.refresh_target());
        }
        LiveEvent::Typing { pubkey } => {
            app.apply_typing_indicator(pubkey);
        }
        LiveEvent::WorkspaceChanged => {
            refresh.request_primary(RefreshKind::Full, app.refresh_target());
        }
        LiveEvent::Notice(message) => {
            app.status = format!("relay notice: {message}");
        }
        LiveEvent::Error(message) => {
            app.status = message;
        }
    }
}

#[derive(Default)]
struct RefreshApplyOutcome {
    hydrated_event_id: Option<String>,
    timeline_applied: bool,
}

fn apply_refresh_event(
    app: &mut App,
    refresh: &RefreshRuntime,
    event: RefreshEvent,
) -> RefreshApplyOutcome {
    match event {
        RefreshEvent::Primary {
            generation,
            target,
            result,
            ..
        } => {
            if !refresh.is_current_primary(generation) {
                return RefreshApplyOutcome::default();
            }
            match *result {
                Ok(result) => {
                    let timeline_applied = result.messages.is_some();
                    app.apply_refresh_result(&target, result);
                    RefreshApplyOutcome {
                        timeline_applied,
                        ..RefreshApplyOutcome::default()
                    }
                }
                Err(error) => {
                    app.loading_channel_id = None;
                    app.status = format!("refresh: {error}");
                    RefreshApplyOutcome::default()
                }
            }
        }
        RefreshEvent::Hydrate {
            generation,
            target,
            result,
        } => {
            if refresh.is_current_hydrate(generation) {
                let selected_message_id = target.selected_message_id.clone();
                app.apply_hydrate_result(&target, result);
                RefreshApplyOutcome {
                    hydrated_event_id: selected_message_id,
                    timeline_applied: false,
                }
            } else {
                RefreshApplyOutcome::default()
            }
        }
    }
}

fn request_active_timeline_hydrate(
    app: &App,
    refresh: &mut RefreshRuntime,
    pending_reaction_hydrate_id: &mut Option<String>,
) {
    let author_pubkeys = app
        .messages
        .iter()
        .flat_map(|message| {
            crate::client::timeline_profile_pubkeys(message.kind, &message.pubkey, &message.content)
        })
        .collect();
    *pending_reaction_hydrate_id = app.selected_timeline_message_id();
    refresh.request_hydrate(app.hydrate_target(author_pubkeys));
}

fn spawn_message_send(
    pending: PendingMessageSend,
    tx: tokio::sync::mpsc::UnboundedSender<MessageSendResult>,
) {
    let event_id = pending.event.id.to_hex();
    tokio::spawn(async move {
        let result = pending
            .client
            .submit_event(&pending.event)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        let _ = tx.send(MessageSendResult {
            event_id,
            channel_name: pending.channel_name,
            reply_to: pending.reply_to,
            result,
        });
    });
}

fn spawn_palette_search(
    app: &App,
    query: String,
    tx: tokio::sync::mpsc::UnboundedSender<PaletteSearchResult>,
) {
    let Some(private_key) = app.cli.private_key() else {
        return;
    };
    let Ok(client) = TuiRelayClient::new(
        app.cli.relay_url().to_string(),
        &private_key,
        app.cli.auth_tag(),
    ) else {
        return;
    };
    tokio::spawn(async move {
        let filters = [TuiRelayClient::search_filter(&query, 50)];
        let (messages, profiles) = tokio::join!(
            client.query_messages(&filters),
            client.search_user_profiles(&query),
        );
        let messages = messages
            .unwrap_or_default()
            .into_iter()
            .map(Message::from)
            .collect();
        let _ = tx.send(PaletteSearchResult {
            query,
            messages,
            profiles: profiles.unwrap_or_default(),
        });
    });
}

fn apply_message_send_result(
    app: &mut App,
    result: MessageSendResult,
    refresh: &mut RefreshRuntime,
) {
    match result.result {
        Ok(()) => {
            app.status = if result.reply_to.is_some() {
                format!("Replied in #{}", result.channel_name)
            } else {
                format!("Sent to #{}", result.channel_name)
            };
            refresh.request_primary(RefreshKind::Active, app.refresh_target());
        }
        Err(error) => {
            app.remove_timeline_message(&result.event_id);
            app.status = format!("send: {error}");
        }
    }
}

fn request_selected_reaction_hydrate(
    app: &App,
    refresh: &mut RefreshRuntime,
    pending_reaction_hydrate_id: &mut Option<String>,
    completed_reaction_hydrate_id: &Option<String>,
) {
    let Some(event_id) = app.selected_timeline_message_id() else {
        *pending_reaction_hydrate_id = None;
        return;
    };
    if pending_reaction_hydrate_id.as_deref() == Some(event_id.as_str())
        || completed_reaction_hydrate_id.as_deref() == Some(event_id.as_str())
    {
        return;
    }
    *pending_reaction_hydrate_id = Some(event_id);
    refresh.request_hydrate(app.hydrate_target(std::collections::BTreeSet::new()));
}

async fn handle_key(
    app: &mut App,
    session_config: &SessionConfig,
    key: KeyEvent,
    send_tx: &tokio::sync::mpsc::UnboundedSender<MessageSendResult>,
) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.quit();
        return;
    }

    // The pairing overlay owns all input so its short-lived QR and SAS state
    // cannot be obscured by navigation or another modal.
    if app.focus == Focus::Pairing {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y')
                if app
                    .pairing
                    .as_ref()
                    .is_some_and(|pairing| pairing.step == app::PairingUiStep::Sas) =>
            {
                app.confirm_mobile_pairing_sas();
            }
            KeyCode::Char('c') | KeyCode::Char('C')
                if app
                    .pairing
                    .as_ref()
                    .is_some_and(|pairing| pairing.step == app::PairingUiStep::Qr) =>
            {
                app.copy_mobile_pairing_code();
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                app.close_mobile_pairing();
            }
            KeyCode::Enter
                if app.pairing.as_ref().is_some_and(|pairing| {
                    matches!(
                        pairing.step,
                        app::PairingUiStep::Done | app::PairingUiStep::Error
                    )
                }) =>
            {
                app.close_mobile_pairing();
            }
            _ => {}
        }
        return;
    }

    // Confirmation overlay captures input until resolved.
    if app.focus == Focus::Confirm {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_pending().await,
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => app.cancel_confirm(),
            _ => {}
        }
        return;
    }

    // Open the command palette from any non-text-input context.
    if !is_text_input_focus(app.focus)
        && (key.code == KeyCode::Char(':')
            || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p')))
    {
        app.open_palette();
        return;
    }

    let text_input = is_text_input_focus(app.focus);

    match key.code {
        KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => app.navigate_back().await,
        KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
            app.navigate_forward().await;
        }
        KeyCode::Left if app.focus == Focus::Composer => {
            app.composer_left();
        }
        KeyCode::Left if app.focus == Focus::WorkflowEdit => {
            app.workflow_yaml_left();
        }
        KeyCode::Right if app.focus == Focus::Composer => {
            app.composer_right();
        }
        KeyCode::Right if app.focus == Focus::WorkflowEdit => {
            app.workflow_yaml_right();
        }
        KeyCode::Home if app.focus == Focus::Composer => {
            app.composer_home();
        }
        KeyCode::Home if app.focus == Focus::WorkflowEdit => {
            app.workflow_yaml_home();
        }
        KeyCode::End if app.focus == Focus::Composer => {
            app.composer_end();
        }
        KeyCode::End if app.focus == Focus::WorkflowEdit => {
            app.workflow_yaml_end();
        }
        KeyCode::Delete if app.focus == Focus::Composer => {
            app.composer_delete();
        }
        KeyCode::Delete if app.focus == Focus::WorkflowEdit => {
            app.workflow_yaml_delete();
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) && !text_input => {
            app.resize_message_detail_height(1);
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) && !text_input => {
            app.resize_message_detail_height(-1);
        }
        KeyCode::Char('{') if !text_input => {
            app.resize_sidebar(-1);
        }
        KeyCode::Char('}') if !text_input => {
            app.resize_sidebar(1);
        }
        KeyCode::Char(',') if !text_input => {
            app.resize_detail_panel(-1);
        }
        KeyCode::Char('.') if !text_input => {
            app.resize_detail_panel(1);
        }
        KeyCode::Char('0') if !text_input => {
            app.reset_panel_sizes();
        }
        KeyCode::Char('q') if !text_input => {
            app.quit();
        }
        KeyCode::Char('g') if app.focus == Focus::IdentityReplace => {
            app.generate_replacement_identity();
        }
        KeyCode::Char('y') if app.focus == Focus::Identity => {
            app.copy_identity_secret();
        }
        KeyCode::Char('y')
            if app.focus == Focus::IdentityRecovery && app.identity_input.is_empty() =>
        {
            app.copy_candidate_identity();
        }
        KeyCode::Tab if app.focus == Focus::CreateAgent => {
            app.next_agent_create_field();
        }
        KeyCode::Tab if app.focus == Focus::CreateChannel => {
            app.next_create_channel_field();
        }
        KeyCode::Tab if app.focus == Focus::NoteEdit => {
            app.next_note_edit_field();
        }
        KeyCode::Tab if app.focus == Focus::RepoCreate => {
            app.next_repo_create_field();
        }
        KeyCode::Tab if app.focus == Focus::RepoIssueCreate => {
            app.next_repo_issue_field();
        }
        KeyCode::Tab if app.focus == Focus::RepoPatchCreate => {
            app.next_repo_patch_field();
        }
        KeyCode::Tab if app.focus == Focus::MemoryPatch => {
            app.next_memory_patch_field();
        }
        KeyCode::Tab if app.focus == Focus::MemoryEdit => {
            app.next_memory_edit_field();
        }
        KeyCode::Tab if app.focus == Focus::SnippetEdit => {
            app.next_snippet_edit_field();
        }
        KeyCode::Tab if app.focus == Focus::EmojiEdit => {
            app.next_emoji_edit_field();
        }
        KeyCode::Tab if app.focus == Focus::Diff => {
            app.next_diff_field();
        }
        KeyCode::Tab if app.focus == Focus::WorkflowApproval => {
            app.next_workflow_approval_field();
        }
        KeyCode::Tab if app.focus == Focus::ReminderCreate => {
            app.next_reminder_preset();
        }
        KeyCode::Tab if app.focus == Focus::Composer && app.composer_completion.is_some() => {
            app.accept_completion();
        }
        KeyCode::Tab => {
            app.next_focus();
            if app.focus == Focus::Agents {
                app.refresh_selected_agent_log();
            }
        }
        KeyCode::BackTab if app.focus == Focus::CreateAgent => {
            app.previous_agent_create_field();
        }
        KeyCode::BackTab if app.focus == Focus::CreateChannel => {
            app.previous_create_channel_field();
        }
        KeyCode::BackTab if app.focus == Focus::NoteEdit => {
            app.previous_note_edit_field();
        }
        KeyCode::BackTab if app.focus == Focus::RepoCreate => {
            app.previous_repo_create_field();
        }
        KeyCode::BackTab if app.focus == Focus::RepoIssueCreate => {
            app.previous_repo_issue_field();
        }
        KeyCode::BackTab if app.focus == Focus::RepoPatchCreate => {
            app.previous_repo_patch_field();
        }
        KeyCode::BackTab if app.focus == Focus::MemoryPatch => {
            app.previous_memory_patch_field();
        }
        KeyCode::BackTab if app.focus == Focus::MemoryEdit => {
            app.previous_memory_edit_field();
        }
        KeyCode::BackTab if app.focus == Focus::SnippetEdit => {
            app.previous_snippet_edit_field();
        }
        KeyCode::BackTab if app.focus == Focus::EmojiEdit => {
            app.previous_emoji_edit_field();
        }
        KeyCode::BackTab if app.focus == Focus::Diff => {
            app.previous_diff_field();
        }
        KeyCode::BackTab if app.focus == Focus::WorkflowApproval => {
            app.previous_workflow_approval_field();
        }
        KeyCode::BackTab if app.focus == Focus::ReminderCreate => {
            app.previous_reminder_preset();
        }
        KeyCode::BackTab => {
            app.previous_focus();
            if app.focus == Focus::Agents {
                app.refresh_selected_agent_log();
            }
        }
        KeyCode::Char('?') if !text_input => {
            app.focus_help();
        }
        KeyCode::Char('!') if app.focus == Focus::Timeline => {
            app.focus_message_report();
        }
        KeyCode::Char('R') if app.focus == Focus::Moderation => {
            app.refresh_moderation_reports().await;
        }
        KeyCode::Char('D') if app.focus == Focus::Moderation => {
            app.dismiss_selected_moderation_report().await;
        }
        KeyCode::Char('R') if app.focus == Focus::RepoPullRequests => {
            app.refresh_repo_pull_requests().await;
        }
        KeyCode::Char('A') if app.focus == Focus::RepoPullRequests => {
            app.review_selected_pull_request(true).await;
        }
        KeyCode::Char('X') if app.focus == Focus::RepoPullRequests => {
            app.review_selected_pull_request(false).await;
        }
        KeyCode::Char('C') if app.focus == Focus::RepoPullRequests => {
            app.set_selected_pull_request_status(buzz_sdk::GitStatus::Closed)
                .await;
        }
        KeyCode::Char('O') if app.focus == Focus::RepoPullRequests => {
            app.set_selected_pull_request_status(buzz_sdk::GitStatus::Open)
                .await;
        }
        KeyCode::Char('D') if app.focus == Focus::RepoPullRequests => {
            app.set_selected_pull_request_status(buzz_sdk::GitStatus::Draft)
                .await;
        }
        KeyCode::Char('r') if !text_input => {
            app.refresh().await;
        }
        KeyCode::Char('/') if !text_input => {
            app.focus_search();
        }
        KeyCode::Char('O') if !text_input => {
            app.focus_channel_search();
        }
        KeyCode::Char('W') if !text_input => {
            app.focus_workspaces();
        }
        KeyCode::Char('n') if !text_input => {
            app.focus_create_channel();
        }
        KeyCode::Char('m') if !text_input => {
            app.focus_direct_message();
        }
        KeyCode::Char('o') if !text_input => {
            app.toggle_channel_scope().await;
        }
        KeyCode::Char('v') if !text_input => {
            app.focus_canvas().await;
        }
        KeyCode::Char('w') if !text_input => {
            app.focus_workflows().await;
        }
        KeyCode::Char('f')
            if key.modifiers.contains(KeyModifiers::CONTROL) && app.focus == Focus::Timeline =>
        {
            app.toggle_selected_thread_follow();
        }
        KeyCode::Char('f') if !text_input => {
            app.focus_feed().await;
        }
        KeyCode::Char('F') if !text_input => {
            app.cycle_feed_filter().await;
        }
        KeyCode::Char('T') if !text_input => {
            app.focus_pulse().await;
        }
        KeyCode::Char('N') if !text_input => {
            app.focus_notes().await;
        }
        KeyCode::Char('L') if app.focus == Focus::Timeline => {
            app.start_reminder_for_selected_message();
        }
        KeyCode::Char('L') if !text_input => {
            app.focus_reminders().await;
        }
        KeyCode::Char('P') if !text_input => {
            app.focus_profile().await;
        }
        KeyCode::Char('C') if !text_input && app.focus != Focus::Reminders => {
            app.focus_contacts().await;
        }
        KeyCode::Char('U') if !text_input => {
            app.focus_selected_user_profile().await;
        }
        KeyCode::Char('G') if app.focus == Focus::Workflows => {
            app.focus_workflow_approval(true);
        }
        KeyCode::Char('S') if app.focus == Focus::Sidebar => {
            app.toggle_selected_channel_star().await;
        }
        KeyCode::Char('M') if app.focus == Focus::Sidebar => {
            app.toggle_selected_channel_mute().await;
        }
        KeyCode::Char('A') if app.focus == Focus::Sidebar => {
            app.focus_channel_section_assignment();
        }
        KeyCode::Char('V') if app.focus == Focus::Sidebar => {
            app.unassign_selected_channel_section().await;
        }
        KeyCode::F(4) if app.focus == Focus::Sidebar => {
            app.cycle_channel_add_policy().await;
        }
        KeyCode::Char('G') if !text_input => {
            app.focus_repos().await;
        }
        KeyCode::Char('M') if !text_input => {
            app.focus_memory().await;
        }
        KeyCode::Char('Y') if !text_input => {
            app.focus_emoji().await;
        }
        KeyCode::Char('y') if app.focus == Focus::Sidebar => {
            app.copy_selected_channel_id_to_clipboard();
        }
        KeyCode::Char(' ') if app.focus == Focus::Sidebar => {
            app.toggle_selected_channel_read_marker().await;
        }
        KeyCode::Char('j') if app.focus == Focus::Sidebar => app.join_selected_channel().await,
        KeyCode::Char('l') if app.focus == Focus::Sidebar => {
            app.request_confirm(ConfirmAction::LeaveChannel);
        }
        KeyCode::Char('h') if app.focus == Focus::Sidebar => {
            app.request_confirm(ConfirmAction::HideDm);
        }
        KeyCode::Char('E') if app.focus == Focus::Sidebar => app.focus_channel_name(),
        KeyCode::Char('D') if app.focus == Focus::Sidebar => app.focus_channel_description(),
        KeyCode::Char('t') if app.focus == Focus::Sidebar => app.focus_channel_topic(),
        KeyCode::Char('p') if app.focus == Focus::Sidebar => app.focus_channel_purpose(),
        KeyCode::Char('u') if app.focus == Focus::Sidebar => app.focus_add_member(),
        KeyCode::Char('x') if app.focus == Focus::Sidebar => app.focus_remove_member(),
        KeyCode::Char('z') if app.focus == Focus::Sidebar => {
            app.request_confirm(ConfirmAction::ArchiveChannel);
        }
        KeyCode::Char('Z') if app.focus == Focus::Sidebar => {
            app.unarchive_selected_channel().await;
        }
        KeyCode::Delete if app.focus == Focus::Sidebar => {
            app.request_confirm(ConfirmAction::DeleteChannel);
        }
        KeyCode::Char('+') if matches!(app.focus, Focus::Timeline | Focus::Pulse) => {
            app.add_reaction_to_selected_message().await;
        }
        KeyCode::Char('-') if matches!(app.focus, Focus::Timeline | Focus::Pulse) => {
            app.remove_reaction_from_selected_message().await;
        }
        KeyCode::Char(']') if matches!(app.focus, Focus::Timeline | Focus::Feed) => {
            app.vote_on_selected_message("up").await;
        }
        KeyCode::Char('[') if matches!(app.focus, Focus::Timeline | Focus::Feed) => {
            app.vote_on_selected_message("down").await;
        }
        KeyCode::Char('R') if app.focus == Focus::Pulse => {
            app.focus_pulse_reply();
        }
        KeyCode::Char('S') if app.focus == Focus::Pulse => {
            app.cycle_pulse_source().await;
        }
        KeyCode::Char('S') if app.focus == Focus::Notes => {
            app.cycle_notes_source().await;
        }
        KeyCode::Char('R') if app.focus == Focus::Workflows => {
            app.refresh_selected_workflow_detail().await;
        }
        KeyCode::Char('d')
            if app.focus == Focus::Timeline && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.request_confirm(ConfirmAction::DeleteMessage);
        }
        KeyCode::Char('e') if app.focus == Focus::Timeline => {
            app.edit_selected_message();
        }
        KeyCode::Char('y') if is_message_detail_focus(app.focus) => {
            app.copy_selected_message_to_clipboard();
        }
        KeyCode::Char('c') if !text_input => {
            app.focus_composer();
        }
        KeyCode::Char('B') if !text_input => {
            app.focus_attachment();
        }
        KeyCode::Char('I') if app.focus == Focus::Workflows => {
            app.focus_workflow_inputs();
        }
        KeyCode::Char('I') if app.focus == Focus::Emoji => {
            app.focus_import_emoji();
        }
        KeyCode::Char('I') if !text_input => {
            app.focus_diff();
        }
        KeyCode::Char('a') if !text_input => {
            app.focus_agents();
        }
        KeyCode::Char('@') if app.focus == Focus::Agents => {
            app.insert_selected_agent_mention();
        }
        KeyCode::Char('@') if app.focus == Focus::Contacts => {
            app.insert_selected_contact_mention();
        }
        KeyCode::Char('u') if app.focus == Focus::Agents => {
            app.add_selected_agent_to_channel().await;
        }
        KeyCode::Char('s') if app.focus == Focus::Agents => {
            app.toggle_selected_agent_autostart().await;
        }
        KeyCode::Char('s') if app.focus == Focus::Profile => {
            app.cycle_presence().await;
        }
        KeyCode::Char('A') if app.focus == Focus::Profile => {
            app.focus_profile_avatar_upload();
        }
        KeyCode::Char('A') if app.focus == Focus::Contacts => {
            app.focus_add_contact();
        }
        KeyCode::Char('A') if app.focus == Focus::Notes => {
            app.focus_create_note();
        }
        KeyCode::Char('A') if app.focus == Focus::Workflows => {
            app.focus_create_workflow();
        }
        KeyCode::Char('A') if app.focus == Focus::Agents => {
            app.focus_create_agent();
        }
        KeyCode::Char('A') if app.focus == Focus::Repos => {
            app.focus_create_repo();
        }
        KeyCode::Char('I') if app.focus == Focus::Repos => {
            app.focus_create_repo_issue();
        }
        KeyCode::Char('P') if app.focus == Focus::Repos => {
            app.focus_create_repo_patch();
        }
        KeyCode::Char('V') if app.focus == Focus::Repos => {
            app.focus_repo_pull_requests().await;
        }
        KeyCode::Char('A') if app.focus == Focus::Memory => {
            app.focus_create_memory();
        }
        KeyCode::Char('A') if app.focus == Focus::Snippets => {
            app.focus_create_snippet();
        }
        KeyCode::Char('A') if app.focus == Focus::Emoji => {
            app.focus_add_emoji();
        }
        KeyCode::Char('A') if app.focus == Focus::Workspaces => {
            app.focus_add_workspace();
        }
        KeyCode::Char('D') if app.focus == Focus::Contacts => {
            app.request_confirm(ConfirmAction::RemoveContact);
        }
        KeyCode::Char('D') if app.focus == Focus::Notes => {
            app.request_confirm(ConfirmAction::DeleteNote);
        }
        KeyCode::Char('C') if app.focus == Focus::Reminders => {
            app.complete_selected_reminder().await;
        }
        KeyCode::Char('D') if app.focus == Focus::Reminders => {
            app.cancel_selected_reminder().await;
        }
        KeyCode::Char('S') if app.focus == Focus::Reminders => {
            app.focus_snooze_selected_reminder();
        }
        KeyCode::Char('D') if app.focus == Focus::Workflows => {
            app.request_confirm(ConfirmAction::DeleteWorkflow);
        }
        KeyCode::Char('D') if app.focus == Focus::Agents => {
            app.request_confirm(ConfirmAction::DeleteManagedAgent);
        }
        KeyCode::Char('D') if app.focus == Focus::Memory => {
            app.request_confirm(ConfirmAction::DeleteMemory);
        }
        KeyCode::Char('D') if app.focus == Focus::Snippets => {
            app.request_confirm(ConfirmAction::DeleteSnippet);
        }
        KeyCode::Char('S') if app.focus == Focus::Drafts => {
            app.request_confirm(ConfirmAction::SendDraft);
        }
        KeyCode::Char('D') if app.focus == Focus::Drafts => {
            app.request_confirm(ConfirmAction::DeleteDraft);
        }
        KeyCode::Char('D') if app.focus == Focus::Emoji => {
            app.request_confirm(ConfirmAction::RemoveEmoji);
        }
        KeyCode::Char('D') if app.focus == Focus::Workspaces => {
            app.request_confirm(ConfirmAction::RemoveWorkspace);
        }
        KeyCode::Char('E') if app.focus == Focus::Notes => {
            app.focus_edit_note();
        }
        KeyCode::Char('E') if app.focus == Focus::Workflows => {
            app.focus_edit_workflow();
        }
        KeyCode::Char('E') if app.focus == Focus::Memory => {
            app.focus_edit_memory();
        }
        KeyCode::Char('E') if app.focus == Focus::Snippets => {
            app.focus_edit_snippet();
        }
        KeyCode::Char('A') if app.focus == Focus::RelayMembers => {
            app.focus_add_relay_member();
        }
        KeyCode::Char('E') if app.focus == Focus::RelayMembers => {
            app.focus_change_relay_member_role();
        }
        KeyCode::Char('X') if app.focus == Focus::RelayMembers => {
            app.focus_remove_relay_member();
        }
        KeyCode::Char('R') if app.focus == Focus::RelayMembers => {
            app.refresh_relay_members().await;
        }
        KeyCode::Char('H') if app.focus == Focus::Memory => {
            app.show_selected_memory_hash().await;
        }
        KeyCode::Char('P') if app.focus == Focus::Memory => {
            app.focus_patch_memory();
        }
        KeyCode::Char('X') if app.focus == Focus::Workflows => {
            app.focus_workflow_approval(false);
        }
        KeyCode::Char('X') if app.focus == Focus::Emoji => {
            app.export_workspace_emoji().await;
        }
        KeyCode::F(2) if app.focus == Focus::CreateAgent => {
            app.toggle_new_agent_start_on_launch();
        }
        KeyCode::F(2) if app.focus == Focus::WorkflowEdit => {
            app.use_scheduled_digest_workflow_template();
        }
        KeyCode::F(2) if app.focus == Focus::CreateChannel => {
            app.cycle_new_channel_type();
        }
        KeyCode::F(3) if app.focus == Focus::CreateAgent => {
            app.cycle_new_agent_respond_to();
        }
        KeyCode::F(3) if app.focus == Focus::WorkflowEdit => {
            app.use_webhook_digest_workflow_template();
        }
        KeyCode::F(4) if app.focus == Focus::CreateAgent => {
            app.toggle_new_agent_reply_placement();
        }
        KeyCode::F(5) if app.focus == Focus::CreateAgent => {
            app.cycle_new_agent_thinking_effort();
        }
        KeyCode::F(4) if app.focus == Focus::WorkflowEdit => {
            app.use_basic_workflow_template();
        }
        KeyCode::F(4) if app.focus == Focus::CreateChannel => {
            app.cycle_new_channel_expiry();
        }
        KeyCode::F(3) if app.focus == Focus::CreateChannel => {
            app.cycle_new_channel_visibility();
        }
        KeyCode::F(2) if app.focus == Focus::EmojiImport => {
            app.toggle_emoji_import_replace();
        }
        KeyCode::PageDown if app.focus == Focus::Agents => {
            app.move_agent_selection(10);
        }
        KeyCode::PageUp if app.focus == Focus::Agents => {
            app.move_agent_selection(-10);
        }
        KeyCode::Home if app.focus == Focus::Agents => {
            app.selected_agent = 0;
            app.refresh_selected_agent_log();
        }
        KeyCode::End if app.focus == Focus::Agents => {
            app.selected_agent = app.agent_count().saturating_sub(1);
            app.refresh_selected_agent_log();
        }
        KeyCode::PageDown if is_message_detail_focus(app.focus) => {
            app.scroll_message_detail_down();
        }
        KeyCode::PageUp if is_message_detail_focus(app.focus) && app.message_detail_scroll > 0 => {
            app.scroll_message_detail_up();
        }
        KeyCode::Char('d')
            if is_message_detail_focus(app.focus)
                && key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.scroll_message_detail_down();
        }
        KeyCode::Char('u')
            if is_message_detail_focus(app.focus)
                && key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.scroll_message_detail_up();
        }
        KeyCode::Home if is_message_detail_focus(app.focus) => {
            app.scroll_message_detail_top();
        }
        KeyCode::End if is_message_detail_focus(app.focus) => {
            app.scroll_message_detail_bottom();
        }
        KeyCode::PageUp if matches!(app.focus, Focus::Timeline) => {
            app.load_older_messages().await;
        }
        KeyCode::Up if app.focus == Focus::Composer => app.composer_up(),
        KeyCode::Up if app.focus == Focus::WorkflowEdit => app.workflow_yaml_up(),
        KeyCode::Down if app.focus == Focus::Composer => app.composer_down(),
        KeyCode::Down if app.focus == Focus::WorkflowEdit => app.workflow_yaml_down(),
        KeyCode::Up if app.focus == Focus::Timeline => {
            app.move_timeline_selection(-1);
        }
        KeyCode::Down if app.focus == Focus::Timeline => {
            app.move_timeline_selection(1);
        }
        KeyCode::Up if app.focus == Focus::Agents => app.move_agent_selection(-1),
        KeyCode::Down if app.focus == Focus::Agents => app.move_agent_selection(1),
        KeyCode::Up => app.move_selection(-1).await,
        KeyCode::Down => app.move_selection(1).await,
        KeyCode::Enter
            if app.focus == Focus::Composer && key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.composer_newline();
        }
        KeyCode::Enter
            if app.focus == Focus::WorkflowEdit && key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.workflow_yaml_newline();
        }
        KeyCode::Enter
            if app.focus == Focus::SnippetEdit && key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.snippet_content_newline();
        }
        KeyCode::Enter if app.focus == Focus::Composer && app.composer_completion.is_some() => {
            app.accept_completion();
        }
        KeyCode::Enter if app.focus == Focus::Composer => {
            match app.prepare_composer_message_send() {
                ComposerSubmit::Queued(pending) => spawn_message_send(*pending, send_tx.clone()),
                ComposerSubmit::Inline => app.activate().await,
                ComposerSubmit::Done => {}
            }
        }
        KeyCode::Enter if app.focus == Focus::Workspaces => {
            switch_selected_workspace(app, session_config).await;
        }
        KeyCode::Enter => app.activate().await,
        KeyCode::Esc => app.escape().await,
        KeyCode::Backspace if app.focus == Focus::CommandPalette => app.palette_pop(),
        KeyCode::Backspace if app.focus == Focus::Composer => app.composer_pop(),
        KeyCode::Backspace if app.focus == Focus::Attachment => app.attachment_pop(),
        KeyCode::Backspace if app.focus == Focus::Diff => app.diff_input_pop(),
        KeyCode::Backspace if app.focus == Focus::Search => app.search_pop(),
        KeyCode::Backspace if app.focus == Focus::ChannelSearch => app.channel_search_pop(),
        KeyCode::Backspace if app.focus == Focus::CreateChannel => app.new_channel_pop(),
        KeyCode::Backspace if app.focus == Focus::DirectMessage => app.dm_pubkey_pop(),
        KeyCode::Backspace if app.focus == Focus::CreateAgent => app.new_agent_name_pop(),
        KeyCode::Backspace if app.focus == Focus::ProfileEdit => app.profile_input_pop(),
        KeyCode::Backspace if app.focus == Focus::ProfileAvatarUpload => {
            app.profile_upload_pop();
        }
        KeyCode::Backspace if app.focus == Focus::ContactAdd => app.contact_input_pop(),
        KeyCode::Backspace if app.focus == Focus::UserLookup => app.user_lookup_pop(),
        KeyCode::Backspace
            if matches!(
                app.focus,
                Focus::AddRelayMember | Focus::RemoveRelayMember | Focus::ChangeRelayMemberRole
            ) =>
        {
            app.relay_member_input_pop();
        }
        KeyCode::Backspace if app.focus == Focus::NoteEdit => app.note_input_pop(),
        KeyCode::Backspace if app.focus == Focus::WorkflowEdit => app.workflow_yaml_pop(),
        KeyCode::Backspace if app.focus == Focus::WorkflowInputs => app.workflow_inputs_pop(),
        KeyCode::Backspace if app.focus == Focus::WorkflowApproval => app.workflow_approval_pop(),
        KeyCode::Backspace if app.focus == Focus::ReminderCreate => app.reminder_note_pop(),
        KeyCode::Backspace if app.focus == Focus::MessageReport => app.message_report_pop(),
        KeyCode::Backspace if app.focus == Focus::RepoCreate => app.repo_input_pop(),
        KeyCode::Backspace if app.focus == Focus::RepoIssueCreate => app.repo_issue_input_pop(),
        KeyCode::Backspace if app.focus == Focus::RepoPatchCreate => app.repo_patch_input_pop(),
        KeyCode::Backspace if app.focus == Focus::MemoryEdit => app.memory_input_pop(),
        KeyCode::Backspace if app.focus == Focus::MemoryPatch => app.memory_patch_input_pop(),
        KeyCode::Backspace if app.focus == Focus::SnippetEdit => app.snippet_input_pop(),
        KeyCode::Backspace if app.focus == Focus::EmojiEdit => app.emoji_input_pop(),
        KeyCode::Backspace if app.focus == Focus::EmojiImport => app.emoji_import_pop(),
        KeyCode::Backspace if app.focus == Focus::WorkspaceAdd => app.workspace_input_pop(),
        KeyCode::Backspace
            if matches!(
                app.focus,
                Focus::IdentityReplace | Focus::IdentityRecovery | Focus::WorkspaceAuthorization
            ) =>
        {
            app.identity_input_pop()
        }
        KeyCode::Backspace if is_channel_input_focus(app.focus) => app.channel_action_pop(),
        KeyCode::Backspace if app.focus == Focus::CanvasEdit => app.canvas_draft_pop(),
        KeyCode::Char(ch) if app.focus == Focus::CommandPalette => app.palette_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::Composer => app.composer_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::Attachment => app.attachment_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::Diff => app.diff_input_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::Search => app.search_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::ChannelSearch => app.channel_search_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::CreateChannel => app.new_channel_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::DirectMessage => app.dm_pubkey_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::CreateAgent => app.new_agent_name_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::ProfileEdit => app.profile_input_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::ProfileAvatarUpload => {
            app.profile_upload_push(ch);
        }
        KeyCode::Char(ch) if app.focus == Focus::ContactAdd => app.contact_input_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::UserLookup => app.user_lookup_push(ch),
        KeyCode::Char(ch)
            if matches!(
                app.focus,
                Focus::AddRelayMember | Focus::RemoveRelayMember | Focus::ChangeRelayMemberRole
            ) =>
        {
            app.relay_member_input_push(ch);
        }
        KeyCode::Char(ch) if app.focus == Focus::NoteEdit => app.note_input_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::WorkflowEdit => app.workflow_yaml_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::WorkflowInputs => app.workflow_inputs_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::WorkflowApproval => {
            app.workflow_approval_push(ch);
        }
        KeyCode::Char(ch) if app.focus == Focus::ReminderCreate => app.reminder_note_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::MessageReport => app.message_report_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::RepoCreate => app.repo_input_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::RepoIssueCreate => {
            app.repo_issue_input_push(ch);
        }
        KeyCode::Char(ch) if app.focus == Focus::RepoPatchCreate => {
            app.repo_patch_input_push(ch);
        }
        KeyCode::Char(ch) if app.focus == Focus::MemoryEdit => app.memory_input_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::MemoryPatch => app.memory_patch_input_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::SnippetEdit => app.snippet_input_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::EmojiEdit => app.emoji_input_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::EmojiImport => app.emoji_import_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::WorkspaceAdd => app.workspace_input_push(ch),
        KeyCode::Char(ch)
            if matches!(
                app.focus,
                Focus::IdentityReplace | Focus::IdentityRecovery | Focus::WorkspaceAuthorization
            ) =>
        {
            app.identity_input_push(ch)
        }
        KeyCode::Char(ch) if is_channel_input_focus(app.focus) => app.channel_action_push(ch),
        KeyCode::Char(ch) if app.focus == Focus::CanvasEdit => app.canvas_draft_push(ch),
        _ => {}
    }
}

async fn switch_selected_workspace(app: &mut App, session_config: &SessionConfig) {
    let Some(workspace) = app.selected_workspace() else {
        app.status = "No workspace selected".to_string();
        return;
    };
    if workspace.id == app.workspace_config.active_id {
        app.status = format!("Already on workspace {}", workspace.name);
        return;
    }

    app.status = format!("Switching to workspace {}...", workspace.name);
    app.shutdown_all_agents().await;
    let mut effective_config = session_config.clone();
    effective_config.private_key = app.cli.private_key();
    let session = build_session(&effective_config, &workspace, &app.managed_agent_store).await;
    app.apply_workspace_session(
        workspace.id.clone(),
        session.cli,
        session.supervisor,
        session.startup_notice,
    )
    .await;
}

fn is_text_input_focus(focus: Focus) -> bool {
    matches!(
        focus,
        Focus::Composer
            | Focus::Attachment
            | Focus::Diff
            | Focus::Search
            | Focus::ChannelSearch
            | Focus::CreateChannel
            | Focus::DirectMessage
            | Focus::ChannelName
            | Focus::ChannelDescription
            | Focus::ChannelTopic
            | Focus::ChannelPurpose
            | Focus::ChannelSectionAssign
            | Focus::AddMember
            | Focus::RemoveMember
            | Focus::CreateAgent
            | Focus::ProfileEdit
            | Focus::ProfileAvatarUpload
            | Focus::ContactAdd
            | Focus::UserLookup
            | Focus::AddRelayMember
            | Focus::RemoveRelayMember
            | Focus::ChangeRelayMemberRole
            | Focus::RepoCreate
            | Focus::RepoIssueCreate
            | Focus::RepoPatchCreate
            | Focus::MemoryEdit
            | Focus::MemoryPatch
            | Focus::SnippetEdit
            | Focus::EmojiEdit
            | Focus::EmojiImport
            | Focus::NoteEdit
            | Focus::WorkspaceAdd
            | Focus::IdentityReplace
            | Focus::IdentityRecovery
            | Focus::WorkspaceAuthorization
            | Focus::CanvasEdit
            | Focus::WorkflowEdit
            | Focus::WorkflowInputs
            | Focus::WorkflowApproval
            | Focus::ReminderCreate
            | Focus::MessageReport
            | Focus::CommandPalette
    )
}

fn is_channel_input_focus(focus: Focus) -> bool {
    matches!(
        focus,
        Focus::ChannelName
            | Focus::ChannelDescription
            | Focus::ChannelTopic
            | Focus::ChannelPurpose
            | Focus::ChannelSectionAssign
            | Focus::AddMember
            | Focus::RemoveMember
    )
}

fn is_message_detail_focus(focus: Focus) -> bool {
    matches!(focus, Focus::Timeline | Focus::Feed | Focus::Pulse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_channel_drain_yields_after_one_frame_budget() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for value in 0..(MAX_EVENTS_PER_FRAME + 5) {
            tx.send(value).unwrap();
        }

        let first = drain_channel(&mut rx);
        let second = drain_channel(&mut rx);

        assert_eq!(first.len(), MAX_EVENTS_PER_FRAME);
        assert_eq!(second.len(), 5);
        assert_eq!(first[0], 0);
        assert_eq!(second[0], MAX_EVENTS_PER_FRAME);
    }

    #[test]
    fn parses_runtime_assignments() {
        let values = vec!["goose=nsec1abc".to_string(), "codex=hex".to_string()];
        let parsed = parse_runtime_assignments(&values).unwrap();
        assert_eq!(parsed.get("goose").map(String::as_str), Some("nsec1abc"));
        assert_eq!(parsed.get("codex").map(String::as_str), Some("hex"));
    }

    #[test]
    fn rejects_malformed_runtime_assignments() {
        let values = vec!["goose".to_string()];
        assert!(parse_runtime_assignments(&values).is_err());
    }

    #[test]
    fn paints_compact_labels_with_complete_osc8_destinations() {
        let url = format!("https://example.com/{}.png", "ab".repeat(24));
        let mut output = Vec::new();

        paint_terminal_hyperlinks(
            &mut output,
            &[ui::TerminalHyperlink {
                x: 12,
                y: 4,
                width: 5,
                text: "image".to_string(),
                url: url.clone(),
            }],
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains(&format!("\x1b]8;;{url}\x1b\\")));
        assert!(output.contains("image"));
        assert!(output.contains("\x1b]8;;\x1b\\"));
    }

    #[test]
    fn terminal_link_hit_testing_includes_the_whole_label() {
        let link = ui::TerminalHyperlink {
            x: 12,
            y: 4,
            width: 5,
            text: "image".to_string(),
            url: "https://example.com/image.png".to_string(),
        };

        assert_eq!(
            terminal_hyperlink_at(std::slice::from_ref(&link), 12, 4),
            Some(&link)
        );
        assert_eq!(
            terminal_hyperlink_at(std::slice::from_ref(&link), 16, 4),
            Some(&link)
        );
        assert_eq!(
            terminal_hyperlink_at(std::slice::from_ref(&link), 17, 4),
            None
        );
        assert_eq!(
            terminal_hyperlink_at(std::slice::from_ref(&link), 12, 5),
            None
        );
    }
}
