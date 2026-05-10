# terminal-cell-lab

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

On Niri, prevent the demo window from stealing focus with a targeted rule:

```kdl
window-rule {
    match app-id=r#"^com\.ligoldragon\.terminalcellwitness$"#
    open-focused false
}
```

The Ghostty app ID can be overridden for experiments:

```sh
TERMINAL_CELL_GHOSTTY_CLASS=com.example.terminalcelltest nix run .#ghostty-agent-witness
```
