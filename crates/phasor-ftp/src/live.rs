//! A [`Session`] over a real FTP server, through `suppaftp`: the public
//! anonymous convention (user `anonymous`, a client name as the token),
//! binary mode, passive data connections, one timeout for every read.
//! Nothing here runs in the unit tests (`just net-test` reaches a server).

use std::net::{TcpStream, ToSocketAddrs as _};
use std::time::Duration;

use suppaftp::types::FileType;
use suppaftp::{FtpError, FtpStream};

use crate::session::{Session, SessionError};

/// The token sent as the anonymous login's second field: who is asking.
pub const CLIENT: &str = "phasor-mirror@pigface.io";

/// A live connection to one host.
pub struct Live {
    stream: FtpStream,
}

impl Live {
    /// Connects to `host` (`host` or `host:port`, port 21 unless given), logs
    /// in as the public anonymous user and switches to binary.
    pub fn connect(host: &str, timeout: Duration) -> Result<Self, SessionError> {
        let target = if host.contains(':') {
            host.to_owned()
        } else {
            format!("{host}:21")
        };
        let addr = target
            .to_socket_addrs()
            .map_err(|e| SessionError::Connection(e.to_string()))?
            .next()
            .ok_or_else(|| SessionError::Connection(format!("{host} has no address")))?;
        let tcp = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| SessionError::Connection(e.to_string()))?;
        tcp.set_read_timeout(Some(timeout))
            .and_then(|()| tcp.set_write_timeout(Some(timeout)))
            .map_err(|e| SessionError::Connection(e.to_string()))?;
        let mut stream = FtpStream::connect_with_stream(tcp).map_err(|e| connection(&e))?;
        stream
            .login("anonymous", CLIENT)
            .map_err(|e| connection(&e))?;
        stream
            .transfer_type(FileType::Binary)
            .map_err(|e| connection(&e))?;
        Ok(Self { stream })
    }
}

/// An error that ends the session.
fn connection(e: &FtpError) -> SessionError {
    SessionError::Connection(e.to_string())
}

/// An error of one command: a 550 is a missing path, any other reply a
/// refusal, a dropped connection the end of the session.
fn command(e: FtpError) -> SessionError {
    match e {
        FtpError::UnexpectedResponse(r) if r.status as u32 == 550 => SessionError::NotFound,
        FtpError::UnexpectedResponse(r) => {
            SessionError::Refused(String::from_utf8_lossy(&r.body).trim().to_owned())
        }
        FtpError::BadResponse => SessionError::Refused("a reply that is not FTP".to_owned()),
        other => SessionError::Connection(other.to_string()),
    }
}

impl Session for Live {
    fn list(&mut self, path: &str) -> Result<Vec<String>, SessionError> {
        self.stream.mlsd(Some(path)).map_err(command)
    }

    fn names(&mut self, path: &str) -> Result<Vec<String>, SessionError> {
        self.stream.nlst(Some(path)).map_err(command)
    }

    fn modified(&mut self, path: &str) -> Result<String, SessionError> {
        let at = self.stream.mdtm(path).map_err(command)?;
        Ok(at.format("%Y%m%d%H%M%S").to_string())
    }

    fn size(&mut self, path: &str) -> Result<u64, SessionError> {
        let n = self.stream.size(path).map_err(command)?;
        Ok(n as u64)
    }

    fn fetch(&mut self, path: &str) -> Result<Vec<u8>, SessionError> {
        let cursor = self.stream.retr_as_buffer(path).map_err(command)?;
        Ok(cursor.into_inner())
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.stream.quit();
    }
}
