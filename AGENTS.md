# Agent Instructions - Terminal Cell

You MUST read lore's `AGENTS.md` and the primary workspace orchestration
protocol before editing this repository.

## Repo Role

Terminal Cell is the low-level durable terminal cell primitive used under
Persona terminal transport. It owns one PTY, append-only transcript truth,
disposable viewers, and typed input/capture messages.

## Boundaries

This repo owns the terminal cell primitive only. It does not define Persona
message semantics, harness identity, provider usage policy, or the
`signal-terminal` contract.

## Rust

Follow the workspace Rust discipline: behavior lives on data-bearing types,
stateful runtime planes are Kameo actors, and tests are named witnesses for
architecture constraints.
