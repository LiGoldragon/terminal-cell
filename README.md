# terminal-cell

Prototype for a minimal durable terminal session owner: PTY ownership,
append-only transcript replay, and raw programmatic input without terminal
multiplexer UI.

Run the witness suite:

```sh
nix flake check
nix run .#session-witnesses
nix run .#agent-terminal-witness
nix run .#daemon-witness
nix run .#ghostty-agent-witness
```

Manual Ghostty attach demo:

```sh
nix run .#ghostty-agent-demo
```

Durable visible Ghostty session:

```sh
nix run .#ghostty-agent-session
nix run .#close-ghostty-agent-sessions
```

The session command leaves a daemon, Ghostty view, socket, and transcript under
`${XDG_RUNTIME_DIR:-/tmp}/terminal-cell/session-*` until the window is closed
or the close app is run.

On Niri, prevent the demo window from stealing focus with a targeted rule:

```kdl
window-rule {
    match app-id=r#"^com\.ligoldragon\.terminalcell(witness|session)$"#
    open-focused false
}
```

The Ghostty app ID can be overridden for experiments:

```sh
TERMINAL_CELL_GHOSTTY_CLASS=com.example.terminalcelltest nix run .#ghostty-agent-witness
```
