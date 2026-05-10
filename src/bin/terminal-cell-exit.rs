use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;

use terminal_cell::TerminalCellSocketClient;

type ExitResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct ExitArguments {
    socket: PathBuf,
}

impl ExitArguments {
    fn from_environment() -> ExitResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut socket = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--socket" => {
                    socket =
                        Some(PathBuf::from(arguments.next().ok_or(
                            "terminal-cell-exit requires a path after --socket",
                        )?));
                }
                other => return Err(format!("unknown exit argument: {other}").into()),
            }
        }

        Ok(Self {
            socket: socket.ok_or("terminal-cell-exit requires --socket <path>")?,
        })
    }

    fn into_command(self) -> ExitCommand {
        ExitCommand::new(self.socket)
    }
}

struct ExitCommand {
    client: TerminalCellSocketClient,
}

impl ExitCommand {
    fn new(socket: PathBuf) -> Self {
        Self {
            client: TerminalCellSocketClient::new(socket),
        }
    }

    fn run(&self) -> ExitResult<()> {
        let status = self.client.wait_for_terminal_exit()?;
        writeln!(io::stdout(), "{status}")?;
        Ok(())
    }
}

fn main() {
    if let Err(error) = ExitArguments::from_environment()
        .map(ExitArguments::into_command)
        .and_then(|command| command.run())
    {
        eprintln!("terminal-cell-exit failed: {error}");
        std::process::exit(1);
    }
}
