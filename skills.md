# terminal-cell skill

Work here on the low-level terminal cell primitive that `terminal`
uses for durable PTY/session ownership.

## Two-plane wire shape

The daemon binds two Unix listeners: `control.sock` for Signal frames plus
the byte-tag CLI protocol, and `data.sock` for raw attached-viewer bytes.

- Bind both sockets at startup before writing `SessionRegistration` or
  enabling client traffic. Apply mode 0600 to both.
- Reject `Attach` on `control.sock` with `ATTACH_REJECTED_REPLY` before any
  byte transport begins; reject every non-`Attach` request on `data.sock`
  with the symmetric reply. Do not mode-shift one socket between roles.
- `TerminalCellSocketClient::new(control, data)` is the full client;
  `for_control_only(control)` is for clients that never attach. The
  control-only constructor returns `io::ErrorKind::Unsupported` from
  `open_attach_stream` — control-only clients cannot silently borrow the
  control socket as a data path.
- Local CLI binaries (`-send`, `-capture`, `-wait`, `-exit`, `-resize`,
  `-resolve`) take `--control-socket` only; `-view` takes both
  `--control-socket` and `--data-socket`.

## Viewer latency and transcript decoupling

- Live human attach is a raw byte pump with minimal attach/detach/resize
  framing. It does not traverse the Kameo actor mailbox.
- Route live human input from the attach stream into `TerminalInputPort`,
  then into the dedicated `TerminalInputWriter` that owns the PTY input
  gate. Do not reintroduce a `Message<TerminalInput>` actor-mailbox path
  for attached keyboard bytes.
- Treat PTY output bytes as transcript truth, but record transcript on a
  separate worker (`TranscriptScriber`) fed by a bounded notification
  queue from `ViewerFanout`. Drop-oldest on overflow; never block the
  viewer write on transcript work.
- Keep attached input responsive while PTY output is flowing. High-volume
  output must not put keyboard bytes behind transcript, projection, or
  observer work.
- Treat screen snapshots as derived projections, not as live display state.

## Actors, workers, subscriptions

- Keep lifecycle and control state actor-shaped. The `TerminalCell` actor
  owns transcript truth, worker-lifecycle observation, transcript and
  worker-lifecycle subscribers, resize authority, exit state, and waiters.
- Keep blocking OS-I/O planes as named, supervisor-observable workers when
  they carry raw bytes or block on the OS. The `TerminalCell` actor records
  `TerminalWorkerLifecycle` events; the worker owns the blocking byte pump.
  This applies to PTY workers, transcript scribers, and daemon workers such
  as socket accept loops and attach pumping.
- Subscription close is a typed retract/close request on the control plane.
  The server emits a final acknowledgement event and ends the stream. Raw
  socket close is not semantic protocol.
- One active attached viewer per terminal cell. A second attach receives an
  explicit rejection before any replay or live bytes can cross.

## Clients and scope

- Keep command-line tools as daemon clients. The daemon owns the Kameo
  `TerminalCell`; clients send socket requests.
- `signal-terminal` is the daemon's typed control surface. The
  byte-tag CLI protocol is a local convenience; Persona control is Signal.
- Announce daemon readiness only after the actor startup hook has
  completed and both listeners are bound.
- For GUI witnesses, make the view process push an attachment-ready
  signal before the script injects input.
- Keep the short Ghostty witness and durable Ghostty session separate: the
  witness cleans up automatically; the session stays alive for human
  inspection and has an explicit close app.
- Do not add Persona message, provider quota, or harness policy semantics
  here. Slash commands are harness input, not terminal-owner semantics.
- Add every repeated test command through Nix, not as an ad hoc script.
