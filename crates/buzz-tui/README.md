# buzz-tui

`buzz-tui` is a Ratatui terminal client for Buzz. It is intentionally a thin UI
over the existing process contracts:

- `buzz` for relay reads and writes. The TUI shells out to `buzz --format json`
  and parses the stable JSON response shapes.
- `buzz agents list --include-secrets` for configured managed agents, with
  `buzz agents runtimes` for runtime templates.
- `buzz-acp` for local agent harnesses. The TUI can start and stop local ACP
  harness processes from managed-agent records or directly from discovered
  Goose, Codex, and Claude Code runtimes.

This keeps the terminal app loosely coupled to relay internals and avoids
duplicating the REST, Nostr signing, and ACP bridge logic that already exists in
the CLI and harness.

## Run

Build the CLI, ACP harness, and TUI first:

```bash
cargo build -p buzz-cli -p buzz-acp -p buzz-tui
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
  --buzz-bin ./target/debug/buzz \
  --acp-bin ./target/debug/buzz-acp
```

Useful options:

- `--relay`: HTTP relay URL for `buzz`; converted to `ws://` or `wss://` for
  `buzz-acp`.
- `--buzz-bin`: path to the `buzz` binary.
- `--acp-bin`: path to the `buzz-acp` harness.
- `--mcp-command`: optional MCP server command exposed to started agents.
  Overrides any agent or runtime-specific MCP default reported by `buzz`.
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
- `Esc`: leave a thread, or return focus to the sidebar.
- `/`: focus search. Type a query and press `Enter` to run it; press `Enter`
  on a search result to open its thread when the result includes a channel id.
- `O`: search visible channels by name. Type a query and press `Enter`, then
  use `Up` / `Down` and `Enter` to open a result without replacing the normal
  sidebar context.
- `n`: create a channel. Use `Tab` to move through name, type, visibility, and
  description; `F2` cycles stream/forum, `F3` cycles open/private, and `Enter`
  creates it.
- `m`: open a direct message. Type a 64-character pubkey and press `Enter`.
- `v`: load the selected channel canvas. Press `Enter` while viewing to edit;
  press `Enter` while editing to save.
- `w`: load workflows for the active channel. Use `Up` / `Down` to select a
  workflow, `Enter` to trigger it with `{}`, `I` to trigger with JSON inputs,
  `G` to grant an approval token, `X` to deny one, `A` to create YAML, `E` to
  edit YAML, and `D` to delete it.
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
  `shortcode` and image URL fields, import a JSON file with `buzz emoji import`
  semantics, or remove the selected emoji from your own set. In the import
  prompt, `F2` toggles merge vs replace mode.
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
  read and unread. Marking read advances NIP-RS read-state through
  `buzz read-state mark`; forcing unread is a local TUI override because relay
  read-state only moves forward.
- `E` / `D`: when the sidebar is focused on a channel, update its name or
  description through `buzz channels update`.
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

- channel list from `buzz channels list --member`;
- channel creation from `buzz channels create` with type, visibility, and
  description fields;
- open channel discovery from `buzz channels list --visibility open`;
- channel browser search from `buzz channels search`;
- channel joins from `buzz channels join`;
- channel leaves from `buzz channels leave`;
- channel metadata and member summaries from `buzz channels get` and
  `buzz channels members`;
- channel topic, purpose, archive/unarchive, and member add/remove writes from
  the corresponding `buzz channels` subcommands;
- channel canvas view/edit from `buzz canvas get` and `buzz canvas set`;
- channel workflow list, run preview, create/update/delete, default trigger,
  JSON input trigger, and approval grant/deny from `buzz workflows list`,
  `buzz workflows runs`, `buzz workflows create`, `buzz workflows update`,
  `buzz workflows delete`, `buzz workflows trigger`, and
  `buzz workflows approve`;
- own long-form note previews from `buzz notes ls`;
- own long-form note create/update/delete from `buzz notes set --content -`
  and `buzz notes rm`;
- current profile view/edit from `buzz users get` and `buzz users set-profile`;
- presence snapshots and updates from `buzz users presence` and
  `buzz users set-presence`;
- contact-list previews from `buzz social contacts`;
- contact-list writes from `buzz social set-contacts`, preserving the current
  list while adding/updating or removing one contact at a time;
- contact-driven DM opens through `buzz dms open`;
- selected author/contact profile lookup from `buzz users get --pubkey`, plus
  manual display-name lookup from `buzz users get --name`;
- global repository announcement discovery from `buzz repos list --all`;
- repository announcement create/update from `buzz repos create`;
- DM list from `buzz dms list`;
- DM creation/opening from `buzz dms open`;
- DM hiding from `buzz dms hide`;
- message timeline from `buzz messages get`;
- thread view from `buzz messages thread`;
- message search from `buzz messages search`, with result-thread opens through
  normalized `channel_id` fields;
- feed inbox views from `buzz feed get`, including `mentions`, `needs_action`,
  `activity`, and `agent_activity` filters;
- feed-result thread opens through normalized `channel_id` fields;
- Pulse people, mine, and managed-agent timelines from `buzz social contacts`
  plus `buzz social notes --pubkey <author>`;
- Pulse social note publish/reply from `buzz social publish`;
- Pulse note upvote/unlike from `buzz reactions add` and `buzz reactions remove`;
- message sending through `buzz messages send --content -`, with thread
  replies routed through `--reply-to`;
- local channel/thread draft preservation around the composer, keyed by TUI
  workspace and visible through sidebar `draft` markers;
- NIP-RS read-state sync through `buzz read-state get` and
  `buzz read-state mark`, with local manual-unread overrides for sidebar `new`
  markers;
- Desktop-compatible NIP-78 channel star/mute sync through
  `buzz channel-prefs get` and `buzz channel-prefs set`, visible as sidebar
  `star` and `muted` markers;
- Desktop-compatible NIP-78 custom channel section reads/writes through
  `buzz channel-sections`, visible as sidebar `[section]` markers;
- file attachments through repeated `buzz messages send --file` flags, using
  the composer text as the caption;
- code diff messages through `buzz messages send-diff --diff -`;
- message edits through `buzz messages edit`;
- message deletes through `buzz messages delete`;
- channel name/description updates through `buzz channels update`;
- default reaction add/remove through `buzz reactions add` and
  `buzz reactions remove`;
- forum upvotes/downvotes through `buzz messages vote`;
- selected-message reaction counts from `buzz reactions get`;
- feed preview from `buzz feed get`;
- local ACP harness toggles from `buzz agents list --include-secrets`;
- selected managed-agent details and log tail from `buzz agents log`;
- selected managed-agent memory listing from `buzz mem ls --json --agent`,
  with decrypted value reads from `buzz mem get --agent`;
- selected managed-agent memory writes and tombstones through `buzz mem set`
  and `buzz mem rm`, executed with the selected managed agent's stored key and
  auth tag;
- custom emoji workspace palette from `buzz emoji list`;
- own custom emoji set from `buzz emoji export --scope own`;
- custom emoji add/update/import/remove from `buzz emoji set`,
  `buzz emoji import`, and `buzz emoji rm`;
- custom emoji reactions from `buzz reactions add --emoji-url`;
- managed-agent creation from selected runtime templates through
  `buzz agents create`, including optional model, system prompt, response
  policy, response allowlist, and start-on-launch flags;
- managed-agent deletion through `buzz agents delete`;
- managed-agent start-on-launch toggles from `buzz agents start-on-launch`;
- managed-agent composer mentions using inline NIP-27 `nostr:npub...` refs,
  which `buzz messages send` resolves into mention `p` tags;
- runtime template toggles from `buzz agents runtimes`.

## Integration Notes

Read-state uses the same NIP-78/NIP-RS event surface as Desktop:

```bash
buzz --format json read-state get
buzz --format json read-state mark --context <channel-id> --timestamp <unix-seconds>
```

The TUI merges fetched contexts into its workspace file as a cache and publishes
channel read frontiers when a conversation is opened or explicitly marked read.
Manual unread markers stay local because NIP-RS timestamps are monotonic.

Channel stars and mutes also use Desktop-compatible NIP-78 encrypted app data:

```bash
buzz --format json channel-prefs get --kind stars
buzz --format json channel-prefs set --kind stars --channel <channel-id> --enabled
buzz --format json channel-prefs get --kind mutes
buzz --format json channel-prefs set --kind mutes --channel <channel-id> --disabled
```

The payloads use the existing Desktop d-tags, `channel-stars` and
`channel-mutes`, so either client can update the same sidebar preferences.

Custom channel sections use the same encrypted app-data path and d-tag as
Desktop:

```bash
buzz --format json channel-sections get
buzz --format json channel-sections create --name Focus
buzz --format json channel-sections assign --channel <channel-id> --section <section-id>
buzz --format json channel-sections unassign --channel <channel-id>
```

The TUI currently renders section assignments in the flat sidebar; full section
grouped navigation can build on the same CLI JSON surface.

Managed-agent records now have a CLI-owned local JSON surface:

```bash
buzz --format json agents list
buzz --format json agents create --name Helper --runtime goose \
  --model gpt-5 --system-prompt "Stay concise" \
  --respond-to allowlist --respond-to-allowlist <pubkey> --start-on-launch
buzz --format json agents start --pubkey <agent-pubkey>
buzz --format json agents stop --pubkey <agent-pubkey>
buzz --format json agents log --pubkey <agent-pubkey>
buzz --format json agents restore
buzz --format json agents start-on-launch --pubkey <agent-pubkey> --enable
buzz --format json agents start-on-launch --pubkey <agent-pubkey> --disable
buzz --format json agents delete --pubkey <agent-pubkey>
```

On startup, the TUI first calls `buzz agents restore --include-secrets` so
records marked `--start-on-launch` are started through the CLI-owned lifecycle
surface. When records exist, it launches those configured agents with their
stored relay URL, ACP command, runtime command/args, MCP command, private key,
auth tag, system prompt, and timeout. Starting and stopping configured agents
goes through `buzz agents start` and `buzz agents stop`, so process state, PID,
last error, and log path stay in the CLI-owned JSON surface. Runtime templates
remain visible alongside managed records so users can create additional managed
agents or launch ad hoc local harnesses.

Restore failures are shown in the TUI status line after startup, and the
selected managed-agent details panel includes the last recorded lifecycle
error.
Local ad hoc ACP harnesses also retain the last observed exit reason from the
process supervisor, so a runtime that exits unexpectedly shows its exit code,
signal, or wait error in the details panel.
Press `s` in the agents panel to toggle whether the selected managed agent is
included in future `buzz agents restore` calls.

ACP runtime discovery now lives in `buzz-cli` as `buzz agents runtimes`, so the
TUI does not need to hardcode which local adapters exist or where they are
installed.

The remaining Desktop-specific gap is full process supervision parity: Desktop
still has richer provider-backed deployment, persona/team integration, and
native OS integration under `desktop/src-tauri/src/managed_agents`. Those should
move behind CLI JSON surfaces incrementally so `buzz-tui` can consume them the
same way it consumes channels and messages today.
