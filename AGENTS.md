# Agent Instructions - Terminal Cell

## Repo Role

Terminal Cell is the low-level durable terminal cell primitive used under
Persona terminal transport. It owns one PTY, append-only transcript truth,
disposable viewers, and typed input/capture messages. It is the active
terminal primitive for V1 harness Claude/Codex tests while `terminal` is
archived/inactive.

## Boundaries

This repo owns the terminal cell primitive only. It does not define Persona
message semantics, harness identity, provider usage policy, or the
`signal-terminal` contract.

## Rust

Follow the workspace Rust discipline: behavior lives on data-bearing types,
stateful runtime planes are Kameo actors, and tests are named witnesses for
architecture constraints.
