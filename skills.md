# terminal-cell-lab skill

Work here when testing the minimal terminal-owner shape.

Rules for work here:

- Keep every stateful runtime plane actor-shaped.
- Treat PTY output bytes as transcript truth.
- Treat screen snapshots as derived projections.
- Keep command-line tools as daemon clients. The daemon owns the Kameo
  `TerminalCell`; clients send socket requests.
- Announce daemon readiness only after the actor startup hook has completed.
- For GUI witnesses, make the view process push an attachment-ready signal
  before the script injects input.
- Do not add Persona message, provider quota, or harness policy semantics here.
- Add every repeated test command through Nix, not as an ad hoc script.
