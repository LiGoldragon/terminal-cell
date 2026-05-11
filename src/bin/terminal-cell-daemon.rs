use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;

use kameo::actor::ActorRef;
use tokio::runtime::Handle;

use terminal_cell::{
    InputSource, SocketReplyWriter, SocketRequest, SocketRequestReader, TerminalCell,
    TerminalCommand, TerminalInput, TerminalInputPort, TerminalLaunch, TerminalOutputPort,
    TerminalSize, TranscriptSnapshotRequest, TranscriptSubscriptionRequest, WaitForTerminalExit,
    WaitForTranscriptText,
};

type TerminalDaemonResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct DaemonArguments {
    socket: PathBuf,
    command: TerminalCommand,
}

impl DaemonArguments {
    fn from_environment() -> TerminalDaemonResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut socket = None;
        let mut command = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--socket" => {
                    socket =
                        Some(PathBuf::from(arguments.next().ok_or(
                            "terminal-cell-daemon requires a path after --socket",
                        )?));
                }
                "--" => {
                    let program = arguments
                        .next()
                        .ok_or("terminal-cell-daemon requires a command after --")?;
                    let rest = arguments.collect::<Vec<_>>();
                    command = Some(TerminalCommand::new(program, rest));
                    break;
                }
                other => {
                    return Err(format!("unknown daemon argument: {other}").into());
                }
            }
        }

        Ok(Self {
            socket: socket.ok_or("terminal-cell-daemon requires --socket <path>")?,
            command: command.ok_or("terminal-cell-daemon requires -- <command> [args...]")?,
        })
    }

    fn into_daemon(self) -> TerminalCellDaemon {
        TerminalCellDaemon::new(
            self.socket,
            TerminalLaunch::new(self.command, TerminalSize::new(24, 80)),
        )
    }
}

struct TerminalCellDaemon {
    socket: PathBuf,
    launch: TerminalLaunch,
}

impl TerminalCellDaemon {
    fn new(socket: PathBuf, launch: TerminalLaunch) -> Self {
        Self { socket, launch }
    }

    async fn run(self) -> TerminalDaemonResult<()> {
        let session = TerminalCell::spawn_session(self.launch);
        let terminal = session.actor();
        let input_port = session.input_port();
        let output_port = session.output_port();
        terminal
            .wait_for_startup_result()
            .await
            .map_err(|error| format!("terminal cell startup failed: {error}"))?;

        TerminalSocketFile::new(self.socket.as_path()).prepare()?;
        let listener = UnixListener::bind(&self.socket)?;
        let runtime = Handle::current();

        println!("terminal-cell-daemon socket={}", self.socket.display());
        io::stdout().flush()?;

        tokio::task::spawn_blocking(move || {
            TerminalCellDaemonLoop::new(listener, terminal, input_port, output_port, runtime).run()
        })
        .await??;
        Ok(())
    }
}

struct TerminalSocketFile<'path> {
    path: &'path Path,
}

impl<'path> TerminalSocketFile<'path> {
    fn new(path: &'path Path) -> Self {
        Self { path }
    }

    fn prepare(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        match fs::symlink_metadata(self.path) {
            Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(self.path),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "refusing to replace non-socket path {}",
                    self.path.display()
                ),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

struct TerminalCellDaemonLoop {
    listener: UnixListener,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    output_port: TerminalOutputPort,
    runtime: Handle,
}

impl TerminalCellDaemonLoop {
    fn new(
        listener: UnixListener,
        terminal: ActorRef<TerminalCell>,
        input_port: TerminalInputPort,
        output_port: TerminalOutputPort,
        runtime: Handle,
    ) -> Self {
        Self {
            listener,
            terminal,
            input_port,
            output_port,
            runtime,
        }
    }

    fn run(self) -> io::Result<()> {
        for incoming in self.listener.incoming() {
            let stream = incoming?;
            let terminal = self.terminal.clone();
            let input_port = self.input_port.clone();
            let output_port = self.output_port.clone();
            let runtime = self.runtime.clone();
            thread::Builder::new()
                .name("terminal-cell-connection".to_string())
                .spawn(move || {
                    if let Err(error) = TerminalCellConnection::new(
                        stream,
                        terminal,
                        input_port,
                        output_port,
                        runtime,
                    )
                    .run()
                    {
                        eprintln!("terminal cell connection failed: {error}");
                    }
                })?;
        }
        Ok(())
    }
}

struct TerminalCellConnection {
    stream: UnixStream,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    output_port: TerminalOutputPort,
    runtime: Handle,
}

impl TerminalCellConnection {
    fn new(
        stream: UnixStream,
        terminal: ActorRef<TerminalCell>,
        input_port: TerminalInputPort,
        output_port: TerminalOutputPort,
        runtime: Handle,
    ) -> Self {
        Self {
            stream,
            terminal,
            input_port,
            output_port,
            runtime,
        }
    }

    fn run(&mut self) -> io::Result<()> {
        let request = SocketRequestReader::new(&mut self.stream).read_request()?;
        match request {
            SocketRequest::Capture => self.write_snapshot(),
            SocketRequest::SubscribeFromBeginning => self.stream_subscription(),
            SocketRequest::Attach => self.attach_viewer(),
            SocketRequest::Input(input) => self.write_input(input),
            SocketRequest::CloseHumanInput => self.close_human_input(),
            SocketRequest::OpenHumanInput(lease) => self.open_human_input(lease),
            SocketRequest::Resize(size) => self.write_resize(size),
            SocketRequest::Wait(wait) => self.wait_for_text(wait),
            SocketRequest::WaitExit => self.wait_for_exit(),
        }
    }

    fn write_snapshot(&mut self) -> io::Result<()> {
        let snapshot = self.snapshot()?;
        SocketReplyWriter::new(&mut self.stream).write_snapshot(snapshot.bytes())
    }

    fn stream_subscription(&mut self) -> io::Result<()> {
        let mut subscription = self.subscription()?;
        self.stream.write_all(&subscription.replay_bytes())?;
        self.stream.flush()?;
        while let Some(delta) = subscription.blocking_next_live_delta() {
            if self.stream.write_all(delta.bytes()).is_err() {
                break;
            }
            if self.stream.flush().is_err() {
                break;
            }
        }
        Ok(())
    }

    fn attach_viewer(&mut self) -> io::Result<()> {
        let snapshot = self.snapshot()?;
        if !snapshot.bytes().is_empty() {
            self.stream.write_all(snapshot.bytes())?;
            self.stream.flush()?;
        }

        self.output_port
            .attach(self.stream.try_clone()?)
            .map_err(Self::terminal_error)?;

        let mut buffer = [0_u8; 8192];
        loop {
            let count = self.stream.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            self.input_port
                .accept(TerminalInput::new(
                    buffer[..count].to_vec(),
                    InputSource::Viewer,
                ))
                .map_err(Self::terminal_error)?;
        }
        Ok(())
    }

    fn write_input(&mut self, input: TerminalInput) -> io::Result<()> {
        let acceptance = self
            .input_port
            .accept(input)
            .map_err(Self::terminal_error)?;
        let _accepted_source = acceptance.source();
        SocketReplyWriter::new(&mut self.stream).write_acceptance()
    }

    fn close_human_input(&mut self) -> io::Result<()> {
        let lease = self
            .input_port
            .close_human_input()
            .map_err(Self::terminal_error)?;
        SocketReplyWriter::new(&mut self.stream).write_gate_lease(lease)
    }

    fn open_human_input(&mut self, lease: terminal_cell::TerminalInputGateLease) -> io::Result<()> {
        let release = self
            .input_port
            .open_human_input(lease)
            .map_err(Self::terminal_error)?;
        SocketReplyWriter::new(&mut self.stream).write_gate_release(release)
    }

    fn write_resize(&mut self, size: TerminalSize) -> io::Result<()> {
        self.runtime
            .block_on(async { self.terminal.ask(size).await })
            .map_err(Self::actor_error)?;
        SocketReplyWriter::new(&mut self.stream).write_acceptance()
    }

    fn wait_for_text(&mut self, wait: WaitForTranscriptText) -> io::Result<()> {
        let matched = self
            .runtime
            .block_on(async { self.terminal.ask(wait).await })
            .map_err(Self::actor_error)?;
        if matched {
            SocketReplyWriter::new(&mut self.stream).write_wait_satisfied()
        } else {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "terminal transcript waiter ended without a match",
            ))
        }
    }

    fn snapshot(&self) -> io::Result<terminal_cell::TranscriptSnapshot> {
        let reply = self
            .runtime
            .block_on(async { self.terminal.ask(TranscriptSnapshotRequest).await })
            .map_err(Self::actor_error)?;
        Ok(reply)
    }

    fn wait_for_exit(&mut self) -> io::Result<()> {
        let exit = self
            .runtime
            .block_on(async { self.terminal.ask(WaitForTerminalExit).await })
            .map_err(Self::actor_error)?;
        SocketReplyWriter::new(&mut self.stream).write_exit_status(exit.status())
    }

    fn subscription(&self) -> io::Result<terminal_cell::TranscriptSubscription> {
        let reply = self
            .runtime
            .block_on(async {
                self.terminal
                    .ask(TranscriptSubscriptionRequest::from_beginning())
                    .await
            })
            .map_err(Self::actor_error)?;
        Ok(reply)
    }

    fn actor_error(error: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, error.to_string())
    }

    fn terminal_error(error: terminal_cell::TerminalCellError) -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, error.to_string())
    }
}

#[tokio::main]
async fn main() {
    let result = match DaemonArguments::from_environment() {
        Ok(arguments) => arguments.into_daemon().run().await,
        Err(error) => Err(error),
    };

    if let Err(error) = result {
        eprintln!("terminal-cell-daemon failed: {error}");
        std::process::exit(1);
    }
}
