#![forbid(unsafe_code)]
//! Prototype durable terminal session owner.

mod error;
mod session;
mod snapshot;

pub use error::TerminalCellError;
pub use session::{
    InputAcceptance, InputSource, ScreenProjectionRequest, TerminalCell, TerminalCommand,
    TerminalExit, TerminalInput, TerminalLaunch, TerminalSequence, TerminalSize, TranscriptDelta,
    TranscriptSnapshot, TranscriptSnapshotRequest, TranscriptSubscription,
    TranscriptSubscriptionRequest, WaitForTranscriptText,
};
pub use snapshot::ScreenProjection;
