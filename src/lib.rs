#![forbid(unsafe_code)]
//! Low-level durable terminal cell.

mod client;
mod error;
mod session;
mod snapshot;
mod socket;

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
