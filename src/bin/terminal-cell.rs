use terminal_cell::TerminalCellCli;

fn main() {
    if let Err(error) = TerminalCellCli::from_environment().and_then(|command| command.run()) {
        eprintln!("terminal-cell failed: {error}");
        std::process::exit(1);
    }
}
