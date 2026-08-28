//! Test helpers for Herdr plugin clients.
//!
//! This module is available only on Unix when the `test-support` feature is
//! enabled.

use std::collections::VecDeque;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Map, Value};

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const SERVER_POLL: Duration = Duration::from_millis(2);
const STREAM_POLL: Duration = Duration::from_millis(100);
static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A failure while starting a fixture server or loading captured JSON.
#[derive(Debug)]
pub enum Error {
    /// A filesystem, socket, or thread operation failed.
    Io(io::Error),
    /// Captured JSON could not be parsed.
    Json(serde_json::Error),
    /// A captured response envelope was not a JSON object.
    InvalidEnvelope,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "test-support I/O error: {error}"),
            Self::Json(error) => write!(f, "invalid captured JSON: {error}"),
            Self::InvalidEnvelope => write!(f, "captured response envelope must be a JSON object"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidEnvelope => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// A captured Herdr response envelope ready for request-ID substitution.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedEnvelope {
    envelope: Value,
}

impl CapturedEnvelope {
    /// Creates a captured response from a whole response envelope.
    ///
    /// The envelope's existing `id` is replaced when the fixture is served.
    /// All other fields, including unknown fields, are preserved.
    pub fn from_envelope(envelope: Value) -> Result<Self, Error> {
        if !envelope.is_object() {
            return Err(Error::InvalidEnvelope);
        }
        Ok(Self { envelope })
    }

    /// Creates a captured response from a bare Herdr result value.
    pub fn from_result(result: Value) -> Self {
        let mut envelope = Map::new();
        envelope.insert("result".into(), result);
        Self {
            envelope: Value::Object(envelope),
        }
    }

    /// Parses a whole response envelope from a string.
    pub fn from_envelope_str(source: &str) -> Result<Self, Error> {
        Self::from_envelope(serde_json::from_str(source)?)
    }

    /// Parses a bare result value from a string.
    pub fn from_result_str(source: &str) -> Result<Self, Error> {
        Ok(Self::from_result(serde_json::from_str(source)?))
    }

    /// Loads and parses a whole response envelope from a file.
    pub fn from_envelope_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let source = fs::read_to_string(path)?;
        Self::from_envelope_str(&source)
    }

    /// Loads and parses a bare result value from a file.
    pub fn from_result_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let source = fs::read_to_string(path)?;
        Self::from_result_str(&source)
    }

    /// Returns a copy with its response ID replaced by `request_id`.
    pub fn with_request_id(&self, request_id: &str) -> Value {
        self.with_request_id_value(Value::String(request_id.to_owned()))
    }

    fn with_request_id_value(&self, request_id: Value) -> Value {
        let mut envelope = self.envelope.clone();
        envelope["id"] = request_id;
        envelope
    }
}

/// One scripted action for a [`FixtureServer`] connection.
#[derive(Clone, Debug, PartialEq)]
pub enum FixtureReply {
    /// Return a JSON result envelope and echo the incoming request ID.
    Result(Value),
    /// Return a JSON error envelope and echo the incoming request ID.
    Error {
        /// Herdr's stable protocol error code.
        code: String,
        /// The human-readable protocol error message.
        message: String,
    },
    /// Return a captured JSON envelope and echo the incoming request ID.
    Captured(CapturedEnvelope),
    /// Write bytes exactly as supplied, without adding a newline.
    Raw(Vec<u8>),
    /// Write a response line larger than Crook's four-MiB limit.
    Oversize,
    /// Write bytes forever without terminating the response line.
    Endless,
    /// Close the connection without writing a response.
    Eof,
}

impl FixtureReply {
    /// Creates a JSON result reply.
    pub fn result(result: Value) -> Self {
        Self::Result(result)
    }

    /// Creates a JSON protocol-error reply.
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Creates a verbatim byte reply.
    pub fn raw(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Raw(bytes.into())
    }
}

#[derive(Default)]
struct CapturedRequests {
    parsed: Vec<Value>,
    raw: Vec<Vec<u8>>,
}

/// A queued fake Herdr server backed by a real Unix socket.
///
/// The server accepts one request per connection. Dropping it stops the
/// nonblocking accept loop, joins its worker, and removes its socket directory.
pub struct FixtureServer {
    socket_path: PathBuf,
    socket_dir: PathBuf,
    captured: Arc<Mutex<CapturedRequests>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl FixtureServer {
    /// Starts a server with replies consumed in the supplied order.
    ///
    /// Once the queue is exhausted, later request connections receive EOF.
    pub fn new(replies: impl IntoIterator<Item = FixtureReply>) -> Result<Self, Error> {
        let (socket_dir, socket_path, listener) = bind_listener()?;
        let captured = Arc::new(Mutex::new(CapturedRequests::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let queue = replies.into_iter().collect();

        let thread_captured = Arc::clone(&captured);
        let thread_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("crook-fixture-server".into())
            .spawn(move || serve(listener, queue, thread_captured, thread_stop));

        match worker {
            Ok(worker) => Ok(Self {
                socket_path,
                socket_dir,
                captured,
                stop,
                worker: Some(worker),
            }),
            Err(error) => {
                let _ = fs::remove_file(&socket_path);
                let _ = fs::remove_dir(&socket_dir);
                Err(Error::Io(error))
            }
        }
    }

    /// Returns the Unix-socket path to pass to `crook::client::Client`.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Returns parsed snapshots of all complete requests received so far.
    pub fn requests(&self) -> Vec<Value> {
        lock_unpoisoned(&self.captured).parsed.clone()
    }

    /// Returns raw request lines received so far, including newline framing.
    pub fn raw_lines(&self) -> Vec<Vec<u8>> {
        lock_unpoisoned(&self.captured).raw.clone()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_dir(&self.socket_dir);
    }
}

/// A process-global lock plus scoped environment-variable restoration.
///
/// Each variable is restored to the value it had before its first mutation by
/// this guard, including non-Unicode values and the unset state. Keep the guard
/// alive for the full duration of any test that relies on the mutated values.
pub struct EnvGuard {
    saved: Vec<(OsString, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Acquires the process-global test environment lock.
    pub fn new() -> Self {
        Self {
            saved: Vec::new(),
            _lock: ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        }
    }

    /// Sets a variable until this guard is dropped.
    pub fn set(&mut self, name: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
        let name = name.as_ref();
        self.remember(name);
        env::set_var(name, value);
    }

    /// Unsets a variable until this guard is dropped.
    pub fn unset(&mut self, name: impl AsRef<OsStr>) {
        let name = name.as_ref();
        self.remember(name);
        env::remove_var(name);
    }

    fn remember(&mut self, name: &OsStr) {
        if self
            .saved
            .iter()
            .any(|(saved, _)| saved.as_os_str() == name)
        {
            return;
        }
        self.saved.push((name.to_os_string(), env::var_os(name)));
    }
}

impl Default for EnvGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.saved.iter().rev() {
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
        }
    }
}

fn bind_listener() -> Result<(PathBuf, PathBuf, UnixListener), Error> {
    loop {
        let sequence = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let socket_dir = env::temp_dir().join(format!("crk-{}-{sequence}", std::process::id()));
        match fs::create_dir(&socket_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(Error::Io(error)),
        }

        let socket_path = socket_dir.join("s");
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = fs::remove_dir(&socket_dir);
                return Err(Error::Io(error));
            }
        };
        if let Err(error) = listener.set_nonblocking(true) {
            drop(listener);
            let _ = fs::remove_file(&socket_path);
            let _ = fs::remove_dir(&socket_dir);
            return Err(Error::Io(error));
        }
        return Ok((socket_dir, socket_path, listener));
    }
}

fn serve(
    listener: UnixListener,
    mut replies: VecDeque<FixtureReply>,
    captured: Arc<Mutex<CapturedRequests>>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(STREAM_POLL));
                let _ = stream.set_write_timeout(Some(STREAM_POLL));
                let Ok(Some(raw)) = read_request(&mut stream, &stop) else {
                    continue;
                };
                let Ok(request) = serde_json::from_slice::<Value>(&raw) else {
                    continue;
                };
                {
                    let mut captured = lock_unpoisoned(&captured);
                    captured.parsed.push(request.clone());
                    captured.raw.push(raw);
                }
                if let Some(reply) = replies.pop_front() {
                    let request_id = request.get("id").cloned().unwrap_or(Value::Null);
                    let _ = write_reply(&mut stream, reply, request_id, &stop);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(SERVER_POLL);
            }
            Err(_) => break,
        }
    }
}

fn read_request(stream: &mut UnixStream, stop: &AtomicBool) -> io::Result<Option<Vec<u8>>> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(None);
        }
        match stream.read(&mut chunk) {
            Ok(0) if request.is_empty() => return Ok(None),
            Ok(0) => return Ok(Some(request)),
            Ok(read) => {
                if let Some(newline) = chunk[..read].iter().position(|byte| *byte == b'\n') {
                    request.extend_from_slice(&chunk[..=newline]);
                    return Ok(Some(request));
                }
                request.extend_from_slice(&chunk[..read]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
    }
}

fn write_reply(
    stream: &mut UnixStream,
    reply: FixtureReply,
    request_id: Value,
    stop: &AtomicBool,
) -> io::Result<()> {
    match reply {
        FixtureReply::Result(result) => {
            let mut envelope = Map::new();
            envelope.insert("id".into(), request_id);
            envelope.insert("result".into(), result);
            write_json_line(stream, &Value::Object(envelope))
        }
        FixtureReply::Error { code, message } => {
            let mut error = Map::new();
            error.insert("code".into(), Value::String(code));
            error.insert("message".into(), Value::String(message));
            let mut envelope = Map::new();
            envelope.insert("id".into(), request_id);
            envelope.insert("error".into(), Value::Object(error));
            write_json_line(stream, &Value::Object(envelope))
        }
        FixtureReply::Captured(captured) => {
            write_json_line(stream, &captured.with_request_id_value(request_id))
        }
        FixtureReply::Raw(bytes) => {
            stream.write_all(&bytes)?;
            stream.flush()
        }
        FixtureReply::Oversize => write_oversize(stream),
        FixtureReply::Endless => write_endless(stream, stop),
        FixtureReply::Eof => Ok(()),
    }
}

fn write_json_line(stream: &mut UnixStream, value: &Value) -> io::Result<()> {
    let mut response = serde_json::to_vec(value).map_err(io::Error::other)?;
    response.push(b'\n');
    stream.write_all(&response)?;
    stream.flush()
}

fn write_oversize(stream: &mut UnixStream) -> io::Result<()> {
    let chunk = [b'x'; 8192];
    let mut remaining = MAX_RESPONSE_BYTES + 1;
    while remaining > 0 {
        let write = remaining.min(chunk.len());
        stream.write_all(&chunk[..write])?;
        remaining -= write;
    }
    stream.write_all(b"\n")?;
    stream.flush()
}

fn write_endless(stream: &mut UnixStream, stop: &AtomicBool) -> io::Result<()> {
    let chunk = [b'y'; 8192];
    while !stop.load(Ordering::Acquire) {
        match stream.write_all(&chunk) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(())
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
