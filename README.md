# terminal-cell

Prototype workspace for durable terminal session ownership: daemon-owned PTY
lifecycle, transcript capture, raw programmatic input, and attach experiments
without terminal multiplexer UI.

Status: this is an attach spike for live human TUI use. The first
`terminal-cell-view` relay design was rejected after manual Pi testing showed
slow, dropped, and stalled keyboard interaction. The current view sends one
attach request, pumps raw bytes over one Unix stream, and forwards terminal
resize events to the daemon; transcript and actor logic observe around that
path instead of rendering the human session through transcript subscriptions.

Do not treat automated Ghostty/Pi witnesses as final proof of a usable human
attach primitive. They are diagnostics for launch, transcript, injection,
resize, and exit behavior. The durable session command is the manual acceptance
surface for typing responsiveness.

Run the witness suite:

```sh
nix flake check
nix run .#session-witnesses
nix run .#agent-terminal-witness
nix run .#daemon-witness
nix run .#ghostty-agent-witness
```

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
nix run .#reattach-ghostty-agent-session
nix run .#close-ghostty-agent-sessions
```

The session command starts the real Pi TUI in a daemon-owned terminal cell,
attaches a Ghostty view, and leaves the daemon, socket, view, and initial
transcript under `${XDG_RUNTIME_DIR:-/tmp}/terminal-cell/session-*` until the
close app is run. Closing the Ghostty window detaches only the view; the
reattach app opens a new Ghostty view against the newest live session socket.
Override Pi with
`TERMINAL_CELL_PI_BIN`, `TERMINAL_CELL_PI_MODEL`, or
`TERMINAL_CELL_PI_WORKSPACE`.

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
