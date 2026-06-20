# buzz-tui

`buzz-tui` is a Ratatui terminal client for Buzz. It uses the relay directly for
reads and writes, and supervises local ACP harness processes for agents:

- Direct relay HTTP/WebSocket calls for channels, messages, profile, notes,
  emoji, workflows, memory, and workspace state.
- A TUI-owned local managed-agent store plus built-in runtime templates for
  Goose, Codex, and Claude Code.
- `buzz-acp` for local agent harnesses. The TUI can start and stop local ACP
  harness processes from managed-agent records or directly from discovered
  Goose, Codex, and Claude Code runtimes.

This keeps the terminal app independent of the `buzz` CLI binary while sharing
the relay event model and ACP harness.

## Run

Build the ACP harness and TUI first:

```bash
cargo build -p buzz-acp -p buzz-tui
```

Then run the TUI against a relay:

```bash
BUZZ_PRIVATE_KEY=<nsec-or-hex> just tui
```

Or run the binary directly:

```bash
BUZZ_PRIVATE_KEY=<nsec-or-hex> \
BUZZ_RELAY_URL=http://localhost:3000 \
cargo run -p buzz-tui -- \
  --acp-bin ./target/debug/buzz-acp
```

Useful options:

- `--relay`: HTTP relay URL; converted to `ws://` or `wss://` for `buzz-acp`.
- `--acp-bin`: path to the `buzz-acp` harness.
- `--mcp-command`: optional MCP server command exposed to started agents.
  Overrides any agent or runtime-specific MCP default.
- `--agent-key id=value`: private key override for one ACP entry. With managed
  agents, `id` is the agent pubkey; in runtime fallback mode, it is a runtime ID
  such as `goose`, `codex`, or `claude`.
- `--agent-auth-tag id=value`: auth tag override for one ACP entry. This can be
  repeated alongside `--agent-key`.
- `--refresh-interval`: seconds between active-channel and feed polling. Set to
  `0` to disable automatic refresh.
- `--workspace-store`: path to the local TUI workspace list. Defaults to
  `$BUZZ_TUI_WORKSPACES`, then `$XDG_CONFIG_HOME/buzz/tui-workspaces.json`,
  then `~/.config/buzz/tui-workspaces.json`.

## Controls

- `?`: show the in-app help panel.
- `W`: open the workspace switcher. Press `Enter` to switch relays, `A` to add
  `name http://relay` or a relay URL, and `D` to remove a selected inactive
  workspace.
- `Tab` / `Shift+Tab`: move focus. While creating a channel or agent, cycle
  forward or backward through the form fields.
- `Up` / `Down`: move within the focused panel.
- `Enter`: open a channel, open a thread, send the composer, or toggle the
  selected ACP runtime.
- `y`: copy the selected timeline/feed/Pulse message body to the terminal
  clipboard.
- `Esc`: leave a thread, or return focus to the sidebar.
- `/`: focus search. Type a query and press `Enter` to run it; press `Enter`
  on a search result to open its thread when the result includes a channel id.
- `O`: search visible channels by name. Type a query and press `Enter`, then
  use `Up` / `Down` and `Enter` to open a result without replacing the normal
  sidebar context.
- `n`: create a channel. Use `Tab` to move through name, type, visibility,
  expiry, and description; `F2` cycles stream/forum, `F3` cycles open/private,
  `F4` cycles permanent/7 days/1 day/1 hour, and `Enter` creates it.
- `m`: open a direct message. Type a 64-character pubkey and press `Enter`.
- `v`: load the selected channel canvas. Press `Enter` while viewing to edit;
  press `Enter` while editing to save.
- `w`: load workflows for the active channel. Use `Up` / `Down` to select a
  workflow, `Enter` to trigger it with `{}`, `I` to trigger with JSON inputs,
  `G` to grant an approval token, `X` to deny one, `A` to create YAML, `E` to
  edit YAML, and `D` to delete it. While editing YAML, arrow keys move the
  cursor, `Alt+Enter` inserts a newline, `F2` loads a scheduled digest template,
  `F3` loads a webhook digest template, and `F4` restores the basic starter
  template.
- `f`: load the feed inbox into the main timeline. Use `Up` / `Down` to move
  through items and `Enter` to open the referenced thread when the event
  includes a channel id.
- `F`: cycle the feed filter through all, mentions, needs-action, activity, and
  agent activity.
- `T`: load the Pulse timeline from recent kind-1 notes published by your
  selected Pulse source. Use `Up` / `Down` to move through notes.
- `S`: while Pulse is focused, cycle Pulse source through people you follow,
  your own notes, and managed-agent notes.
- `c`: while Pulse is active, compose a new social note instead of a channel
  message.
- `R`: while Pulse is active, reply to the selected note.
- `N`: load your long-form notes. Use `Up` / `Down` to move through note
  previews.
- `A` / `E` / `D`: when the notes panel is focused, create, edit, or delete a
  long-form note. The editor uses `Tab` / `Shift+Tab` to move through slug,
  title, summary, tags, and body fields.
- `G`: load NIP-34 repository announcements. Use `Up` / `Down` to move through
  announcements.
- `A`: when the repos panel is focused, announce or update a repository. The
  editor uses `Tab` / `Shift+Tab` to move through id, name, description, clone
  URLs, web URL, and relay fields.
- `M`: load decrypted memory for the selected managed agent. Select a managed
  agent in the ACP runtime panel first; use `Up` / `Down` to move through
  memory slugs. Use `A` to create a memory, `E` to edit the selected memory,
  and `D` to tombstone it.
- `Y`: load the custom emoji palette. My emoji are listed first, followed by
  workspace emoji from other members. Press `Enter` to react to the selected
  timeline/feed/Pulse item with the selected custom emoji.
- `A` / `I` / `D`: when the emoji panel is focused, add/update your emoji with
  `shortcode` and image URL fields, import a JSON emoji file, or remove the
  selected emoji from your own set. In the import prompt, `F2` toggles merge vs
  replace mode.
- `P`: load your profile. Use `Up` / `Down` to select display name, about,
  avatar URL, or NIP-05; press `Enter` to edit and `Enter` again to save.
- `A`: when the profile panel is focused, upload an avatar image through the
  relay media endpoint and save the returned URL on your profile.
- `s`: when the profile panel is focused, cycle your presence through online,
  away, and offline, then refresh the relay presence snapshot.
- `C`: load your contact list. Use `Up` / `Down` to select a contact and
  `Enter` to open a DM.
- `A` / `D`: when the contacts panel is focused, add a contact with
  `pubkey [relay_url] [petname]` or remove the selected contact.
- `U`: view a user profile. From the timeline, feed, Pulse, or contacts panel
  this opens the selected author/contact profile; elsewhere it opens a pubkey
  or display-name lookup prompt. Press `Enter` on a viewed profile to open a
  DM.
- `o`: toggle the sidebar between joined conversations and open channels.
- `j`: when the sidebar is focused, join the selected channel.
- `l`: when the sidebar is focused on joined conversations, leave the selected
  channel.
- `h`: when the sidebar is focused on joined conversations, hide the selected
  DM.
- `S` / `M`: when the sidebar is focused, star or mute the selected
  conversation through Desktop-compatible NIP-78 channel preference state.
- `A` / `V`: when the sidebar is focused on a channel, assign it to a custom
  section or remove it from its section. Assignment accepts an existing section
  name/id or creates a new section from the typed name. Section assignments are
  shown as sidebar `[section]` markers.
- `Space`: when the sidebar is focused, toggle the selected conversation between
  read and unread. Marking read advances NIP-RS read-state; forcing unread is a
  local TUI override because relay read-state only moves forward.
- `E` / `D`: when the sidebar is focused on a channel, update its name or
  description.
- `t` / `p`: when the sidebar is focused on a channel, set its topic or
  purpose.
- `u`: when the sidebar is focused on a channel, add a member. Input accepts
  `pubkey` or `pubkey role`.
- `x`: when the sidebar is focused on a channel, remove a member by pubkey.
- `z` / `Z`: when the sidebar is focused on a channel, archive or unarchive it.
- `c`: focus the composer.
- Composer text is remembered per workspace, channel, and thread while the TUI
  is running; channels with saved text show a `draft` marker in the sidebar.
- `B`: attach one or more files to the selected channel/thread. Type
  whitespace-separated paths and press `Enter`; current composer text is used
  as the optional caption.
- `I`: compose a code diff event for the selected channel/thread. The editor
  uses `Tab` / `Shift+Tab` to move through repo URL, commit SHA, optional file,
  optional description, and diff body.
- `e`: when the timeline is focused, edit the selected message in the composer.
- `d`: when the timeline is focused, delete the selected message.
- `+` / `-`: when the timeline is focused, add or remove a default `+`
  reaction on the selected message.
- `]` / `[`: when the timeline or feed is focused, upvote or downvote the
  selected forum post/comment.
- `a`: focus the ACP runtime panel and show the selected agent's log/details.
- `Enter`: when the ACP runtime panel is focused, start or stop the selected
  runtime. Local harness exits show the last exit code or signal in the detail
  panel.
- `A`: when the ACP runtime panel is focused on a runtime template, create a
  managed agent from it. Type a name, optionally set model and system prompt
  fields with `Tab`, choose its response policy with `F3`, optionally enter
  allowlist pubkeys, toggle start-on-launch with `F2`, toggle threaded direct
  mentions with `F4`, and press `Enter`.
- `D`: when the ACP runtime panel is focused on a managed agent, stop it if
  needed and delete its local record.
- `s`: when the ACP runtime panel is focused, toggle start-on-launch for the
  selected managed agent.
- `@`: when the ACP runtime panel is focused on a managed agent, insert that
  agent into the channel composer as a NIP-27 `nostr:npub...` mention.
- `r`: refresh channels, messages, and feed.
- `q` / `Ctrl-C`: quit.

## Current Surface

The first terminal surface covers the core collaborative loop:

- joined channel list, open channel discovery, and channel browser search;
- channel creation with type, visibility, and description fields;
- channel joins, leaves, metadata, member summaries, archive/unarchive, topic,
  purpose, and member add/remove writes;
- channel canvas view/edit;
- channel workflow list, run preview, create/update/delete, starter/digest YAML
  templates, default trigger, JSON input trigger, and approval grant/deny;
- own long-form note previews and create/update/delete;
- current profile view/edit, avatar upload, presence snapshots, and presence
  updates;
- contact-list previews and writes, preserving the current list while
  adding/updating or removing one contact at a time;
- contact-driven DM opens, DM list, DM creation/opening, and DM hiding;
- selected author/contact profile lookup plus manual display-name lookup;
- global repository announcement discovery and create/update;
- message timeline and thread view;
- message search from relay Nostr filters, with result-thread opens through
  normalized `channel_id` fields;
- feed inbox views from relay queries, including `mentions`, `needs_action`,
  `activity`, and `agent_activity` filters;
- feed-result thread opens through normalized `channel_id` fields;
- Pulse people, mine, and managed-agent timelines from direct relay social and
  contact queries;
- Pulse social note publish/reply through direct relay events;
- Pulse note upvote/unlike through reaction events;
- message sending through direct relay events, with thread replies resolved
  from the parent event;
- local channel/thread draft preservation around the composer, keyed by TUI
  workspace and visible through sidebar `draft` markers;
- NIP-RS read-state sync through direct relay app-data events, with local
  manual-unread overrides for sidebar `new` markers;
- Desktop-compatible NIP-78 channel star/mute sync through
  direct relay app-data events, visible as sidebar `star` and `muted` markers;
- Desktop-compatible NIP-78 custom channel section reads/writes through
  direct relay app-data events, visible as sidebar `[section]` markers;
- file attachments, code diff messages, edits, deletes, reactions, and forum
  votes through direct relay events;
- channel name/description/topic/purpose/member/policy updates through direct
  relay events;
- selected-message reaction counts and feed previews from relay queries;
- local ACP harness toggles from the TUI managed-agent store and built-in
  runtime templates;
- selected managed-agent details and log tail from TUI-managed process state;
- selected managed-agent memory listing, reads, writes, patches, and tombstones
  through encrypted engram events, using the selected managed agent's stored key
  and auth tag;
- custom emoji workspace palette, add/update/import/remove, export, and
  reactions through direct relay events;
- managed-agent creation, deletion, and start-on-launch toggles through the
  local TUI store, including optional model, system prompt, response policy,
  response allowlist, and start-on-launch flags;
- managed-agent composer mentions using inline NIP-27 `nostr:npub...` refs,
  which the TUI resolves into mention `p` tags;
- runtime template toggles for known local ACP adapters.

## Integration Notes

Read-state uses the same NIP-78/NIP-RS event surface as Desktop:

The TUI merges fetched contexts into its workspace file as a cache and publishes
channel read frontiers when a conversation is opened or explicitly marked read.
Manual unread markers stay local because NIP-RS timestamps are monotonic.

Channel stars and mutes also use Desktop-compatible NIP-78 encrypted app data:

The payloads use the existing Desktop d-tags, `channel-stars` and
`channel-mutes`, so either client can update the same sidebar preferences.

Custom channel sections use the same encrypted app-data path and d-tag as
Desktop:

The TUI currently renders section assignments in the flat sidebar.

Managed-agent records are stored in a TUI-owned local JSON file. On startup, the
TUI starts records marked `start_on_launch` directly through the local ACP
supervisor. When records exist, it launches those configured agents with their
stored relay URL, ACP command, runtime command/args, MCP command, private key,
auth tag, system prompt, and timeout. Runtime templates remain visible alongside
managed records so users can create additional managed agents or launch ad hoc
local harnesses.

Restore failures are shown in the TUI status line after startup, and the
selected managed-agent details panel includes the last recorded lifecycle
error.
Local ad hoc ACP harnesses also retain the last observed exit reason from the
process supervisor, so a runtime that exits unexpectedly shows its exit code,
signal, or wait error in the details panel.
Press `s` in the agents panel to toggle whether the selected managed agent is
started automatically on future TUI launches.

The remaining Desktop-specific gap is full process supervision parity: Desktop
still has richer provider-backed deployment, persona/team integration, and
native OS integration under `desktop/src-tauri/src/managed_agents`. Those should
move behind shared local abstractions incrementally so `buzz-tui` can consume
them the same way it consumes channels and messages today.
