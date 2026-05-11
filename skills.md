# terminal-cell skill

Work here when testing the minimal terminal-owner shape.

Rules for work here:

- Keep lifecycle and control state actor-shaped.
- Keep the live human attach path out of actor mailboxes, transcript replay,
  and screen projection. It is a raw byte pump with minimal attach/detach/resize
  framing.
- Route live human input from the attach stream into `TerminalInputPort`, then
  into the dedicated `TerminalInputWriter` that owns the PTY input gate. Do not
  reintroduce a `Message<TerminalInput>` actor-mailbox path for attached
  keyboard bytes.
- Treat PTY output bytes as transcript truth, but record transcript as a
  side-channel observer of the live path.
- Treat screen snapshots as derived projections, not as live display state.
- Keep command-line tools as daemon clients. The daemon owns the Kameo
  `TerminalCell`; clients send socket requests.
- Announce daemon readiness only after the actor startup hook has completed.
- For GUI witnesses, make the view process push an attachment-ready signal
  before the script injects input.
- Keep the short Ghostty witness and durable Ghostty session separate: the
  witness cleans up automatically; the session stays alive for human
  inspection and has an explicit close app.
- Do not add Persona message, provider quota, or harness policy semantics here.
- Add every repeated test command through Nix, not as an ad hoc script.
