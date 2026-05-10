use std::io::{Read, Write};
use std::thread;

use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::error::Infallible;
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::broadcast;

use crate::error::TerminalCellError;
use crate::snapshot::ScreenProjection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCommand {
    program: String,
    arguments: Vec<String>,
}

impl TerminalCommand {
    pub fn new(program: impl Into<String>, arguments: impl Into<Vec<String>>) -> Self {
        Self {
            program: program.into(),
            arguments: arguments.into(),
        }
    }

    fn into_builder(self) -> CommandBuilder {
        let mut builder = CommandBuilder::new(self.program);
        for argument in self.arguments {
            builder.arg(argument);
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
}

impl TerminalLaunch {
    pub fn new(command: TerminalCommand, size: TerminalSize) -> Self {
        Self { command, size }
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

    pub const fn source(&self) -> InputSource {
        self.source
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSnapshotRequest;

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

pub struct TerminalCell {
    master: Box<dyn MasterPty + Send>,
    input_writer: Box<dyn Write + Send>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
    transcript: TerminalTranscript,
    subscribers: broadcast::Sender<TranscriptDelta>,
    waiters: Vec<TranscriptWaiter>,
    exit: Option<TerminalExit>,
}

impl TerminalCell {
    pub fn spawn_cell(launch: TerminalLaunch) -> ActorRef<Self> {
        Self::spawn(launch)
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
}

impl Actor for TerminalCell {
    type Args = TerminalLaunch;
    type Error = TerminalCellError;

    async fn on_start(args: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(args.size.into_pty_size())
            .map_err(TerminalCellError::pty)?;
        let mut child = pair
            .slave
            .spawn_command(args.command.into_builder())
            .map_err(TerminalCellError::pty)?;
        let child_killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(TerminalCellError::pty)?;
        let input_writer = pair.master.take_writer().map_err(TerminalCellError::pty)?;
        drop(pair.slave);

        let output_ref = actor_ref.clone();
        thread::Builder::new()
            .name("terminal-cell-output".to_string())
            .spawn(move || {
                let mut buffer = [0_u8; 8192];
                while let Ok(count) = reader.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    let _ = output_ref
                        .tell(TerminalOutput::new(buffer[..count].to_vec()))
                        .try_send();
                }
            })
            .expect("prototype output thread starts");

        let exit_ref = actor_ref;
        thread::Builder::new()
            .name("terminal-cell-exit".to_string())
            .spawn(move || {
                let status = child
                    .wait()
                    .map(|status| format!("{status:?}"))
                    .unwrap_or_else(|error| format!("wait failed: {error}"));
                let _ = exit_ref
                    .tell(TerminalExited::new(TerminalExit::new(status)))
                    .try_send();
            })
            .expect("prototype exit thread starts");

        let (subscribers, _) = broadcast::channel(1024);
        Ok(Self {
            master: pair.master,
            input_writer,
            child_killer,
            transcript: TerminalTranscript::new(),
            subscribers,
            waiters: Vec::new(),
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
    }
}

impl Message<TerminalInput> for TerminalCell {
    type Reply = Result<InputAcceptance, TerminalCellError>;

    async fn handle(
        &mut self,
        message: TerminalInput,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.input_writer
            .write_all(message.bytes())
            .map_err(TerminalCellError::pty)?;
        self.input_writer.flush().map_err(TerminalCellError::pty)?;
        Ok(InputAcceptance::new(message.source()))
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
