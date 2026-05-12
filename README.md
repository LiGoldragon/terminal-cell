# terminal-cell

Low-level durable terminal cell primitive: daemon-owned PTY lifecycle,
transcript capture, raw programmatic input, and attach without terminal
multiplexer UI.

Status: this is a production-candidate low-level terminal cell for live human
TUI use. The first
`terminal-cell-view` relay design was rejected after manual Pi testing showed
slow, dropped, and stalled keyboard interaction. The current view sends one
attach request, pumps raw bytes over one Unix stream, and forwards terminal
resize events to the daemon; transcript and actor logic observe around that
path instead of rendering the human session through transcript subscriptions.
The daemon also still accepts `signal-persona-terminal` control frames for
prompt patterns, input gate leases, write injection, capture/resize, and worker
lifecycle subscription. That direct Signal endpoint is transitional witness
code retained while `persona-terminal` takes over the production control plane.
The production Persona endpoint is `persona-terminal`; attached viewer bytes
remain raw.

Do not treat automated Ghostty/Pi witnesses as final proof of a usable human
attach primitive. They are diagnostics for launch, transcript, injection,
resize, and exit behavior. The durable session command is the manual acceptance
surface for typing responsiveness.

Run the witness suite:

```sh
nix flake check
nix run .#production-witnesses
nix run .#session-witnesses
nix run .#agent-terminal-witness
nix run .#daemon-witness
nix run .#signal-control-plane-witness
nix run .#signal-worker-lifecycle-witness
nix run .#raw-data-plane-witness
nix run .#ghostty-agent-witness
```

The Signal and raw-data-plane witnesses allocate a host PTY, so they live as
Nix apps rather than pure Nix builder checks.

Live coding-agent witness:

```sh
nix run .#live-coding-agent-witness
nix run .#live-pi-agent-witness
```

This starts the real Codex CLI by default, injects a prompt through the
terminal-cell socket, waits for a marker that only the model response should
spell, and writes the captured transcript to
`target/live-coding-agent-witness/transcript.txt`.
Override the command path with `TERMINAL_CELL_AGENT_BIN` when testing another
Codex-compatible coding-agent CLI.

The Pi witness starts the real Pi TUI with `--offline --thinking off
--no-tools`, injects a prompt through the same socket, and writes
`target/live-pi-agent-witness/transcript.txt`.

Manual Ghostty attach demo:

```sh
nix run .#ghostty-agent-demo
```

Durable visible Ghostty session:

```sh
nix run .#ghostty-agent-session
nix run .#list-ghostty-agent-sessions
nix run .#rename-ghostty-agent-session -- terminal-cell-main
nix run .#reattach-ghostty-agent-session
nix run .#terminal-cell-resize -- --socket "$TERMINAL_CELL_SOCKET" --rows 41 --columns 113
nix run .#close-ghostty-agent-sessions
```

The session command starts the real Pi TUI in a daemon-owned terminal cell,
attaches a Ghostty view, and leaves the daemon, socket, view, and initial
transcript under `${XDG_RUNTIME_DIR:-/tmp}/terminal-cell/session-*` until the
close app is run. Closing the Ghostty window detaches only the view; the
reattach app opens a new Ghostty view against the newest live session socket
and skips stale runtime directories whose daemon process is gone.
Name a session at launch with `TERMINAL_CELL_SESSION_NAME`; rename the newest
live session with the rename app, or pass an explicit session path as the
second argument. Override Pi with
`TERMINAL_CELL_PI_BIN`, `TERMINAL_CELL_PI_MODEL`, or
`TERMINAL_CELL_PI_WORKSPACE`.

`terminal-cell-resize` sends a daemon resize control request. It does not
require an attached Ghostty view.

This component does not have a Sema database. Session listing and naming are
local runtime-directory metadata (`session.name`, `session.env`, pid files, and
`cell.sock`). The production registry belongs in a `persona-terminal`
supervisor daemon with Sema-owned session state.

On Niri, prevent the demo window from stealing focus with a targeted rule:

```kdl
window-rule {
    match app-id=r#"^com\.ligoldragon\.terminalcell(witness|session|pi)$"#
    open-focused false
}
```

The Ghostty app ID can be overridden for experiments:

```sh
TERMINAL_CELL_GHOSTTY_CLASS=com.example.terminalcelltest nix run .#ghostty-agent-witness
```
