use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use kameo::actor::ActorRef;
use regex::bytes::Regex;
use signal_persona_terminal as terminal_signal;
use tokio::runtime::Handle;

use terminal_cell::{
    InputSource, SignalSocketRequest, SocketReplyWriter, SocketRequest, SocketRequestReader,
    TerminalCell, TerminalCellError, TerminalCommand, TerminalInput, TerminalInputGateLease,
    TerminalInputGateSequence, TerminalInputPort, TerminalLaunch, TerminalOutputPort, TerminalSize,
    TerminalViewerLease, TerminalWorkerKind, TerminalWorkerLifecycle,
    TerminalWorkerLifecycleSubscriptionRequest, TerminalWorkerObservationRequest,
    TerminalWorkerStop, TranscriptSnapshotRequest, TranscriptSubscriptionRequest,
    WaitForTerminalExit, WaitForTranscriptText,
};

type TerminalDaemonResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct DaemonArguments {
    control_socket: PathBuf,
    data_socket: PathBuf,
    command: TerminalCommand,
}

impl DaemonArguments {
    fn from_environment() -> TerminalDaemonResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut control_socket = None;
        let mut data_socket = None;
        let mut command = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--control-socket" => {
                    control_socket = Some(PathBuf::from(arguments.next().ok_or(
                        "terminal-cell-daemon requires a path after --control-socket",
                    )?));
                }
                "--data-socket" => {
                    data_socket = Some(PathBuf::from(arguments.next().ok_or(
                        "terminal-cell-daemon requires a path after --data-socket",
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
            control_socket: control_socket
                .ok_or("terminal-cell-daemon requires --control-socket <path>")?,
            data_socket: data_socket
                .ok_or("terminal-cell-daemon requires --data-socket <path>")?,
            command: command.ok_or("terminal-cell-daemon requires -- <command> [args...]")?,
        })
    }

    fn into_daemon(self) -> TerminalCellDaemon {
        TerminalCellDaemon::new(
            self.control_socket,
            self.data_socket,
            TerminalLaunch::new(self.command, TerminalSize::new(24, 80)),
        )
    }
}

struct TerminalCellDaemon {
    control_socket: PathBuf,
    data_socket: PathBuf,
    launch: TerminalLaunch,
}

impl TerminalCellDaemon {
    fn new(control_socket: PathBuf, data_socket: PathBuf, launch: TerminalLaunch) -> Self {
        Self {
            control_socket,
            data_socket,
            launch,
        }
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

        let control_listener =
            TerminalSocketFile::new(self.control_socket.as_path()).bind_listener()?;
        let data_listener = TerminalSocketFile::new(self.data_socket.as_path()).bind_listener()?;
        let runtime = Handle::current();

        println!(
            "terminal-cell-daemon control-socket={} data-socket={}",
            self.control_socket.display(),
            self.data_socket.display()
        );
        io::stdout().flush()?;

        let signal_state = Arc::new(Mutex::new(TerminalSignalControlState::new()));

        let control_loop = TerminalControlPlaneLoop::new(
            control_listener,
            terminal.clone(),
            input_port.clone(),
            signal_state,
            runtime.clone(),
        );
        let data_loop = TerminalDataPlaneLoop::new(
            data_listener,
            terminal.clone(),
            input_port,
            output_port,
            runtime.clone(),
        );

        let control_task = tokio::task::spawn_blocking(move || control_loop.run());
        let data_task = tokio::task::spawn_blocking(move || data_loop.run());

        let (control_result, data_result) = tokio::join!(control_task, data_task);
        control_result??;
        data_result??;
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

    fn bind_listener(&self) -> io::Result<UnixListener> {
        self.prepare()?;
        let listener = UnixListener::bind(self.path)?;
        self.apply_socket_mode()?;
        Ok(listener)
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

    fn apply_socket_mode(&self) -> io::Result<()> {
        fs::set_permissions(self.path, fs::Permissions::from_mode(0o600))
    }
}

struct TerminalControlPlaneLoop {
    listener: UnixListener,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    signal_state: Arc<Mutex<TerminalSignalControlState>>,
    runtime: Handle,
}

impl TerminalControlPlaneLoop {
    fn new(
        listener: UnixListener,
        terminal: ActorRef<TerminalCell>,
        input_port: TerminalInputPort,
        signal_state: Arc<Mutex<TerminalSignalControlState>>,
        runtime: Handle,
    ) -> Self {
        Self {
            listener,
            terminal,
            input_port,
            signal_state,
            runtime,
        }
    }

    fn run(self) -> io::Result<()> {
        let _ = self
            .terminal
            .tell(TerminalWorkerLifecycle::Started(
                TerminalWorkerKind::SocketAcceptLoop,
            ))
            .try_send();
        for incoming in self.listener.incoming() {
            let stream = match incoming {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = self
                        .terminal
                        .tell(TerminalWorkerLifecycle::Stopped {
                            worker: TerminalWorkerKind::SocketAcceptLoop,
                            reason: TerminalWorkerStop::SocketAcceptFailed(error.to_string()),
                        })
                        .try_send();
                    return Err(error);
                }
            };
            let terminal = self.terminal.clone();
            let input_port = self.input_port.clone();
            let signal_state = self.signal_state.clone();
            let runtime = self.runtime.clone();
            thread::Builder::new()
                .name("terminal-cell-control-connection".to_string())
                .spawn(move || {
                    if let Err(error) = TerminalControlConnection::new(
                        stream,
                        terminal,
                        input_port,
                        signal_state,
                        runtime,
                    )
                    .run()
                    {
                        eprintln!("terminal cell control connection failed: {error}");
                    }
                })?;
        }
        Ok(())
    }
}

struct TerminalDataPlaneLoop {
    listener: UnixListener,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    output_port: TerminalOutputPort,
    runtime: Handle,
}

impl TerminalDataPlaneLoop {
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
                .name("terminal-cell-data-connection".to_string())
                .spawn(move || {
                    if let Err(error) = TerminalDataConnection::new(
                        stream,
                        terminal,
                        input_port,
                        output_port,
                        runtime,
                    )
                    .run()
                    {
                        eprintln!("terminal cell data connection failed: {error}");
                    }
                })?;
        }
        Ok(())
    }
}

/// TRANSITIONAL: direct `signal-persona-terminal` handling in this daemon is a
/// witness path while `persona-terminal` becomes the production Signal
/// endpoint. Keep this state local to the terminal-cell daemon and do not grow
/// it into a Persona-facing registry or policy owner.
struct TerminalSignalControlState {
    next_prompt_pattern: u64,
    prompt_patterns: HashMap<String, terminal_signal::PromptPattern>,
    signal_leases: HashMap<u64, terminal_signal::PromptState>,
}

impl TerminalSignalControlState {
    fn new() -> Self {
        Self {
            next_prompt_pattern: 1,
            prompt_patterns: HashMap::new(),
            signal_leases: HashMap::new(),
        }
    }

    fn register_prompt_pattern(
        &mut self,
        pattern: terminal_signal::PromptPattern,
    ) -> terminal_signal::PromptPatternId {
        let id = terminal_signal::PromptPatternId::new(format!(
            "prompt-pattern-{}",
            self.next_prompt_pattern
        ));
        self.next_prompt_pattern = self.next_prompt_pattern.saturating_add(1);
        self.prompt_patterns
            .insert(id.as_str().to_string(), pattern);
        id
    }

    fn unregister_prompt_pattern(&mut self, id: &terminal_signal::PromptPatternId) {
        self.prompt_patterns.remove(id.as_str());
    }

    fn prompt_pattern_entries(&self) -> Vec<terminal_signal::PromptPatternEntry> {
        self.prompt_patterns
            .iter()
            .map(
                |(pattern_id, pattern)| terminal_signal::PromptPatternEntry {
                    pattern_id: terminal_signal::PromptPatternId::new(pattern_id.clone()),
                    pattern: pattern.clone(),
                },
            )
            .collect()
    }

    fn prompt_pattern(
        &self,
        id: &terminal_signal::PromptPatternId,
    ) -> Option<terminal_signal::PromptPattern> {
        self.prompt_patterns.get(id.as_str()).cloned()
    }

    fn record_signal_lease(
        &mut self,
        lease: terminal_signal::InputGateLease,
        prompt_state: terminal_signal::PromptState,
    ) {
        self.signal_leases.insert(lease.id.into_u64(), prompt_state);
    }

    fn signal_lease_prompt_state(
        &self,
        lease: &terminal_signal::InputGateLease,
    ) -> Option<&terminal_signal::PromptState> {
        self.signal_leases.get(&lease.id.into_u64())
    }

    fn release_signal_lease(&mut self, lease: &terminal_signal::InputGateLease) {
        self.signal_leases.remove(&lease.id.into_u64());
    }
}

/// A control-plane connection. Carries every kind of request *except*
/// `Attach`: an attach request on this socket is an architectural-truth
/// violation and is explicitly rejected with the `ATTACH_REJECTED` reply
/// so the wire boundary stays clean.
struct TerminalControlConnection {
    stream: UnixStream,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    signal_state: Arc<Mutex<TerminalSignalControlState>>,
    runtime: Handle,
}

impl TerminalControlConnection {
    fn new(
        stream: UnixStream,
        terminal: ActorRef<TerminalCell>,
        input_port: TerminalInputPort,
        signal_state: Arc<Mutex<TerminalSignalControlState>>,
        runtime: Handle,
    ) -> Self {
        Self {
            stream,
            terminal,
            input_port,
            signal_state,
            runtime,
        }
    }

    fn run(&mut self) -> io::Result<()> {
        let request = SocketRequestReader::new(&mut self.stream).read_request()?;
        match request {
            SocketRequest::Capture => self.write_snapshot(),
            SocketRequest::SubscribeFromBeginning => self.stream_subscription(),
            SocketRequest::Attach => self.reject_attach_on_control_plane(),
            SocketRequest::Input(input) => self.write_input(input),
            SocketRequest::CloseHumanInput => self.close_human_input(),
            SocketRequest::OpenHumanInput(lease) => self.open_human_input(lease),
            SocketRequest::Resize(size) => self.write_resize(size),
            SocketRequest::Wait(wait) => self.wait_for_text(wait),
            SocketRequest::WaitExit => self.wait_for_exit(),
            SocketRequest::WorkerObservation => self.write_worker_observation(),
            SocketRequest::Signal(request) => self.handle_signal_request(request),
        }
    }

    fn reject_attach_on_control_plane(&mut self) -> io::Result<()> {
        SocketReplyWriter::new(&mut self.stream).write_attach_rejected(
            "control socket does not accept viewer attach; use the data socket",
        )?;
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "attach request arrived on terminal-cell control socket",
        ))
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
        self.signal_state()?
            .release_signal_lease(&Self::signal_lease(release.lease()));
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

    fn write_worker_observation(&mut self) -> io::Result<()> {
        let observation = self
            .runtime
            .block_on(async { self.terminal.ask(TerminalWorkerObservationRequest).await })
            .map_err(Self::actor_error)?;
        SocketReplyWriter::new(&mut self.stream).write_snapshot(observation.to_text().as_bytes())
    }

    fn handle_signal_request(&mut self, request: SignalSocketRequest) -> io::Result<()> {
        match request.into_payload() {
            terminal_signal::TerminalRequest::SubscribeTerminalWorkerLifecycle(subscription) => {
                self.stream_signal_worker_lifecycle(subscription)
            }
            terminal_signal::TerminalRequest::TerminalWorkerLifecycleRetraction(token) => {
                let ack = terminal_signal::SubscriptionRetracted { token };
                SocketReplyWriter::new(&mut self.stream).write_signal_event(ack.into())
            }
            payload => {
                let event = self.signal_event(payload)?;
                SocketReplyWriter::new(&mut self.stream).write_signal_event(event)
            }
        }
    }

    fn signal_event(
        &mut self,
        request: terminal_signal::TerminalRequest,
    ) -> io::Result<terminal_signal::TerminalReply> {
        match request {
            terminal_signal::TerminalRequest::TerminalConnection(connection) => {
                Ok(terminal_signal::TerminalReady {
                    terminal: connection.terminal,
                    generation: terminal_signal::TerminalGeneration::new(1),
                }
                .into())
            }
            terminal_signal::TerminalRequest::TerminalInput(input) => {
                self.input_port
                    .accept(TerminalInput::new(
                        input.bytes.as_slice().to_vec(),
                        InputSource::Programmatic,
                    ))
                    .map_err(Self::terminal_error)?;
                Ok(terminal_signal::TerminalInputAccepted {
                    terminal: input.terminal,
                    generation: terminal_signal::TerminalGeneration::new(1),
                }
                .into())
            }
            terminal_signal::TerminalRequest::TerminalResize(resize) => {
                let size = TerminalSize::new(resize.rows.into_u16(), resize.columns.into_u16());
                self.runtime
                    .block_on(async { self.terminal.ask(size).await })
                    .map_err(Self::actor_error)?;
                Ok(terminal_signal::TerminalResized {
                    terminal: resize.terminal,
                    rows: resize.rows,
                    columns: resize.columns,
                    generation: terminal_signal::TerminalGeneration::new(1),
                }
                .into())
            }
            terminal_signal::TerminalRequest::TerminalDetachment(detachment) => {
                Ok(terminal_signal::TerminalDetached {
                    terminal: detachment.terminal,
                    generation: terminal_signal::TerminalGeneration::new(1),
                    reason: detachment.reason,
                }
                .into())
            }
            terminal_signal::TerminalRequest::TerminalCapture(capture) => {
                let snapshot = self.snapshot()?;
                Ok(terminal_signal::TerminalCaptured {
                    terminal: capture.terminal,
                    generation: terminal_signal::TerminalGeneration::new(1),
                    bytes: terminal_signal::TerminalTranscriptBytes::new(snapshot.bytes().to_vec()),
                }
                .into())
            }
            terminal_signal::TerminalRequest::RegisterPromptPattern(registration) => {
                let pattern_id = self
                    .signal_state()?
                    .register_prompt_pattern(registration.pattern);
                Ok(terminal_signal::PromptPatternRegistered {
                    terminal: registration.terminal,
                    pattern_id,
                }
                .into())
            }
            terminal_signal::TerminalRequest::UnregisterPromptPattern(unregistration) => {
                self.signal_state()?
                    .unregister_prompt_pattern(&unregistration.pattern_id);
                Ok(terminal_signal::PromptPatternUnregistered {
                    terminal: unregistration.terminal,
                    pattern_id: unregistration.pattern_id,
                }
                .into())
            }
            terminal_signal::TerminalRequest::ListPromptPatterns(list) => {
                let entries = self.signal_state()?.prompt_pattern_entries();
                Ok(terminal_signal::PromptPatternList {
                    terminal: list.terminal,
                    entries,
                }
                .into())
            }
            terminal_signal::TerminalRequest::AcquireInputGate(acquire) => {
                self.acquire_signal_input_gate(acquire)
            }
            terminal_signal::TerminalRequest::ReleaseInputGate(release) => {
                self.release_signal_input_gate(release)
            }
            terminal_signal::TerminalRequest::WriteInjection(injection) => {
                self.write_signal_injection(injection)
            }
            terminal_signal::TerminalRequest::SubscribeTerminalWorkerLifecycle(subscription) => {
                Ok(terminal_signal::TerminalRejected {
                    terminal: subscription.terminal,
                    reason: terminal_signal::TerminalRejectionReason::TransportFailed,
                }
                .into())
            }
            terminal_signal::TerminalRequest::TerminalWorkerLifecycleRetraction(token) => {
                Ok(terminal_signal::TerminalRejected {
                    terminal: token.terminal,
                    reason: terminal_signal::TerminalRejectionReason::TransportFailed,
                }
                .into())
            }
        }
    }

    fn acquire_signal_input_gate(
        &mut self,
        acquire: terminal_signal::AcquireInputGate,
    ) -> io::Result<terminal_signal::TerminalReply> {
        let prompt_state = self.signal_prompt_state(acquire.prompt_pattern_id.as_ref())?;
        match self.input_port.close_human_input() {
            Ok(lease) => {
                let signal_lease = Self::signal_lease(lease);
                self.signal_state()?
                    .record_signal_lease(signal_lease.clone(), prompt_state.clone());
                Ok(terminal_signal::GateAcquired {
                    terminal: acquire.terminal,
                    lease: signal_lease,
                    prompt_state,
                }
                .into())
            }
            Err(TerminalCellError::InputGateAlreadyClosed(lease)) => {
                Ok(terminal_signal::GateBusy {
                    terminal: acquire.terminal,
                    current_holder: terminal_signal::InputGateLeaseId::new(
                        lease.sequence().into_u64(),
                    ),
                }
                .into())
            }
            Err(error) => Err(Self::terminal_error(error)),
        }
    }

    fn release_signal_input_gate(
        &mut self,
        release: terminal_signal::ReleaseInputGate,
    ) -> io::Result<terminal_signal::TerminalReply> {
        if self
            .signal_state()?
            .signal_lease_prompt_state(&release.lease)
            .is_none()
        {
            return Ok(terminal_signal::InjectionRejected {
                terminal: release.terminal,
                reason: terminal_signal::InjectionRejectionReason::UnknownLease,
            }
            .into());
        }

        let lease = Self::terminal_lease(&release.lease);
        match self.input_port.open_human_input(lease) {
            Ok(gate_release) => {
                self.signal_state()?.release_signal_lease(&release.lease);
                Ok(terminal_signal::GateReleased {
                    terminal: release.terminal,
                    lease: release.lease,
                    cached_human_bytes: terminal_signal::TerminalByteCount::new(
                        gate_release.held_byte_count() as u64,
                    ),
                }
                .into())
            }
            Err(TerminalCellError::StaleInputGateLease) => {
                self.signal_state()?.release_signal_lease(&release.lease);
                Ok(terminal_signal::InjectionRejected {
                    terminal: release.terminal,
                    reason: terminal_signal::InjectionRejectionReason::UnknownLease,
                }
                .into())
            }
            Err(error) => Err(Self::terminal_error(error)),
        }
    }

    fn write_signal_injection(
        &mut self,
        injection: terminal_signal::WriteInjection,
    ) -> io::Result<terminal_signal::TerminalReply> {
        let prompt_state = self
            .signal_state()?
            .signal_lease_prompt_state(&injection.lease)
            .cloned();
        let Some(prompt_state) = prompt_state else {
            return Ok(terminal_signal::InjectionRejected {
                terminal: injection.terminal,
                reason: terminal_signal::InjectionRejectionReason::UnknownLease,
            }
            .into());
        };

        if matches!(prompt_state, terminal_signal::PromptState::Dirty { .. }) {
            return Ok(terminal_signal::InjectionRejected {
                terminal: injection.terminal,
                reason: terminal_signal::InjectionRejectionReason::DirtyPrompt,
            }
            .into());
        }

        self.input_port
            .accept(TerminalInput::new(
                injection.bytes.as_slice().to_vec(),
                InputSource::Programmatic,
            ))
            .map_err(Self::terminal_error)?;
        let snapshot = self.snapshot()?;
        Ok(terminal_signal::InjectionAck {
            terminal: injection.terminal,
            generation: terminal_signal::TerminalGeneration::new(1),
            sequence: terminal_signal::TerminalSequence::new(snapshot.last_sequence().into_u64()),
        }
        .into())
    }

    fn stream_signal_worker_lifecycle(
        &mut self,
        subscription: terminal_signal::SubscribeTerminalWorkerLifecycle,
    ) -> io::Result<()> {
        let mut lifecycle = self
            .runtime
            .block_on(async {
                self.terminal
                    .ask(TerminalWorkerLifecycleSubscriptionRequest)
                    .await
            })
            .map_err(Self::actor_error)?;
        SocketReplyWriter::new(&mut self.stream).write_signal_event(
            terminal_signal::TerminalWorkerLifecycleSnapshot {
                terminal: subscription.terminal.clone(),
                observations: lifecycle
                    .replay()
                    .iter()
                    .cloned()
                    .map(Self::signal_worker_lifecycle)
                    .collect(),
            }
            .into(),
        )?;

        while let Some(event) = lifecycle.blocking_next_live_event() {
            SocketReplyWriter::new(&mut self.stream).write_signal_subscription_event(
                terminal_signal::TerminalWorkerLifecycleEvent {
                    terminal: subscription.terminal.clone(),
                    observation: Self::signal_worker_lifecycle(event),
                }
                .into(),
            )?;
        }
        Ok(())
    }

    fn signal_prompt_state(
        &self,
        pattern_id: Option<&terminal_signal::PromptPatternId>,
    ) -> io::Result<terminal_signal::PromptState> {
        let Some(pattern_id) = pattern_id else {
            return Ok(terminal_signal::PromptState::NotChecked);
        };
        let pattern = self.signal_state()?.prompt_pattern(pattern_id);
        let Some(pattern) = pattern else {
            return Ok(terminal_signal::PromptState::Dirty {
                trailing_count: terminal_signal::TerminalByteCount::new(
                    self.snapshot()?.bytes().len() as u64,
                ),
            });
        };
        let snapshot = self.snapshot()?;
        let trailing_count = Self::prompt_suffix_trailing_count(&pattern, snapshot.bytes())?;
        if trailing_count == 0 {
            Ok(terminal_signal::PromptState::Clean)
        } else {
            Ok(terminal_signal::PromptState::Dirty {
                trailing_count: terminal_signal::TerminalByteCount::new(trailing_count as u64),
            })
        }
    }

    fn prompt_suffix_trailing_count(
        pattern: &terminal_signal::PromptPattern,
        transcript: &[u8],
    ) -> io::Result<usize> {
        match pattern {
            terminal_signal::PromptPattern::LiteralSuffix(suffix) => {
                Ok(Self::literal_suffix_gap(transcript, suffix.as_slice()))
            }
            terminal_signal::PromptPattern::RegexSuffix { pattern } => {
                let pattern = std::str::from_utf8(pattern.as_slice()).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("prompt regex pattern is not utf-8: {error}"),
                    )
                })?;
                Regex::new(pattern)
                    .map(|regex| {
                        regex
                            .find_iter(transcript)
                            .last()
                            .map_or(transcript.len(), |matched| transcript.len() - matched.end())
                    })
                    .map_err(|error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("prompt regex pattern is invalid: {error}"),
                        )
                    })
            }
        }
    }

    fn literal_suffix_gap(transcript: &[u8], suffix: &[u8]) -> usize {
        if suffix.is_empty() || transcript.ends_with(suffix) {
            return 0;
        }

        transcript
            .windows(suffix.len())
            .rposition(|window| window == suffix)
            .map_or(transcript.len(), |position| {
                transcript.len() - position - suffix.len()
            })
    }

    fn signal_lease(lease: TerminalInputGateLease) -> terminal_signal::InputGateLease {
        terminal_signal::InputGateLease {
            id: terminal_signal::InputGateLeaseId::new(lease.sequence().into_u64()),
        }
    }

    fn terminal_lease(lease: &terminal_signal::InputGateLease) -> TerminalInputGateLease {
        TerminalInputGateLease::new(TerminalInputGateSequence::new(lease.id.into_u64()))
    }

    fn signal_worker_lifecycle(
        lifecycle: TerminalWorkerLifecycle,
    ) -> terminal_signal::TerminalWorkerLifecycle {
        match lifecycle {
            TerminalWorkerLifecycle::Started(worker) => {
                terminal_signal::TerminalWorkerLifecycle::Started(Self::signal_worker_kind(worker))
            }
            TerminalWorkerLifecycle::Stopped { worker, reason } => {
                terminal_signal::TerminalWorkerLifecycle::Stopped {
                    worker: Self::signal_worker_kind(worker),
                    reason: Self::signal_worker_stop(reason),
                }
            }
        }
    }

    fn signal_worker_kind(worker: TerminalWorkerKind) -> terminal_signal::TerminalWorkerKind {
        match worker {
            TerminalWorkerKind::InputWriter => terminal_signal::TerminalWorkerKind::InputWriter,
            TerminalWorkerKind::ViewerFanout => terminal_signal::TerminalWorkerKind::ViewerFanout,
            TerminalWorkerKind::TranscriptScriber => {
                terminal_signal::TerminalWorkerKind::TranscriptScriber
            }
            TerminalWorkerKind::OutputReader => terminal_signal::TerminalWorkerKind::OutputReader,
            TerminalWorkerKind::ChildExitWatcher => {
                terminal_signal::TerminalWorkerKind::ChildExitWatcher
            }
            TerminalWorkerKind::SocketAcceptLoop => {
                terminal_signal::TerminalWorkerKind::SocketAcceptLoop
            }
            TerminalWorkerKind::AttachConnectionPump => {
                terminal_signal::TerminalWorkerKind::AttachConnectionPump
            }
        }
    }

    fn signal_worker_stop(reason: TerminalWorkerStop) -> terminal_signal::TerminalWorkerStopReason {
        match reason {
            TerminalWorkerStop::InputCommandChannelClosed => {
                terminal_signal::TerminalWorkerStopReason::InputCommandChannelClosed
            }
            TerminalWorkerStop::InputWriteFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::InputWriteFailed(error)
            }
            TerminalWorkerStop::OutputCommandChannelClosed => {
                terminal_signal::TerminalWorkerStopReason::OutputCommandChannelClosed
            }
            TerminalWorkerStop::TranscriptNoticeChannelClosed => {
                terminal_signal::TerminalWorkerStopReason::TranscriptNoticeChannelClosed
            }
            TerminalWorkerStop::OutputReaderFinished => {
                terminal_signal::TerminalWorkerStopReason::OutputReaderFinished
            }
            TerminalWorkerStop::OutputReadFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::OutputReadFailed(error)
            }
            TerminalWorkerStop::OutputPortClosed => {
                terminal_signal::TerminalWorkerStopReason::OutputPortClosed
            }
            TerminalWorkerStop::ChildExited(status) => {
                terminal_signal::TerminalWorkerStopReason::ChildExited(status)
            }
            TerminalWorkerStop::ChildWaitFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::ChildWaitFailed(error)
            }
            TerminalWorkerStop::SocketAcceptFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::SocketAcceptFailed(error)
            }
            TerminalWorkerStop::AttachConnectionClosed => {
                terminal_signal::TerminalWorkerStopReason::AttachConnectionClosed
            }
            TerminalWorkerStop::AttachConnectionFailed(error) => {
                terminal_signal::TerminalWorkerStopReason::AttachConnectionFailed(error)
            }
        }
    }

    fn signal_state(&self) -> io::Result<MutexGuard<'_, TerminalSignalControlState>> {
        self.signal_state
            .lock()
            .map_err(|_| io::Error::other("terminal signal control state lock poisoned"))
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

/// A data-plane connection. Carries only an attach handshake followed by
/// raw bidirectional bytes between the viewer and the child PTY. The
/// connection rejects every kind of request other than `Attach` with an
/// explicit attach-rejection reply so the wire boundary stays clean.
struct TerminalDataConnection {
    stream: UnixStream,
    terminal: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    output_port: TerminalOutputPort,
    runtime: Handle,
}

impl TerminalDataConnection {
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
            SocketRequest::Attach => self.attach_viewer(),
            other => self.reject_non_attach_on_data_plane(other),
        }
    }

    fn reject_non_attach_on_data_plane(&mut self, _request: SocketRequest) -> io::Result<()> {
        SocketReplyWriter::new(&mut self.stream).write_attach_rejected(
            "data socket only accepts viewer attach; use the control socket",
        )?;
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "non-attach request arrived on terminal-cell data socket",
        ))
    }

    fn attach_viewer(&mut self) -> io::Result<()> {
        let lease = match self.output_port.reserve_viewer() {
            Ok(lease) => lease,
            Err(TerminalCellError::ViewerAlreadyAttached) => {
                SocketReplyWriter::new(&mut self.stream)
                    .write_attach_rejected("terminal cell already has an attached viewer")?;
                return Ok(());
            }
            Err(error) => return Err(Self::terminal_error(error)),
        };

        let result = self.complete_viewer_attach(lease);
        if result.is_err() {
            let _ = self.output_port.detach(lease);
        }
        result
    }

    fn complete_viewer_attach(&mut self, lease: TerminalViewerLease) -> io::Result<()> {
        SocketReplyWriter::new(&mut self.stream).write_attach_accepted()?;

        let snapshot = self.snapshot()?;
        if !snapshot.bytes().is_empty() {
            self.stream.write_all(snapshot.bytes())?;
            self.stream.flush()?;
        }

        self.output_port
            .activate_viewer(lease, self.stream.try_clone()?)
            .map_err(Self::terminal_error)?;

        self.record_worker_started(TerminalWorkerKind::AttachConnectionPump);
        let result = self.pump_viewer_input();
        let reason = match &result {
            Ok(()) => TerminalWorkerStop::AttachConnectionClosed,
            Err(error) => TerminalWorkerStop::AttachConnectionFailed(error.to_string()),
        };
        self.record_worker_stopped(TerminalWorkerKind::AttachConnectionPump, reason);
        let _ = self.output_port.detach(lease);
        result
    }

    fn pump_viewer_input(&mut self) -> io::Result<()> {
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

    fn snapshot(&self) -> io::Result<terminal_cell::TranscriptSnapshot> {
        // The data plane reads the transcript snapshot through the actor
        // only to replay history bytes once at attach time. Bytes after
        // replay flow directly from the PTY-output fanout to the data
        // stream, never through the actor mailbox.
        let reply = self
            .runtime
            .block_on(async { self.terminal.ask(TranscriptSnapshotRequest).await })
            .map_err(Self::actor_error)?;
        Ok(reply)
    }

    fn record_worker_started(&self, worker: TerminalWorkerKind) {
        let _ = self
            .terminal
            .tell(TerminalWorkerLifecycle::Started(worker))
            .try_send();
    }

    fn record_worker_stopped(&self, worker: TerminalWorkerKind, reason: TerminalWorkerStop) {
        let _ = self
            .terminal
            .tell(TerminalWorkerLifecycle::Stopped { worker, reason })
            .try_send();
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

#[cfg(test)]
mod tests {
    use super::TerminalSocketFile;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct SocketPathFixture {
        path: PathBuf,
        root: PathBuf,
    }

    impl SocketPathFixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("terminal-cell-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("socket fixture root created");

            Self {
                path: root.join("control.sock"),
                root,
            }
        }
    }

    impl Drop for SocketPathFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn terminal_socket_file_bind_listener_applies_mode() {
        let fixture = SocketPathFixture::new("socket-mode");
        let _listener = TerminalSocketFile::new(fixture.path.as_path())
            .bind_listener()
            .expect("socket listener binds");
        let metadata = fs::metadata(fixture.path.as_path()).expect("socket metadata readable");
        let mode = metadata.permissions().mode() & 0o777;

        assert_eq!(mode, 0o600);
    }
}
