use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::{
    SocketReplyReader, SocketRequestWriter, TerminalInputGateLease, TerminalInputGateRelease,
    TerminalSize,
};

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

    pub fn open_attach_stream(&self) -> io::Result<UnixStream> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_attach_request()?;
        SocketReplyReader::new(&mut stream).read_attach_acceptance()?;
        Ok(stream)
    }

    pub fn close_human_input(&self) -> io::Result<TerminalInputGateLease> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_close_human_input_request()?;
        SocketReplyReader::new(&mut stream).read_gate_lease()
    }

    pub fn open_human_input(
        &self,
        lease: TerminalInputGateLease,
    ) -> io::Result<TerminalInputGateRelease> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_open_human_input_request(lease)?;
        SocketReplyReader::new(&mut stream).read_gate_release()
    }

    pub fn resize(&self, size: TerminalSize) -> io::Result<()> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_resize_request(size)?;
        SocketReplyReader::new(&mut stream).read_acceptance()
    }

    pub fn wait_for_transcript(&self, bytes: &[u8]) -> io::Result<()> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_wait_request(bytes)?;
        SocketReplyReader::new(&mut stream).read_wait_satisfied()
    }

    pub fn wait_for_terminal_exit(&self) -> io::Result<String> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_wait_exit_request()?;
        SocketReplyReader::new(&mut stream).read_exit_status()
    }

    pub fn worker_observation_text(&self) -> io::Result<String> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_worker_observation_request()?;
        String::from_utf8(SocketReplyReader::new(&mut stream).read_snapshot()?).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("worker observation was not utf-8: {error}"),
            )
        })
    }

    pub fn subscribe_from_beginning(&self) -> io::Result<UnixStream> {
        let mut stream = UnixStream::connect(&self.socket)?;
        SocketRequestWriter::new(&mut stream).write_subscription_request()?;
        Ok(stream)
    }
}
