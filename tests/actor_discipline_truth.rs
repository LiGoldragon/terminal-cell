//! Architectural-truth witnesses for terminal-cell's actor
//! discipline.
//!
//! `TerminalCell` is the single Kameo actor that owns transcript
//! truth, worker-lifecycle observation, transcript and worker-
//! lifecycle subscribers, resize authority, child-exit state, and
//! waiters. Blocking byte planes (PTY reads/writes, fanout,
//! scriber, accept loops) live as named workers around it.
//!
//! - Public actor noun carries data — `mem::size_of::<TerminalCell>() > 0`.
//! - No shared `Arc<Mutex<_>>` / `Arc<RwLock<_>>` in library
//!   source files (per `~/primary/skills/actor-systems.md`
//!   §"No shared locks").
//!
//! The scan covers `src/**` except `src/bin/`. The daemon binary
//! at `src/bin/terminal-cell-daemon.rs` still carries
//! `Arc<Mutex<TerminalSignalControlState>>` for prompt-pattern
//! registry sharing across socket-accept-loop tasks; this is
//! known drift from the destination shape (the persona-terminal
//! ARCH §1.5 names `TerminalSignalControl` as a Kameo actor
//! owned in `persona-terminal`, not a shared lock in this
//! daemon). Fixing the drift requires moving signal-control
//! ownership behind a Kameo actor in this repo's daemon, or
//! retiring the daemon's signal-control surface in favor of
//! the persona-terminal supervisor — an operator/designer
//! decision, not a witness scope.
//!
//! A future refactor that collapses `TerminalCell` to a marker
//! ZST, or wires shared locks between actors in library code,
//! breaks these witnesses.

use std::fs;
use std::path::{Path, PathBuf};

use terminal_cell::TerminalCell;

#[test]
fn public_actor_noun_carries_data() {
    assert!(std::mem::size_of::<TerminalCell>() > 0);
}

#[test]
fn actor_source_does_not_share_locks_between_actors() {
    let forbidden = [
        ("Arc<Mutex", "shared mutex state between actors"),
        ("Arc < Mutex", "shared mutex state between actors"),
        ("RwLock", "shared read-write lock state between actors"),
    ];

    let mut violations: Vec<String> = Vec::new();
    for path in production_source_files() {
        let text = fs::read_to_string(&path).expect("read source file");
        for (fragment, reason) in forbidden {
            for (index, line) in text.lines().enumerate() {
                if line.contains(fragment) {
                    violations.push(format!(
                        "{}:{}: {reason} ({line})",
                        path.display(),
                        index + 1,
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "shared-lock violations in actor source:\n{}",
        violations.join("\n"),
    );
}

fn production_source_files() -> Vec<PathBuf> {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = crate_root.join("src");
    let bin = src.join("bin");
    let mut output = Vec::new();
    collect_rust_files(&src, &bin, &mut output);
    output
}

fn collect_rust_files(directory: &Path, skip: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == *skip {
            continue;
        }
        if path.is_dir() {
            collect_rust_files(&path, skip, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}
