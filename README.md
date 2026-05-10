# terminal-cell

Prototype for a minimal durable terminal session owner: PTY ownership,
append-only transcript replay, and raw programmatic input without terminal
multiplexer UI.

Interactive viewers keep one persistent input stream to the daemon. Keyboard
bytes are accepted by the same typed PTY input port as programmatic bytes, not
by a per-key socket request that waits behind transcript work.

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
nix run .#close-ghostty-agent-sessions
```

The session command starts the real Pi TUI in a daemon-owned terminal cell,
attaches a Ghostty view, and leaves the daemon, socket, view, and initial
transcript under `${XDG_RUNTIME_DIR:-/tmp}/terminal-cell/session-*` until the
window is closed or the close app is run. Override Pi with
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
