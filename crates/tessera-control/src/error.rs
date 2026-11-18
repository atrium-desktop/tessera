//! Typed command errors and exit-code mapping.
//!
//! Replaces the previous `Result<_, String>` surface with a structured enum.
//! The dispatcher returns [`CliError`]; the binary maps each variant to a
//! stable process exit code (0 success, 1 runtime, 2 usage).

/// All failures surfaced through native `tessera` domain commands.
///
/// `clap::Error` is wrapped verbatim because clap already renders an
/// appropriate `error: ... \n\nUsage: ...` message; we just need to forward
/// its choice of stdout vs stderr and exit code.
#[derive(Debug)]
pub enum CliError {
    /// Argument parsing or unknown subcommand. Exit code 2.
    Usage(crate::cli::ParseError),
    /// Could not open the IPC socket or negotiate capabilities.
    Connect(String),
    /// IPC read/write failure or server-side protocol error.
    Io(String),
    /// Filesystem error writing a capture or creating a directory.
    Fs(String),
    /// An Interaction Domain id of `0` was supplied; the protocol reserves it.
    ZeroInteractionDomainId,
    /// An Interaction Domain argument referenced an unknown id in a fallback slot.
    InvalidFallbackInteractionDomain(u64),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(err) => write!(f, "{err}"),
            Self::Connect(msg) => write!(f, "connect: {msg}"),
            Self::Io(msg) | Self::Fs(msg) => f.write_str(msg),
            Self::ZeroInteractionDomainId => f.write_str("Interaction Domain id zero is invalid"),
            Self::InvalidFallbackInteractionDomain(id) => {
                write!(f, "fallback Interaction Domain {id} is invalid")
            }
        }
    }
}

impl std::error::Error for CliError {}

impl From<crate::cli::ParseError> for CliError {
    fn from(err: crate::cli::ParseError) -> Self {
        Self::Usage(err)
    }
}

impl CliError {
    /// Map to the same exit-code contract documented in `docs/reference/cli.md`:
    /// 0 success, 1 any runtime failure, 2 usage / argument error.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            _ => 1,
        }
    }
}
