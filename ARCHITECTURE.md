# terminal-cell - architecture

*Durable terminal session experiments. The live viewer uses an abduco-like byte
pump while transcript and control state remain side-channel concerns.*

---

## 0 - Status

`terminal-cell` explores one narrow capability: a long-lived daemon owns a child
process group and PTY while disposable frontends attach, detach, inject bytes,
capture transcript, resize, and observe exit.

The current implementation proves several useful pieces:

- daemon-owned child PTY lifecycle;
- real Pi launch under Ghostty;
- transcript capture;
- programmatic input injection;
- a PTY-writer input gate for non-interleaved Persona injection;
- child-exit observation;
- resize plumbing;
- an attach stream that carries raw viewer bytes in both directions;
- explicit attach accept/reject before raw replay;
- durable detach and reattach;
- single active attached viewer authority;
- headless daemon resize control;
- live-session selection that skips stale runtime directories;
- behavioral witnesses for slow transcript subscribers, reattach, stale session
  rejection, and attached input under output load.

The previous live-viewer path was rejected after manual Pi testing in Ghostty
showed slow, dropped, and eventually stalled keyboard interaction:

```text
Ghostty tty -> terminal-cell-view -> socket protocol -> daemon -> child PTY
child PTY -> actor/transcript/subscription -> socket protocol -> terminal-cell-view -> Ghostty tty
```

That path puts terminal bytes behind application-level relay, transcript
subscription, and actor/control-plane scheduling. It is the wrong boundary for
human TUI interaction.

The current live path is:

```text
Ghostty tty <-> attach pump <-> daemon byte pump <-> child PTY
                                  |
                                  +-> transcript recorder
                                  +-> observer parser
                                  +-> actor/control plane
```

The live attach path moves raw content bytes between the viewer terminal and
the child PTY. Minimal framing for content, attach, detach, resize, exit, and
lifecycle is allowed. Attach is framed only long enough to return explicit
accept/reject; raw transcript replay starts after acceptance. Terminal escape
interpretation, transcript replay, screen projection, actor mailbox delivery,
and wait conditions stay off the hot path.

Abduco is the concrete reference. Its client and server move `MSG_CONTENT`
packets between stdin/stdout, a Unix socket, and the child PTY; the other
packet kinds only describe attach, detach, resize, exit, and pid. It operates
on the raw byte stream and does not parse terminal escape sequences.

## 1 - Ownership

The daemon owns the child process group, PTY, socket, and session lifecycle.
Command-line tools and GUI frontends are clients.

This component has no Sema database. A running session is discoverable through
its runtime directory under `${XDG_RUNTIME_DIR:-/tmp}/terminal-cell/session-*`.
That directory holds `cell.sock`, pid files, `session.env`, `session.name`, and
diagnostic logs. The list and rename tools inspect or update those files; they
are a local convenience registry, not durable system truth.

The production shape belongs in a higher-level `persona-terminal` supervisor.
`terminal-cell` remains the low-level PTY owner for one terminal cell. The
supervisor owns the global terminal registry: one well-known daemon socket,
named terminal cells, a Sema-owned session table, per-cell attach/control
handles, lifecycle health, and policy decisions about which viewer adapter
opens a visible terminal. `persona-terminal` talks through the
`signal-persona-terminal` contract; it does not move Persona messages or infer
harness quota meaning from terminal bytes.

The clean production split is:

```text
persona-terminal        registry, names, Sema state, lifecycle policy
signal-persona-terminal typed terminal requests and events
terminal-cell           one child process group, one PTY, raw attach/control
viewer adapters         disposable visible windows around the terminal owner
persona-system          OS facts such as focus and window state
persona-harness         harness-specific prompts, usage probes, quota parsing
```

This repository may seed the `terminal-cell` primitive inside that stack, but
it does not become the global `persona-terminal` registry by accretion.

The live byte pump owns the latency-sensitive path:

```text
attach stdin  -> attached Unix stream -> daemon socket -> child PTY
attach stdout <- attached Unix stream <- daemon socket <- child PTY
```

Human input reaches the PTY through the writer port, not through the
`TerminalCell` actor mailbox:

```text
terminal-cell-view stdin
  -> attached Unix stream
  -> TerminalCellConnection::attach_viewer
  -> TerminalInputPort
  -> TerminalInputWriter + TerminalInputGate
  -> child PTY writer
```

That shape is deliberate. The attached GUI terminal is latency-sensitive, and
the user must feel as if the keyboard is talking to the child TUI directly. The
writer port keeps one serialized PTY writer and one gate for human and
programmatic input, while avoiding Kameo mailbox scheduling, transcript replay,
screen projection, and wait conditions on the live keyboard path.

Programmatic input writes through the same `TerminalInputPort`. The difference
is source provenance (`Viewer` or `Programmatic`), not a different PTY writer.

Actors remain the right shape for state that lives across time, but they do not
render the human session or carry live keyboard bytes. Actor-owned concerns are
lifecycle, metadata, health, resize authority, child exit, waiters, transcript
append, and Persona control decisions.

The `TerminalCell` actor is also the supervisor-observation root for the
blocking workers around it. `TerminalInputWriter`, `TerminalOutputFanout`, the
PTY output reader, the child-exit watcher, the daemon socket accept loop, and
the attach connection pump stay blocking workers because they own OS I/O or
latency-sensitive byte movement. They report typed `TerminalWorkerLifecycle`
start/stop events to the actor, so worker failure or shutdown becomes
actor-observable state without putting every terminal byte through the actor
mailbox.

Transcript recording observes PTY output as a side effect. A slow transcript
sink records backpressure or loss; it does not slow the attached viewer.

The PTY write path also owns the input gate. Persona can temporarily close the
gate to attached human input, write an injected byte sequence, then reopen the
gate. This is writer arbitration, not terminal semantics: blocked human bytes
are either buffered in order or rejected with an explicit gate state, while the
injected bytes are written contiguously to the child PTY. The gate must sit at
the PTY writer, not in the viewer, so every frontend obeys the same rule.

One terminal cell has at most one active attached viewer. The active viewer is
the only human byte source for the cell. A second attach request while a viewer
is active receives an explicit rejection before replay or live bytes can cross.
When the active viewer disconnects, the daemon keeps the child PTY alive and a
later viewer can reattach, receive transcript replay, and continue sending
input.

## 2 - Current Components

These are the checked-in components:

- `TerminalCell` - Kameo actor that owns lifecycle, transcript, resize
  authority, diagnostic subscribers, exit state, and waiters.
- `TerminalTranscript` - append-only output log sequenced by
  `TerminalSequence`.
- `TerminalOutputPort` - typed ingress to the PTY-output fanout used by the
  daemon attach path.
- `TerminalOutputFanout` - non-actor thread that writes PTY output to attached
  viewer before sending the same bytes to the transcript actor.
- `TerminalViewerLease` - active-viewer authority returned by the output fanout
  and released when the attach stream ends.
- `TranscriptSubscription` - replay plus live delta receiver for diagnostics.
- `ScreenProjection` - derived `vt100` snapshot over transcript bytes.
- `TerminalInput` - raw bytes plus source provenance, written to the PTY.
- `TerminalInputPort` - typed ingress to the PTY writer.
- `TerminalInputWriter` - blocking PTY writer plane that owns the input gate
  and serializes all human and programmatic bytes.
- `TerminalWorkerLifecycle` / `TerminalWorkerObservation` - actor-recorded
  lifecycle state for blocking PTY and daemon workers.
- `TerminalInputGateLease` - writer-side lease proving human input is closed
  before a programmatic injection sequence.
- `TerminalInputGateRelease` - writer-side release record naming the lease and
  how many held human bytes were flushed when the gate reopened.
- `TerminalExit` - recorded child status.
- `TerminalCellSocketClient` - Unix-socket client used by command-line tools
  and viewers.
- `terminal-cell-daemon` - daemon that owns the `TerminalCell` actor and serves
  socket requests.
- `terminal-cell-send` / `capture` / `wait` / `exit` - thin command-line
  clients.
- `terminal-cell-resize` - thin resize client that can resize the child PTY
  without an attached viewer.
- `terminal-cell-session-select` - runtime-directory selector used by reattach
  tooling to choose only live daemon-backed sessions.
- `terminal-cell-view` - interactive attach client. It sends one attach request,
  pumps stdin/stdout over the attached Unix stream, and forwards terminal
  `SIGWINCH` resize events to the daemon.
- `agent-terminal-fixture` - deterministic agent-like terminal process used by
  stateful witnesses.
- `output-flood-fixture` - deterministic high-volume output process used to
  prove attached input still reaches the child under output load.

## 3 - Constraints

- A terminal cell owns one child process group and PTY for the lifetime of the
  session.
- Output emitted while no viewer is attached is still available to transcript
  capture.
- A late viewer may receive transcript replay before live attach, but replay is
  not the live display path.
- Closing a viewer detaches only that viewer; it does not end the daemon-owned
  child PTY.
- A terminal cell admits one active attached viewer at a time.
- A rejected second viewer cannot send input to the child PTY.
- A rejected second viewer receives an explicit attach rejection before replay
  or raw output bytes.
- Human keyboard bytes and Persona programmatic input write to the same child
  PTY input path.
- Live human keyboard bytes enter `TerminalInputPort` directly from the attach
  connection; they do not go through a Kameo actor mailbox.
- Persona injection can acquire the PTY input gate so injected bytes are not
  interleaved with human keyboard bytes.
- The input gate is writer arbitration only; it does not parse slash commands
  or infer harness prompt state.
- The live attach path is a raw byte transport with only minimal session
  framing.
- The live attach path does not wait on actor handlers, transcript append,
  screen projection, waiters, or Persona decisions.
- A slow transcript subscriber does not block the attached viewer's output.
- High-volume child output still reaches the attached viewer while transcript
  subscribers are slow.
- High-volume child output does not starve attached input. Keyboard bytes sent
  through the attach stream still reach the child PTY promptly while output is
  flowing.
- Attached viewers push terminal resize events to the daemon; a PTY must not
  keep drawing at the size from initial attach after the GUI window changes.
- Resize is also a daemon control request; an attached viewer is not required
  to resize the child PTY.
- Reattach tooling selects only live sessions: a runtime directory with a socket
  but no live daemon process is stale and must be skipped.
- Terminal input is raw bytes; slash commands are harness input, not terminal
  owner semantics.
- Blocking PTY reads and writes are isolated from actor handlers.
- Blocking workers report typed start/stop lifecycle events to the
  `TerminalCell` actor. They are supervised by observation: failures become
  queryable terminal state instead of silent thread death.
- Daemon-owned blocking planes, including socket accept and attach pumping,
  use the same worker lifecycle channel instead of inventing a separate
  monitoring path.
- CLIs are daemon clients; they do not own the runtime or transcript.
- Child exit is pushed session state; clients do not poll process tables.
- GUI witness readiness is a pushed event from the attached view process, not a
  polling sleep.

## 4 - Witnesses

Current useful witnesses:

- `detached_output_is_replayed_to_late_subscriber`
- `programmatic_input_uses_the_same_pty_input_port`
- `screen_projection_is_derived_from_transcript`
- `terminal_exit_is_observable_without_polling_the_child`
- `terminal_worker_lifecycle_is_actor_observable`
- `agent_terminal_accepts_prompt_and_terminal_cell_reads_response`
- `agent_terminal_usage_probe_is_prompt_input_not_terminal_semantics`
- `daemon_accepts_programmatic_prompt_and_capture_reads_transcript`
- `attach_stream_is_raw_bidirectional_byte_path`
- `input_gate_holds_human_bytes_during_programmatic_injection`
- `daemon_exposes_terminal_exit_status`
- `daemon_resizes_the_owned_pty`
- `detached_viewer_leaves_daemon_alive_and_late_viewer_receives_replay`
- `second_attached_viewer_is_rejected_while_first_viewer_is_active`
- `headless_resize_cli_resizes_without_attached_viewer`
- `slow_transcript_subscriber_does_not_block_attached_viewer_output`
- `attached_input_reaches_child_during_high_volume_output`
- `daemon_worker_lifecycle_is_observable_over_socket`
- `session_selector_skips_newer_stale_sessions`
- `nix run .#production-witnesses`
- `nix run .#live-coding-agent-witness`
- `nix run .#live-pi-agent-witness`
- `nix run .#ghostty-agent-witness`
- `nix run .#ghostty-agent-session`

Rejected as acceptance evidence for live attach:

- `attach_view_replays_transcript_without_owning_the_child`
- the removed persistent viewer-input stream, because it tested only input and
  left output behind transcript subscription.

Required next witnesses:

- Manual Pi TUI in Ghostty accepts human typing immediately and losslessly.
- The same session accepts Persona programmatic input.
- A gated Persona injection is delivered contiguously while simultaneous human
  input is buffered or rejected according to the gate state.

## Code Map

```text
src/lib.rs        public surface
src/error.rs      typed errors
src/session.rs    TerminalCell actor and terminal records
src/socket.rs     Unix-socket request/reply protocol
src/client.rs     socket client used by CLIs/viewers
src/snapshot.rs   vt100 screen projection
src/bin/          daemon, clients, view, and deterministic fixture
tests/            architecture witnesses
```
