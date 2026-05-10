use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;

use terminal_cell::TerminalCellSocketClient;

type CaptureResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct CaptureArguments {
    socket: PathBuf,
}

impl CaptureArguments {
    fn from_environment() -> CaptureResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut socket = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--socket" => {
                    socket =
                        Some(PathBuf::from(arguments.next().ok_or(
                            "terminal-cell-capture requires a path after --socket",
                        )?));
                }
                other => return Err(format!("unknown capture argument: {other}").into()),
            }
        }

        Ok(Self {
            socket: socket.ok_or("terminal-cell-capture requires --socket <path>")?,
        })
    }

    fn into_command(self) -> CaptureCommand {
        CaptureCommand::new(self.socket)
    }
}

struct CaptureCommand {
    client: TerminalCellSocketClient,
}

impl CaptureCommand {
    fn new(socket: PathBuf) -> Self {
        Self {
            client: TerminalCellSocketClient::new(socket),
        }
    }

    fn run(&self) -> CaptureResult<()> {
        let bytes = self.client.capture()?;
        io::stdout().write_all(&bytes)?;
        Ok(())
    }
}

fn main() {
    if let Err(error) = CaptureArguments::from_environment()
        .map(CaptureArguments::into_command)
        .and_then(|command| command.run())
    {
        eprintln!("terminal-cell-capture failed: {error}");
        std::process::exit(1);
    }
}
