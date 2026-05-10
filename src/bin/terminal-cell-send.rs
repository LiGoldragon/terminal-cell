use std::env;
use std::error::Error;
use std::path::PathBuf;

use terminal_cell::TerminalCellSocketClient;

type SendResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct SendArguments {
    socket: PathBuf,
    input: Vec<u8>,
}

impl SendArguments {
    fn from_environment() -> SendResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut socket = None;
        let mut input = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--socket" => {
                    socket =
                        Some(PathBuf::from(arguments.next().ok_or(
                            "terminal-cell-send requires a path after --socket",
                        )?));
                }
                "--line" => {
                    let line = arguments
                        .next()
                        .ok_or("terminal-cell-send requires text after --line")?;
                    let mut bytes = line.into_bytes();
                    bytes.push(b'\r');
                    input = Some(bytes);
                }
                "--bytes" => {
                    input = Some(
                        arguments
                            .next()
                            .ok_or("terminal-cell-send requires text after --bytes")?
                            .into_bytes(),
                    );
                }
                other => return Err(format!("unknown send argument: {other}").into()),
            }
        }

        Ok(Self {
            socket: socket.ok_or("terminal-cell-send requires --socket <path>")?,
            input: input.ok_or("terminal-cell-send requires --line <text> or --bytes <text>")?,
        })
    }

    fn into_command(self) -> SendCommand {
        SendCommand::new(self.socket, self.input)
    }
}

struct SendCommand {
    client: TerminalCellSocketClient,
    input: Vec<u8>,
}

impl SendCommand {
    fn new(socket: PathBuf, input: Vec<u8>) -> Self {
        Self {
            client: TerminalCellSocketClient::new(socket),
            input,
        }
    }

    fn run(&self) -> SendResult<()> {
        self.client.send_programmatic_input(&self.input)?;
        Ok(())
    }
}

fn main() {
    if let Err(error) = SendArguments::from_environment()
        .map(SendArguments::into_command)
        .and_then(|command| command.run())
    {
        eprintln!("terminal-cell-send failed: {error}");
        std::process::exit(1);
    }
}
