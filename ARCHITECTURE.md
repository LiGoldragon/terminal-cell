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
- resize plumbing.
- an attach stream that carries raw viewer bytes in both directions.

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
lifecycle is allowed. Terminal escape interpretation, transcript replay,
screen projection, actor mailbox delivery, and wait conditions stay off the
hot path.

Abduco is the concrete reference. Its client and server move `MSG_CONTENT`
packets between stdin/stdout, a Unix socket, and the child PTY; the other
packet kinds only describe attach, detach, resize, exit, and pid. It operates
on the raw byte stream and does not parse terminal escape sequences.

## 1 - Ownership

The daemon owns the child process group, PTY, socket, and session lifecycle.
Command-line tools and GUI frontends are clients.

The live byte pump owns the latency-sensitive path:

```text
attach stdin  -> attached Unix stream -> daemon socket -> child PTY
attach stdout <- attached Unix stream <- daemon socket <- child PTY
```

Actors remain the right shape for state that lives across time, but they do not
render the human session. Actor-owned concerns are lifecycle, metadata, health,
resize authority, child exit, waiters, transcript sink supervision, and Persona
control decisions.

Transcript recording observes PTY output as a side effect. A slow transcript
sink records backpressure or loss; it does not slow the attached viewer.

Programmatic input writes to the same child PTY write path as human input, with
source provenance outside the live transport.

The PTY write path also owns the input gate. Persona can temporarily close the
gate to attached human input, write an injected byte sequence, then reopen the
gate. This is writer arbitration, not terminal semantics: blocked human bytes
are either buffered in order or rejected with an explicit gate state, while the
injected bytes are written contiguously to the child PTY. The gate must sit at
the PTY writer, not in the viewer, so every frontend obeys the same rule.

## 2 - Current Spike Components

These are the checked-in components of the attach spike:

- `TerminalCell` - Kameo actor that owns the PTY writer, transcript, resize
  authority, diagnostic subscribers, and waiters.
- `TerminalTranscript` - append-only output log sequenced by
  `TerminalSequence`.
- `TerminalOutputPort` - typed ingress to the PTY-output fanout used by the
  daemon attach path.
- `TerminalOutputFanout` - non-actor thread that writes PTY output to attached
  viewers before sending the same bytes to the transcript actor.
- `TranscriptSubscription` - replay plus live delta receiver for diagnostics.
- `ScreenProjection` - derived `vt100` snapshot over transcript bytes.
- `TerminalInput` - raw bytes plus source provenance, written to the PTY.
- `TerminalInputPort` - typed ingress to the PTY writer.
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
- `terminal-cell-view` - interactive attach client. It sends one attach request,
  pumps stdin/stdout over the attached Unix stream, and forwards terminal
  `SIGWINCH` resize events to the daemon.
- `agent-terminal-fixture` - deterministic agent-like terminal process used by
  stateful witnesses.

## 3 - Constraints

- A terminal cell owns one child process group and PTY for the lifetime of the
  session.
- Output emitted while no viewer is attached is still available to transcript
  capture.
- A late viewer may receive transcript replay before live attach, but replay is
  not the live display path.
- Human keyboard bytes and Persona programmatic input write to the same child
  PTY input path.
- Persona injection can acquire the PTY input gate so injected bytes are not
  interleaved with human keyboard bytes.
- The input gate is writer arbitration only; it does not parse slash commands
  or infer harness prompt state.
- The live attach path is a raw byte transport with only minimal session
  framing.
- The live attach path does not wait on actor handlers, transcript append,
  screen projection, waiters, or Persona decisions.
- Attached viewers push terminal resize events to the daemon; a PTY must not
  keep drawing at the size from initial attach after the GUI window changes.
- Terminal input is raw bytes; slash commands are harness input, not terminal
  owner semantics.
- Blocking PTY reads and writes are isolated from actor handlers.
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
- `agent_terminal_accepts_prompt_and_terminal_cell_reads_response`
- `agent_terminal_usage_probe_is_prompt_input_not_terminal_semantics`
- `daemon_accepts_programmatic_prompt_and_capture_reads_transcript`
- `attach_stream_is_raw_bidirectional_byte_path`
- `input_gate_holds_human_bytes_during_programmatic_injection`
- `daemon_exposes_terminal_exit_status`
- `daemon_resizes_the_owned_pty`
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
- High-volume output does not make keyboard input lag.
- A deliberately slow transcript sink does not affect the attached viewer.
- Source inspection of the live path finds no actor mailbox, transcript replay,
  screen projection, or wait condition between attach stdin/stdout and the child
  PTY.

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
