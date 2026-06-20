# buzz-tui Completion Plan

## Goal

Build `buzz-tui` as a standalone, high-quality terminal client for Buzz.
The TUI should be direct-to-relay, live, and self-contained while staying
compatible with the same event formats used by Desktop and `buzz-cli`.

`buzz-cli` may be used as a reference or temporary fallback, but it should not
be the primary runtime wiring for the finished TUI.

## Scope

- Add and complete `crates/buzz-tui`.
- Keep `crates/buzz-cli` source unchanged.
- Allow only minimal repository wiring outside the TUI crate:
  - root `Cargo.toml`
  - `Cargo.lock`
  - `justfile` entries for running the TUI
- Use existing workspace crates where they fit:
  - `nostr`
  - `buzz-sdk`
  - `buzz-ws-client`
  - `buzz-core`

## Architecture

- `buzz-tui` owns its backend client and application state.
- Direct relay access is the primary transport:
  - HTTP `/query`
  - HTTP `/events`
  - HTTP `/count`
  - media upload where needed
  - websocket subscriptions for live events
- Signing and auth happen inside `buzz-tui`:
  - Nostr event signing
  - NIP-98 HTTP request auth
  - NIP-OA auth-tag forwarding
- `buzz-acp` remains a subprocess for long-running managed agents.
- Secrets must be passed through environment variables only, never argv:
  - `BUZZ_PRIVATE_KEY`
  - `BUZZ_AUTH_TAG`
  - `BUZZ_RELAY_URL`

## Implementation Plan

1. **Lock the crate boundary**
   - [x] Keep TUI feature code in `crates/buzz-tui`.
   - [x] Remove or avoid any `buzz-cli` implementation changes.
   - [x] Keep the current CLI-spawn adapter only as a temporary compatibility path
     while native relay operations are added.

2. **Add a TUI backend boundary**
   - [x] Introduce a TUI-local backend trait for the app state to call.
   - [x] Cover channels, DMs, messages, threads, reactions, search, feed, Pulse,
     workflows, profile, presence, contacts, emoji, repos, read-state,
     channel preferences, channel sections, agent logs, and memory.
   - [x] Keep command construction, relay requests, parsing, and auth out of the
     UI/state code.

3. **Build the native relay client**
   - [x] Implement a `TuiRelayClient` around `reqwest`, `nostr`, and
     `buzz-ws-client`.
   - [x] Implement direct `/query`, `/events`, and `/count` helpers.
   - [x] Implement upload helpers.
   - [x] Implement NIP-98 signing for HTTP requests.
   - [x] Forward NIP-OA auth tags as relay headers when configured.
   - [x] Normalize relay events into TUI view models locally.
   - [x] Prefer constants and builders from existing Buzz crates over duplicated
     magic numbers.

4. **Add live updates**
   - [x] Subscribe to active channel messages.
   - [x] Subscribe to joined-channel live updates.
   - [x] Subscribe to mention events.
   - [x] Subscribe to read-state, channel star/mute, and channel section app-data.
   - [x] Subscribe to presence and user status.
   - [x] Subscribe to custom emoji updates.
   - [x] Use polling only as reconnect/backstop behavior.
   - [x] On reconnect, run catch-up fetches and merge/dedupe results.

5. **Make state modular and testable**
   - [x] Split the current large app state into focused modules:
     - [x] channels/sidebar
     - [x] timeline/thread
     - [x] composer
     - [x] agents
     - [x] workspaces
     - [x] profile/social
     - [x] workflows
   - [x] Keep current read-state, preference, draft, workspace, and editor state
     transitions testable without terminal rendering.
   - [x] Reset workspace-scoped state and subscriptions on workspace switch.

6. **Implement Desktop-compatible data paths**
   - [x] Implement NIP-RS read-state natively in the TUI.
   - [x] Implement Desktop-compatible NIP-78 channel stars, mutes, and sections
     using the same d-tags and payload shapes.
   - [x] Keep manual unread overrides local to the TUI.
   - [x] Keep agent memory event formats compatible with existing Desktop and CLI
     behavior.

7. **Complete managed agent support**
   - [x] Let the TUI own terminal managed-agent configuration.
   - [x] Start and stop `buzz-acp` directly.
   - [x] Store agent records with:
     - name
     - pubkey
     - relay URL
     - ACP command
     - agent command and args
     - MCP command
     - system prompt
     - response policy
     - start-on-launch
     - log path
     - per-agent credential references or values
   - [x] Pass credentials to agent processes through env only.

## Feature Priorities

### Must Have

- Channels, DMs, messages, and threads
- Send, edit, delete, and react to messages
- Search
- Feed and Pulse
- Read-state
- Channel stars, mutes, and sections
- Workflows
- Profile, presence, and contacts
- Agents, logs, and memory
- Custom emoji
- Attachments and diffs

### Next

- Older history pagination
- Forum thread mode
- Project detail view
- Deep links
- Mention/channel/autocomplete support
- Better markdown, code, and diff rendering
- Confirmations for destructive actions

### Non-Goals For TUI v1

- Huddle audio controls
- Updater UI
- Mobile pairing UI
- Animated avatar editor
- Full Desktop settings surface

## Testing

- [x] Run `cargo check -p buzz-tui`.
- [x] Add unit tests for:
  - [x] event builders
  - [x] NIP-98 auth
  - [x] read-state merging
  - [x] channel star/mute/section payloads
  - [x] workspace reset
  - [x] agent spawn environment
  - [x] timeline merge/dedupe behavior
  - [x] active-channel live subscription target and merge behavior
  - [x] workspace live event invalidation behavior
  - [x] live reconnect cursor behavior
  - [x] managed-agent record storage
  - [x] agent memory credential routing
- [x] Run a live relay smoke test:
  - [x] connect to a relay
  - [x] receive a message without manual refresh
  - [x] send a message
  - [x] open a thread
  - [x] mark a channel read
  - [x] star and mute a channel
  - [x] switch workspaces
  - [x] start and stop a managed agent

## Completion Criteria

- `buzz-tui` runs without requiring `buzz-cli` as its primary runtime backend.
- Live message updates arrive through websocket subscriptions.
- Core terminal workflows are complete and keyboard-first.
- Desktop-compatible relay data remains compatible across Desktop, CLI, and TUI.
- Secrets never appear in subprocess argv.
- `cargo check -p buzz-tui` passes.
