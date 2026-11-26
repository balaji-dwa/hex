use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use url::Url;

use super::{AuthError, check_cancelled};

const CALLBACK_WAIT: Duration = Duration::from_secs(5 * 60);
const IO_POLL: Duration = Duration::from_millis(50);
const REQUEST_WAIT: Duration = Duration::from_secs(2);
const MAX_REQUEST_HEAD: usize = 16 * 1024;

pub(super) struct Pkce {
    pub state: String,
    pub verifier: String,
}

impl Pkce {
    pub(super) fn new() -> Result<Self, AuthError> {
        Ok(Self {
            state: random_secret()?,
            verifier: random_secret()?,
        })
    }

    pub(super) fn challenge(&self) -> String {
        base64url(&Sha256::digest(self.verifier.as_bytes()))
    }
}

fn random_secret() -> Result<String, AuthError> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| AuthError::Service)?;
    Ok(base64url(&bytes))
}

pub(super) fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        let symbols = [
            first >> 2,
            (first & 3) << 4 | second >> 4,
            (second & 15) << 2 | third >> 6,
            third & 63,
        ];
        for index in symbols.into_iter().take(chunk.len() + 1) {
            encoded.push(char::from(ALPHABET[usize::from(index)]));
        }
    }
    encoded
}

pub(super) struct Loopback {
    listener: TcpListener,
    pub port: u16,
}

pub(super) fn bind(preferred_port: Option<u16>) -> Result<(Loopback, bool), AuthError> {
    let preferred_port = preferred_port.filter(|port| *port != 0);
    let listener = preferred_port
        .and_then(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok())
        .map_or_else(|| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)), Ok)
        .map_err(|_| AuthError::Callback)?;
    let port = listener
        .local_addr()
        .map_err(|_| AuthError::Callback)?
        .port();
    listener
        .set_nonblocking(true)
        .map_err(|_| AuthError::Callback)?;
    Ok((Loopback { listener, port }, preferred_port == Some(port)))
}

impl Loopback {
    pub(super) fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.port)
    }

    pub(super) fn wait(
        &self,
        state: &str,
        cancelled: &AtomicBool,
    ) -> Result<(TcpStream, String), AuthError> {
        let deadline = Instant::now() + CALLBACK_WAIT;
        while Instant::now() < deadline {
            check_cancelled(cancelled)?;
            let mut stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(IO_POLL);
                    continue;
                }
                Err(_) => return Err(AuthError::Callback),
            };
            let request_deadline = deadline.min(Instant::now() + REQUEST_WAIT);
            let Some(url) = callback_url(&mut stream, request_deadline, cancelled) else {
                continue;
            };
            return match callback_code(&url, state) {
                Ok(code) => Ok((stream, code)),
                Err(error) => {
                    reply(stream, false);
                    Err(error)
                }
            };
        }
        Err(AuthError::Timeout)
    }
}

/// Reads the whole request head, so the reply's close never resets a
/// connection that still holds unread headers (the browser would show an error
/// instead of the page).
fn callback_url(stream: &mut TcpStream, deadline: Instant, cancelled: &AtomicBool) -> Option<Url> {
    stream.set_read_timeout(Some(IO_POLL)).ok()?;
    let mut reader = BufReader::with_capacity(1024, stream);
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if Instant::now() >= deadline || check_cancelled(cancelled).is_err() {
            return None;
        }
        let data = match reader.fill_buf() {
            Ok([]) => return None,
            Ok(data) => data,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(_) => return None,
        };
        let end = data.iter().position(|byte| *byte == b'\n');
        let size = end.map_or(data.len(), |index| index + 1);
        if head.len() + size > MAX_REQUEST_HEAD {
            return None;
        }
        head.extend_from_slice(&data[..size]);
        reader.consume(size);
    }
    let line = head.split(|byte| *byte == b'\n').next()?;
    let mut words = std::str::from_utf8(line).ok()?.split_ascii_whitespace();
    if words.next()? != "GET" {
        return None;
    }
    let target = words.next()?;
    if target.split('?').next()? != "/callback" {
        return None;
    }
    Url::parse(&format!("http://127.0.0.1{target}")).ok()
}

fn callback_code(url: &Url, expected_state: &str) -> Result<String, AuthError> {
    let mut state = None;
    let mut code = None;
    let mut error = None;
    for (key, value) in url.query_pairs() {
        let slot = match key.as_ref() {
            "state" => &mut state,
            "code" => &mut code,
            "error" => &mut error,
            _ => continue,
        };
        if slot.replace(value.into_owned()).is_some() {
            return Err(AuthError::Callback);
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Err(AuthError::Callback);
    }
    if error.is_some() {
        return Err(AuthError::Denied);
    }
    code.filter(|code| !code.is_empty())
        .ok_or(AuthError::Callback)
}

pub(super) fn reply(mut stream: TcpStream, success: bool) {
    let (status, message) = if success {
        ("200 OK", "Signed in to Hex. You can close this tab.")
    } else {
        (
            "400 Bad Request",
            "Hex couldn't complete sign-in. Return to Hex to try again.",
        )
    };
    let page = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Hex sign-in</title><p>{message}</p></html>"
    );
    let _ = stream.set_write_timeout(Some(REQUEST_WAIT));
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\n\r\n{page}",
        page.len(),
    );
}
