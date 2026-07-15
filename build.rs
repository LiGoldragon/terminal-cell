use std::{env, path::PathBuf};

use schema_rust::{
    MetaListenerTier, NexusDaemonShape, SocketModeBits, WorkingListenerTier,
    build::{GenerationDriver, GenerationPlan, ModuleEmission},
};

const OWNER_ONLY_SOCKET_MODE: u32 = 0o600;

fn main() {
    SchemaBuild::from_environment().run();
}

struct SchemaBuild {
    crate_root: PathBuf,
}

impl SchemaBuild {
    fn from_environment() -> Self {
        Self {
            crate_root: PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir set")),
        }
    }

    fn run(&self) {
        println!("cargo:rerun-if-changed=schema/daemon.schema");
        println!("cargo:rerun-if-changed=src/schema/daemon.rs");

        let plan = GenerationPlan::new(&self.crate_root, "terminal_cell", "0.2.0").with_module(
            ModuleEmission::daemon_module("daemon", Self::daemon_shape()),
        );
        GenerationDriver::new(plan)
            .generate()
            .expect("generate terminal-cell schema artifacts")
            .write_or_check("TERMINAL_CELL_UPDATE_SCHEMA_ARTIFACTS")
            .expect("checked-in terminal-cell schema artifacts are fresh");
    }

    /// The control plane is the peer-callable working listener, decoded by the
    /// component (terminal-cell speaks its own `SocketRequest` wire, not a
    /// schema-derived contract). The data plane reuses the owner-only meta
    /// listener slot for the raw attached-viewer byte path. Both sockets are
    /// owner-only — terminal-cell binds them 0o600.
    fn daemon_shape() -> NexusDaemonShape {
        NexusDaemonShape::new(
            "terminal-cell-daemon",
            WorkingListenerTier::component_decoded(),
        )
        .with_meta_tier(MetaListenerTier::new(SocketModeBits::new(
            OWNER_ONLY_SOCKET_MODE,
        )))
    }
}
