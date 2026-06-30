# INTENT — terminal-cell

*What the psyche has explicitly intended for this project. Synthesised
from psyche statements and the applicable workspace constraints; not
embellished. Maintenance: `primary/skills/repo-intent.md`.*

`terminal-cell` is the low-level daemon-owned PTY/transcript cell
primitive: one child process group, one PTY, raw input ports,
append-only transcript replay, worker-lifecycle observation, and one
active viewer attachment. It is the active terminal primitive for V1 harness
work, including Claude/Codex tests, and should be used directly while
`terminal` is archived/inactive.

## Repo-scope only

This file carries primitive-side intent for `terminal-cell`. Higher-level
Persona-facing session ownership, naming, registry, and policy are not active
in `terminal` while that repo is archived. Wire vocabulary stays in
`signal-terminal`. Workspace-shape intent stays in `primary/INTENT.md`.

## Goals

- Own exactly one child process group and one PTY for the lifetime of
  a session, with the transcript as append-only truth: every output
  byte read from the PTY master receives a generation and sequence
  before any viewer replay, screen projection, or capture.
- Provide a clean raw-attach primitive: a single active viewer can
  attach, receive transcript replay from a known sequence, then receive
  live deltas — and detach, close, or crash without ending the
  daemon-owned child.

## Constraints

- **Two wire planes, never mode-shifting.** `control.sock` carries
  `signal-terminal` frames plus the byte-tag CLI protocol; `data.sock`
  carries the raw bidirectional byte stream. An `Attach` request on the
  control socket is rejected with a typed reply; any non-`Attach`
  request on the data socket is symmetrically rejected. There is no
  single-socket path that changes role.
- **Viewer latency lives off the actor mailbox.** Live viewer bytes
  never traverse a Kameo actor mailbox: `ViewerFanout` writes the
  active viewer and returns; transcript work runs on a separate
  `TranscriptScriber` worker fed by a bounded drop-oldest queue, so a
  slow transcript subscriber cannot back-pressure the viewer path.
- **The input gate is writer arbitration, not terminal semantics.**
  Human keyboard bytes and Persona programmatic injection write to the
  same PTY through one writer and one gate; the gate does not parse
  slash commands or infer harness prompt state. Prompt-pattern checks
  here are a witness aid for safe injection while higher-level control
  ownership is inactive.
- **One active viewer per cell.** A second attach while a viewer is
  active is explicitly rejected before any replay or live bytes cross.
- **Inter-component traffic is Signal; NOTA renders only at edges.**
  The daemon's only typed-control surface is `signal-terminal`; the
  byte-tag CLI protocol is a local convenience for command-line
  clients, not the Persona control path.
- **Workers report typed lifecycle to the actor.** Blocking PTY reads,
  writes, fanout, scriber, accept loops, and the attach pump report
  typed `TerminalWorkerLifecycle` events so worker failure becomes
  observable state instead of silent thread death.

## Anti-patterns

- No Sema database here — a running session is discoverable through its
  runtime directory, which is the current V1 harness registry surface. Durable
  named-session state would belong in a future reactivated higher-level owner.
- Do not wait on the archived `terminal` owner for V1 harness testing.
  `terminal-cell` is the active direct terminal primitive until another
  owner is explicitly reactivated.

## See also

- `ARCHITECTURE.md` — plane isolation, the worker/actor split, input
  gate, subscription lifecycle, witnesses.
- `../terminal/INTENT.md` — the archived Persona-facing terminal session
  owner design.
- `../signal-terminal/INTENT.md` — typed terminal request/event vocabulary.
- `primary/skills/component-triad.md` — the data-plane carve-out for
  high-bandwidth byte paths outside the triad.
