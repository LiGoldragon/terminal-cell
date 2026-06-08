use terminal_cell::{DaemonEntry, TerminalCellProcessDaemon};

fn main() -> std::process::ExitCode {
    <TerminalCellProcessDaemon as DaemonEntry>::run_to_exit_code()
}
