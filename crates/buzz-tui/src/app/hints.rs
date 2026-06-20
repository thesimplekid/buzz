use super::{App, Focus, TimelineMode};

/// A single contextual key hint shown in the status line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hint {
    pub key: &'static str,
    pub label: &'static str,
}

const fn hint(key: &'static str, label: &'static str) -> Hint {
    Hint { key, label }
}

impl App {
    /// The most relevant action hints for the current focus. The full key map
    /// is always available with `?`; these keep the status line high-signal.
    pub fn contextual_hints(&self) -> Vec<Hint> {
        match self.focus {
            Focus::Sidebar => vec![
                hint("Enter", "open"),
                hint("n", "channel"),
                hint("m", "DM"),
                hint("j", "join"),
                hint("l", "leave"),
                hint("F4", "add policy"),
                hint("S", "star"),
                hint("M", "mute"),
                hint("{/}", "width"),
                hint("z", "archive"),
                hint("Del", "delete"),
            ],
            Focus::Timeline => match self.timeline_mode {
                TimelineMode::Channel => vec![
                    hint("Enter", "thread"),
                    hint("PgDn", "read"),
                    hint("Ctrl-U", "up"),
                    hint(",/.", "detail"),
                    hint("Alt+↑/↓", "height"),
                    hint("c", "compose"),
                    hint("e", "edit"),
                    hint("d", "delete"),
                    hint("+", "react"),
                    hint("]", "vote"),
                ],
                TimelineMode::Search => {
                    vec![
                        hint("Enter", "open"),
                        hint("PgDn", "read"),
                        hint(",/.", "detail"),
                        hint("Alt+↑/↓", "height"),
                        hint("+", "react"),
                        hint("Esc", "back"),
                    ]
                }
                TimelineMode::Feed => vec![
                    hint("Enter", "open"),
                    hint("PgDn", "read"),
                    hint(",/.", "detail"),
                    hint("Alt+↑/↓", "height"),
                    hint("]", "vote"),
                    hint("Esc", "back"),
                ],
                TimelineMode::Pulse => {
                    vec![
                        hint("PgDn", "read"),
                        hint(",/.", "detail"),
                        hint("Alt+↑/↓", "height"),
                        hint("R", "reply"),
                        hint("S", "source"),
                        hint("+", "react"),
                    ]
                }
            },
            Focus::Composer => vec![
                hint("Enter", "send"),
                hint("Alt+Enter", "newline"),
                hint("Esc", "cancel"),
            ],
            Focus::Agents => vec![
                hint("Enter", "start/stop"),
                hint("A", "new"),
                hint("D", "delete"),
                hint("@", "mention"),
                hint("u", "add to channel"),
            ],
            Focus::CreateAgent => vec![
                hint("Tab", "field"),
                hint("F2", "autostart"),
                hint("F3", "respond"),
                hint("F4", "thread"),
                hint("Enter", "create"),
                hint("Esc", "cancel"),
            ],
            Focus::Pulse => vec![
                hint("PgDn", "read"),
                hint(",/.", "detail"),
                hint("Alt+↑/↓", "height"),
                hint("R", "reply"),
                hint("S", "source"),
                hint("+", "react"),
            ],
            Focus::Feed => vec![
                hint("Enter", "open"),
                hint("PgDn", "read"),
                hint(",/.", "detail"),
                hint("Alt+↑/↓", "height"),
                hint("F", "filter"),
            ],
            Focus::Profile => vec![
                hint("s", "presence"),
                hint("A", "avatar"),
                hint("Enter", "edit"),
            ],
            Focus::Contacts => vec![hint("Enter", "DM"), hint("A", "add"), hint("D", "remove")],
            Focus::Workflows => vec![
                hint("Enter", "run"),
                hint("R", "refresh"),
                hint("A", "new"),
                hint("E", "edit"),
                hint("D", "delete"),
                hint("G/X", "approve"),
            ],
            Focus::Memory => vec![
                hint("Enter", "view"),
                hint("A", "new"),
                hint("E", "edit"),
                hint("H", "hash"),
                hint("P", "patch"),
                hint("D", "delete"),
            ],
            Focus::MemoryPatch => vec![
                hint("Tab", "field"),
                hint("Enter", "apply"),
                hint("Esc", "cancel"),
            ],
            Focus::Emoji => vec![
                hint("A", "add"),
                hint("E", "edit"),
                hint("D", "remove"),
                hint("I", "import"),
                hint("X", "export"),
            ],
            Focus::Notes => vec![
                hint("S", "source"),
                hint("A", "new"),
                hint("E", "edit"),
                hint("D", "delete"),
            ],
            Focus::Repos => vec![
                hint("A", "new repo"),
                hint("I", "new issue"),
                hint("P", "new patch"),
                hint("Esc", "back"),
            ],
            Focus::RepoIssueCreate | Focus::RepoPatchCreate => vec![
                hint("Tab", "field"),
                hint("Enter", "save"),
                hint("Esc", "cancel"),
            ],
            Focus::Workspaces => vec![
                hint("Enter", "switch"),
                hint("A", "add"),
                hint("D", "remove"),
            ],
            Focus::CommandPalette => vec![
                hint("type", "filter"),
                hint("Enter", "run"),
                hint("Esc", "cancel"),
            ],
            Focus::Confirm => vec![hint("Enter", "confirm"), hint("Esc", "cancel")],
            Focus::Help => vec![hint("Esc", "return")],
            Focus::Canvas => vec![hint("Enter", "edit"), hint("Esc", "back")],
            Focus::UserProfile => vec![hint("Enter", "DM"), hint("Esc", "back")],
            _ => vec![hint("Enter", "confirm"), hint("Esc", "cancel")],
        }
    }
}
