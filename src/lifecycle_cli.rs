use std::env;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dotos::{DotosDecode, DotosEncode, DotosSource};

use crate::{Configuration, ConfigurationEnvironmentVariable, TerminalCellSocketClient};

type CliResult<Value> = Result<Value, Box<dyn Error + Send + Sync>>;

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(10);
const CLOSE_WAIT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub enum CellRequest {
    LaunchCell(LaunchCell),
    SendLine(SendLine),
    AttachViewer(AttachViewer),
    CloseCell(CloseCell),
    ObserveCell(ObserveCell),
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct LaunchCell {
    pub requested_name: Option<String>,
    pub working_directory: Option<String>,
    pub command: String,
    pub arguments: Vec<String>,
    pub environment: Vec<CellEnvironmentVariable>,
}

impl LaunchCell {
    fn into_launcher(self) -> CellLauncher {
        CellLauncher::new(self)
    }
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct CellEnvironmentVariable {
    pub name: String,
    pub value: String,
}

impl CellEnvironmentVariable {
    fn into_configuration(self) -> ConfigurationEnvironmentVariable {
        ConfigurationEnvironmentVariable::new(self.name, self.value)
    }
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct SendLine {
    pub cell: String,
    pub line: String,
}

impl SendLine {
    fn send(self) -> CliResult<CellResponse> {
        let session = RuntimeSession::from_locator(self.cell)?;
        let mut bytes = self.line.into_bytes();
        bytes.push(b'\r');
        session.control_client().send_programmatic_input(&bytes)?;
        Ok(CellResponse::LineSent(LineSent {
            cell: session.name()?,
            control_socket: session.control_socket_text(),
        }))
    }
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct AttachViewer {
    pub cell: String,
    pub mode: ViewerMode,
}

impl AttachViewer {
    fn attach(self) -> CliResult<CellResponse> {
        let session = RuntimeSession::from_locator(self.cell)?;
        Ok(CellResponse::ViewerAttached(
            ViewerProcess::new(session, self.mode).attach()?,
        ))
    }
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub enum ViewerMode {
    Interactive,
    Snapshot,
}

impl ViewerMode {
    fn arguments(&self, session: &RuntimeSession) -> Vec<String> {
        let mut arguments = vec![
            "--control-socket".to_owned(),
            session.control_socket_text(),
            "--data-socket".to_owned(),
            session.data_socket_text(),
        ];
        if matches!(self, Self::Snapshot) {
            arguments.push("--once".to_owned());
        }
        arguments
    }
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct ObserveCell {
    pub cell: String,
}

impl ObserveCell {
    fn observe(self) -> CliResult<CellResponse> {
        let session = RuntimeSession::from_locator(self.cell)?;
        Ok(CellResponse::CellObserved(session.observe()?))
    }
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct CloseCell {
    pub cell: String,
}

impl CloseCell {
    fn close(self) -> CliResult<CellResponse> {
        let session = RuntimeSession::from_locator(self.cell)?;
        let daemon = ProcessHandle::new(session.daemon_pid()?);
        let child = session.child_pid()?.map(ProcessHandle::new);
        let before = daemon.state();
        let child_outcome = child.as_ref().map(ProcessHandle::terminate);
        let daemon_outcome = daemon.terminate();
        let child_terminated = child_outcome
            .as_ref()
            .map(TerminationOutcome::terminated)
            .unwrap_or(false);
        let daemon_terminated = daemon_outcome.terminated();
        Ok(CellResponse::CellClosed(CellClosed {
            cell: session.name()?,
            session_path: session.path_text(),
            daemon_pid: daemon.pid(),
            child_pid: child.map(ProcessHandle::pid),
            was_live: before.is_live(),
            terminated: daemon_terminated && child_terminated,
            daemon_terminated,
            child_terminated,
        }))
    }
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub enum CellResponse {
    CellLaunched(CellLaunched),
    LineSent(LineSent),
    ViewerAttached(ViewerAttached),
    CellObserved(CellObservation),
    CellClosed(CellClosed),
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct CellLaunched {
    pub cell: String,
    pub session_path: String,
    pub control_socket: String,
    pub data_socket: String,
    pub daemon_pid: u64,
    pub working_directory: String,
    pub command: String,
    pub arguments: Vec<String>,
    pub child_pid: Option<u64>,
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct LineSent {
    pub cell: String,
    pub control_socket: String,
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct ViewerAttached {
    pub cell: String,
    pub session_path: String,
    pub control_socket: String,
    pub data_socket: String,
    pub viewer_pid: Option<u64>,
    pub mode: ViewerMode,
    pub snapshot: Option<String>,
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct CellObservation {
    pub cell: String,
    pub session_path: String,
    pub control_socket: String,
    pub data_socket: String,
    pub daemon_pid: u64,
    pub daemon_state: ProcessState,
    pub working_directory: String,
    pub exit_state: Option<String>,
    pub stall_state: StallState,
    pub transcript_offset: u64,
    pub transcript_bytes: u64,
    pub worker_observation: String,
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub enum ProcessState {
    Live,
    Exited,
}

impl ProcessState {
    fn from_pid(pid: u64) -> Self {
        let path = Path::new("/proc").join(pid.to_string());
        if !path.exists() {
            return Self::Exited;
        }
        let status = fs::read_to_string(path.join("status")).unwrap_or_default();
        if ProcessStatusText::new(&status).is_zombie() {
            Self::Exited
        } else {
            Self::Live
        }
    }

    fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub enum StallState {
    NotMeasured,
}

#[derive(Clone, Debug, Eq, DotosDecode, DotosEncode, PartialEq)]
pub struct CellClosed {
    pub cell: String,
    pub session_path: String,
    pub daemon_pid: u64,
    pub child_pid: Option<u64>,
    pub was_live: bool,
    pub terminated: bool,
    pub daemon_terminated: bool,
    pub child_terminated: bool,
}

pub struct TerminalCellCli {
    input: CliInput,
}

impl TerminalCellCli {
    pub fn from_environment() -> CliResult<Self> {
        Ok(Self {
            input: CliInput::from_environment()?,
        })
    }

    pub fn run(&self) -> CliResult<()> {
        let request = self.input.read_request()?;
        let response = request.execute()?;
        writeln!(io::stdout(), "{}", response.to_dotos())?;
        Ok(())
    }
}

impl CellRequest {
    fn execute(self) -> CliResult<CellResponse> {
        match self {
            Self::LaunchCell(request) => request.into_launcher().launch(),
            Self::SendLine(request) => request.send(),
            Self::AttachViewer(request) => request.attach(),
            Self::CloseCell(request) => request.close(),
            Self::ObserveCell(request) => request.observe(),
        }
    }
}

struct CliInput {
    source: CliInputSource,
}

impl CliInput {
    fn from_environment() -> CliResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut source = None;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--file" => {
                    if source.is_some() {
                        return Err("terminal-cell accepts only one input source".into());
                    }
                    source = Some(CliInputSource::File(PathBuf::from(
                        arguments
                            .next()
                            .ok_or("terminal-cell requires a path after --file")?,
                    )));
                }
                other => return Err(format!("unknown terminal-cell argument: {other}").into()),
            }
        }
        Ok(Self {
            source: source.unwrap_or(CliInputSource::Stdin),
        })
    }

    fn read_request(&self) -> CliResult<CellRequest> {
        let mut text = String::new();
        match &self.source {
            CliInputSource::Stdin => {
                io::stdin().read_to_string(&mut text)?;
            }
            CliInputSource::File(path) => {
                text = fs::read_to_string(path)?;
            }
        }
        Ok(DotosSource::new(&text).parse::<CellRequest>()?)
    }
}

enum CliInputSource {
    Stdin,
    File(PathBuf),
}

struct CellLauncher {
    request: LaunchCell,
}

impl CellLauncher {
    fn new(request: LaunchCell) -> Self {
        Self { request }
    }

    fn launch(self) -> CliResult<CellResponse> {
        let session = RuntimeSession::create(self.request.requested_name.as_deref())?;
        let working_directory = self.working_directory()?;
        session.write_metadata(&self.request, working_directory.as_path())?;
        session.write_configuration(&self.request, working_directory.as_path())?;
        let pid = session.spawn_daemon(working_directory.as_path())?;
        session.wait_until_ready(DEFAULT_READY_TIMEOUT)?;
        let child_pid = session.child_pid()?;
        Ok(CellResponse::CellLaunched(CellLaunched {
            cell: session.name()?,
            session_path: session.path_text(),
            control_socket: session.control_socket_text(),
            data_socket: session.data_socket_text(),
            daemon_pid: pid,
            working_directory: working_directory.to_string_lossy().into_owned(),
            command: self.request.command,
            arguments: self.request.arguments,
            child_pid,
        }))
    }

    fn working_directory(&self) -> CliResult<PathBuf> {
        match &self.request.working_directory {
            Some(path) => Ok(PathBuf::from(path)),
            None => Ok(env::current_dir()?),
        }
    }
}

#[derive(Clone)]
struct RuntimeSession {
    path: PathBuf,
}

impl RuntimeSession {
    fn create(requested_name: Option<&str>) -> CliResult<Self> {
        let root = Self::runtime_root();
        fs::create_dir_all(root.as_path())?;
        let name = SessionName::new(requested_name);
        let path = root.join(name.directory_name());
        fs::create_dir_all(path.as_path())?;
        Ok(Self { path })
    }

    fn from_locator(locator: String) -> CliResult<Self> {
        let direct = PathBuf::from(&locator);
        if direct.is_dir() {
            return Ok(Self { path: direct });
        }
        let named = Self::runtime_root().join(locator);
        if named.is_dir() {
            return Ok(Self { path: named });
        }
        Err(format!("terminal-cell session not found: {}", direct.display()).into())
    }

    fn runtime_root() -> PathBuf {
        env::var_os("TERMINAL_CELL_RUNTIME_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
            .unwrap_or_else(env::temp_dir)
            .join("terminal-cell")
    }

    fn write_metadata(&self, request: &LaunchCell, working_directory: &Path) -> CliResult<()> {
        fs::write(self.path.join("session.name"), self.directory_name()?)?;
        fs::write(
            self.path.join("session.cwd"),
            working_directory.to_string_lossy().as_bytes(),
        )?;
        fs::write(
            self.path.join("session.command"),
            request.command.as_bytes(),
        )?;
        fs::write(
            self.path.join("session.arguments"),
            request.arguments.join("\n").as_bytes(),
        )?;
        Ok(())
    }

    fn write_configuration(&self, request: &LaunchCell, working_directory: &Path) -> CliResult<()> {
        let configuration = Configuration::with_working_directory_and_environment(
            self.control_socket_text(),
            self.data_socket_text(),
            request.command.clone(),
            request.arguments.clone(),
            Some(working_directory.to_string_lossy().into_owned()),
            request
                .environment
                .clone()
                .into_iter()
                .map(CellEnvironmentVariable::into_configuration)
                .collect(),
        )
        .with_child_process_identifier_path(Some(self.child_process_identifier_path_text()));
        fs::write(self.configuration_path(), configuration.to_signal_bytes()?)?;
        Ok(())
    }

    fn spawn_daemon(&self, working_directory: &Path) -> CliResult<u64> {
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.join("daemon.stdout"))?;
        let stderr = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.join("daemon.stderr"))?;
        let mut command = Command::new(Self::daemon_binary()?);
        command
            .arg(self.configuration_path())
            .current_dir(working_directory)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .process_group(0);
        let child = command.spawn()?;
        let pid = u64::from(child.id());
        fs::write(self.path.join("daemon.pid"), pid.to_string())?;
        Ok(pid)
    }

    fn daemon_binary() -> CliResult<PathBuf> {
        if let Some(path) = env::var_os("TERMINAL_CELL_DAEMON_BIN") {
            return Ok(PathBuf::from(path));
        }
        let mut path = env::current_exe()?;
        path.set_file_name("terminal-cell-daemon");
        Ok(path)
    }

    fn viewer_binary() -> CliResult<PathBuf> {
        if let Some(path) = env::var_os("TERMINAL_CELL_VIEWER_BIN") {
            return Ok(PathBuf::from(path));
        }
        let mut path = env::current_exe()?;
        path.set_file_name("terminal-cell-view");
        Ok(path)
    }

    fn wait_until_ready(&self, timeout: Duration) -> CliResult<()> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.socket_is_present("control.sock") && self.socket_is_present("data.sock") {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
        }
        Err(format!(
            "terminal-cell daemon did not bind sockets under {}",
            self.path.display()
        )
        .into())
    }

    fn observe(&self) -> CliResult<CellObservation> {
        let client = self.control_client();
        let transcript = client.capture().unwrap_or_default();
        let worker_observation = client
            .worker_observation_text()
            .unwrap_or_else(|error| format!("worker observation unavailable: {error}"));
        let pid = self.daemon_pid()?;
        Ok(CellObservation {
            cell: self.name()?,
            session_path: self.path_text(),
            control_socket: self.control_socket_text(),
            data_socket: self.data_socket_text(),
            daemon_pid: pid,
            daemon_state: ProcessState::from_pid(pid),
            working_directory: self.working_directory()?,
            exit_state: WorkerObservationText::new(&worker_observation).exit_state(),
            stall_state: StallState::NotMeasured,
            transcript_offset: transcript.len() as u64,
            transcript_bytes: transcript.len() as u64,
            worker_observation,
        })
    }

    fn control_client(&self) -> TerminalCellSocketClient {
        TerminalCellSocketClient::for_control_only(self.control_socket())
    }

    fn daemon_pid(&self) -> CliResult<u64> {
        let text = fs::read_to_string(self.path.join("daemon.pid"))?;
        Ok(text.trim().parse::<u64>()?)
    }

    fn child_pid(&self) -> CliResult<Option<u64>> {
        let path = self.child_process_identifier_path();
        match fs::read_to_string(path) {
            Ok(text) => Ok(Some(text.trim().parse::<u64>()?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn working_directory(&self) -> CliResult<String> {
        Ok(fs::read_to_string(self.path.join("session.cwd"))?)
    }

    fn name(&self) -> CliResult<String> {
        Ok(fs::read_to_string(self.path.join("session.name"))?)
    }

    fn directory_name(&self) -> CliResult<String> {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .map(std::string::ToString::to_string)
            .ok_or_else(|| {
                format!(
                    "invalid terminal-cell session path: {}",
                    self.path.display()
                )
                .into()
            })
    }

    fn path_text(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn control_socket_text(&self) -> String {
        self.control_socket().to_string_lossy().into_owned()
    }

    fn data_socket_text(&self) -> String {
        self.data_socket().to_string_lossy().into_owned()
    }

    fn configuration_path(&self) -> PathBuf {
        self.path.join("daemon-configuration.rkyv")
    }

    fn child_process_identifier_path(&self) -> PathBuf {
        self.path.join("child.pid")
    }

    fn child_process_identifier_path_text(&self) -> String {
        self.child_process_identifier_path()
            .to_string_lossy()
            .into_owned()
    }

    fn control_socket(&self) -> PathBuf {
        self.path.join("control.sock")
    }

    fn data_socket(&self) -> PathBuf {
        self.path.join("data.sock")
    }

    fn socket_is_present(&self, name: &str) -> bool {
        fs::symlink_metadata(self.path.join(name))
            .map(|metadata| metadata.file_type().is_socket())
            .unwrap_or(false)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProcessHandle {
    pid: u64,
}

impl ProcessHandle {
    fn new(pid: u64) -> Self {
        Self { pid }
    }

    const fn pid(self) -> u64 {
        self.pid
    }

    fn state(self) -> ProcessState {
        ProcessState::from_pid(self.pid)
    }

    fn terminate(&self) -> TerminationOutcome {
        let was_live = self.state().is_live();
        if !was_live {
            return TerminationOutcome::new(*self, was_live);
        }

        self.signal(TerminationSignal::Terminate, SignalTarget::ProcessGroup);
        self.signal(TerminationSignal::Terminate, SignalTarget::Process);
        if self.wait_for_exit(CLOSE_WAIT) {
            return TerminationOutcome::new(*self, was_live);
        }

        self.signal(TerminationSignal::Kill, SignalTarget::ProcessGroup);
        self.signal(TerminationSignal::Kill, SignalTarget::Process);
        let _ = self.wait_for_exit(CLOSE_WAIT);
        TerminationOutcome::new(*self, was_live)
    }

    fn signal(&self, signal: TerminationSignal, target: SignalTarget) {
        let _ = Command::new("kill")
            .arg(signal.argument())
            .arg(target.argument(self.pid))
            .status();
    }

    fn wait_for_exit(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if !self.state().is_live() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        !self.state().is_live()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TerminationOutcome {
    process: ProcessHandle,
    was_live: bool,
}

impl TerminationOutcome {
    fn new(process: ProcessHandle, was_live: bool) -> Self {
        Self { process, was_live }
    }

    fn terminated(&self) -> bool {
        self.was_live && !self.process.state().is_live()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminationSignal {
    Terminate,
    Kill,
}

impl TerminationSignal {
    const fn argument(self) -> &'static str {
        match self {
            Self::Terminate => "-TERM",
            Self::Kill => "-KILL",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SignalTarget {
    Process,
    ProcessGroup,
}

impl SignalTarget {
    fn argument(self, pid: u64) -> String {
        match self {
            Self::Process => pid.to_string(),
            Self::ProcessGroup => format!("-{pid}"),
        }
    }
}

struct ProcessStatusText<'text> {
    text: &'text str,
}

impl<'text> ProcessStatusText<'text> {
    fn new(text: &'text str) -> Self {
        Self { text }
    }

    fn is_zombie(&self) -> bool {
        self.text
            .lines()
            .any(|line| line.starts_with("State:") && line.contains('Z'))
    }
}

struct SessionName {
    stem: String,
    suffix: u128,
}

impl SessionName {
    fn new(requested_name: Option<&str>) -> Self {
        Self {
            stem: requested_name
                .map(Self::sanitize)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "cell".to_owned()),
            suffix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0),
        }
    }

    fn sanitize(name: &str) -> String {
        name.chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                    character
                } else {
                    '-'
                }
            })
            .collect()
    }

    fn directory_name(&self) -> String {
        format!("session-{}-{}", self.stem, self.suffix)
    }
}

struct ViewerProcess {
    session: RuntimeSession,
    mode: ViewerMode,
}

impl ViewerProcess {
    fn new(session: RuntimeSession, mode: ViewerMode) -> Self {
        Self { session, mode }
    }

    fn attach(&self) -> CliResult<ViewerAttached> {
        match self.mode {
            ViewerMode::Interactive => self.spawn_interactive(),
            ViewerMode::Snapshot => self.print_snapshot(),
        }
    }

    fn spawn_interactive(&self) -> CliResult<ViewerAttached> {
        let child = Command::new(RuntimeSession::viewer_binary()?)
            .args(self.mode.arguments(&self.session))
            .spawn()?;
        Ok(self.reply(Some(u64::from(child.id())), None))
    }

    fn print_snapshot(&self) -> CliResult<ViewerAttached> {
        let output = Command::new(RuntimeSession::viewer_binary()?)
            .args(self.mode.arguments(&self.session))
            .output()?;
        if output.status.success() {
            Ok(self.reply(
                None,
                Some(String::from_utf8_lossy(&output.stdout).into_owned()),
            ))
        } else {
            Err(format!(
                "terminal-cell-view snapshot failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into())
        }
    }

    fn reply(&self, viewer_pid: Option<u64>, snapshot: Option<String>) -> ViewerAttached {
        ViewerAttached {
            cell: self.session.name().unwrap_or_else(|_| String::new()),
            session_path: self.session.path_text(),
            control_socket: self.session.control_socket_text(),
            data_socket: self.session.data_socket_text(),
            viewer_pid,
            mode: self.mode.clone(),
            snapshot,
        }
    }
}

struct WorkerObservationText<'text> {
    text: &'text str,
}

impl<'text> WorkerObservationText<'text> {
    fn new(text: &'text str) -> Self {
        Self { text }
    }

    fn exit_state(&self) -> Option<String> {
        self.text
            .lines()
            .rev()
            .find(|line| line.contains("ChildExited") || line.contains("ChildWaitFailed"))
            .map(std::string::ToString::to_string)
    }
}
