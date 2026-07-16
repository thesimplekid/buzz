# buzz-acp-sandbox

Linux-only Bubblewrap wrapper for `buzz-acp`.

`buzz-acp-sandbox` is a drop-in ACP command for Desktop managed agents or the
TUI `--acp-bin` flag. It preserves the existing `buzz-acp` environment and
arguments, then launches the real harness inside a small Bubblewrap sandbox.

```bash
BUZZ_REAL_ACP=/path/to/buzz-acp buzz-acp-sandbox --respond-to owner-only
```

Configuration:

| Variable | Default | Description |
|----------|---------|-------------|
| `BUZZ_REAL_ACP` | `buzz-acp` on `PATH` | Real ACP harness to execute. |
| `BUZZ_SANDBOX_MODE` | `auto` | `auto`, `required`, or `disabled`. |
| `BUZZ_SANDBOX_ROOT` | `~/.config/buzz/sandboxes` | Host directory for sandbox homes and temp dirs. |
| `BUZZ_SANDBOX_ID` | derived | Stable directory name for this agent sandbox. |
| `BUZZ_SANDBOX_CWD` | `/home/buzz` | Working directory passed to the real ACP harness inside the sandbox. |
| `BUZZ_SANDBOX_BIND` | empty | Extra comma-separated binds such as `/repo:ro,/work:rw,/host=/sandbox:ro`. |

`auto` falls back to the real `buzz-acp` when Bubblewrap is unavailable or the
platform is not Linux. `required` fails clearly in those cases. `disabled`
always executes the real harness directly.

By default, the sandbox gets a private writable `/home/buzz` and `/tmp`, keeps
host networking, and does not bind the real host home. Runtime/system paths
such as `/usr`, `/bin`, `/lib`, `/etc`, and `/nix` are bound read-only when
present. Add explicit read-only or read-write paths with `BUZZ_SANDBOX_BIND`
when the agent needs workspace access. Prefer mounting host workspaces at a
neutral sandbox path, for example
`BUZZ_SANDBOX_CWD=/workspace/repo` with
`BUZZ_SANDBOX_BIND=/path/to/repo=/workspace/repo:rw`, so the sandbox does not
recreate host home-directory parent paths.
Bind targets and `BUZZ_SANDBOX_CWD` under `/home` are rejected unless they stay
inside `/home/buzz`.
If `BUZZ_ACP_AGENT_COMMAND` or `BUZZ_ACP_MCP_COMMAND` is an existing absolute
host executable path, the wrapper bind-mounts that executable into
`/run/buzz/bin` and rewrites the env var before launching `buzz-acp`, so agent
adapters and MCP tool servers run inside the same Bubblewrap namespace.
