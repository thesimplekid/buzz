# buzz-tui UX Improvement Plan

## Progress

- [x] 1. Command palette (`:` / `Ctrl-P`)
- [x] 2. Confirmation prompts for destructive actions
- [x] 3. Composer multiline + reply/edit context + autocomplete
- [x] 4. Contextual action hints
- [x] 5. Navigation history (Alt-←/→ + palette, auto back/forward stack)
- [x] 6. Direct relay backend adoption
- [x] 7. Rich timeline rendering + older-history pagination

## Goal

Make `buzz-tui` easier to learn, safer to operate, and faster for everyday
chat, agent, workflow, and workspace tasks.

The desktop app is the reference point for interaction quality: it gives users
persistent navigation, visible actions, contextual panels, safe destructive
flows, rich compose tools, and live state that feels immediate. The TUI already
covers a large feature surface, but many actions are hidden behind dense
keybindings and single-line prompts. This plan focuses on translating the best
desktop affordances into terminal-native patterns.

## Current Gaps

1. Keybindings are too dense.
   The TUI exposes many actions through direct keys and a long status-line hint.
   This is powerful once memorized, but hard to discover and easy to misfire.

2. Destructive actions are too immediate.
   Deletes, removals, archive operations, and leave/hide actions should require
   confirmation before changing relay or local state.

3. Composer is too basic for normal Buzz usage.
   Desktop supports drafts, rich text, mentions, channel links, emoji, files,
   edit context, reply context, and typing broadcasts. The TUI currently uses a
   plain string editor and separate modes for attachments and diffs.

4. Contextual actions are not visible enough.
   The right panel shows details, but available actions depend on focus and are
   mostly learned from help text rather than shown near the selected item.

5. Navigation has no real history model.
   Desktop has route navigation and back/forward controls. The TUI has thread
   return context and `Esc`, but not a general navigation stack.

6. Runtime architecture still mixes direct relay work with CLI-spawn fallback.
   The TUI has native relay helpers and live subscriptions, but high-traffic
   operations still route through `BuzzCli` in parts of the app.

7. Timeline rendering loses important structure.
   Message rows compact markdown, code, diffs, and media into plain text. That
   makes review and agent output harder to scan in terminal.

## Plan

### 1. Add A Command Palette

Add a searchable command palette opened with `:` or `Ctrl-P`.

Requirements:

- Show available actions for the current focus first.
- Include global actions such as workspace switch, search, feed, Pulse, agents,
  profile, contacts, workflows, refresh, and quit.
- Support fuzzy filtering by command label and aliases.
- Display each command's keybinding when one exists.
- Execute the selected command through the same app methods used by keybindings.
- Keep direct keybindings for expert use.

Implementation notes:

- Add a `Focus::CommandPalette` state.
- Add command descriptors with `id`, `label`, `aliases`, `scope`, optional
  keybinding text, and enabled predicate.
- Refactor `handle_key` command branches so palette execution and direct keys
  share one dispatch path.
- Render the palette as an overlay-like centered panel or reuse the right panel
  on narrow terminals.

Acceptance checks:

- A new user can type `:` and find "create channel", "open DM", "start agent",
  "edit message", and "delete message" without opening help.
- Palette results change based on selected panel and selected item.
- Disabled commands explain why they cannot run.

### 2. Add Confirmation Prompts

Add confirmations for destructive or hard-to-undo actions.

Actions requiring confirmation:

- Delete message.
- Delete workflow.
- Delete managed agent.
- Delete memory.
- Remove custom emoji.
- Remove workspace.
- Leave channel.
- Hide DM.
- Archive channel.
- Remove channel member.
- Remove contact.
- Delete long-form note.

Implementation notes:

- Add a reusable confirmation state with title, body, confirm label, cancel
  label, and action enum.
- Render confirmations in the bottom input area or as a centered overlay.
- Use `Enter` to confirm and `Esc` to cancel.
- Preserve the previous focus after cancel or completion.

Acceptance checks:

- Pressing a destructive key first opens a confirmation instead of mutating.
- `Esc` cancels without side effects.
- Confirmed actions still report success or failure in the status line.

### 3. Upgrade The Composer

Make the composer closer to desktop while staying terminal-native.

Requirements:

- Multiline editing.
- Visible reply and edit context above the input.
- Mention autocomplete for channel members and managed agents.
- Channel autocomplete for `#channel` references.
- Custom emoji autocomplete for `:emoji:` shortcodes.
- `Up` in an empty composer edits the last own message in the current scope.
- Drafts keep working per workspace, channel, and thread.
- Attachments remain possible, but the composer should show pending attachment
  paths before sending.

Implementation notes:

- Introduce a small line editor abstraction instead of directly appending chars
  to `String`.
- Track cursor position, lines, and completion state.
- Add completion sources for people, agents, channels, and emoji.
- Keep the existing `B` attachment path flow initially, then fold pending files
  into composer state.

Acceptance checks:

- A user can write a multiline message and send it.
- Typing `@`, `#`, or `:` opens relevant suggestions.
- Canceling an edit restores the previous draft.
- Empty-composer `Up` edits the most recent own message.

### 4. Show Contextual Action Hints

Replace the single long global status hint with short contextual hints.

Requirements:

- Each focus mode shows only the most relevant actions.
- The global help remains available with `?`.
- The status line should prioritize current state, errors, and the selected
  panel's actions.
- The right panel should include action rows for selected messages, channels,
  agents, contacts, workflows, and profiles.

Implementation notes:

- Add a `contextual_hints(&App) -> Vec<Hint>` helper.
- Render hints compactly in the status line.
- Keep high-signal hints in detail panels where space allows.

Acceptance checks:

- On a message, the user sees reply/thread/edit/delete/react/profile actions.
- On a channel, the user sees join/leave/star/mute/member/topic actions.
- On an agent, the user sees start/stop/mention/add-to-channel/log actions.

### 5. Add Navigation History

Add terminal-native back and forward navigation.

Requirements:

- Track navigation entries for channel, thread, search result, feed item, Pulse,
  profile, workflow, contacts, agents, and workspace views.
- Bind back and forward to `Alt-Left` / `Alt-Right` where terminal support
  allows, plus command palette actions.
- `Esc` keeps its local cancel/close behavior.
- Opening a search or feed result pushes a history entry.

Implementation notes:

- Add a `NavigationEntry` enum to app state.
- Push entries from high-level focus/open methods, not raw key handlers.
- Keep selection indexes where useful so returning restores context.

Acceptance checks:

- Search result -> thread -> profile can be navigated back step by step.
- Switching channels preserves enough previous selection to resume scanning.

### 6. Finish Direct Relay Backend Adoption

Reduce CLI-spawn dependency for common interactive paths.

Priority paths:

- Channel list and channel detail.
- Active channel messages and threads.
- Send, edit, delete, and react.
- Search.
- Feed.
- Read-state, stars, mutes, and sections.
- Profile and presence.

Implementation notes:

- Continue using `buzz-acp` as a subprocess for long-running agents.
- Keep `BuzzCli` as compatibility fallback where native support is incomplete.
- Move app calls behind the existing `TuiBackend` trait so state code does not
  care whether the implementation is native or fallback.
- Prefer existing Buzz crates for event builders and kind constants.

Acceptance checks:

- The TUI can run core chat flows without `buzz` on `PATH`.
- Live updates and direct writes converge without manual refresh.
- Secrets are never passed through process argv.

### 7. Improve Timeline Rendering

Make messages easier to scan in terminal.

Requirements:

- Render markdown paragraphs, quotes, lists, and code blocks distinctly.
- Render diffs with added/removed/context line styling.
- Show media attachments as file cards with type, size, and URL.
- Show custom emoji shortcodes consistently.
- Show thread/reply counts and unread markers where known.
- Add older-history pagination.

Implementation notes:

- Add a message rendering module separate from layout code.
- Keep compact rows in the timeline, but show richer content in the detail panel.
- Start with deterministic markdown/code/diff formatting before adding complex
  wrapping behavior.

Acceptance checks:

- Code blocks and diffs are readable without opening desktop.
- A selected message detail preserves structure instead of flattening all newlines.
- Users can load older messages from the keyboard or command palette.

## Suggested Order

1. Command palette.
2. Confirmation prompts.
3. Contextual action hints.
4. Composer multiline and visible reply/edit context.
5. Composer autocomplete.
6. Navigation history.
7. Native backend adoption for core chat paths.
8. Rich timeline rendering and older-history pagination.

This order improves usability quickly without blocking on the larger backend
migration. It also keeps every step useful on its own.

## Non-Goals

- Recreating the full desktop UI in terminal.
- Huddle audio controls.
- Animated avatar editing.
- Mobile pairing UI.
- Full desktop settings parity.

## Validation

Run focused checks after each phase:

```bash
cargo check -p buzz-tui
```

Add unit tests for:

- Command palette filtering and enabled states.
- Confirmation action dispatch.
- Composer cursor, multiline, draft, and autocomplete behavior.
- Navigation history push/pop.
- Message rendering snapshots.

Run a live smoke test before marking the plan complete:

- Switch workspace.
- Open channel.
- Search and open a result.
- Send a multiline message.
- Reply in a thread.
- Edit last own message.
- React to a message.
- Confirm and cancel at least one destructive action.
- Start and stop a managed agent.
