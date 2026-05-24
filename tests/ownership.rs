#[test]
fn direct_signal_endpoint_is_transitional_witness_code() {
    let daemon_source = include_str!("../src/bin/terminal-cell-daemon.rs");

    assert!(daemon_source.contains("TRANSITIONAL"));
    assert!(daemon_source.contains("terminal"));
    assert!(daemon_source.contains("becomes the production Signal"));
}
