use std::path::Path;

use terminal_cell::Configuration;
use triad_runtime::BindingSurface;

#[test]
fn configuration_declares_owner_only_working_socket_mode() {
    let configuration = Configuration::new(
        "/tmp/terminal-cell-control.sock",
        "/tmp/terminal-cell-data.sock",
        "/bin/sh",
        Vec::<String>::new(),
    );

    assert_eq!(
        configuration.socket_path(),
        Path::new("/tmp/terminal-cell-control.sock")
    );
    assert_eq!(
        configuration
            .socket_mode()
            .map(|socket_mode| socket_mode.bits()),
        Some(0o600)
    );
}

#[test]
fn generated_daemon_declares_owner_only_meta_socket_mode() {
    let generated_daemon_source = include_str!("../src/schema/daemon.rs");

    assert!(generated_daemon_source.contains("ListenerTier::Meta"));
    assert!(generated_daemon_source.contains("SocketMode::new(0o600)"));
}
