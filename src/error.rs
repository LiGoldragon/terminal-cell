use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum TerminalCellError {
    #[error("pty operation failed: {0}")]
    Pty(String),
    #[error("terminal cell has no active input writer")]
    MissingInputWriter,
    #[error("terminal cell has no active pty master")]
    MissingPtyMaster,
    #[error("terminal input port is closed")]
    InputClosed,
    #[error("terminal output port is closed")]
    OutputClosed,
    #[error("terminal cell already has an attached viewer")]
    ViewerAlreadyAttached,
    #[error("terminal viewer lease is stale")]
    StaleViewerLease,
    #[error("terminal input gate lease is stale")]
    StaleInputGateLease,
}

impl TerminalCellError {
    pub fn pty(error: impl std::fmt::Display) -> Self {
        Self::Pty(error.to_string())
    }
}
