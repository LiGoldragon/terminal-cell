#![forbid(unsafe_code)]
//! Low-level durable terminal cell.

extern crate nota as nota;

mod client;
mod configuration;
mod daemon;
mod error;
mod lifecycle_cli;
mod session;
mod snapshot;
mod socket;

pub mod schema;

pub use configuration::{Configuration, ConfigurationEnvironmentVariable, ConfigurationError};
pub use daemon::{
    TerminalCellDaemonError, TerminalCellEngine, TerminalCellProcessDaemon, TerminalSession,
};
pub use lifecycle_cli::{
    AttachViewer, CellClosed, CellEnvironmentVariable, CellLaunched, CellObservation, CellRequest,
    CellResponse, CloseCell, LaunchCell, LineSent, ObserveCell, ProcessState, SendLine, StallState,
    TerminalCellCli, ViewerAttached, ViewerMode,
};
pub use schema::daemon::DaemonEntry;

pub use client::TerminalCellSocketClient;
pub use error::TerminalCellError;
pub use session::{
    InputAcceptance, InputSource, ScreenProjectionRequest, TerminalCell, TerminalCellSession,
    TerminalCellStart, TerminalCommand, TerminalExit, TerminalExitRequest, TerminalInput,
    TerminalInputGateLease, TerminalInputGateRelease, TerminalInputGateSequence, TerminalInputPort,
    TerminalLaunch, TerminalOutputPort, TerminalSequence, TerminalSize, TerminalViewerLease,
    TerminalWorkerKind, TerminalWorkerLifecycle, TerminalWorkerLifecycleSubscription,
    TerminalWorkerLifecycleSubscriptionRequest, TerminalWorkerObservation,
    TerminalWorkerObservationRequest, TerminalWorkerStop, TranscriptDelta, TranscriptSnapshot,
    TranscriptSnapshotRequest, TranscriptSubscription, TranscriptSubscriptionRequest,
    WaitForTerminalExit, WaitForTerminalWorkerStop, WaitForTranscriptText,
};
pub use snapshot::ScreenProjection;
pub use socket::{
    SignalSocketRequest, SocketReplyReader, SocketReplyWriter, SocketRequest, SocketRequestReader,
    SocketRequestWriter,
};
