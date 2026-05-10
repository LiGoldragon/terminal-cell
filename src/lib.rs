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
    InputAcceptance, InputSource, ScreenProjectionRequest, TerminalCell, TerminalCommand,
    TerminalExit, TerminalInput, TerminalLaunch, TerminalSequence, TerminalSize, TranscriptDelta,
    TranscriptSnapshot, TranscriptSnapshotRequest, TranscriptSubscription,
    TranscriptSubscriptionRequest, WaitForTranscriptText,
};
pub use snapshot::ScreenProjection;
pub use socket::{
    SocketReplyReader, SocketReplyWriter, SocketRequest, SocketRequestReader, SocketRequestWriter,
};
