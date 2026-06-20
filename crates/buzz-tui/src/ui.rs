use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::acp::AgentStatus;
use crate::app::{
    diff_field_label, note_edit_field_label, profile_field_label, repo_create_field_label,
    AgentCreateField, App, ChannelScope, CreateChannelField, DiffField, EmojiEditField, Focus,
    MemoryEditField, MemoryPatchField, NoteEditField, RepoCreateField, RepoIssueField,
    RepoPatchField, TimelineMode, WorkflowApprovalField, MIN_AGENT_PANEL_HEIGHT, MIN_DETAIL_WIDTH,
    MIN_MESSAGE_DETAIL_HEIGHT, MIN_SIDEBAR_WIDTH,
};
use crate::cli::{ConversationKind, ProfileField};

const MIN_TIMELINE_WIDTH: u16 = 30;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let (sidebar_width, detail_width) = layout_panel_widths(root[0].width, app);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(sidebar_width),
            Constraint::Min(MIN_TIMELINE_WIDTH),
            Constraint::Length(detail_width),
        ])
        .split(root[0]);

    draw_sidebar(frame, app, body[0]);
    draw_messages(frame, app, body[1]);
    draw_side_panel(frame, app, body[2]);
    draw_composer(frame, app, root[1]);
    draw_status(frame, app, root[2]);

    if app.focus == Focus::Composer && app.composer_completion.is_some() {
        draw_completion_popup(frame, app, root[1]);
    }
    if app.focus == Focus::CommandPalette {
        draw_command_palette(frame, app);
    }
    if let Some(confirm) = &app.confirm {
        draw_confirm(frame, confirm);
    }
}

fn layout_panel_widths(total_width: u16, app: &App) -> (u16, u16) {
    let fixed_budget = total_width.saturating_sub(MIN_TIMELINE_WIDTH);
    if fixed_budget == 0 {
        return (0, 0);
    }
    if fixed_budget < MIN_SIDEBAR_WIDTH.saturating_add(MIN_DETAIL_WIDTH) {
        let sidebar = app.sidebar_width.min(fixed_budget);
        return (sidebar, fixed_budget.saturating_sub(sidebar));
    }

    let sidebar = app
        .sidebar_width
        .clamp(MIN_SIDEBAR_WIDTH, fixed_budget - MIN_DETAIL_WIDTH);
    let detail_budget = fixed_budget.saturating_sub(sidebar);
    let detail = app.detail_width.clamp(MIN_DETAIL_WIDTH, detail_budget);
    (sidebar, detail)
}

fn layout_sidebar_agent_height(total_height: u16, app: &App) -> u16 {
    let fixed_budget = total_height.saturating_sub(MIN_MESSAGE_DETAIL_HEIGHT);
    if fixed_budget == 0 {
        return 0;
    }
    if fixed_budget < MIN_AGENT_PANEL_HEIGHT {
        return fixed_budget;
    }
    app.agent_panel_height
        .clamp(MIN_AGENT_PANEL_HEIGHT, fixed_budget)
}

fn draw_sidebar(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let agent_panel_height = layout_sidebar_agent_height(area.height, app);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(MIN_MESSAGE_DETAIL_HEIGHT),
            Constraint::Length(agent_panel_height),
        ])
        .split(area);

    draw_channels(frame, app, chunks[0]);
    if agent_panel_height > 0 {
        draw_agents_list(frame, app, chunks[1]);
    }
}

fn draw_chat_composer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let dim = Style::new().fg(Color::DarkGray);
    let (title, placeholder) = if app.timeline_mode == TimelineMode::Pulse {
        if app.pulse_reply_target.is_some() {
            ("Pulse Reply", "Type Pulse reply, Enter to publish")
        } else {
            ("Pulse Note", "Type Pulse note, Enter to publish")
        }
    } else if app.edit_target.is_some() {
        ("Edit Message", "Type updated content, Enter to save")
    } else {
        (
            "Composer",
            "Type a message, Enter to send, Alt+Enter for newline",
        )
    };

    let mut lines: Vec<Line> = Vec::new();
    if let Some(id) = &app.edit_target {
        lines.push(Line::from(Span::styled(
            format!("✎ editing {} — Esc cancels", short_id(id)),
            Style::new().fg(Color::Yellow),
        )));
    } else if let Some(id) = &app.pulse_reply_target {
        lines.push(Line::from(Span::styled(
            format!("↳ replying to Pulse {}", short_id(id)),
            Style::new().fg(Color::Cyan),
        )));
    } else if let Some(id) = &app.thread_root {
        lines.push(Line::from(Span::styled(
            format!("↳ replying in thread {}", short_id(id)),
            Style::new().fg(Color::Cyan),
        )));
    }

    if app.composer.is_empty() {
        lines.push(Line::from(Span::styled(placeholder, dim)));
    } else {
        for line in app.composer.split('\n') {
            lines.push(Line::from(line.to_string()));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel_block(title, true)),
        area,
    );
    if let Some(position) = composer_cursor_position(app, area) {
        frame.set_cursor_position(position);
    }
}

fn composer_cursor_position(app: &App, area: Rect) -> Option<Position> {
    if area.width <= 2 || area.height <= 2 {
        return None;
    }
    let mut cursor = app.composer_cursor.min(app.composer.len());
    while cursor > 0 && !app.composer.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let prefix = &app.composer[..cursor];
    let context_lines = u16::from(
        app.edit_target.is_some() || app.pulse_reply_target.is_some() || app.thread_root.is_some(),
    );
    let line = prefix.chars().filter(|ch| *ch == '\n').count() as u16;
    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() as u16;
    let inner_right = area.x.saturating_add(area.width.saturating_sub(2));
    let x = area
        .x
        .saturating_add(1)
        .saturating_add(column)
        .min(inner_right);
    let y = area
        .y
        .saturating_add(1)
        .saturating_add(context_lines)
        .saturating_add(line);
    if y >= area.y.saturating_add(area.height.saturating_sub(1)) {
        return None;
    }
    Some(Position { x, y })
}

fn draw_completion_popup(frame: &mut Frame<'_>, app: &App, composer_area: Rect) {
    use ratatui::widgets::Clear;

    let Some(state) = &app.composer_completion else {
        return;
    };
    if state.matches.is_empty() {
        return;
    }
    let height = (state.matches.len() as u16 + 2).min(10);
    let width = 46u16.min(frame.area().width);
    let y = composer_area.y.saturating_sub(height);
    let area = Rect {
        x: composer_area.x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);

    let title = match state.kind {
        crate::app::CompletionKind::Mention => "Mention",
        crate::app::CompletionKind::Channel => "Channel",
        crate::app::CompletionKind::Emoji => "Emoji",
    };
    let items: Vec<ListItem> = state
        .matches
        .iter()
        .map(|item| ListItem::new(Line::from(item.display.clone())))
        .collect();
    let list = selectable_list(items, panel_block(title, true));
    let mut list_state = list_state(state.selected, state.matches.len());
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// A centered rectangle `width` x `height` (clamped to the area).
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn draw_command_palette(frame: &mut Frame<'_>, app: &App) {
    use ratatui::widgets::Clear;

    let area = centered_rect(64, 18, frame.area());
    frame.render_widget(Clear, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);

    let query = if app.palette_query.is_empty() {
        "Type to filter commands…".to_string()
    } else {
        app.palette_query.clone()
    };
    frame.render_widget(
        Paragraph::new(query).block(panel_block("Command Palette", true)),
        layout[0],
    );

    let commands = app.palette_filtered();
    let items: Vec<ListItem> = if commands.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "No matching commands",
            Style::new().fg(Color::DarkGray),
        )))]
    } else {
        commands
            .iter()
            .map(|command| {
                let mut spans = vec![Span::styled(
                    command.label,
                    if command.enabled {
                        Style::new().fg(Color::White)
                    } else {
                        Style::new().fg(Color::DarkGray)
                    },
                )];
                if let Some(key) = command.keybinding {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        format!("[{key}]"),
                        Style::new().fg(Color::Cyan),
                    ));
                }
                if let Some(reason) = command.disabled_reason {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        format!("— {reason}"),
                        Style::new().fg(Color::Red),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect()
    };
    let list = selectable_list(items, panel_block("Commands", true));
    let mut state = list_state(app.palette_selected, commands.len().max(1));
    frame.render_stateful_widget(list, layout[1], &mut state);
}

fn draw_confirm(frame: &mut Frame<'_>, confirm: &crate::app::ConfirmState) {
    use ratatui::widgets::Clear;

    let area = centered_rect(60, 8, frame.area());
    frame.render_widget(Clear, area);

    let mut lines: Vec<Line> = confirm
        .body
        .lines()
        .map(|line| Line::from(line.to_string()))
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            confirm.confirm_label.clone(),
            Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        Span::raw("    "),
        Span::styled(confirm.cancel_label.clone(), Style::new().fg(Color::Red)),
    ]));

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel_block(&confirm.title, true)),
        area,
    );
}

fn draw_channels(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let channels = if app.focus == Focus::ChannelSearch {
        app.channel_search_results.as_slice()
    } else {
        app.channels.as_slice()
    };
    let selected = if app.focus == Focus::ChannelSearch {
        app.selected_channel_search
    } else {
        app.selected_channel
    };
    let items: Vec<ListItem> = channels
        .iter()
        .map(|channel| {
            let prefix = match channel.kind {
                ConversationKind::Channel => "#",
                ConversationKind::DirectMessage => "@",
            };
            let suffix = if channel.archived { " archived" } else { "" };
            let draft = if app.has_channel_draft(&channel.id) {
                " draft"
            } else {
                ""
            };
            let starred = if app.channel_is_starred(&channel.id) {
                " star"
            } else {
                ""
            };
            let muted = if app.channel_is_muted(&channel.id) {
                " muted"
            } else {
                ""
            };
            let section = app
                .channel_section_name(&channel.id)
                .map(|name| format!(" [{name}]"))
                .unwrap_or_default();
            let unread = if app.channel_has_unread(&channel.id) {
                " new"
            } else {
                ""
            };
            ListItem::new(Line::from(vec![
                Span::raw(prefix),
                Span::raw(channel.name.clone()),
                Span::styled(unread, Style::new().fg(Color::Red)),
                Span::styled(starred, Style::new().fg(Color::LightYellow)),
                Span::styled(muted, Style::new().fg(Color::DarkGray)),
                Span::styled(section, Style::new().fg(Color::Blue)),
                Span::styled(suffix, Style::new().fg(Color::DarkGray)),
                Span::styled(draft, Style::new().fg(Color::Yellow)),
            ]))
        })
        .collect();

    let title = if app.focus == Focus::ChannelSearch {
        if app.channel_search_last_query.trim().is_empty() {
            "Channel Search".to_string()
        } else {
            format!("Channel Search: {}", app.channel_search_last_query)
        }
    } else {
        match app.channel_scope {
            ChannelScope::Conversations => "Conversations",
            ChannelScope::OpenChannels => "Open Channels",
        }
        .to_string()
    };
    let block = panel_block(
        &title,
        matches!(app.focus, Focus::Sidebar | Focus::ChannelSearch),
    );
    let list = selectable_list(items, block);
    let mut state = list_state(selected, channels.len());
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_messages(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let (title, messages, selected) = match app.timeline_mode {
        TimelineMode::Channel => {
            let title = app
                .active_channel()
                .map(|channel| {
                    if app.thread_root.is_some() {
                        format!("#{} thread", channel.name)
                    } else {
                        format!("#{}", channel.name)
                    }
                })
                .unwrap_or_else(|| "Timeline".to_string());
            (title, app.messages.as_slice(), app.selected_message)
        }
        TimelineMode::Search => {
            let title = if app.search_query.trim().is_empty() {
                "Search".to_string()
            } else {
                format!("Search: {}", app.search_query.trim())
            };
            (
                title,
                app.search_results.as_slice(),
                app.selected_search_result,
            )
        }
        TimelineMode::Feed => (
            format!("Feed: {}", app.feed_filter.label()),
            app.feed.as_slice(),
            app.selected_feed,
        ),
        TimelineMode::Pulse => (
            format!("Pulse: {}", app.pulse_source.label()),
            app.pulse.as_slice(),
            app.selected_pulse,
        ),
    };
    let items: Vec<ListItem> = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let author = app.author_label(&message.pubkey);
            if index == selected {
                return ListItem::new(crate::render::render_selected_timeline_item(
                    &author,
                    &message.content,
                    area.width,
                ));
            }

            let mut spans = vec![Span::styled(
                format!(" {author} "),
                Style::new().fg(Color::Cyan),
            )];
            spans.extend(crate::render::render_message_preview(&message.content, 120));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = selectable_list(
        items,
        panel_block(
            &title,
            matches!(app.focus, Focus::Timeline | Focus::Feed | Focus::Pulse),
        ),
    );
    let mut state = list_state(selected, messages.len());
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_side_panel(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.focus == Focus::Agents {
        draw_agent_detail(frame, app, area);
        return;
    }
    if matches!(app.focus, Focus::Timeline | Focus::Feed | Focus::Pulse) {
        draw_message_detail(frame, app, area);
        return;
    }
    if matches!(app.focus, Focus::Canvas | Focus::CanvasEdit) {
        draw_canvas(frame, app, area);
        return;
    }
    if app.focus == Focus::Diff {
        draw_diff(frame, app, area);
        return;
    }
    if app.focus == Focus::Workflows {
        draw_workflows(frame, app, area);
        return;
    }
    if matches!(app.focus, Focus::Notes | Focus::NoteEdit) {
        draw_notes(frame, app, area);
        return;
    }
    if matches!(
        app.focus,
        Focus::Profile | Focus::ProfileEdit | Focus::ProfileAvatarUpload
    ) {
        draw_profile(frame, app, area);
        return;
    }
    if matches!(app.focus, Focus::Contacts | Focus::ContactAdd) {
        draw_contacts(frame, app, area);
        return;
    }
    if matches!(app.focus, Focus::UserLookup | Focus::UserProfile) {
        draw_user_profile(frame, app, area);
        return;
    }
    if matches!(
        app.focus,
        Focus::Repos | Focus::RepoCreate | Focus::RepoIssueCreate | Focus::RepoPatchCreate
    ) {
        draw_repos(frame, app, area);
        return;
    }
    if matches!(
        app.focus,
        Focus::Memory | Focus::MemoryEdit | Focus::MemoryPatch
    ) {
        draw_memory(frame, app, area);
        return;
    }
    if matches!(app.focus, Focus::Workspaces | Focus::WorkspaceAdd) {
        draw_workspaces(frame, app, area);
        return;
    }
    if matches!(
        app.focus,
        Focus::Emoji | Focus::EmojiEdit | Focus::EmojiImport
    ) {
        draw_emoji(frame, app, area);
        return;
    }
    if app.focus == Focus::ChannelSearch {
        draw_channel_search_detail(frame, app, area);
        return;
    }
    if app.focus == Focus::Help {
        draw_help(frame, area);
        return;
    }
    if app.focus == Focus::Sidebar {
        draw_channel_detail(frame, app, area);
        return;
    }

    let feed_items: Vec<ListItem> = app
        .feed
        .iter()
        .take(12)
        .map(|message| {
            ListItem::new(Line::from(crate::render::render_message_preview(
                &message.content,
                120,
            )))
        })
        .collect();
    frame.render_widget(
        List::new(feed_items).block(panel_block(
            &format!("Feed: {}", app.feed_filter.label()),
            false,
        )),
        area,
    );
}

fn draw_agents_list(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let agent_items: Vec<ListItem> = app
        .acp
        .agents()
        .map(|agent| {
            let status = if !agent.runtime.available {
                "missing"
            } else {
                match agent.status {
                    AgentStatus::Stopped => "stopped",
                    AgentStatus::Running => "running",
                    AgentStatus::Exited => "exited",
                }
            };
            ListItem::new(Line::from(vec![
                Span::styled(agent.runtime.label.as_str(), Style::new().fg(Color::Yellow)),
                Span::raw(" "),
                Span::raw(status),
            ]))
        })
        .collect();
    let list = selectable_list(
        agent_items,
        panel_block("ACP Agents", app.focus == Focus::Agents),
    );
    let mut state = list_state(app.selected_agent, app.acp.agents().count());
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_help(frame: &mut Frame<'_>, area: Rect) {
    let text = [
        "Global",
        ": or Ctrl-P: command palette (fuzzy search every action)",
        "Tab / Shift+Tab: move focus",
        "Alt+Left / Alt+Right: navigate back / forward",
        "r: refresh",
        "?: show help",
        "W: workspace switcher",
        "q: quit",
        "{ / }: shrink / grow the left sidebar",
        ", / .: shrink / grow the right detail panel",
        "Alt+Up / Alt+Down: shrink / grow the ACP Agents section",
        "0: reset panel sizes",
        "Destructive actions (delete, leave, archive, remove) prompt to confirm.",
        "",
        "Conversations",
        "o: toggle joined/open channels",
        "O: search channels",
        "n: create channel (Tab fields, F2 type, F3 visibility)",
        "m: open DM",
        "j / l: join or leave selected channel",
        "h: hide selected DM",
        "S / M: star or mute selected conversation",
        "F4: cycle channel add policy",
        "A / V: assign or remove selected channel section",
        "E / D: edit channel name or description",
        "t / p: set topic or purpose",
        "u / x: add or remove member",
        "z / Z: archive or unarchive",
        "Delete: delete selected channel",
        "",
        "Messages",
        "c: compose (Alt+Enter newline, @/#/: autocomplete, Up edits last own)",
        "B: attach files",
        "I: send code diff",
        "/: search messages",
        "PageUp: load older history",
        "PageDown / Ctrl-D: scroll selected message detail down",
        "Ctrl-U / Home: scroll selected message detail up / top",
        "End: jump selected message detail to bottom",
        "Enter: open selected thread",
        "e / d: edit or delete selected message",
        "+ / -: react or remove reaction",
        "[ / ]: forum downvote or upvote",
        "",
        "People And Profile",
        "P: profile",
        "A in Profile: upload avatar",
        "s in Profile: cycle presence",
        "C: contacts",
        "A / D in Contacts: add or remove contact",
        "U: view selected user or lookup profile",
        "",
        "Knowledge And Work",
        "f: feed",
        "F: cycle feed type",
        "T: Pulse",
        "S in Pulse: cycle source",
        "R in Pulse: reply",
        "v: canvas",
        "w: workflows (Enter/I trigger, G/X approve/deny, A/E/D YAML)",
        "N: long-form notes",
        "G: repositories",
        "M: selected agent memory (A create, E edit, D remove)",
        "Y: custom emoji",
        "",
        "Agents",
        "a: ACP runtimes and managed agents",
        "Enter in Agents: start or stop selected agent",
        "@ in Agents: mention selected managed agent",
        "u in Agents: add selected managed agent to selected channel as bot",
        "A / D in Agents: create or delete managed agent",
        "s in Agents: toggle start-on-launch",
        "F2 / F3 / F4 in Create Agent: launch, respond-to, and threading options",
        "",
        "Esc returns from focused panels and cancels editors.",
    ]
    .join("\n");

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Help", true)),
        area,
    );
}

fn draw_workspaces(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let mut text = String::new();
    for (index, workspace) in app.workspace_config.workspaces.iter().enumerate() {
        let selected = if index == app.selected_workspace {
            ">"
        } else {
            " "
        };
        let active = if workspace.id == app.workspace_config.active_id {
            "*"
        } else {
            " "
        };
        text.push_str(&format!(
            "{selected}{active} {}  {}\n",
            workspace.name,
            compact_text(&workspace.relay, 42)
        ));
    }
    if text.is_empty() {
        text.push_str("No workspaces configured\n");
    }
    text.push_str(
        "\nEnter switches to selected workspace\nA adds a workspace\nD removes selected inactive workspace\nEsc returns",
    );

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Workspaces", true)),
        area,
    );
}

fn draw_contacts(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.contacts.is_empty() {
        frame.render_widget(
            Paragraph::new("No contacts found\n\nA adds a contact")
                .block(panel_block("Contacts", true)),
            area,
        );
        return;
    }

    let mut text = String::new();
    for (index, contact) in app.contacts.iter().take(18).enumerate() {
        let marker = if index == app.selected_contact {
            ">"
        } else {
            " "
        };
        let label = if contact.petname.trim().is_empty() {
            short_id(&contact.pubkey).to_string()
        } else {
            contact.petname.clone()
        };
        text.push_str(&format!("{marker} {label} {}", short_id(&contact.pubkey)));
        if !contact.relay_url.trim().is_empty() {
            text.push_str(&format!(" {}", compact_text(&contact.relay_url, 34)));
        }
        text.push('\n');
    }
    if app.contacts.len() > 18 {
        text.push_str(&format!("... {} more\n", app.contacts.len() - 18));
    }
    text.push_str("\nEnter opens a DM\nA adds a contact\nD removes selected contact");
    text.push_str("\nU views selected profile");

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Contacts", true)),
        area,
    );
}

fn draw_user_profile(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(profile) = app.viewed_profile.as_ref() else {
        frame.render_widget(
            Paragraph::new("Type a pubkey or display name below\n\nEnter searches")
                .wrap(Wrap { trim: false })
                .block(panel_block("User Profile", true)),
            area,
        );
        return;
    };

    let display_name = if profile.display_name.trim().is_empty() {
        &profile.name
    } else {
        &profile.display_name
    };
    let text = format!(
        "{}\npubkey: {}\nname: {}\nNIP-05: {}\navatar: {}\n\nabout:\n{}\n\nEnter opens a DM\nEsc returns",
        empty_dash(display_name),
        short_id(&profile.pubkey),
        empty_dash(&profile.name),
        empty_dash(&profile.nip05),
        empty_dash(&profile.picture),
        compact_text(&profile.about, 900)
    );

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("User Profile", app.focus == Focus::UserProfile)),
        area,
    );
}

fn draw_channel_search_detail(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(channel) = app.channel_search_results.get(app.selected_channel_search) else {
        frame.render_widget(
            Paragraph::new("Type a channel name below\n\nEnter searches")
                .wrap(Wrap { trim: false })
                .block(panel_block("Channel Search", true)),
            area,
        );
        return;
    };

    let text = format!(
        "#{}\nid: {}\ntype: {}\nvisibility: {}\narchived: {}\nowner: {}\n\n{}\n\nEnter opens selected channel",
        channel.name,
        channel.id,
        empty_dash(&channel.channel_type),
        empty_dash(&channel.visibility),
        channel.archived,
        empty_dash(short_id(&channel.owner_pubkey)),
        compact_text(&channel.description, 700),
    );
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Channel Search", true)),
        area,
    );
}

fn draw_repos(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.focus == Focus::RepoCreate {
        let text = format!(
            "New repo announcement\nid: {}\nname: {}\ndescription: {}\nclone URLs: {}\nweb URL: {}\nrelays: {}\n\nTab cycles fields\nEnter saves\nEsc cancels",
            empty_dash(app.repo_id.trim()),
            empty_dash(app.repo_name.trim()),
            empty_dash(app.repo_description.trim()),
            empty_dash(app.repo_clone_urls.trim()),
            empty_dash(app.repo_web_url.trim()),
            empty_dash(app.repo_relays.trim())
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block("Repos", true)),
            area,
        );
        return;
    }
    if app.focus == Focus::RepoIssueCreate {
        let repo_name = app
            .repos
            .get(app.selected_repo)
            .map(|repo| repo.name.as_str())
            .unwrap_or("-");
        let text = format!(
            "New issue for {repo_name}\ntitle: {}\nlabels: {}\nbody: {}\n\nTab cycles fields\nEnter saves\nEsc cancels",
            empty_dash(app.repo_issue_title.trim()),
            empty_dash(app.repo_issue_labels.trim()),
            empty_dash(app.repo_issue_content.trim())
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block("Repo Issue", true)),
            area,
        );
        return;
    }
    if app.focus == Focus::RepoPatchCreate {
        let repo_name = app
            .repos
            .get(app.selected_repo)
            .map(|repo| repo.name.as_str())
            .unwrap_or("-");
        let text = format!(
            "New patch for {repo_name}\ncommit: {}\nparent: {}\npatch: {}\n\nTab cycles fields\nEnter saves\nEsc cancels",
            empty_dash(app.repo_patch_commit.trim()),
            empty_dash(app.repo_patch_parent_commit.trim()),
            compact_text(app.repo_patch_content.trim(), 900)
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block("Repo Patch", true)),
            area,
        );
        return;
    }

    let Some(repo) = app.repos.get(app.selected_repo) else {
        frame.render_widget(
            Paragraph::new("No repo announcements found\n\nA announces a repo")
                .block(panel_block("Repos", true)),
            area,
        );
        return;
    };

    let clone_urls = if repo.clone_urls.is_empty() {
        "-".to_string()
    } else {
        repo.clone_urls.join("\n  ")
    };
    let relays = if repo.relays.is_empty() {
        "-".to_string()
    } else {
        repo.relays.join("\n  ")
    };
    let issues = if app.repo_issues.is_empty() {
        "  -".to_string()
    } else {
        app.repo_issues
            .iter()
            .take(6)
            .map(|issue| {
                let labels = if issue.labels.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", issue.labels.join(","))
                };
                format!(
                    "  {} {}{}",
                    short_id(&issue.id),
                    compact_text(&issue.title, 80),
                    labels
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let patches = if app.repo_patches.is_empty() {
        "  -".to_string()
    } else {
        app.repo_patches
            .iter()
            .take(6)
            .map(|patch| {
                let marker = if patch.root {
                    " root"
                } else if patch.root_revision {
                    " revision"
                } else {
                    ""
                };
                format!(
                    "  {}{} {}",
                    short_id(&patch.id),
                    marker,
                    compact_text(patch.content.lines().next().unwrap_or("patch"), 80)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let text = format!(
        "{}\nid: {}\nowner: {}\ncreated: {}\nweb: {}\n\nclone:\n  {}\n\nrelays:\n  {}\n\ndescription:\n{}\n\nissues:\n{}\n\npatches:\n{}\n\nA announces or updates a repo\nI creates an issue\nP publishes a patch",
        repo.name,
        repo.dtag,
        short_id(&repo.owner),
        repo.created_at,
        repo.web_url.as_deref().unwrap_or("-"),
        clone_urls,
        relays,
        compact_text(&repo.description, 500),
        issues,
        patches
    );

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Repos", true)),
        area,
    );
}

fn draw_memory(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let title = if app.memory_agent_name.trim().is_empty() {
        "Memory".to_string()
    } else {
        format!("Memory: {}", app.memory_agent_name)
    };
    if app.focus == Focus::MemoryEdit {
        let mode = if app.memory_edit_existing {
            "Edit memory"
        } else {
            "New memory"
        };
        let text = format!(
            "{mode}\nagent: {}\nslug: {}\nvalue: {}\n\nTab cycles fields\nEnter saves\nEsc cancels",
            short_id(&app.memory_agent_pubkey),
            empty_dash(app.memory_slug.trim()),
            compact_text(app.memory_value.trim(), 1100)
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block(&title, true)),
            area,
        );
        return;
    }
    if app.focus == Focus::MemoryPatch {
        let text = format!(
            "Patch memory\nagent: {}\nslug: {}\nbase hash: {}\npatch: {}\n\nTab cycles fields\nEnter applies patch\nEsc cancels",
            short_id(&app.memory_agent_pubkey),
            empty_dash(app.memory_slug.trim()),
            empty_dash(app.memory_patch_base_hash.trim()),
            compact_text(app.memory_patch_content.trim(), 1000)
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block(&title, true)),
            area,
        );
        return;
    }
    let Some(memory) = app.memories.get(app.selected_memory) else {
        frame.render_widget(
            Paragraph::new("No memories found for selected managed agent\n\nA creates a memory")
                .block(panel_block(&title, true)),
            area,
        );
        return;
    };

    let refs = extract_memory_refs(&memory.value);
    let refs_text = if refs.is_empty() {
        "-".to_string()
    } else {
        refs.join(", ")
    };
    let mut text = format!(
        "agent: {}\nslug: {}\nevent: {}\ncreated: {}\nrefs: {}\n\n{}",
        short_id(&app.memory_agent_pubkey),
        memory.slug,
        empty_dash(short_id(&memory.event_id)),
        memory.created_at,
        refs_text,
        compact_text(&memory.value, 1200)
    );
    if app.memories.len() > 1 {
        text.push_str("\n\nslugs");
        for (index, entry) in app.memories.iter().take(12).enumerate() {
            let marker = if index == app.selected_memory {
                ">"
            } else {
                " "
            };
            text.push_str(&format!("\n{marker} {}", entry.slug));
        }
        if app.memories.len() > 12 {
            text.push_str(&format!("\n... {} more", app.memories.len() - 12));
        }
    }
    text.push_str("\n\nA creates  E edits  H hashes  P patches  D removes selected memory");

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block(&title, true)),
        area,
    );
}

fn draw_emoji(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.focus == Focus::EmojiEdit {
        let text = format!(
            "Custom emoji\nshortcode: {}\nurl: {}\n\nTab cycles fields\nEnter saves\nEsc cancels",
            empty_dash(app.emoji_shortcode.trim()),
            empty_dash(app.emoji_url.trim())
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block("Emoji", true)),
            area,
        );
        return;
    }
    if app.focus == Focus::EmojiImport {
        let mode = if app.emoji_import_replace {
            "replace existing set"
        } else {
            "merge with existing set"
        };
        let text = format!(
            "Import emoji\nfile: {}\nmode: {}\n\nF2 toggles merge/replace\nEnter imports\nEsc cancels",
            empty_dash(app.emoji_import_path.trim()),
            mode
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block("Emoji", true)),
            area,
        );
        return;
    }

    let other = app.workspace_other_emoji();
    let mut text = format!("My emoji ({})\n", app.own_emoji.len());
    if app.own_emoji.is_empty() {
        text.push_str("none\n");
    } else {
        for (index, emoji) in app.own_emoji.iter().take(12).enumerate() {
            let marker = if index == app.selected_emoji {
                ">"
            } else {
                " "
            };
            text.push_str(&format!(
                "{marker} :{}: {}\n",
                emoji.shortcode,
                compact_text(&emoji.url, 44)
            ));
        }
        if app.own_emoji.len() > 12 {
            text.push_str(&format!("... {} more\n", app.own_emoji.len() - 12));
        }
    }

    text.push_str(&format!("\nWorkspace emoji ({})\n", other.len()));
    if other.is_empty() {
        text.push_str("none\n");
    } else {
        for (offset, emoji) in other.iter().take(12).enumerate() {
            let index = app.own_emoji.len() + offset;
            let marker = if index == app.selected_emoji {
                ">"
            } else {
                " "
            };
            text.push_str(&format!(
                "{marker} :{}: {}\n",
                emoji.shortcode,
                compact_text(&emoji.url, 44)
            ));
        }
        if other.len() > 12 {
            text.push_str(&format!("... {} more\n", other.len() - 12));
        }
    }
    text.push_str(
        "\nEnter reacts with selected emoji\nA adds/updates your emoji\nI imports JSON\nX exports workspace JSON\nD removes selected own emoji",
    );

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Emoji", true)),
        area,
    );
}

fn draw_profile(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(profile) = app.profile.as_ref() else {
        frame.render_widget(
            Paragraph::new("No profile found\nPress Enter to set display name")
                .wrap(Wrap { trim: false })
                .block(panel_block("Profile", true)),
            area,
        );
        return;
    };

    let display_name = if profile.display_name.trim().is_empty() {
        &profile.name
    } else {
        &profile.display_name
    };
    let rows = [
        (
            ProfileField::DisplayName,
            "display name",
            empty_dash(display_name),
        ),
        (ProfileField::About, "about", empty_dash(&profile.about)),
        (
            ProfileField::Picture,
            "avatar",
            empty_dash(&profile.picture),
        ),
        (ProfileField::Nip05, "NIP-05", empty_dash(&profile.nip05)),
    ];
    let presence = app
        .presence
        .as_ref()
        .map(|presence| presence.status.as_str())
        .or_else(|| app.last_presence_status.map(|status| status.as_str()))
        .unwrap_or("not set");
    let presence_updated = app
        .presence
        .as_ref()
        .map(|presence| presence.updated_at)
        .unwrap_or_default();
    let mut text = format!(
        "pubkey: {}\npresence: {presence}\n",
        short_id(&profile.pubkey)
    );
    if presence_updated > 0 {
        text.push_str(&format!("presence updated: {presence_updated}\n"));
    }
    for (field, label, value) in rows {
        let marker = if field == app.selected_profile_field {
            ">"
        } else {
            " "
        };
        text.push_str(&format!("\n{marker} {label}: {value}"));
    }
    text.push_str("\n\nEnter edits selected field\nA uploads avatar\ns cycles presence");

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Profile", true)),
        area,
    );
}

fn draw_notes(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.focus == Focus::NoteEdit {
        let text = format!(
            "{} long-form note\nslug: {}\ntitle: {}\nsummary: {}\ntags: {}\ncontent: {}\n\nTab cycles fields\nEnter saves\nEsc cancels",
            if app.note_edit_existing { "Edit" } else { "New" },
            empty_dash(app.note_name.trim()),
            empty_dash(app.note_title.trim()),
            empty_dash(app.note_summary.trim()),
            empty_dash(app.note_tags.trim()),
            compact_text(app.note_content.trim(), 700)
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block("Notes", true)),
            area,
        );
        return;
    }

    let Some(note) = app.notes.get(app.selected_note) else {
        frame.render_widget(
            Paragraph::new("No notes found\n\nA creates a note\nS toggles mine/all")
                .block(panel_block("Notes", true)),
            area,
        );
        return;
    };

    let title = if note.title.trim().is_empty() {
        "(untitled)"
    } else {
        note.title.as_str()
    };
    let tags = if note.tags.is_empty() {
        "-".to_string()
    } else {
        note.tags.join(", ")
    };
    let summary = note.summary.as_deref().unwrap_or("-");
    let text = format!(
        "{}\nsource: {}\nslug: {}\nauthor: {}\nupdated: {}\ntags: {}\nsummary: {}\n\n{}",
        title,
        app.notes_source.label(),
        note.slug,
        short_id(&note.pubkey),
        note.updated_at,
        tags,
        summary,
        compact_text(&note.content, 1200)
    );
    let text = format!("{text}\n\nA creates  E edits  S toggles mine/all  D deletes");

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Notes", true)),
        area,
    );
}

fn draw_workflows(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.focus == Focus::WorkflowEdit {
        let mode = if app.workflow_edit_existing {
            "Edit workflow"
        } else {
            "New workflow"
        };
        let text = format!(
            "{mode}\nchannel: {}\nyaml: {}\n\nEnter saves\nEsc cancels",
            empty_dash(short_id(&app.workflow_channel_id)),
            compact_text(app.workflow_yaml.trim(), 1100)
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block("Workflows", true)),
            area,
        );
        return;
    }
    if app.focus == Focus::WorkflowInputs {
        let text = format!(
            "Trigger workflow\nworkflow: {}\ninputs: {}\n\nEnter triggers\nEsc cancels",
            app.workflow_edit_id.as_deref().map(short_id).unwrap_or("-"),
            compact_text(app.workflow_inputs.trim(), 1100)
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block("Workflows", true)),
            area,
        );
        return;
    }
    if app.focus == Focus::WorkflowApproval {
        let decision = if app.workflow_approval_approved {
            "Approve"
        } else {
            "Deny"
        };
        let text = format!(
            "{decision} workflow step\ntoken: {}\nnote: {}\n\nTab cycles fields\nEnter submits\nEsc cancels",
            empty_dash(app.workflow_approval_token.trim()),
            empty_dash(app.workflow_approval_note.trim())
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(panel_block("Workflows", true)),
            area,
        );
        return;
    }

    let Some(workflow) = app.workflows.get(app.selected_workflow) else {
        frame.render_widget(
            Paragraph::new("No workflows for this channel\n\nA creates a workflow")
                .block(panel_block("Workflows", true)),
            area,
        );
        return;
    };

    let detail = app.selected_workflow_detail.as_ref();
    let workflow_id = detail
        .map(|detail| detail.workflow_id.as_str())
        .unwrap_or(&workflow.workflow_id);
    let workflow_author = detail
        .map(|detail| detail.pubkey.as_str())
        .unwrap_or(&workflow.pubkey);
    let workflow_created = detail
        .map(|detail| detail.created_at)
        .unwrap_or(workflow.created_at);
    let workflow_content = detail
        .map(|detail| detail.content.as_str())
        .unwrap_or(&workflow.content);
    let mut text = format!(
        "workflow: {}\nauthor: {}\ncreated: {}\n\n{}",
        short_id(workflow_id),
        short_id(workflow_author),
        workflow_created,
        compact_text(workflow_content, 900)
    );
    text.push_str("\n\nruns");
    if app.workflow_runs.is_empty() {
        text.push_str("\nnone");
    } else {
        for run in app.workflow_runs.iter().take(8) {
            text.push_str(&format!(
                "\n{} kind:{} {}",
                short_id(&run.event_id),
                run.kind,
                compact_text(&run.content, 80)
            ));
        }
        if app.workflow_runs.len() > 8 {
            text.push_str(&format!("\n... {} more", app.workflow_runs.len() - 8));
        }
    }
    text.push_str(
        "\n\nEnter triggers  R refreshes detail  I triggers with inputs  G approves  X denies  A creates  E edits  D deletes",
    );

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Workflows", true)),
        area,
    );
}

fn draw_canvas(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let text = if app.focus == Focus::CanvasEdit {
        if app.canvas_draft.is_empty() {
            "Canvas is empty. Type content below, Enter to save.".to_string()
        } else {
            app.canvas_draft.clone()
        }
    } else if app.canvas_content.is_empty() {
        "No canvas set for this channel. Press Enter to create one.".to_string()
    } else {
        app.canvas_content.clone()
    };

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Canvas", app.focus == Focus::Canvas)),
        area,
    );
}

fn draw_diff(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let text = format!(
        "Code diff\nrepo: {}\ncommit: {}\nfile: {}\ndescription: {}\ndiff: {}\n\nTab cycles fields\nEnter sends\nEsc cancels",
        empty_dash(app.diff_repo.trim()),
        empty_dash(app.diff_commit.trim()),
        empty_dash(app.diff_file.trim()),
        empty_dash(app.diff_description.trim()),
        compact_text(app.diff_content.trim(), 900)
    );
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Diff", true)),
        area,
    );
}

fn draw_channel_detail(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(channel) = app
        .selected_channel_detail
        .as_ref()
        .or_else(|| app.channels.get(app.selected_channel))
    else {
        frame.render_widget(
            Paragraph::new("No channel selected").block(panel_block("Channel", false)),
            area,
        );
        return;
    };

    if channel.kind == ConversationKind::DirectMessage {
        frame.render_widget(
            Paragraph::new(format!(
                "Direct message\n{}\n{}\n\nu adds a member to this DM",
                channel.name, channel.description
            ))
            .wrap(Wrap { trim: false })
            .block(panel_block("Conversation", false)),
            area,
        );
        return;
    }

    let mut text = format!(
        "#{}\nid: {}\ntype: {}\nvisibility: {}\narchived: {}\nowner: {}\nabout: {}",
        channel.name,
        short_id(&channel.id),
        empty_dash(&channel.channel_type),
        empty_dash(&channel.visibility),
        if channel.archived { "yes" } else { "no" },
        empty_dash(short_id(&channel.owner_pubkey)),
        empty_dash(&channel.description),
    );
    if !channel.topic.trim().is_empty() {
        text.push_str(&format!("\ntopic: {}", channel.topic));
    }
    if !channel.purpose.trim().is_empty() {
        text.push_str(&format!("\npurpose: {}", channel.purpose));
    }

    text.push_str("\n\nmembers");
    if app.channel_members.is_empty() {
        text.push_str("\nnone loaded");
    } else {
        for member in app.channel_members.iter().take(12) {
            text.push_str(&format!(
                "\n{} {}",
                short_id(&member.pubkey),
                empty_dash(&member.role)
            ));
        }
        if app.channel_members.len() > 12 {
            text.push_str(&format!("\n... {} more", app.channel_members.len() - 12));
        }
    }

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Channel", false)),
        area,
    );
}

fn draw_message_detail(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(message) = app.selected_timeline_message() else {
        frame.render_widget(
            Paragraph::new("No message selected").block(panel_block("Message", false)),
            area,
        );
        return;
    };

    let reactions = if app.selected_reactions.is_empty() {
        "none".to_string()
    } else {
        app.selected_reactions
            .iter()
            .map(|reaction| format!("{} {}", reaction.emoji, reaction.count))
            .collect::<Vec<_>>()
            .join("  ")
    };
    let channel = if message.channel_id.is_empty() {
        app.active_channel()
            .map(|channel| channel.id)
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        message.channel_id.clone()
    };
    let meta = Style::new().fg(Color::DarkGray);
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("event ", meta),
            Span::raw(short_id(&message.id).to_string()),
        ]),
        Line::from(vec![
            Span::styled("author ", meta),
            Span::raw(app.author_label(&message.pubkey)),
        ]),
        Line::from(vec![
            Span::styled("kind ", meta),
            Span::raw(message.kind.to_string()),
            Span::styled("  channel ", meta),
            Span::raw(short_id(&channel).to_string()),
        ]),
        Line::from(vec![Span::styled("reactions ", meta), Span::raw(reactions)]),
        Line::from(""),
    ];
    lines.extend(crate::render::render_message_body(&message.content));
    let detail_width = area.width.saturating_sub(2) as usize;
    let lines = crate::render::wrap_message_detail_lines(lines, detail_width);
    let visible_rows = area.height.saturating_sub(2) as usize;
    let max_scroll = lines
        .len()
        .saturating_sub(visible_rows.max(1))
        .min(u16::MAX as usize) as u16;
    let scroll = app.message_detail_scroll.min(max_scroll);
    let title = if max_scroll == 0 {
        "Message".to_string()
    } else {
        format!("Message {scroll}/{max_scroll}")
    };

    frame.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)).block(panel_block(
            &title,
            matches!(app.focus, Focus::Timeline | Focus::Feed | Focus::Pulse),
        )),
        area,
    );
}

fn draw_agent_detail(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(agent) = app.acp.agent_at(app.selected_agent) else {
        frame.render_widget(
            Paragraph::new("No agent selected").block(panel_block("Agent", false)),
            area,
        );
        return;
    };

    let status = match agent.status {
        AgentStatus::Stopped => "stopped",
        AgentStatus::Running => "running",
        AgentStatus::Exited => "exited",
    };
    let args = if agent.runtime.args.is_empty() {
        String::new()
    } else {
        format!(" {}", agent.runtime.args.join(" "))
    };
    let mut text = format!(
        "{}\n{}\n{}{}\nresponds: {}",
        agent.runtime.label, status, agent.runtime.command, args, agent.runtime.respond_to
    );
    if agent.runtime.managed {
        text.push_str(&format!("\nkey: {}", short_id(&agent.runtime.id)));
        text.push_str(&format!(
            "\nstart on launch: {}",
            if agent.runtime.start_on_launch {
                "yes"
            } else {
                "no"
            }
        ));
        text.push_str(&format!(
            "\nthread direct mentions: {}",
            if agent.runtime.reply_placement == "top-level" {
                "no"
            } else {
                "yes"
            }
        ));
        text.push_str("\n@: mention in composer");
    }
    if app.focus == Focus::CreateAgent {
        text.push_str("\n\nNew managed agent");
        text.push_str(&format!("\nruntime: {}", agent.runtime.id));
        text.push_str(&format!(
            "\nname: {}",
            empty_dash(app.new_agent_name.trim())
        ));
        text.push_str(&format!(
            "\nmodel: {}",
            empty_dash(app.new_agent_model.trim())
        ));
        text.push_str(&format!(
            "\nstart on launch: {}",
            if app.new_agent_start_on_launch {
                "yes"
            } else {
                "no"
            }
        ));
        text.push_str(&format!("\nrespond to: {}", app.new_agent_respond_to));
        text.push_str(&format!(
            "\nthread direct mentions: {}",
            if app.new_agent_reply_placement == "top-level" {
                "no"
            } else {
                "yes"
            }
        ));
        if !app.new_agent_allowlist.trim().is_empty() {
            text.push_str(&format!(
                "\nallowlist: {}",
                compact_text(app.new_agent_allowlist.trim(), 90)
            ));
        }
        text.push_str(&format!(
            "\nsystem prompt: {}",
            empty_dash(app.new_agent_system_prompt.trim())
        ));
    }
    if !app.agent_log_path.is_empty() {
        text.push_str(&format!("\nlog: {}", app.agent_log_path));
    }
    if let Some(error) = agent.runtime.last_error.as_deref() {
        if !error.trim().is_empty() {
            text.push_str(&format!("\nlast error: {error}"));
        }
    }
    if let Some(last_exit) = agent.last_exit.as_deref() {
        if !last_exit.trim().is_empty() {
            text.push_str(&format!("\nlast exit: {last_exit}"));
        }
    }
    if !app.agent_log.is_empty() {
        text.push_str("\n\n");
        text.push_str(&app.agent_log);
    }

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel_block("Agent Log", false)),
        area,
    );
}

fn draw_composer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.focus == Focus::Composer {
        draw_chat_composer(frame, app, area);
        return;
    }
    let is_search = app.focus == Focus::Search;
    let is_channel_search = app.focus == Focus::ChannelSearch;
    let is_attachment = app.focus == Focus::Attachment;
    let is_diff = app.focus == Focus::Diff;
    let is_create_channel = app.focus == Focus::CreateChannel;
    let is_direct_message = app.focus == Focus::DirectMessage;
    let is_channel_name = app.focus == Focus::ChannelName;
    let is_channel_description = app.focus == Focus::ChannelDescription;
    let is_channel_topic = app.focus == Focus::ChannelTopic;
    let is_channel_purpose = app.focus == Focus::ChannelPurpose;
    let is_channel_section_assign = app.focus == Focus::ChannelSectionAssign;
    let is_add_member = app.focus == Focus::AddMember;
    let is_remove_member = app.focus == Focus::RemoveMember;
    let is_canvas_edit = app.focus == Focus::CanvasEdit;
    let is_create_agent = app.focus == Focus::CreateAgent;
    let is_profile_edit = app.focus == Focus::ProfileEdit;
    let is_profile_avatar_upload = app.focus == Focus::ProfileAvatarUpload;
    let is_contact_add = app.focus == Focus::ContactAdd;
    let is_user_lookup = app.focus == Focus::UserLookup;
    let is_note_edit = app.focus == Focus::NoteEdit;
    let is_workflow_edit = app.focus == Focus::WorkflowEdit;
    let is_workflow_inputs = app.focus == Focus::WorkflowInputs;
    let is_workflow_approval = app.focus == Focus::WorkflowApproval;
    let is_repo_create = app.focus == Focus::RepoCreate;
    let is_repo_issue_create = app.focus == Focus::RepoIssueCreate;
    let is_repo_patch_create = app.focus == Focus::RepoPatchCreate;
    let is_memory_edit = app.focus == Focus::MemoryEdit;
    let is_memory_patch = app.focus == Focus::MemoryPatch;
    let is_emoji_edit = app.focus == Focus::EmojiEdit;
    let is_emoji_import = app.focus == Focus::EmojiImport;
    let is_workspace_add = app.focus == Focus::WorkspaceAdd;
    let create_agent_title = agent_create_title(app);
    let profile_edit_title = format!("Edit {}", profile_field_label(app.selected_profile_field));
    let channel_create_title = channel_create_title(app);
    let note_edit_title = format!("Note {}", note_edit_field_label(app.note_edit_field));
    let workflow_approval_title = workflow_approval_title(app);
    let repo_create_title = format!("Repo {}", repo_create_field_label(app.repo_create_field));
    let repo_issue_title = format!("Issue {}", repo_issue_field_label(app.repo_issue_field));
    let repo_patch_title = format!("Patch {}", repo_patch_field_label(app.repo_patch_field));
    let memory_edit_title = format!("Memory {}", memory_edit_field_label(app.memory_edit_field));
    let memory_patch_title = format!(
        "Memory Patch {}",
        memory_patch_field_label(app.memory_patch_field)
    );
    let diff_title = format!("Diff {}", diff_field_label(app.diff_field));
    let emoji_edit_title = format!("Emoji {}", emoji_edit_field_label(app.emoji_edit_field));
    let emoji_import_title = if app.emoji_import_replace {
        "Import Emoji (Replace)"
    } else {
        "Import Emoji (Merge)"
    };
    let block = if is_search {
        panel_block("Search", true)
    } else if is_channel_search {
        panel_block("Channel Search", true)
    } else if is_attachment {
        panel_block("Attach Files", true)
    } else if is_diff {
        panel_block(&diff_title, true)
    } else if is_create_channel {
        panel_block(&channel_create_title, true)
    } else if is_direct_message {
        panel_block("New DM", true)
    } else if is_create_agent {
        panel_block(&create_agent_title, true)
    } else if is_profile_edit {
        panel_block(&profile_edit_title, true)
    } else if is_profile_avatar_upload {
        panel_block("Upload Avatar", true)
    } else if is_contact_add {
        panel_block("Add Contact", true)
    } else if is_user_lookup {
        panel_block("User Lookup", true)
    } else if is_note_edit {
        panel_block(&note_edit_title, true)
    } else if is_workflow_edit {
        panel_block("Workflow YAML", true)
    } else if is_workflow_inputs {
        panel_block("Workflow Inputs", true)
    } else if is_workflow_approval {
        panel_block(&workflow_approval_title, true)
    } else if is_repo_create {
        panel_block(&repo_create_title, true)
    } else if is_repo_issue_create {
        panel_block(&repo_issue_title, true)
    } else if is_repo_patch_create {
        panel_block(&repo_patch_title, true)
    } else if is_memory_edit {
        panel_block(&memory_edit_title, true)
    } else if is_memory_patch {
        panel_block(&memory_patch_title, true)
    } else if is_emoji_edit {
        panel_block(&emoji_edit_title, true)
    } else if is_emoji_import {
        panel_block(emoji_import_title, true)
    } else if is_workspace_add {
        panel_block("Add Workspace", true)
    } else if is_channel_name {
        panel_block("Rename Channel", true)
    } else if is_channel_description {
        panel_block("Set Description", true)
    } else if is_channel_topic {
        panel_block("Set Topic", true)
    } else if is_channel_purpose {
        panel_block("Set Purpose", true)
    } else if is_add_member {
        panel_block("Add Member", true)
    } else if is_remove_member {
        panel_block("Remove Member", true)
    } else if is_canvas_edit {
        panel_block("Edit Canvas", true)
    } else if app.focus == Focus::Composer && app.timeline_mode == TimelineMode::Pulse {
        if app.pulse_reply_target.is_some() {
            panel_block("Pulse Reply", true)
        } else {
            panel_block("Pulse Note", true)
        }
    } else if app.edit_target.is_some() {
        panel_block("Edit Message", app.focus == Focus::Composer)
    } else {
        panel_block("Composer", app.focus == Focus::Composer)
    };
    let text = if is_search {
        if app.search_query.is_empty() {
            "Type a search query, Enter to run"
        } else {
            &app.search_query
        }
    } else if is_channel_search {
        if app.channel_search_query.is_empty() {
            "Type a channel name, Enter to search/open"
        } else {
            &app.channel_search_query
        }
    } else if is_attachment {
        if app.attachment_input.is_empty() {
            "Type file path(s), Enter to upload and send"
        } else {
            &app.attachment_input
        }
    } else if is_diff {
        let current = diff_current_text(app);
        if current.is_empty() {
            diff_placeholder(app.diff_field)
        } else {
            current
        }
    } else if is_create_channel {
        let current = channel_create_current_text(app);
        if current.is_empty() {
            channel_create_placeholder(app.new_channel_field)
        } else {
            current
        }
    } else if is_direct_message {
        if app.dm_pubkey.is_empty() {
            "Type a 64-char pubkey, Enter to open DM"
        } else {
            &app.dm_pubkey
        }
    } else if is_create_agent {
        let current = agent_create_current_text(app);
        if current.is_empty() {
            agent_create_placeholder(app)
        } else {
            current
        }
    } else if is_profile_edit {
        if app.profile_input.is_empty() {
            "Type a value, Enter to save"
        } else {
            &app.profile_input
        }
    } else if is_profile_avatar_upload {
        if app.profile_upload_path.is_empty() {
            "Type an image path, Enter to upload and save"
        } else {
            &app.profile_upload_path
        }
    } else if is_contact_add {
        if app.contact_input.is_empty() {
            "Type pubkey [relay_url] [petname], Enter to save"
        } else {
            &app.contact_input
        }
    } else if is_user_lookup {
        if app.user_lookup_input.is_empty() {
            "Type a pubkey or display name, Enter to view profile"
        } else {
            &app.user_lookup_input
        }
    } else if is_note_edit {
        let current = note_edit_current_text(app);
        if current.is_empty() {
            note_edit_placeholder(app.note_edit_field)
        } else {
            current
        }
    } else if is_workflow_edit {
        if app.workflow_yaml.is_empty() {
            "Type workflow YAML, Enter to save"
        } else {
            &app.workflow_yaml
        }
    } else if is_workflow_inputs {
        if app.workflow_inputs.is_empty() {
            "Type JSON object inputs, Enter to trigger"
        } else {
            &app.workflow_inputs
        }
    } else if is_workflow_approval {
        let current = workflow_approval_current_text(app);
        if current.is_empty() {
            workflow_approval_placeholder(app)
        } else {
            current
        }
    } else if is_repo_create {
        let current = repo_create_current_text(app);
        if current.is_empty() {
            repo_create_placeholder(app.repo_create_field)
        } else {
            current
        }
    } else if is_repo_issue_create {
        let current = repo_issue_current_text(app);
        if current.is_empty() {
            repo_issue_placeholder(app.repo_issue_field)
        } else {
            current
        }
    } else if is_repo_patch_create {
        let current = repo_patch_current_text(app);
        if current.is_empty() {
            repo_patch_placeholder(app.repo_patch_field)
        } else {
            current
        }
    } else if is_memory_edit {
        let current = memory_edit_current_text(app);
        if current.is_empty() {
            memory_edit_placeholder(app.memory_edit_field)
        } else {
            current
        }
    } else if is_memory_patch {
        let current = memory_patch_current_text(app);
        if current.is_empty() {
            memory_patch_placeholder(app.memory_patch_field)
        } else {
            current
        }
    } else if is_emoji_edit {
        let current = emoji_edit_current_text(app);
        if current.is_empty() {
            emoji_edit_placeholder(app.emoji_edit_field)
        } else {
            current
        }
    } else if is_emoji_import {
        if app.emoji_import_path.is_empty() {
            "Type emoji JSON file path, F2 toggles replace, Enter imports"
        } else {
            &app.emoji_import_path
        }
    } else if is_workspace_add {
        if app.workspace_input.is_empty() {
            "Type 'name http://relay' or just a relay URL"
        } else {
            &app.workspace_input
        }
    } else if is_channel_name {
        if app.channel_action_input.is_empty() {
            "Type channel name, Enter to save"
        } else {
            &app.channel_action_input
        }
    } else if is_channel_description {
        if app.channel_action_input.is_empty() {
            "Type channel description, Enter to save"
        } else {
            &app.channel_action_input
        }
    } else if is_channel_topic {
        if app.channel_action_input.is_empty() {
            "Type topic text, Enter to save"
        } else {
            &app.channel_action_input
        }
    } else if is_channel_purpose {
        if app.channel_action_input.is_empty() {
            "Type purpose text, Enter to save"
        } else {
            &app.channel_action_input
        }
    } else if is_channel_section_assign {
        if app.channel_action_input.is_empty() {
            "Type section name or id, Enter to assign"
        } else {
            &app.channel_action_input
        }
    } else if is_add_member {
        if app.channel_action_input.is_empty() {
            "Type pubkey or 'pubkey role', Enter to add"
        } else {
            &app.channel_action_input
        }
    } else if is_remove_member {
        if app.channel_action_input.is_empty() {
            "Type pubkey, Enter to remove"
        } else {
            &app.channel_action_input
        }
    } else if is_canvas_edit {
        if app.canvas_draft.is_empty() {
            "Type canvas markdown, Enter to save"
        } else {
            &app.canvas_draft
        }
    } else if app.focus == Focus::Composer
        && app.timeline_mode == TimelineMode::Pulse
        && app.composer.is_empty()
    {
        if app.pulse_reply_target.is_some() {
            "Type Pulse reply, Enter to publish"
        } else {
            "Type Pulse note, Enter to publish"
        }
    } else if app.edit_target.is_some() && app.composer.is_empty() {
        "Type updated content, Enter to save"
    } else if app.composer.is_empty() {
        "Press c, type a message, Enter to send"
    } else {
        &app.composer
    };
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(block),
        area,
    );
}

fn draw_status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(app.status.clone(), Style::new().fg(Color::Green)),
        Span::raw("   "),
    ];
    for hint in app.contextual_hints() {
        spans.push(Span::styled(hint.key, Style::new().fg(Color::Cyan)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(hint.label, Style::new().fg(Color::Gray)));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled("·  ", Style::new().fg(Color::DarkGray)));
    spans.push(Span::styled(":", Style::new().fg(Color::Cyan)));
    spans.push(Span::styled(" palette  ", Style::new().fg(Color::Gray)));
    spans.push(Span::styled("?", Style::new().fg(Color::Cyan)));
    spans.push(Span::styled(" help", Style::new().fg(Color::Gray)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn panel_block(title: &str, active: bool) -> Block<'_> {
    let style = if active {
        Style::new()
            .fg(Color::LightMagenta)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::Gray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(Line::from(title.to_string()).style(style))
}

fn selectable_list<'a>(items: Vec<ListItem<'a>>, block: Block<'a>) -> List<'a> {
    List::new(items)
        .block(block)
        .highlight_symbol(">")
        .highlight_style(Style::new().bg(Color::DarkGray).fg(Color::White))
}

fn list_state(selected: usize, len: usize) -> ListState {
    let mut state = ListState::default();
    if len > 0 {
        state.select(Some(selected.min(len - 1)));
    }
    state
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let mut content = value.replace('\n', " ");
    if content.chars().count() > max_chars {
        content = content
            .chars()
            .take(max_chars.saturating_sub(3))
            .collect::<String>();
        content.push_str("...");
    }
    content
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn empty_dash(value: &str) -> &str {
    if value.trim().is_empty() {
        "-"
    } else {
        value
    }
}

fn extract_memory_refs(value: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("[[") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("]]") else {
            break;
        };
        let slug = after_start[..end].trim();
        if !slug.is_empty() && !refs.iter().any(|entry| entry == slug) {
            refs.push(slug.to_string());
        }
        rest = &after_start[end + 2..];
    }
    refs
}

fn agent_create_title(app: &App) -> String {
    let field = match app.new_agent_field {
        AgentCreateField::Name => "Name",
        AgentCreateField::Model => "Model",
        AgentCreateField::SystemPrompt => "System Prompt",
        AgentCreateField::RespondTo => "Respond To",
        AgentCreateField::Allowlist => "Allowlist",
    };
    format!("New Agent: {field}")
}

fn workflow_approval_title(app: &App) -> String {
    let decision = if app.workflow_approval_approved {
        "Approve"
    } else {
        "Deny"
    };
    format!(
        "{decision} Workflow: {}",
        workflow_approval_field_label(app.workflow_approval_field)
    )
}

fn workflow_approval_field_label(field: WorkflowApprovalField) -> &'static str {
    match field {
        WorkflowApprovalField::Token => "Token",
        WorkflowApprovalField::Note => "Note",
    }
}

fn workflow_approval_current_text(app: &App) -> &str {
    match app.workflow_approval_field {
        WorkflowApprovalField::Token => &app.workflow_approval_token,
        WorkflowApprovalField::Note => &app.workflow_approval_note,
    }
}

fn workflow_approval_placeholder(app: &App) -> &'static str {
    match app.workflow_approval_field {
        WorkflowApprovalField::Token => "Approval token UUID, Enter submits",
        WorkflowApprovalField::Note => "Optional note, Enter submits",
    }
}

fn channel_create_title(app: &App) -> String {
    format!(
        "New Channel: {}",
        channel_create_field_label(app.new_channel_field)
    )
}

fn channel_create_field_label(field: CreateChannelField) -> &'static str {
    match field {
        CreateChannelField::Name => "Name",
        CreateChannelField::Type => "Type",
        CreateChannelField::Visibility => "Visibility",
        CreateChannelField::Description => "Description",
    }
}

fn channel_create_current_text(app: &App) -> &str {
    match app.new_channel_field {
        CreateChannelField::Name => &app.new_channel_name,
        CreateChannelField::Type => app.new_channel_type.label(),
        CreateChannelField::Visibility => app.new_channel_visibility.label(),
        CreateChannelField::Description => &app.new_channel_description,
    }
}

fn channel_create_placeholder(field: CreateChannelField) -> &'static str {
    match field {
        CreateChannelField::Name => "Type a channel name, Enter to create",
        CreateChannelField::Type => "F2 cycles stream or forum",
        CreateChannelField::Visibility => "F3 cycles open or private",
        CreateChannelField::Description => "Optional description, Enter to create",
    }
}

fn agent_create_current_text(app: &App) -> &str {
    match app.new_agent_field {
        AgentCreateField::Name => &app.new_agent_name,
        AgentCreateField::Model => &app.new_agent_model,
        AgentCreateField::SystemPrompt => &app.new_agent_system_prompt,
        AgentCreateField::RespondTo => &app.new_agent_respond_to,
        AgentCreateField::Allowlist => &app.new_agent_allowlist,
    }
}

fn agent_create_placeholder(app: &App) -> &'static str {
    match app.new_agent_field {
        AgentCreateField::Name => "Type an agent name, Enter to create",
        AgentCreateField::Model => "Optional model id, Tab for next field",
        AgentCreateField::SystemPrompt => "Optional system prompt, Tab for next field",
        AgentCreateField::RespondTo => "F3 cycles owner-only, allowlist, anyone",
        AgentCreateField::Allowlist => {
            "Pubkeys separated by commas or spaces; F2 start-on-launch; F4 thread mentions"
        }
    }
}

fn note_edit_current_text(app: &App) -> &str {
    match app.note_edit_field {
        NoteEditField::Name => &app.note_name,
        NoteEditField::Title => &app.note_title,
        NoteEditField::Summary => &app.note_summary,
        NoteEditField::Tags => &app.note_tags,
        NoteEditField::Content => &app.note_content,
    }
}

fn note_edit_placeholder(field: NoteEditField) -> &'static str {
    match field {
        NoteEditField::Name => "Slug like release-notes, Enter saves",
        NoteEditField::Title => "Title, Enter saves",
        NoteEditField::Summary => "Optional summary, Enter saves",
        NoteEditField::Tags => "Optional tags separated by commas/spaces",
        NoteEditField::Content => "Markdown body, Enter saves",
    }
}

fn repo_create_current_text(app: &App) -> &str {
    match app.repo_create_field {
        RepoCreateField::Id => &app.repo_id,
        RepoCreateField::Name => &app.repo_name,
        RepoCreateField::Description => &app.repo_description,
        RepoCreateField::CloneUrls => &app.repo_clone_urls,
        RepoCreateField::WebUrl => &app.repo_web_url,
        RepoCreateField::Relays => &app.repo_relays,
    }
}

fn repo_create_placeholder(field: RepoCreateField) -> &'static str {
    match field {
        RepoCreateField::Id => "Repo id like sprout, Enter saves",
        RepoCreateField::Name => "Optional display name, Tab for next field",
        RepoCreateField::Description => "Optional description, Tab for next field",
        RepoCreateField::CloneUrls => "Clone URLs separated by commas or spaces",
        RepoCreateField::WebUrl => "Optional https:// web URL",
        RepoCreateField::Relays => "Optional ws:// or wss:// relays separated by commas",
    }
}

fn repo_issue_field_label(field: RepoIssueField) -> &'static str {
    match field {
        RepoIssueField::Title => "Title",
        RepoIssueField::Labels => "Labels",
        RepoIssueField::Content => "Body",
    }
}

fn repo_issue_current_text(app: &App) -> &str {
    match app.repo_issue_field {
        RepoIssueField::Title => &app.repo_issue_title,
        RepoIssueField::Labels => &app.repo_issue_labels,
        RepoIssueField::Content => &app.repo_issue_content,
    }
}

fn repo_issue_placeholder(field: RepoIssueField) -> &'static str {
    match field {
        RepoIssueField::Title => "Issue title, Enter saves",
        RepoIssueField::Labels => "Optional labels separated by commas or spaces",
        RepoIssueField::Content => "Markdown body, Enter saves",
    }
}

fn repo_patch_field_label(field: RepoPatchField) -> &'static str {
    match field {
        RepoPatchField::Commit => "Commit",
        RepoPatchField::ParentCommit => "Parent",
        RepoPatchField::Content => "Patch",
    }
}

fn repo_patch_current_text(app: &App) -> &str {
    match app.repo_patch_field {
        RepoPatchField::Commit => &app.repo_patch_commit,
        RepoPatchField::ParentCommit => &app.repo_patch_parent_commit,
        RepoPatchField::Content => &app.repo_patch_content,
    }
}

fn repo_patch_placeholder(field: RepoPatchField) -> &'static str {
    match field {
        RepoPatchField::Commit => "Optional commit hash, Tab for next field",
        RepoPatchField::ParentCommit => "Optional parent commit hash, Tab for next field",
        RepoPatchField::Content => "git format-patch content, Enter saves",
    }
}

fn memory_edit_field_label(field: MemoryEditField) -> &'static str {
    match field {
        MemoryEditField::Slug => "Slug",
        MemoryEditField::Value => "Value",
    }
}

fn memory_edit_current_text(app: &App) -> &str {
    match app.memory_edit_field {
        MemoryEditField::Slug => &app.memory_slug,
        MemoryEditField::Value => &app.memory_value,
    }
}

fn memory_edit_placeholder(field: MemoryEditField) -> &'static str {
    match field {
        MemoryEditField::Slug => "Memory slug like mem/values/honesty, Enter saves",
        MemoryEditField::Value => "Memory value, Enter saves",
    }
}

fn memory_patch_field_label(field: MemoryPatchField) -> &'static str {
    match field {
        MemoryPatchField::BaseHash => "Base Hash",
        MemoryPatchField::Patch => "Patch",
    }
}

fn memory_patch_current_text(app: &App) -> &str {
    match app.memory_patch_field {
        MemoryPatchField::BaseHash => &app.memory_patch_base_hash,
        MemoryPatchField::Patch => &app.memory_patch_content,
    }
}

fn memory_patch_placeholder(field: MemoryPatchField) -> &'static str {
    match field {
        MemoryPatchField::BaseHash => "sha256 from H or mem hash, Tab for patch",
        MemoryPatchField::Patch => "Unified diff for selected memory, Enter applies",
    }
}

fn diff_current_text(app: &App) -> &str {
    match app.diff_field {
        DiffField::Repo => &app.diff_repo,
        DiffField::Commit => &app.diff_commit,
        DiffField::File => &app.diff_file,
        DiffField::Description => &app.diff_description,
        DiffField::Diff => &app.diff_content,
    }
}

fn diff_placeholder(field: DiffField) -> &'static str {
    match field {
        DiffField::Repo => "Repository URL, Tab for next field",
        DiffField::Commit => "Commit SHA, Tab for next field",
        DiffField::File => "Optional file path, Tab for next field",
        DiffField::Description => "Optional summary, Tab for next field",
        DiffField::Diff => "Paste diff body, Enter sends",
    }
}

fn emoji_edit_field_label(field: EmojiEditField) -> &'static str {
    match field {
        EmojiEditField::Shortcode => "Shortcode",
        EmojiEditField::Url => "URL",
    }
}

fn emoji_edit_current_text(app: &App) -> &str {
    match app.emoji_edit_field {
        EmojiEditField::Shortcode => &app.emoji_shortcode,
        EmojiEditField::Url => &app.emoji_url,
    }
}

fn emoji_edit_placeholder(field: EmojiEditField) -> &'static str {
    match field {
        EmojiEditField::Shortcode => "Shortcode without colons, Enter saves",
        EmojiEditField::Url => "Image URL, Enter saves",
    }
}

#[cfg(test)]
mod tests {
    use super::extract_memory_refs;

    #[test]
    fn extracts_unique_memory_refs() {
        assert_eq!(
            extract_memory_refs(
                "see [[mem/values/honesty]] and [[ core ]] and [[mem/values/honesty]]"
            ),
            vec!["mem/values/honesty".to_string(), "core".to_string()]
        );
    }
}
