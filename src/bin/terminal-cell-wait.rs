use std::env;
use std::error::Error;
use std::path::PathBuf;

use terminal_cell::TerminalCellSocketClient;

type WaitResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct WaitArguments {
    control_socket: PathBuf,
    text: Vec<u8>,
}

impl WaitArguments {
    fn from_environment() -> WaitResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut control_socket = None;
        let mut text = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--control-socket" => {
                    control_socket = Some(PathBuf::from(arguments.next().ok_or(
                        "terminal-cell-wait requires a path after --control-socket",
                    )?));
                }
                "--text" => {
                    text = Some(
                        arguments
                            .next()
                            .ok_or("terminal-cell-wait requires text after --text")?
                            .into_bytes(),
                    );
                }
                other => return Err(format!("unknown wait argument: {other}").into()),
            }
        }

        Ok(Self {
            control_socket: control_socket
                .ok_or("terminal-cell-wait requires --control-socket <path>")?,
            text: text.ok_or("terminal-cell-wait requires --text <text>")?,
        })
    }

    fn into_command(self) -> WaitCommand {
        WaitCommand::new(self.control_socket, self.text)
    }
}

struct WaitCommand {
    client: TerminalCellSocketClient,
    text: Vec<u8>,
}

impl WaitCommand {
    fn new(control_socket: PathBuf, text: Vec<u8>) -> Self {
        Self {
            client: TerminalCellSocketClient::for_control_only(control_socket),
            text,
        }
    }

    fn run(&self) -> WaitResult<()> {
        self.client.wait_for_transcript(&self.text)?;
        Ok(())
    }
}

fn main() {
    if let Err(error) = WaitArguments::from_environment()
        .map(WaitArguments::into_command)
        .and_then(|command| command.run())
    {
        eprintln!("terminal-cell-wait failed: {error}");
        std::process::exit(1);
    }
}
