use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::thread;

use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size as terminal_size};
use signal_hook::consts::signal::SIGWINCH;
use signal_hook::iterator::Signals;
use terminal_cell::{TerminalCellSocketClient, TerminalSize};

type ViewResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct ViewArguments {
    socket: PathBuf,
    mode: ViewMode,
    ready_file: Option<PathBuf>,
}

impl ViewArguments {
    fn from_environment() -> ViewResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut socket = None;
        let mut mode = ViewMode::Interactive;
        let mut ready_file = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--socket" => {
                    socket =
                        Some(PathBuf::from(arguments.next().ok_or(
                            "terminal-cell-view requires a path after --socket",
                        )?));
                }
                "--once" => mode = ViewMode::Snapshot,
                "--ready-file" => {
                    ready_file =
                        Some(PathBuf::from(arguments.next().ok_or(
                            "terminal-cell-view requires a path after --ready-file",
                        )?));
                }
                other => return Err(format!("unknown view argument: {other}").into()),
            }
        }

        Ok(Self {
            socket: socket.ok_or("terminal-cell-view requires --socket <path>")?,
            mode,
            ready_file,
        })
    }

    fn into_viewer(self) -> TerminalViewer {
        TerminalViewer::new(self.socket, self.mode, self.ready_file)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Interactive,
    Snapshot,
}

struct TerminalViewer {
    client: TerminalCellSocketClient,
    mode: ViewMode,
    readiness: TerminalViewerReadiness,
}

impl TerminalViewer {
    fn new(socket: PathBuf, mode: ViewMode, ready_file: Option<PathBuf>) -> Self {
        Self {
            client: TerminalCellSocketClient::new(socket),
            mode,
            readiness: TerminalViewerReadiness::new(ready_file),
        }
    }

    fn run(&self) -> ViewResult<()> {
        match self.mode {
            ViewMode::Interactive => self.attach(),
            ViewMode::Snapshot => self.print_snapshot(),
        }
    }

    fn print_snapshot(&self) -> ViewResult<()> {
        let bytes = self.client.capture()?;
        io::stdout().write_all(&bytes)?;
        Ok(())
    }

    fn attach(&self) -> ViewResult<()> {
        let mut resize_watcher = TerminalResizeWatcher::new(self.client.clone());
        resize_watcher.resize_now()?;
        let _resize_thread = resize_watcher.spawn()?;
        let mut attach_stream = self.client.open_attach_stream()?;
        let mut output_stream = attach_stream.try_clone()?;
        self.readiness.confirm_control_plane(&self.client)?;
        self.readiness.announce()?;
        let output = thread::Builder::new()
            .name("terminal-cell-view-output".to_string())
            .spawn(move || -> io::Result<()> {
                let mut stdout = io::stdout();
                let mut buffer = [0_u8; 8192];
                loop {
                    let count = output_stream.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    stdout.write_all(&buffer[..count])?;
                    stdout.flush()?;
                }
                Ok(())
            })?;

        let _raw_mode = TerminalRawMode::enter()?;
        let mut stdin = io::stdin();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stdin.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            attach_stream.write_all(&buffer[..count])?;
        }

        output
            .join()
            .map_err(|_| "terminal view output thread panicked")??;
        Ok(())
    }
}

struct TerminalResizeWatcher {
    client: TerminalCellSocketClient,
    last_size: Option<TerminalSize>,
}

impl TerminalResizeWatcher {
    fn new(client: TerminalCellSocketClient) -> Self {
        Self {
            client,
            last_size: None,
        }
    }

    fn spawn(mut self) -> io::Result<thread::JoinHandle<()>> {
        let mut signals = Signals::new([SIGWINCH])?;
        thread::Builder::new()
            .name("terminal-cell-view-resize".to_string())
            .spawn(move || {
                for _signal in signals.forever() {
                    if self.resize_now().is_err() {
                        break;
                    }
                }
            })
    }

    fn resize_now(&mut self) -> io::Result<()> {
        let size = self.current_attached_terminal_size()?;
        if self.last_size == Some(size) {
            return Ok(());
        }
        self.client.resize(size)?;
        self.last_size = Some(size);
        Ok(())
    }

    fn current_attached_terminal_size(&self) -> io::Result<TerminalSize> {
        let (columns, rows) = terminal_size()?;
        Ok(TerminalSize::new(rows, columns))
    }
}

struct TerminalViewerReadiness {
    ready_file: Option<PathBuf>,
}

impl TerminalViewerReadiness {
    fn new(ready_file: Option<PathBuf>) -> Self {
        Self { ready_file }
    }

    fn announce(&self) -> io::Result<()> {
        if let Some(path) = &self.ready_file {
            fs::write(path, b"terminal-cell-view attached\n")?;
        }
        Ok(())
    }

    fn confirm_control_plane(&self, client: &TerminalCellSocketClient) -> io::Result<()> {
        client.capture().map(|_snapshot| ())
    }
}

struct TerminalRawMode {
    enabled: bool,
}

impl TerminalRawMode {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self { enabled: true })
    }
}

impl Drop for TerminalRawMode {
    fn drop(&mut self) {
        if self.enabled {
            let _ = disable_raw_mode();
            self.enabled = false;
        }
    }
}

fn main() {
    if let Err(error) = ViewArguments::from_environment()
        .map(ViewArguments::into_viewer)
        .and_then(|viewer| viewer.run())
    {
        eprintln!("terminal-cell-view failed: {error}");
        std::process::exit(1);
    }
}
