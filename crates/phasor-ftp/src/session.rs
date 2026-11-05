//! One FTP control connection as the transport uses it: list a folder, ask
//! a file's time and size, fetch it whole. The trait is what a test fakes;
//! [`crate::live::Live`] is the connection to a real server.

/// Why a command failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The server could not be reached or the connection dropped: the
    /// session is finished, a new one is opened for the next request.
    Connection(String),
    /// The server answered that the path is not there (550).
    NotFound,
    /// The server answered that it will not perform the command (any
    /// other failure reply): the session stands.
    Refused(String),
}

/// The commands a session answers; paths are absolute on the server.
pub trait Session: Send {
    /// The `MLSD` lines of `path`, a folder.
    fn list(&mut self, path: &str) -> Result<Vec<String>, SessionError>;
    /// The names in `path`, a folder, when `MLSD` is not served (`NLST`).
    fn names(&mut self, path: &str) -> Result<Vec<String>, SessionError>;
    /// The `MDTM` reply for `path`, a file (`YYYYMMDDHHMMSS`).
    fn modified(&mut self, path: &str) -> Result<String, SessionError>;
    /// The `SIZE` of `path`, a file, in bytes.
    fn size(&mut self, path: &str) -> Result<u64, SessionError>;
    /// The bytes of `path`, a file, whole.
    fn fetch(&mut self, path: &str) -> Result<Vec<u8>, SessionError>;
}

/// How a session is opened: a function, so the transport can be given a
/// fake in tests and a live connection in the binary.
pub type Open = Box<dyn Fn(&str) -> Result<Box<dyn Session>, SessionError> + Send + Sync>;
