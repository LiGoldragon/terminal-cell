use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::{SocketReplyReader, SocketRequestWriter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCellSocketClient {
    socket: PathBuf,
}

impl TerminalCellSocketClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn socket(&self) -> &Path {
        self.socket.as_path()
    }

    pub fn capture(&self) -> io::Result<Vec<u8>> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_capture_request()?;
        SocketReplyReader::new(&mut stream).read_snapshot()
    }

    pub fn send_programmatic_input(&self, bytes: &[u8]) -> io::Result<()> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_programmatic_input(bytes)?;
        SocketReplyReader::new(&mut stream).read_acceptance()
    }

    pub fn send_viewer_input(&self, bytes: &[u8]) -> io::Result<()> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_viewer_input(bytes)?;
        SocketReplyReader::new(&mut stream).read_acceptance()
    }

    pub fn wait_for_transcript(&self, bytes: &[u8]) -> io::Result<()> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_wait_request(bytes)?;
        SocketReplyReader::new(&mut stream).read_wait_satisfied()
    }

    pub fn subscribe_from_beginning(&self) -> io::Result<UnixStream> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_subscription_request()?;
        Ok(stream)
    }
}
