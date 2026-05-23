use std::env;
use std::error::Error;
use std::path::PathBuf;

use terminal_cell::{TerminalCellSocketClient, TerminalSize};

type ResizeResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct ResizeArguments {
    control_socket: PathBuf,
    size: TerminalSize,
}

impl ResizeArguments {
    fn from_environment() -> ResizeResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut control_socket = None;
        let mut rows = None;
        let mut columns = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--control-socket" => {
                    control_socket =
                        Some(PathBuf::from(arguments.next().ok_or(
                            "terminal-cell-resize requires a path after --control-socket",
                        )?));
                }
                "--rows" => rows = Some(Self::parse_dimension(arguments.next(), "--rows")?),
                "--columns" => {
                    columns = Some(Self::parse_dimension(arguments.next(), "--columns")?);
                }
                other => return Err(format!("unknown resize argument: {other}").into()),
            }
        }

        Ok(Self {
            control_socket: control_socket
                .ok_or("terminal-cell-resize requires --control-socket <path>")?,
            size: TerminalSize::new(
                rows.ok_or("terminal-cell-resize requires --rows <count>")?,
                columns.ok_or("terminal-cell-resize requires --columns <count>")?,
            ),
        })
    }

    fn parse_dimension(value: Option<String>, flag: &str) -> ResizeResult<u16> {
        let value =
            value.ok_or_else(|| format!("terminal-cell-resize requires a value after {flag}"))?;
        let parsed = value
            .parse::<u16>()
            .map_err(|error| format!("terminal-cell-resize {flag} must be a u16: {error}"))?;
        if parsed == 0 {
            return Err(format!("terminal-cell-resize {flag} must be greater than zero").into());
        }
        Ok(parsed)
    }

    fn into_command(self) -> ResizeCommand {
        ResizeCommand::new(self.control_socket, self.size)
    }
}

struct ResizeCommand {
    client: TerminalCellSocketClient,
    size: TerminalSize,
}

impl ResizeCommand {
    fn new(control_socket: PathBuf, size: TerminalSize) -> Self {
        Self {
            client: TerminalCellSocketClient::for_control_only(control_socket),
            size,
        }
    }

    fn run(&self) -> ResizeResult<()> {
        self.client.resize(self.size)?;
        Ok(())
    }
}

fn main() {
    if let Err(error) = ResizeArguments::from_environment()
        .map(ResizeArguments::into_command)
        .and_then(|command| command.run())
    {
        eprintln!("terminal-cell-resize failed: {error}");
        std::process::exit(1);
    }
}
