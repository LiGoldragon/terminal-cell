use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use kameo::actor::{ActorRef, Spawn};
use kameo::error::Infallible;
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use kameo::{Actor, Reply};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::broadcast;

use crate::error::TerminalCellError;
use crate::snapshot::ScreenProjection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCommand {
    program: String,
    arguments: Vec<String>,
    working_directory: Option<String>,
    environment: Vec<(String, String)>,
}

impl TerminalCommand {
    pub fn new(program: impl Into<String>, arguments: impl Into<Vec<String>>) -> Self {
        Self::with_working_directory_and_environment(program, arguments, None, Vec::new())
    }

    pub fn with_working_directory_and_environment(
        program: impl Into<String>,
        arguments: impl Into<Vec<String>>,
        working_directory: Option<String>,
        environment: Vec<(String, String)>,
    ) -> Self {
        Self {
            program: program.into(),
            arguments: arguments.into(),
            working_directory,
            environment,
        }
    }

    fn into_builder(self) -> CommandBuilder {
        let mut builder = CommandBuilder::new(self.program);
        for argument in self.arguments {
            builder.arg(argument);
        }
        if let Some(working_directory) = self.working_directory {
            builder.cwd(working_directory);
        }
        for (name, value) in self.environment {
            builder.env(name, value);
        }
        builder.env("TERM", "xterm-256color");
        builder
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    rows: u16,
    columns: u16,
}

impl TerminalSize {
    pub const fn new(rows: u16, columns: u16) -> Self {
        Self { rows, columns }
    }

    pub const fn rows(self) -> u16 {
        self.rows
    }

    pub const fn columns(self) -> u16 {
        self.columns
    }

    fn into_pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.columns,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLaunch {
    command: TerminalCommand,
    size: TerminalSize,
    child_process_identifier_path: Option<String>,
}

impl TerminalLaunch {
    pub fn new(command: TerminalCommand, size: TerminalSize) -> Self {
        Self {
            command,
            size,
            child_process_identifier_path: None,
        }
    }

    pub fn with_child_process_identifier_path(mut self, path: Option<String>) -> Self {
        self.child_process_identifier_path = path;
        self
    }
}

#[doc(hidden)]
pub struct TerminalCellStart {
    launch: TerminalLaunch,
    input_receiver: Receiver<TerminalInputCommand>,
    output_port: TerminalOutputPort,
    output_receiver: Receiver<TerminalOutputCommand>,
}

impl TerminalCellStart {
    fn new(
        launch: TerminalLaunch,
        input_receiver: Receiver<TerminalInputCommand>,
        output_port: TerminalOutputPort,
        output_receiver: Receiver<TerminalOutputCommand>,
    ) -> Self {
        Self {
            launch,
            input_receiver,
            output_port,
            output_receiver,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TerminalSequence(u64);

impl TerminalSequence {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn into_u64(self) -> u64 {
        self.0
    }

    fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptDelta {
    sequence: TerminalSequence,
    bytes: Vec<u8>,
}

impl TranscriptDelta {
    fn new(sequence: TerminalSequence, bytes: Vec<u8>) -> Self {
        Self { sequence, bytes }
    }

    pub const fn sequence(&self) -> TerminalSequence {
        self.sequence
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSnapshot {
    bytes: Vec<u8>,
    last_sequence: TerminalSequence,
}

impl TranscriptSnapshot {
    fn new(bytes: Vec<u8>, last_sequence: TerminalSequence) -> Self {
        Self {
            bytes,
            last_sequence,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub const fn last_sequence(&self) -> TerminalSequence {
        self.last_sequence
    }

    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    pub fn contains(&self, needle: &[u8]) -> bool {
        self.bytes
            .windows(needle.len())
            .any(|window| window == needle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalWorkerKind {
    InputWriter,
    ViewerFanout,
    TranscriptScriber,
    OutputReader,
    ChildExitWatcher,
    SocketAcceptLoop,
    AttachConnectionPump,
}

#[derive(Debug, Clone, PartialEq, Eq, Reply)]
pub enum TerminalWorkerStop {
    InputCommandChannelClosed,
    InputWriteFailed(String),
    OutputCommandChannelClosed,
    TranscriptNoticeChannelClosed,
    OutputReaderFinished,
    OutputReadFailed(String),
    OutputPortClosed,
    ChildExited(String),
    ChildWaitFailed(String),
    SocketAcceptFailed(String),
    AttachConnectionClosed,
    AttachConnectionFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalWorkerLifecycle {
    Started(TerminalWorkerKind),
    Stopped {
        worker: TerminalWorkerKind,
        reason: TerminalWorkerStop,
    },
}

impl TerminalWorkerLifecycle {
    pub const fn worker(&self) -> TerminalWorkerKind {
        match self {
            Self::Started(worker) | Self::Stopped { worker, .. } => *worker,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalWorkerObservation {
    events: Vec<TerminalWorkerLifecycle>,
}

impl TerminalWorkerObservation {
    fn new() -> Self {
        Self { events: Vec::new() }
    }

    fn record(&mut self, lifecycle: TerminalWorkerLifecycle) {
        self.events.push(lifecycle);
    }

    pub fn events(&self) -> &[TerminalWorkerLifecycle] {
        self.events.as_slice()
    }

    pub fn has_started(&self, worker: TerminalWorkerKind) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event, TerminalWorkerLifecycle::Started(kind) if *kind == worker))
    }

    pub fn stopped_reason(&self, worker: TerminalWorkerKind) -> Option<&TerminalWorkerStop> {
        self.events.iter().rev().find_map(|event| match event {
            TerminalWorkerLifecycle::Stopped {
                worker: stopped_worker,
                reason,
            } if *stopped_worker == worker => Some(reason),
            TerminalWorkerLifecycle::Started(_)
            | TerminalWorkerLifecycle::Stopped {
                worker: _,
                reason: _,
            } => None,
        })
    }

    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for event in &self.events {
            match event {
                TerminalWorkerLifecycle::Started(worker) => {
                    text.push_str(&format!("started:{worker:?}\n"));
                }
                TerminalWorkerLifecycle::Stopped { worker, reason } => {
                    text.push_str(&format!("stopped:{worker:?}:{reason:?}\n"));
                }
            }
        }
        text
    }
}

struct TerminalWorkerReporter {
    actor: ActorRef<TerminalCell>,
    worker: TerminalWorkerKind,
}

impl TerminalWorkerReporter {
    fn new(actor: ActorRef<TerminalCell>, worker: TerminalWorkerKind) -> Self {
        Self { actor, worker }
    }

    fn started(&self) {
        let _ = self
            .actor
            .tell(TerminalWorkerLifecycle::Started(self.worker))
            .try_send();
    }

    fn stopped(&self, reason: TerminalWorkerStop) {
        let _ = self
            .actor
            .tell(TerminalWorkerLifecycle::Stopped {
                worker: self.worker,
                reason,
            })
            .try_send();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalTranscript {
    deltas: Vec<TranscriptDelta>,
    last_sequence: TerminalSequence,
}

impl TerminalTranscript {
    fn new() -> Self {
        Self {
            deltas: Vec::new(),
            last_sequence: TerminalSequence::new(0),
        }
    }

    fn append(&mut self, bytes: Vec<u8>) -> TranscriptDelta {
        self.last_sequence = self.last_sequence.next();
        let delta = TranscriptDelta::new(self.last_sequence, bytes);
        self.deltas.push(delta.clone());
        delta
    }

    fn snapshot(&self) -> TranscriptSnapshot {
        let bytes = self
            .deltas
            .iter()
            .flat_map(|delta| delta.bytes.iter().copied())
            .collect::<Vec<_>>();
        TranscriptSnapshot::new(bytes, self.last_sequence)
    }

    fn replay_after(&self, sequence: TerminalSequence) -> Vec<TranscriptDelta> {
        self.deltas
            .iter()
            .filter(|delta| delta.sequence > sequence)
            .cloned()
            .collect()
    }

    fn contains(&self, needle: &[u8]) -> bool {
        self.snapshot().contains(needle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    Viewer,
    Programmatic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalInput {
    bytes: Vec<u8>,
    source: InputSource,
}

impl TerminalInput {
    pub fn new(bytes: impl Into<Vec<u8>>, source: InputSource) -> Self {
        Self {
            bytes: bytes.into(),
            source,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub const fn source(&self) -> InputSource {
        self.source
    }
}

enum TerminalInputCommand {
    Write(TerminalInput),
    CloseHumanInput(Sender<Result<TerminalInputGateLease, TerminalCellError>>),
    OpenHumanInput(
        TerminalInputGateLease,
        Sender<Result<TerminalInputGateRelease, TerminalCellError>>,
    ),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TerminalInputGateSequence(u64);

impl TerminalInputGateSequence {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn into_u64(self) -> u64 {
        self.0
    }

    fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalInputGateLease {
    sequence: TerminalInputGateSequence,
}

impl TerminalInputGateLease {
    pub const fn new(sequence: TerminalInputGateSequence) -> Self {
        Self { sequence }
    }

    pub const fn sequence(self) -> TerminalInputGateSequence {
        self.sequence
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalInputGateRelease {
    lease: TerminalInputGateLease,
    held_byte_count: usize,
}

impl TerminalInputGateRelease {
    pub const fn new(lease: TerminalInputGateLease, held_byte_count: usize) -> Self {
        Self {
            lease,
            held_byte_count,
        }
    }

    pub const fn lease(self) -> TerminalInputGateLease {
        self.lease
    }

    pub const fn held_byte_count(self) -> usize {
        self.held_byte_count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalInputGateState {
    Open,
    Closed(TerminalInputGateLease),
}

struct TerminalInputGate {
    state: TerminalInputGateState,
    next_sequence: TerminalInputGateSequence,
    held_human_input: VecDeque<Vec<u8>>,
}

impl TerminalInputGate {
    fn new() -> Self {
        Self {
            state: TerminalInputGateState::Open,
            next_sequence: TerminalInputGateSequence::new(1),
            held_human_input: VecDeque::new(),
        }
    }

    fn close_human_input(&mut self) -> Result<TerminalInputGateLease, TerminalCellError> {
        match self.state {
            TerminalInputGateState::Closed(lease) => {
                Err(TerminalCellError::InputGateAlreadyClosed(lease))
            }
            TerminalInputGateState::Open => {
                let lease = TerminalInputGateLease::new(self.next_sequence);
                self.next_sequence = self.next_sequence.next();
                self.state = TerminalInputGateState::Closed(lease);
                Ok(lease)
            }
        }
    }

    fn open_human_input(
        &mut self,
        lease: TerminalInputGateLease,
        writer: &mut dyn Write,
    ) -> Result<TerminalInputGateRelease, TerminalCellError> {
        match self.state {
            TerminalInputGateState::Closed(active) if active == lease => {
                let held_byte_count = self.held_byte_count();
                self.flush_held_human_input(writer)?;
                self.state = TerminalInputGateState::Open;
                Ok(TerminalInputGateRelease::new(lease, held_byte_count))
            }
            TerminalInputGateState::Closed(_) | TerminalInputGateState::Open => {
                Err(TerminalCellError::StaleInputGateLease)
            }
        }
    }

    fn write_input(
        &mut self,
        writer: &mut dyn Write,
        input: TerminalInput,
    ) -> Result<(), TerminalCellError> {
        match (self.state, input.source()) {
            (TerminalInputGateState::Closed(_), InputSource::Viewer) => {
                self.held_human_input.push_back(input.into_bytes());
                Ok(())
            }
            _ => Self::write_bytes(writer, input.bytes()),
        }
    }

    fn held_byte_count(&self) -> usize {
        self.held_human_input.iter().map(std::vec::Vec::len).sum()
    }

    fn flush_held_human_input(&mut self, writer: &mut dyn Write) -> Result<(), TerminalCellError> {
        while let Some(bytes) = self.held_human_input.pop_front() {
            Self::write_bytes(writer, &bytes)?;
        }
        Ok(())
    }

    fn write_bytes(writer: &mut dyn Write, bytes: &[u8]) -> Result<(), TerminalCellError> {
        writer.write_all(bytes).map_err(TerminalCellError::pty)?;
        writer.flush().map_err(TerminalCellError::pty)
    }
}

struct TerminalInputWriter {
    writer: Box<dyn Write + Send>,
    input_gate: TerminalInputGate,
}

impl TerminalInputWriter {
    fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            writer,
            input_gate: TerminalInputGate::new(),
        }
    }

    fn run(&mut self, receiver: Receiver<TerminalInputCommand>) -> TerminalWorkerStop {
        while let Ok(command) = receiver.recv() {
            if let Err(reason) = self.handle(command) {
                return reason;
            }
        }
        TerminalWorkerStop::InputCommandChannelClosed
    }

    fn handle(&mut self, command: TerminalInputCommand) -> Result<(), TerminalWorkerStop> {
        match command {
            TerminalInputCommand::Write(input) => self
                .input_gate
                .write_input(self.writer.as_mut(), input)
                .map_err(|error| TerminalWorkerStop::InputWriteFailed(error.to_string())),
            TerminalInputCommand::CloseHumanInput(reply) => {
                let _ = reply.send(self.input_gate.close_human_input());
                Ok(())
            }
            TerminalInputCommand::OpenHumanInput(lease, reply) => {
                let _ = reply.send(
                    self.input_gate
                        .open_human_input(lease, self.writer.as_mut()),
                );
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TerminalInputPort {
    sender: Sender<TerminalInputCommand>,
}

impl TerminalInputPort {
    fn new(sender: Sender<TerminalInputCommand>) -> Self {
        Self { sender }
    }

    pub fn accept(&self, input: TerminalInput) -> Result<InputAcceptance, TerminalCellError> {
        let source = input.source();
        self.sender
            .send(TerminalInputCommand::Write(input))
            .map_err(|_| TerminalCellError::InputClosed)?;
        Ok(InputAcceptance::new(source))
    }

    pub fn close_human_input(&self) -> Result<TerminalInputGateLease, TerminalCellError> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .send(TerminalInputCommand::CloseHumanInput(sender))
            .map_err(|_| TerminalCellError::InputClosed)?;
        receiver
            .recv()
            .map_err(|_| TerminalCellError::InputClosed)?
    }

    pub fn open_human_input(
        &self,
        lease: TerminalInputGateLease,
    ) -> Result<TerminalInputGateRelease, TerminalCellError> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .send(TerminalInputCommand::OpenHumanInput(lease, sender))
            .map_err(|_| TerminalCellError::InputClosed)?;
        receiver
            .recv()
            .map_err(|_| TerminalCellError::InputClosed)?
    }
}

#[derive(Debug, Clone)]
pub struct TerminalOutputPort {
    sender: Sender<TerminalOutputCommand>,
}

impl TerminalOutputPort {
    fn new(sender: Sender<TerminalOutputCommand>) -> Self {
        Self { sender }
    }

    fn write_bytes(&self, bytes: Vec<u8>) -> Result<(), TerminalCellError> {
        self.sender
            .send(TerminalOutputCommand::Bytes(bytes))
            .map_err(|_| TerminalCellError::OutputClosed)
    }

    pub fn reserve_viewer(&self) -> Result<TerminalViewerLease, TerminalCellError> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .send(TerminalOutputCommand::ReserveViewer(sender))
            .map_err(|_| TerminalCellError::OutputClosed)?;
        receiver
            .recv()
            .map_err(|_| TerminalCellError::OutputClosed)?
    }

    pub fn activate_viewer(
        &self,
        lease: TerminalViewerLease,
        stream: UnixStream,
    ) -> Result<(), TerminalCellError> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .send(TerminalOutputCommand::ActivateViewer(lease, stream, sender))
            .map_err(|_| TerminalCellError::OutputClosed)?;
        receiver
            .recv()
            .map_err(|_| TerminalCellError::OutputClosed)?
    }

    pub fn attach(&self, stream: UnixStream) -> Result<TerminalViewerLease, TerminalCellError> {
        let lease = self.reserve_viewer()?;
        if let Err(error) = self.activate_viewer(lease, stream) {
            let _ = self.detach(lease);
            return Err(error);
        }
        Ok(lease)
    }

    pub fn detach(&self, lease: TerminalViewerLease) -> Result<(), TerminalCellError> {
        self.sender
            .send(TerminalOutputCommand::Detach(lease))
            .map_err(|_| TerminalCellError::OutputClosed)
    }
}

enum TerminalOutputCommand {
    Bytes(Vec<u8>),
    ReserveViewer(Sender<Result<TerminalViewerLease, TerminalCellError>>),
    ActivateViewer(
        TerminalViewerLease,
        UnixStream,
        Sender<Result<(), TerminalCellError>>,
    ),
    Detach(TerminalViewerLease),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalViewerLease {
    sequence: TerminalViewerSequence,
}

impl TerminalViewerLease {
    const fn new(sequence: TerminalViewerSequence) -> Self {
        Self { sequence }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TerminalViewerSequence(u64);

impl TerminalViewerSequence {
    const fn first() -> Self {
        Self(1)
    }

    fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

struct TerminalAttachedViewer {
    lease: TerminalViewerLease,
    stream: UnixStream,
}

impl TerminalAttachedViewer {
    fn new(lease: TerminalViewerLease, stream: UnixStream) -> Self {
        Self { lease, stream }
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()
    }

    const fn lease(&self) -> TerminalViewerLease {
        self.lease
    }
}

struct TerminalReservedViewer {
    lease: TerminalViewerLease,
    backlog: VecDeque<Vec<u8>>,
}

impl TerminalReservedViewer {
    fn new(lease: TerminalViewerLease) -> Self {
        Self {
            lease,
            backlog: VecDeque::new(),
        }
    }

    fn push_bytes(&mut self, bytes: Vec<u8>) {
        self.backlog.push_back(bytes);
    }

    fn pop_bytes(&mut self) -> Option<Vec<u8>> {
        self.backlog.pop_front()
    }
}

enum TerminalViewerSlot {
    Empty,
    Reserved(TerminalReservedViewer),
    Active(TerminalAttachedViewer),
}

/// The bounded transcript-notice queue capacity. When the queue fills, the
/// oldest pending notice is dropped to keep `ViewerFanout` non-blocking.
const TRANSCRIPT_NOTICE_QUEUE_CAPACITY: usize = 1024;

/// Notices flow `ViewerFanout` → `TranscriptScriber` through this typed
/// queue. The queue is bounded; on overflow the oldest pending notice is
/// dropped. The scriber is the only consumer; the fanout is the only
/// producer.
#[derive(Clone)]
struct TranscriptNoticeQueue {
    inner: Arc<TranscriptNoticeQueueInner>,
}

struct TranscriptNoticeQueueInner {
    state: Mutex<TranscriptNoticeQueueState>,
    not_empty: Condvar,
}

struct TranscriptNoticeQueueState {
    pending: VecDeque<TranscriptNotice>,
    capacity: usize,
    closed: bool,
    dropped_oldest: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TranscriptNotice {
    bytes: Vec<u8>,
}

impl TranscriptNoticeQueue {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(TranscriptNoticeQueueInner {
                state: Mutex::new(TranscriptNoticeQueueState {
                    pending: VecDeque::with_capacity(capacity),
                    capacity,
                    closed: false,
                    dropped_oldest: 0,
                }),
                not_empty: Condvar::new(),
            }),
        }
    }

    /// Non-blocking enqueue. Returns the number of oldest notices dropped
    /// during this push (0 in steady state, 1 when the queue is full).
    fn push_drop_oldest(&self, notice: TranscriptNotice) -> u64 {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("transcript notice queue mutex");
        let mut dropped = 0;
        while state.pending.len() >= state.capacity {
            state.pending.pop_front();
            dropped += 1;
            state.dropped_oldest = state.dropped_oldest.saturating_add(1);
        }
        state.pending.push_back(notice);
        self.inner.not_empty.notify_one();
        dropped
    }

    /// Blocking dequeue. Returns `None` only after the queue has been
    /// closed and drained.
    fn pop_blocking(&self) -> Option<TranscriptNotice> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("transcript notice queue mutex");
        loop {
            if let Some(notice) = state.pending.pop_front() {
                return Some(notice);
            }
            if state.closed {
                return None;
            }
            state = self
                .inner
                .not_empty
                .wait(state)
                .expect("transcript notice queue condvar");
        }
    }

    fn close(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("transcript notice queue mutex");
        state.closed = true;
        self.inner.not_empty.notify_all();
    }
}

/// `ViewerFanout` owns the active-viewer authority and the live byte path
/// from PTY → viewer. It writes PTY output bytes to the active viewer and
/// notifies `TranscriptScriber` over a bounded notification queue. It does
/// not append transcript itself; the actor mailbox is not on this path.
struct ViewerFanout {
    transcript: TranscriptNoticeQueue,
    viewer: TerminalViewerSlot,
    next_viewer_sequence: TerminalViewerSequence,
}

impl ViewerFanout {
    fn new(transcript: TranscriptNoticeQueue) -> Self {
        Self {
            transcript,
            viewer: TerminalViewerSlot::Empty,
            next_viewer_sequence: TerminalViewerSequence::first(),
        }
    }

    fn run(&mut self, receiver: Receiver<TerminalOutputCommand>) -> TerminalWorkerStop {
        while let Ok(command) = receiver.recv() {
            match command {
                TerminalOutputCommand::Bytes(bytes) => self.write_bytes(bytes),
                TerminalOutputCommand::ReserveViewer(reply) => self.reserve(reply),
                TerminalOutputCommand::ActivateViewer(lease, stream, reply) => {
                    self.activate(lease, stream, reply);
                }
                TerminalOutputCommand::Detach(lease) => self.detach(lease),
            }
        }
        TerminalWorkerStop::OutputCommandChannelClosed
    }

    fn reserve(&mut self, reply: Sender<Result<TerminalViewerLease, TerminalCellError>>) {
        if !matches!(self.viewer, TerminalViewerSlot::Empty) {
            let _ = reply.send(Err(TerminalCellError::ViewerAlreadyAttached));
            return;
        }

        let lease = TerminalViewerLease::new(self.next_viewer_sequence);
        self.next_viewer_sequence = self.next_viewer_sequence.next();
        self.viewer = TerminalViewerSlot::Reserved(TerminalReservedViewer::new(lease));
        let _ = reply.send(Ok(lease));
    }

    fn activate(
        &mut self,
        lease: TerminalViewerLease,
        stream: UnixStream,
        reply: Sender<Result<(), TerminalCellError>>,
    ) {
        let slot = std::mem::replace(&mut self.viewer, TerminalViewerSlot::Empty);
        match slot {
            TerminalViewerSlot::Reserved(reserved) if reserved.lease == lease => {
                match self.activate_reserved_viewer(reserved, stream) {
                    Ok(viewer) => {
                        self.viewer = TerminalViewerSlot::Active(viewer);
                        let _ = reply.send(Ok(()));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            TerminalViewerSlot::Reserved(reserved) => {
                self.viewer = TerminalViewerSlot::Reserved(reserved);
                let _ = reply.send(Err(TerminalCellError::StaleViewerLease));
            }
            TerminalViewerSlot::Active(viewer) => {
                self.viewer = TerminalViewerSlot::Active(viewer);
                let _ = reply.send(Err(TerminalCellError::ViewerAlreadyAttached));
            }
            TerminalViewerSlot::Empty => {
                let _ = reply.send(Err(TerminalCellError::StaleViewerLease));
            }
        }
    }

    fn activate_reserved_viewer(
        &self,
        mut reserved: TerminalReservedViewer,
        stream: UnixStream,
    ) -> Result<TerminalAttachedViewer, TerminalCellError> {
        let mut viewer = TerminalAttachedViewer::new(reserved.lease, stream);
        while let Some(bytes) = reserved.pop_bytes() {
            if let Err(error) = viewer.write_bytes(&bytes) {
                self.notify_transcript(bytes);
                self.notify_reserved_transcript(reserved);
                return Err(TerminalCellError::pty(error));
            }
            self.notify_transcript(bytes);
        }
        Ok(viewer)
    }

    fn detach(&mut self, lease: TerminalViewerLease) {
        let slot = std::mem::replace(&mut self.viewer, TerminalViewerSlot::Empty);
        match slot {
            TerminalViewerSlot::Reserved(reserved) if reserved.lease == lease => {
                self.notify_reserved_transcript(reserved);
            }
            TerminalViewerSlot::Active(viewer) if viewer.lease() == lease => {}
            other => {
                self.viewer = other;
            }
        }
    }

    fn write_bytes(&mut self, bytes: Vec<u8>) {
        let mut clear_viewer = false;
        match &mut self.viewer {
            TerminalViewerSlot::Active(viewer) => {
                if viewer.write_bytes(&bytes).is_err() {
                    clear_viewer = true;
                }
            }
            TerminalViewerSlot::Reserved(viewer) => {
                viewer.push_bytes(bytes);
                return;
            }
            TerminalViewerSlot::Empty => {}
        }
        if clear_viewer {
            self.viewer = TerminalViewerSlot::Empty;
        }
        self.notify_transcript(bytes);
    }

    fn notify_reserved_transcript(&self, mut reserved: TerminalReservedViewer) {
        while let Some(bytes) = reserved.pop_bytes() {
            self.notify_transcript(bytes);
        }
    }

    fn notify_transcript(&self, bytes: Vec<u8>) {
        let _ = self.transcript.push_drop_oldest(TranscriptNotice { bytes });
    }
}

/// `TranscriptScriber` is the decoupled transcript appender. It reads
/// notices from a bounded queue fed by `ViewerFanout` and sends each notice
/// to the `TerminalCell` actor as a `TerminalOutput` message. The queue's
/// drop-oldest discipline keeps the fanout non-blocking when the scriber
/// (or the actor mailbox) falls behind.
struct TranscriptScriber {
    actor: ActorRef<TerminalCell>,
    transcript: TranscriptNoticeQueue,
}

impl TranscriptScriber {
    fn new(actor: ActorRef<TerminalCell>, transcript: TranscriptNoticeQueue) -> Self {
        Self { actor, transcript }
    }

    fn run(&self) -> TerminalWorkerStop {
        while let Some(notice) = self.transcript.pop_blocking() {
            let _ = self
                .actor
                .tell(TerminalOutput::new(notice.bytes))
                .try_send();
        }
        TerminalWorkerStop::TranscriptNoticeChannelClosed
    }
}

#[derive(Debug, Clone)]
pub struct TerminalCellSession {
    actor: ActorRef<TerminalCell>,
    input_port: TerminalInputPort,
    output_port: TerminalOutputPort,
}

impl TerminalCellSession {
    fn new(
        actor: ActorRef<TerminalCell>,
        input_port: TerminalInputPort,
        output_port: TerminalOutputPort,
    ) -> Self {
        Self {
            actor,
            input_port,
            output_port,
        }
    }

    pub fn actor(&self) -> ActorRef<TerminalCell> {
        self.actor.clone()
    }

    pub fn input_port(&self) -> TerminalInputPort {
        self.input_port.clone()
    }

    pub fn output_port(&self) -> TerminalOutputPort {
        self.output_port.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputAcceptance {
    source: InputSource,
}

impl InputAcceptance {
    fn new(source: InputSource) -> Self {
        Self { source }
    }

    pub const fn source(self) -> InputSource {
        self.source
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSubscriptionRequest {
    after: TerminalSequence,
}

impl TranscriptSubscriptionRequest {
    pub const fn from_beginning() -> Self {
        Self {
            after: TerminalSequence::new(0),
        }
    }

    pub const fn after(sequence: TerminalSequence) -> Self {
        Self { after: sequence }
    }
}

#[derive(Debug)]
pub struct TranscriptSubscription {
    replay: Vec<TranscriptDelta>,
    live: broadcast::Receiver<TranscriptDelta>,
}

impl TranscriptSubscription {
    fn new(replay: Vec<TranscriptDelta>, live: broadcast::Receiver<TranscriptDelta>) -> Self {
        Self { replay, live }
    }

    pub fn replay(&self) -> &[TranscriptDelta] {
        self.replay.as_slice()
    }

    pub fn replay_bytes(&self) -> Vec<u8> {
        self.replay
            .iter()
            .flat_map(|delta| delta.bytes.iter().copied())
            .collect()
    }

    pub async fn next_live_delta(&mut self) -> Option<TranscriptDelta> {
        self.live.recv().await.ok()
    }

    pub fn blocking_next_live_delta(&mut self) -> Option<TranscriptDelta> {
        self.live.blocking_recv().ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSnapshotRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalExitRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenProjectionRequest {
    size: TerminalSize,
}

impl ScreenProjectionRequest {
    pub const fn new(size: TerminalSize) -> Self {
        Self { size }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitForTranscriptText {
    needle: Vec<u8>,
}

impl WaitForTranscriptText {
    pub fn new(needle: impl Into<Vec<u8>>) -> Self {
        Self {
            needle: needle.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitForTerminalExit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalWorkerObservationRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalWorkerLifecycleSubscriptionRequest;

#[derive(Debug)]
pub struct TerminalWorkerLifecycleSubscription {
    replay: Vec<TerminalWorkerLifecycle>,
    live: broadcast::Receiver<TerminalWorkerLifecycle>,
}

impl TerminalWorkerLifecycleSubscription {
    fn new(
        replay: Vec<TerminalWorkerLifecycle>,
        live: broadcast::Receiver<TerminalWorkerLifecycle>,
    ) -> Self {
        Self { replay, live }
    }

    pub fn replay(&self) -> &[TerminalWorkerLifecycle] {
        self.replay.as_slice()
    }

    pub fn blocking_next_live_event(&mut self) -> Option<TerminalWorkerLifecycle> {
        self.live.blocking_recv().ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitForTerminalWorkerStop {
    worker: TerminalWorkerKind,
}

impl WaitForTerminalWorkerStop {
    pub const fn new(worker: TerminalWorkerKind) -> Self {
        Self { worker }
    }

    pub const fn worker(&self) -> TerminalWorkerKind {
        self.worker
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Reply)]
pub struct TerminalExit {
    status: String,
}

impl TerminalExit {
    fn new(status: String) -> Self {
        Self { status }
    }

    pub fn status(&self) -> &str {
        self.status.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalOutput {
    bytes: Vec<u8>,
}

impl TerminalOutput {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalExited {
    exit: TerminalExit,
}

impl TerminalExited {
    fn new(exit: TerminalExit) -> Self {
        Self { exit }
    }
}

struct TranscriptWaiter {
    needle: Vec<u8>,
    sender: ReplySender<bool>,
}

impl TranscriptWaiter {
    fn new(needle: Vec<u8>, sender: ReplySender<bool>) -> Self {
        Self { needle, sender }
    }

    fn matches(&self, transcript: &TerminalTranscript) -> bool {
        transcript.contains(&self.needle)
    }
}

struct TerminalExitWaiter {
    sender: ReplySender<TerminalExit>,
}

impl TerminalExitWaiter {
    fn new(sender: ReplySender<TerminalExit>) -> Self {
        Self { sender }
    }
}

struct TerminalWorkerWaiter {
    worker: TerminalWorkerKind,
    sender: ReplySender<TerminalWorkerStop>,
}

impl TerminalWorkerWaiter {
    fn new(worker: TerminalWorkerKind, sender: ReplySender<TerminalWorkerStop>) -> Self {
        Self { worker, sender }
    }

    const fn worker(&self) -> TerminalWorkerKind {
        self.worker
    }
}

pub struct TerminalCell {
    master: Box<dyn MasterPty + Send>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
    transcript: TerminalTranscript,
    workers: TerminalWorkerObservation,
    subscribers: broadcast::Sender<TranscriptDelta>,
    worker_subscribers: broadcast::Sender<TerminalWorkerLifecycle>,
    waiters: Vec<TranscriptWaiter>,
    exit_waiters: Vec<TerminalExitWaiter>,
    worker_waiters: Vec<TerminalWorkerWaiter>,
    exit: Option<TerminalExit>,
}

impl TerminalCell {
    pub fn spawn_cell(launch: TerminalLaunch) -> ActorRef<Self> {
        Self::spawn_session(launch).actor()
    }

    pub fn spawn_session(launch: TerminalLaunch) -> TerminalCellSession {
        let (input_sender, input_receiver) = mpsc::channel();
        let (output_sender, output_receiver) = mpsc::channel();
        let input_port = TerminalInputPort::new(input_sender);
        let output_port = TerminalOutputPort::new(output_sender);
        let actor = Self::spawn(TerminalCellStart::new(
            launch,
            input_receiver,
            output_port.clone(),
            output_receiver,
        ));
        TerminalCellSession::new(actor, input_port, output_port)
    }

    fn notify_waiters(&mut self) {
        let transcript = &self.transcript;
        let mut waiting = Vec::new();
        for waiter in self.waiters.drain(..) {
            if waiter.matches(transcript) {
                waiter.sender.send(true);
            } else {
                waiting.push(waiter);
            }
        }
        self.waiters = waiting;
    }

    fn notify_exit_waiters(&mut self) {
        let Some(exit) = &self.exit else {
            return;
        };

        for waiter in self.exit_waiters.drain(..) {
            waiter.sender.send(exit.clone());
        }
    }

    fn notify_worker_waiters(&mut self, worker: TerminalWorkerKind, reason: &TerminalWorkerStop) {
        let mut waiting = Vec::new();
        for waiter in self.worker_waiters.drain(..) {
            if waiter.worker() == worker {
                waiter.sender.send(reason.clone());
            } else {
                waiting.push(waiter);
            }
        }
        self.worker_waiters = waiting;
    }
}

impl Actor for TerminalCell {
    type Args = TerminalCellStart;
    type Error = TerminalCellError;

    async fn on_start(args: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(args.launch.size.into_pty_size())
            .map_err(TerminalCellError::pty)?;
        let mut child = pair
            .slave
            .spawn_command(args.launch.command.into_builder())
            .map_err(TerminalCellError::pty)?;
        if let Some(path) = args.launch.child_process_identifier_path {
            let identifier = child.process_id().ok_or_else(|| {
                TerminalCellError::pty("spawned terminal child has no process identifier")
            })?;
            std::fs::write(path, identifier.to_string()).map_err(TerminalCellError::pty)?;
        }
        let child_killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(TerminalCellError::pty)?;
        let input_writer = pair.master.take_writer().map_err(TerminalCellError::pty)?;
        let input_receiver = args.input_receiver;
        let output_port = args.output_port;
        let output_receiver = args.output_receiver;
        drop(pair.slave);

        let input_reporter =
            TerminalWorkerReporter::new(actor_ref.clone(), TerminalWorkerKind::InputWriter);
        thread::Builder::new()
            .name("terminal-cell-input".to_string())
            .spawn(move || {
                input_reporter.started();
                let reason = TerminalInputWriter::new(input_writer).run(input_receiver);
                input_reporter.stopped(reason);
            })
            .expect("terminal input thread starts");

        let transcript_queue = TranscriptNoticeQueue::new(TRANSCRIPT_NOTICE_QUEUE_CAPACITY);

        let viewer_fanout_queue = transcript_queue.clone();
        let viewer_fanout_reporter =
            TerminalWorkerReporter::new(actor_ref.clone(), TerminalWorkerKind::ViewerFanout);
        let viewer_fanout_close = transcript_queue.clone();
        thread::Builder::new()
            .name("terminal-cell-viewer-fanout".to_string())
            .spawn(move || {
                viewer_fanout_reporter.started();
                let reason = ViewerFanout::new(viewer_fanout_queue).run(output_receiver);
                viewer_fanout_close.close();
                viewer_fanout_reporter.stopped(reason);
            })
            .expect("terminal viewer fanout thread starts");

        let scriber_actor = actor_ref.clone();
        let scriber_queue = transcript_queue;
        let scriber_reporter =
            TerminalWorkerReporter::new(actor_ref.clone(), TerminalWorkerKind::TranscriptScriber);
        thread::Builder::new()
            .name("terminal-cell-transcript-scriber".to_string())
            .spawn(move || {
                scriber_reporter.started();
                let reason = TranscriptScriber::new(scriber_actor, scriber_queue).run();
                scriber_reporter.stopped(reason);
            })
            .expect("terminal transcript scriber thread starts");

        let output_reader_port = output_port;
        let output_reader_reporter =
            TerminalWorkerReporter::new(actor_ref.clone(), TerminalWorkerKind::OutputReader);
        thread::Builder::new()
            .name("terminal-cell-output-reader".to_string())
            .spawn(move || {
                let mut buffer = [0_u8; 8192];
                output_reader_reporter.started();
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => {
                            output_reader_reporter
                                .stopped(TerminalWorkerStop::OutputReaderFinished);
                            break;
                        }
                        Ok(count) => {
                            if output_reader_port
                                .write_bytes(buffer[..count].to_vec())
                                .is_err()
                            {
                                output_reader_reporter
                                    .stopped(TerminalWorkerStop::OutputPortClosed);
                                break;
                            }
                        }
                        Err(error) => {
                            output_reader_reporter
                                .stopped(TerminalWorkerStop::OutputReadFailed(error.to_string()));
                            break;
                        }
                    }
                }
            })
            .expect("terminal output reader thread starts");

        let exit_ref = actor_ref;
        let exit_reporter =
            TerminalWorkerReporter::new(exit_ref.clone(), TerminalWorkerKind::ChildExitWatcher);
        thread::Builder::new()
            .name("terminal-cell-exit".to_string())
            .spawn(move || {
                exit_reporter.started();
                let (exit, stop) = match child.wait() {
                    Ok(status) => {
                        let status = format!("{status:?}");
                        (
                            TerminalExit::new(status.clone()),
                            TerminalWorkerStop::ChildExited(status),
                        )
                    }
                    Err(error) => {
                        let error = error.to_string();
                        (
                            TerminalExit::new(format!("wait failed: {error}")),
                            TerminalWorkerStop::ChildWaitFailed(error),
                        )
                    }
                };
                let _ = exit_ref.tell(TerminalExited::new(exit)).try_send();
                exit_reporter.stopped(stop);
            })
            .expect("terminal exit thread starts");

        let (subscribers, _) = broadcast::channel(1024);
        let (worker_subscribers, _) = broadcast::channel(1024);
        Ok(Self {
            master: pair.master,
            child_killer,
            transcript: TerminalTranscript::new(),
            workers: TerminalWorkerObservation::new(),
            subscribers,
            worker_subscribers,
            waiters: Vec::new(),
            exit_waiters: Vec::new(),
            worker_waiters: Vec::new(),
            exit: None,
        })
    }

    async fn on_stop(
        &mut self,
        _ref: kameo::actor::WeakActorRef<Self>,
        _reason: kameo::error::ActorStopReason,
    ) -> Result<(), Self::Error> {
        let _ = self.child_killer.kill();
        Ok(())
    }
}

impl Message<TerminalOutput> for TerminalCell {
    type Reply = ();

    async fn handle(&mut self, message: TerminalOutput, _context: &mut Context<Self, Self::Reply>) {
        let delta = self.transcript.append(message.bytes);
        let _ = self.subscribers.send(delta);
        self.notify_waiters();
    }
}

impl Message<TerminalExited> for TerminalCell {
    type Reply = ();

    async fn handle(&mut self, message: TerminalExited, _context: &mut Context<Self, Self::Reply>) {
        self.exit = Some(message.exit);
        self.notify_exit_waiters();
    }
}

impl Message<TerminalWorkerLifecycle> for TerminalCell {
    type Reply = ();

    async fn handle(
        &mut self,
        message: TerminalWorkerLifecycle,
        _context: &mut Context<Self, Self::Reply>,
    ) {
        if let TerminalWorkerLifecycle::Stopped { worker, reason } = &message {
            self.notify_worker_waiters(*worker, reason);
        }
        self.workers.record(message.clone());
        let _ = self.worker_subscribers.send(message);
    }
}

impl Message<TerminalSize> for TerminalCell {
    type Reply = Result<TerminalSize, TerminalCellError>;

    async fn handle(
        &mut self,
        message: TerminalSize,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.master
            .resize(message.into_pty_size())
            .map_err(TerminalCellError::pty)?;
        Ok(message)
    }
}

impl Message<TranscriptSnapshotRequest> for TerminalCell {
    type Reply = Result<TranscriptSnapshot, Infallible>;

    async fn handle(
        &mut self,
        _message: TranscriptSnapshotRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.transcript.snapshot())
    }
}

impl Message<TerminalExitRequest> for TerminalCell {
    type Reply = Result<Option<TerminalExit>, Infallible>;

    async fn handle(
        &mut self,
        _message: TerminalExitRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.exit.clone())
    }
}

impl Message<TerminalWorkerObservationRequest> for TerminalCell {
    type Reply = Result<TerminalWorkerObservation, Infallible>;

    async fn handle(
        &mut self,
        _message: TerminalWorkerObservationRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.workers.clone())
    }
}

impl Message<TerminalWorkerLifecycleSubscriptionRequest> for TerminalCell {
    type Reply = Result<TerminalWorkerLifecycleSubscription, Infallible>;

    async fn handle(
        &mut self,
        _message: TerminalWorkerLifecycleSubscriptionRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(TerminalWorkerLifecycleSubscription::new(
            self.workers.events().to_vec(),
            self.worker_subscribers.subscribe(),
        ))
    }
}

impl Message<TranscriptSubscriptionRequest> for TerminalCell {
    type Reply = Result<TranscriptSubscription, Infallible>;

    async fn handle(
        &mut self,
        message: TranscriptSubscriptionRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(TranscriptSubscription::new(
            self.transcript.replay_after(message.after),
            self.subscribers.subscribe(),
        ))
    }
}

impl Message<ScreenProjectionRequest> for TerminalCell {
    type Reply = Result<ScreenProjection, Infallible>;

    async fn handle(
        &mut self,
        message: ScreenProjectionRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(ScreenProjection::from_transcript(
            &self.transcript.snapshot(),
            message.size,
        ))
    }
}

impl Message<WaitForTranscriptText> for TerminalCell {
    type Reply = DelegatedReply<bool>;

    async fn handle(
        &mut self,
        message: WaitForTranscriptText,
        context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (delegated, sender) = context.reply_sender();
        if let Some(sender) = sender {
            if self.transcript.contains(&message.needle) {
                sender.send(true);
            } else {
                self.waiters
                    .push(TranscriptWaiter::new(message.needle, sender));
            }
        }
        delegated
    }
}

impl Message<WaitForTerminalExit> for TerminalCell {
    type Reply = DelegatedReply<TerminalExit>;

    async fn handle(
        &mut self,
        _message: WaitForTerminalExit,
        context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (delegated, sender) = context.reply_sender();
        if let Some(sender) = sender {
            if let Some(exit) = &self.exit {
                sender.send(exit.clone());
            } else {
                self.exit_waiters.push(TerminalExitWaiter::new(sender));
            }
        }
        delegated
    }
}

impl Message<WaitForTerminalWorkerStop> for TerminalCell {
    type Reply = DelegatedReply<TerminalWorkerStop>;

    async fn handle(
        &mut self,
        message: WaitForTerminalWorkerStop,
        context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (delegated, sender) = context.reply_sender();
        if let Some(sender) = sender {
            if let Some(reason) = self.workers.stopped_reason(message.worker()) {
                sender.send(reason.clone());
            } else {
                self.worker_waiters
                    .push(TerminalWorkerWaiter::new(message.worker(), sender));
            }
        }
        delegated
    }
}
