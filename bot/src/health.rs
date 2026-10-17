//! Health HTTP thread: bind on the configured port (bad values fall back
//! to 8000), write the same fixed 200 OK reply to every accepted
//! connection without reading it, then close. Poll-free blocking accept;
//! the thread lives for the process lifetime (there is no stop path).

use std::io::Write;
use std::net::{TcpListener, TcpStream};

use crate::logger::{LogLevel, Logger};

const REPLY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nOK";

/// Unparseable values fall back to 8000 (classic stoi-catch behavior).
pub fn parse_port(port: &str) -> u16 {
    port.trim().parse().unwrap_or(8000)
}

/// Serve one already-accepted connection: write the fixed response
/// without consuming the request, then close.
pub fn serve_once(stream: TcpStream) {
    let mut s = stream;
    let _ = s.write_all(REPLY);
    let _ = s.flush();
    // s dropped here -> close
}

/// Bind + detached accept loop; bind failure only logs and the bot
/// keeps running. Never joined.
pub fn spawn(port: &str) -> std::io::Result<()> {
    let p = parse_port(port);
    let listener = TcpListener::bind(("0.0.0.0", p))?;
    Logger::instance().log(
        LogLevel::Info,
        "http",
        &format!("HTTP server listening on 0.0.0.0:{p}"),
    );
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => serve_once(s),
                Err(_) => continue,
            }
        }
    });
    Ok(())
}
