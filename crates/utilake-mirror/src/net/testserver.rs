//! A local HTTP/1.1 server for tests: each path answers with the replies
//! queued for it, in order, the last one repeating; any other path is a
//! 404. It lets the real transport, and the binary, run against something
//! that behaves like nemweb, including the ways nemweb fails.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a path answers.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A 200 with this body, an ETag and a Last-Modified.
    Ok(Vec<u8>),
    /// This status with a small HTML body.
    Status(u16),
    /// Reads the request and never answers.
    Stall,
    /// Claims `claimed` bytes, sends `sent`, then closes the connection.
    Truncate {
        /// The Content-Length sent.
        claimed: usize,
        /// The bytes sent.
        sent: Vec<u8>,
    },
    /// Claims `claimed` bytes, sends `sent`, then never sends the rest.
    StallBody {
        /// The Content-Length sent.
        claimed: usize,
        /// The bytes sent.
        sent: Vec<u8>,
    },
}

type Routes = Arc<Mutex<HashMap<String, Vec<Reply>>>>;

/// A running server; it stops with the test process.
pub struct Server {
    /// `http://127.0.0.1:{port}`, without a trailing slash.
    pub base: String,
    routes: Routes,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Server {
    /// Starts a server on a free port.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
        let base = format!("http://{}", listener.local_addr().expect("bound"));
        let routes: Routes = Arc::default();
        let hits: Arc<Mutex<Vec<String>>> = Arc::default();
        let (r, h) = (Arc::clone(&routes), Arc::clone(&hits));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (r, h) = (Arc::clone(&r), Arc::clone(&h));
                std::thread::spawn(move || serve(stream, &r, &h));
            }
        });
        Self { base, routes, hits }
    }

    /// Queues a reply for `path`.
    pub fn route(&self, path: &str, reply: Reply) -> &Self {
        self.routes
            .lock()
            .expect("not poisoned")
            .entry(path.to_owned())
            .or_default()
            .push(reply);
        self
    }

    /// The paths asked for so far, in order.
    pub fn hits(&self) -> Vec<String> {
        self.hits.lock().expect("not poisoned").clone()
    }

    /// The URL of `path`.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

fn serve(stream: TcpStream, routes: &Routes, hits: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("a socket clones"));
    let mut request_line = String::new();
    let _ = reader.read_line(&mut request_line);
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_owned();
    let mut header = String::new();
    while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
        header.clear();
    }
    hits.lock().expect("not poisoned").push(path.clone());
    let reply = {
        let mut routes = routes.lock().expect("not poisoned");
        match routes.get_mut(&path) {
            Some(q) if q.len() > 1 => q.remove(0),
            Some(q) => q[0].clone(),
            None => Reply::Status(404),
        }
    };
    let mut out = stream;
    let head = |status: u16, len: usize| {
        format!(
            "HTTP/1.1 {status} X\r\nContent-Length: {len}\r\nETag: \"e\"\r\nLast-Modified: Mon, 12 Oct 2026 00:32:30 GMT\r\nConnection: close\r\n\r\n"
        )
    };
    match reply {
        Reply::Ok(body) => {
            let _ = out.write_all(head(200, body.len()).as_bytes());
            let _ = out.write_all(&body);
        }
        Reply::Status(status) => {
            let body = format!("<html>{status}</html>");
            let _ = out.write_all(head(status, body.len()).as_bytes());
            let _ = out.write_all(body.as_bytes());
        }
        Reply::Stall => std::thread::sleep(Duration::from_secs(4)),
        Reply::Truncate { claimed, sent } => {
            let _ = out.write_all(head(200, claimed).as_bytes());
            let _ = out.write_all(&sent);
        }
        Reply::StallBody { claimed, sent } => {
            let _ = out.write_all(head(200, claimed).as_bytes());
            let _ = out.write_all(&sent);
            let _ = out.flush();
            std::thread::sleep(Duration::from_secs(4));
        }
    }
}
