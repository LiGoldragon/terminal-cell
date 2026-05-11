#![forbid(unsafe_code)]
//! Prototype durable terminal session owner.

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
    TranscriptDelta, TranscriptSnapshot, TranscriptSnapshotRequest, TranscriptSubscription,
    TranscriptSubscriptionRequest, WaitForTerminalExit, WaitForTranscriptText,
};
pub use snapshot::ScreenProjection;
pub use socket::{
    SocketReplyReader, SocketReplyWriter, SocketRequest, SocketRequestReader, SocketRequestWriter,
};
