//! The terminal-cell daemon's binary startup configuration.
//!
//! The daemon takes exactly one argument: a pre-generated rkyv file carrying
//! this `Configuration` (the daemon-binary-only hard override — no flags, no
//! inline DOTOS, no `.dotos` paths). The deploy/launch tool encodes the typed
//! configuration into binary before it reaches the daemon; the CLI test
//! fixtures do the same with [`Configuration::to_signal_bytes`].

use std::path::Path;

use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use triad_runtime::{BindingSurface, SocketMode};

const OWNER_ONLY_SOCKET_MODE: u32 = 0o600;

/// The terminal-cell daemon's startup configuration: the two socket paths it
/// binds (control plane = working tier, data plane = meta tier) and the child
/// program it launches in its owned PTY.
#[derive(Archive, RkyvSerialize, RkyvDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    control_socket_path: String,
    data_socket_path: String,
    program: String,
    arguments: Vec<String>,
    working_directory: Option<String>,
    environment: Vec<ConfigurationEnvironmentVariable>,
    child_process_identifier_path: Option<String>,
}

impl Configuration {
    pub fn new(
        control_socket_path: impl Into<String>,
        data_socket_path: impl Into<String>,
        program: impl Into<String>,
        arguments: impl Into<Vec<String>>,
    ) -> Self {
        Self::with_working_directory_and_environment(
            control_socket_path,
            data_socket_path,
            program,
            arguments,
            None,
            Vec::new(),
        )
    }

    pub fn with_working_directory_and_environment(
        control_socket_path: impl Into<String>,
        data_socket_path: impl Into<String>,
        program: impl Into<String>,
        arguments: impl Into<Vec<String>>,
        working_directory: Option<String>,
        environment: Vec<ConfigurationEnvironmentVariable>,
    ) -> Self {
        Self {
            control_socket_path: control_socket_path.into(),
            data_socket_path: data_socket_path.into(),
            program: program.into(),
            arguments: arguments.into(),
            working_directory,
            environment,
            child_process_identifier_path: None,
        }
    }

    pub fn with_child_process_identifier_path(mut self, path: Option<String>) -> Self {
        self.child_process_identifier_path = path;
        self
    }

    pub fn control_socket_path(&self) -> &str {
        &self.control_socket_path
    }

    pub fn data_socket_path(&self) -> &str {
        &self.data_socket_path
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    pub fn working_directory(&self) -> Option<&str> {
        self.working_directory.as_deref()
    }

    pub fn environment(&self) -> &[ConfigurationEnvironmentVariable] {
        self.environment.as_slice()
    }

    pub fn child_process_identifier_path(&self) -> Option<&str> {
        self.child_process_identifier_path.as_deref()
    }

    /// Encode the configuration to the binary rkyv form the daemon accepts as
    /// its single startup argument (daemons never parse DOTOS — hard override).
    pub fn to_signal_bytes(&self) -> Result<Vec<u8>, ConfigurationError> {
        rkyv::to_bytes::<rkyv::rancor::Error>(self)
            .map(|bytes| bytes.to_vec())
            .map_err(|_| ConfigurationError::ArchiveEncode)
    }

    /// Decode the configuration from binary rkyv bytes.
    pub fn from_signal_bytes(bytes: &[u8]) -> Result<Self, ConfigurationError> {
        rkyv::from_bytes::<Self, rkyv::rancor::Error>(bytes)
            .map_err(|_| ConfigurationError::ArchiveDecode)
    }

    /// Read and decode the binary rkyv configuration from the daemon's single
    /// startup-argument file path.
    pub fn from_signal_file(path: &Path) -> Result<Self, ConfigurationError> {
        let bytes = std::fs::read(path).map_err(ConfigurationError::Read)?;
        Self::from_signal_bytes(&bytes)
    }
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationEnvironmentVariable {
    name: String,
    value: String,
}

impl ConfigurationEnvironmentVariable {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn value(&self) -> &str {
        self.value.as_str()
    }
}

impl BindingSurface for Configuration {
    fn socket_path(&self) -> &Path {
        Path::new(&self.control_socket_path)
    }

    fn socket_mode(&self) -> Option<SocketMode> {
        Some(SocketMode::new(OWNER_ONLY_SOCKET_MODE))
    }

    fn meta_socket_path(&self) -> Option<&Path> {
        Some(Path::new(&self.data_socket_path))
    }

    fn meta_socket_mode(&self) -> Option<SocketMode> {
        Some(SocketMode::new(OWNER_ONLY_SOCKET_MODE))
    }

    /// terminal-cell owns no durable store — its state is the live PTY and
    /// transcript held by the `TerminalCell` actor. The trait requires a path,
    /// so the control socket's path stands in; nothing opens it as a database.
    fn database_path(&self) -> &Path {
        Path::new(&self.control_socket_path)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigurationError {
    #[error("read terminal-cell daemon configuration file: {0}")]
    Read(std::io::Error),

    #[error("terminal-cell daemon configuration rkyv encode failed")]
    ArchiveEncode,

    #[error("terminal-cell daemon configuration rkyv decode failed")]
    ArchiveDecode,
}
