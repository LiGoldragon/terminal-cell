# Agent Instructions - Terminal Cell

You MUST read lore's `AGENTS.md` and the primary workspace orchestration
protocol before editing this repository.

## Repo Role

Terminal Cell is a prototype for a durable terminal session owner. It
explores the shape that can later become a production terminal transport:
one PTY owner, append-only transcript truth, disposable viewers, and typed
input/capture messages.

## Boundaries

This repo owns experiments only. It does not define Persona message
semantics, harness identity, provider usage policy, or the production
`signal-persona-terminal` contract.

## Rust

Follow the workspace Rust discipline: behavior lives on data-bearing types,
stateful runtime planes are Kameo actors, and tests are named witnesses for
architecture constraints.

