//! Bounded, one-request-per-connection client for the Herdr Unix socket.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Map, Value};

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;

const IO_TIMEOUT: Duration = Duration::from_secs(15);
const RETRY_BACKOFF: Duration = Duration::from_millis(150);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Whether a request may be repeated after a transport failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrySafety {
    /// Never repeat the request.
    Never,
    /// Repeat the request once if, and only if, transport failed.
    Idempotent,
}

/// A client or server failure, classified by whether retrying is safe.
#[derive(Debug)]
pub enum Error {
    /// The socket could not be connected to, written to, or read from.
    Transport(io::Error),
    /// Herdr returned a well-formed error response.
    Protocol { code: String, message: String },
    /// A request or response violated the client/server envelope contract.
    Contract(String),
    /// A response line exceeded the four-MiB wire limit.
    ResponseTooLarge { limit: usize },
}

impl Error {
    /// Returns Herdr's stable protocol error code, when this is a protocol error.
    pub fn protocol_code(&self) -> Option<&str> {
        match self {
            Self::Protocol { code, .. } => Some(code),
            _ => None,
        }
    }

    fn transport(error: io::Error) -> Self {
        Self::Transport(error)
    }

    fn contract(message: impl Into<String>) -> Self {
        Self::Contract(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "herdr transport error: {error}"),
            Self::Protocol { code, message } => {
                write!(f, "herdr protocol error {code}: {message}")
            }
            Self::Contract(message) => write!(f, "invalid herdr response contract: {message}"),
            Self::ResponseTooLarge { limit } => {
                write!(f, "herdr response went past the {limit}-byte ceiling")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            _ => None,
        }
    }
}

/// A redial-per-request Herdr client.
#[derive(Debug)]
pub struct Client {
    socket_path: PathBuf,
    request_prefix: String,
    next_id: AtomicU64,
}

impl Client {
    /// Constructs a client without touching the socket.
    pub fn new(socket_path: impl Into<PathBuf>, request_prefix: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            request_prefix: request_prefix.into(),
            next_id: AtomicU64::new(0),
        }
    }

    /// Constructs a client after proving that the socket can be reached.
    ///
    /// The probe connection is immediately discarded. Every request still
    /// opens its own connection because Herdr serves one request per connection.
    pub fn connect(
        socket_path: impl Into<PathBuf>,
        request_prefix: impl Into<String>,
    ) -> Result<Self, Error> {
        let client = Self::new(socket_path, request_prefix);
        preflight_with(
            || {
                dial(&client.socket_path)
                    .map(drop)
                    .map_err(Error::transport)
            },
            std::thread::sleep,
        )?;
        Ok(client)
    }

    /// Returns the socket path selected for this client.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Sends one NDJSON request and returns its owned `result` value.
    pub fn request(
        &self,
        method: &str,
        params: Value,
        retry_safety: RetrySafety,
    ) -> Result<Value, Error> {
        if !params.is_object() {
            return Err(Error::contract("request params must be a JSON object"));
        }

        let counter = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| Error::contract("request ID counter exhausted"))?
            + 1;
        let id = format!("{}:{counter}", self.request_prefix);

        let mut envelope = Map::new();
        envelope.insert("id".into(), Value::String(id.clone()));
        envelope.insert("method".into(), Value::String(method.to_owned()));
        envelope.insert("params".into(), params);
        let mut encoded = serde_json::to_vec(&Value::Object(envelope))
            .map_err(|error| Error::contract(format!("could not encode request: {error}")))?;
        encoded.push(b'\n');

        match self.request_once(&id, &encoded) {
            Err(first @ Error::Transport(_)) if retry_safety == RetrySafety::Idempotent => {
                std::thread::sleep(RETRY_BACKOFF);
                match self.request_once(&id, &encoded) {
                    Err(second @ Error::Transport(_)) => {
                        Err(combine_transport_failures(first, second))
                    }
                    outcome => outcome,
                }
            }
            outcome => outcome,
        }
    }

    fn request_once(&self, id: &str, encoded: &[u8]) -> Result<Value, Error> {
        request_once(&self.socket_path, id, encoded)
    }
}

fn preflight_with(
    mut attempt: impl FnMut() -> Result<(), Error>,
    mut sleep: impl FnMut(Duration),
) -> Result<(), Error> {
    match attempt() {
        Err(first @ Error::Transport(_)) => {
            sleep(RETRY_BACKOFF);
            match attempt() {
                Err(second @ Error::Transport(_)) => Err(combine_transport_failures(first, second)),
                outcome => outcome,
            }
        }
        outcome => outcome,
    }
}

fn combine_transport_failures(first: Error, second: Error) -> Error {
    let (Error::Transport(first), Error::Transport(second)) = (first, second) else {
        unreachable!("only transport errors are combined")
    };
    let kind = second.kind();
    Error::Transport(io::Error::new(
        kind,
        format!("failed twice: first attempt: {first}; retry: {second}"),
    ))
}

#[cfg(unix)]
fn dial(socket_path: &Path) -> io::Result<UnixStream> {
    let stream = UnixStream::connect(socket_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot reach herdr at {}: {error}", socket_path.display()),
        )
    })?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "cannot set read timeout for {}: {error}",
                socket_path.display()
            ),
        )
    })?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "cannot set write timeout for {}: {error}",
                    socket_path.display()
                ),
            )
        })?;
    Ok(stream)
}

#[cfg(not(unix))]
fn dial(socket_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "Unix socket transport is unsupported on this platform (socket {})",
            socket_path.display()
        ),
    ))
}

#[cfg(unix)]
fn request_once(socket_path: &Path, id: &str, encoded: &[u8]) -> Result<Value, Error> {
    let mut stream = dial(socket_path).map_err(Error::transport)?;
    stream.write_all(encoded).map_err(|error| {
        Error::transport(io::Error::new(
            error.kind(),
            format!("write to {} failed: {error}", socket_path.display()),
        ))
    })?;
    stream.flush().map_err(|error| {
        Error::transport(io::Error::new(
            error.kind(),
            format!("flush to {} failed: {error}", socket_path.display()),
        ))
    })?;

    let response = read_response_line(&mut stream, socket_path)?;
    parse_response(response, id)
}

#[cfg(not(unix))]
fn request_once(socket_path: &Path, _id: &str, _encoded: &[u8]) -> Result<Value, Error> {
    dial(socket_path).map_err(Error::transport)?;
    unreachable!("the unsupported-platform dial always fails")
}

#[cfg(unix)]
fn read_response_line(stream: &mut UnixStream, socket_path: &Path) -> Result<Vec<u8>, Error> {
    let mut response = Vec::new();
    let mut chunk = [0_u8; 8192];

    loop {
        let read = stream.read(&mut chunk).map_err(|error| {
            Error::transport(io::Error::new(
                error.kind(),
                format!("read from {} failed: {error}", socket_path.display()),
            ))
        })?;
        if read == 0 {
            if response.is_empty() {
                return Err(Error::transport(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "herdr at {} closed the connection without answering",
                        socket_path.display()
                    ),
                )));
            }
            break;
        }

        let newline = chunk[..read].iter().position(|byte| *byte == b'\n');
        let line_bytes = newline.unwrap_or(read);
        let required = response.len().saturating_add(line_bytes);
        if required > MAX_RESPONSE_BYTES {
            return Err(Error::ResponseTooLarge {
                limit: MAX_RESPONSE_BYTES,
            });
        }
        if required > response.capacity() {
            let bounded_capacity = response
                .capacity()
                .saturating_mul(2)
                .max(required)
                .min(MAX_RESPONSE_BYTES);
            response.reserve_exact(bounded_capacity - response.len());
        }
        response.extend_from_slice(&chunk[..line_bytes]);
        if newline.is_some() {
            break;
        }
    }

    Ok(response)
}

fn parse_response(bytes: Vec<u8>, expected_id: &str) -> Result<Value, Error> {
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| Error::contract(format!("malformed JSON response: {error}")))?;
    let Value::Object(mut envelope) = value else {
        return Err(Error::contract("response envelope must be a JSON object"));
    };

    match envelope.remove("id") {
        Some(Value::String(actual)) if actual == expected_id => {}
        Some(Value::String(actual)) => {
            return Err(Error::contract(format!(
                "response `id` did not match request `id`: expected {expected_id:?}, got {actual:?}"
            )))
        }
        Some(_) => return Err(Error::contract("response `id` must be a string")),
        None => return Err(Error::contract("response is missing required string `id`")),
    }

    let has_result = envelope.contains_key("result");
    let has_error = envelope.contains_key("error");
    match (has_result, has_error) {
        (false, false) => {
            return Err(Error::contract(
                "response carried neither `result` nor `error`",
            ))
        }
        (true, true) => {
            return Err(Error::contract(
                "response carried both `result` and `error`",
            ))
        }
        _ => {}
    }

    if has_result {
        return Ok(envelope
            .remove("result")
            .expect("result presence was checked"));
    }

    let error = envelope
        .remove("error")
        .expect("error presence was checked");
    let Value::Object(mut error) = error else {
        return Err(Error::contract("response error must be a JSON object"));
    };
    let code = match error.remove("code") {
        Some(Value::String(code)) => code,
        Some(_) => return Err(Error::contract("response error code must be a string")),
        None => return Err(Error::contract("response error is missing its code")),
    };
    let message = match error.remove("message") {
        Some(Value::String(message)) => message,
        Some(_) => return Err(Error::contract("response error message must be a string")),
        None => return Err(Error::contract("response error is missing its message")),
    };
    Err(Error::Protocol { code, message })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    use serde_json::json;

    static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);
    const MOCK_TIMEOUT: Duration = Duration::from_secs(2);

    struct TestSocket {
        path: PathBuf,
        dir: PathBuf,
    }

    impl TestSocket {
        fn bind() -> (Self, UnixListener) {
            let sequence = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("crook-client-{}-{sequence}", std::process::id()));
            fs::create_dir(&dir).expect("create mock socket directory");
            let path = dir.join("herdr.sock");
            let listener = UnixListener::bind(&path).expect("bind mock socket");
            (Self { path, dir }, listener)
        }
    }

    impl Drop for TestSocket {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn spawn_server(
        handler: impl FnOnce(UnixListener) + Send + 'static,
    ) -> (TestSocket, JoinHandle<()>) {
        let (socket, listener) = TestSocket::bind();
        let handle = thread::spawn(move || handler(listener));
        (socket, handle)
    }

    fn accept(listener: &UnixListener) -> UnixStream {
        listener
            .set_nonblocking(true)
            .expect("make mock listener nonblocking");
        let deadline = Instant::now() + MOCK_TIMEOUT;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("make mock stream blocking");
                    return stream;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "timed out awaiting client");
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("mock accept failed: {error}"),
            }
        }
    }

    fn assert_no_connection(listener: &UnixListener) {
        listener
            .set_nonblocking(true)
            .expect("make mock listener nonblocking");
        let deadline = Instant::now() + Duration::from_millis(350);
        loop {
            match listener.accept() {
                Ok(_) => panic!("client unexpectedly retried the request"),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("mock accept failed: {error}"),
            }
        }
    }

    fn read_request(stream: &mut UnixStream) -> Value {
        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .expect("read mock request");
        assert!(line.ends_with('\n'), "request must be NDJSON");
        serde_json::from_str(&line).expect("parse mock request")
    }

    fn write_json(stream: &mut UnixStream, value: Value) {
        let mut response = serde_json::to_vec(&value).expect("encode mock response");
        response.push(b'\n');
        stream.write_all(&response).expect("write mock response");
    }

    #[test]
    fn success_returns_owned_result() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert_eq!(request["id"], "test:1");
            assert_eq!(request["method"], "session.snapshot");
            assert_eq!(request["params"], json!({"scope": "all"}));
            write_json(
                &mut stream,
                json!({"id": request["id"], "result": {"ready": true}}),
            );
        });
        let client = Client::new(&socket.path, "test");
        assert_eq!(client.socket_path(), socket.path.as_path());

        let result = client
            .request(
                "session.snapshot",
                json!({"scope": "all"}),
                RetrySafety::Never,
            )
            .expect("successful request");

        assert_eq!(result, json!({"ready": true}));
        server.join().expect("mock server");
    }

    #[test]
    fn protocol_error_exposes_its_code() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            write_json(
                &mut stream,
                json!({
                    "id": request["id"],
                    "error": {"code": "workspace_not_found", "message": "gone"}
                }),
            );
        });
        let client = Client::new(&socket.path, "protocol");

        let error = client
            .request("workspace.get", json!({}), RetrySafety::Never)
            .expect_err("server error");

        assert_eq!(error.protocol_code(), Some("workspace_not_found"));
        assert!(matches!(
            &error,
            Error::Protocol { code, message }
                if code == "workspace_not_found" && message == "gone"
        ));
        server.join().expect("mock server");
    }

    #[test]
    fn response_id_must_match_request() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let _ = read_request(&mut stream);
            write_json(&mut stream, json!({"id": "someone-else:1", "result": null}));
        });
        let client = Client::new(&socket.path, "expected");

        let error = client
            .request("test", json!({}), RetrySafety::Never)
            .expect_err("mismatched ID");

        assert!(
            matches!(&error, Error::Contract(message) if message.contains("`id` did not match request `id`"))
        );
        server.join().expect("mock server");
    }

    #[test]
    fn malformed_response_is_contract_failure_and_is_not_retried() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let _ = read_request(&mut stream);
            stream
                .write_all(b"{not-json}\n")
                .expect("write malformed response");
            assert_no_connection(&listener);
        });
        let client = Client::new(&socket.path, "malformed");

        let error = client
            .request("test", json!({}), RetrySafety::Idempotent)
            .expect_err("malformed response");

        assert!(matches!(&error, Error::Contract(message) if message.contains("malformed")));
        server.join().expect("mock server");
    }

    #[test]
    fn response_has_exactly_one_result_or_error() {
        let both = serde_json::to_vec(&json!({
            "id": "shape:1",
            "result": null,
            "error": {"code": "bad", "message": "bad"}
        }))
        .expect("encode response");
        let neither = serde_json::to_vec(&json!({"id": "shape:1"})).expect("encode response");

        for response in [both, neither] {
            let error = parse_response(response, "shape:1").expect_err("invalid shape");
            assert!(
                matches!(&error, Error::Contract(message) if message.contains("`result`") && message.contains("`error`"))
            );
        }
    }

    #[test]
    fn non_object_params_are_rejected_before_dial_or_id_allocation() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert_eq!(request["id"], "params:1");
            write_json(&mut stream, json!({"id": request["id"], "result": true}));
        });
        let client = Client::new(&socket.path, "params");

        let error = client
            .request("bad", Value::Null, RetrySafety::Never)
            .expect_err("non-object params");
        assert!(matches!(error, Error::Contract(_)));
        assert_eq!(
            client
                .request("good", json!({}), RetrySafety::Never)
                .expect("valid request"),
            Value::Bool(true)
        );
        server.join().expect("mock server");
    }

    #[test]
    fn response_at_exact_limit_is_accepted() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            let empty = serde_json::to_vec(&json!({"id": request["id"], "result": ""}))
                .expect("encode empty response");
            let payload = "x".repeat(MAX_RESPONSE_BYTES - empty.len());
            let response = serde_json::to_vec(&json!({
                "id": request["id"],
                "result": payload
            }))
            .expect("encode limit response");
            assert_eq!(response.len(), MAX_RESPONSE_BYTES);
            stream.write_all(&response).expect("write limit response");
            stream.write_all(b"\n").expect("terminate limit response");
        });
        let client = Client::new(&socket.path, "limit");

        let result = client
            .request("limit", json!({}), RetrySafety::Never)
            .expect("response at limit");

        assert_eq!(result.as_str().map(str::len), Some(MAX_RESPONSE_BYTES - 28));
        server.join().expect("mock server");
    }

    #[test]
    fn response_over_limit_is_rejected_before_appending_excess() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            let empty = serde_json::to_vec(&json!({"id": request["id"], "result": ""}))
                .expect("encode empty response");
            let payload = "x".repeat(MAX_RESPONSE_BYTES + 1 - empty.len());
            let response = serde_json::to_vec(&json!({
                "id": request["id"],
                "result": payload
            }))
            .expect("encode oversized response");
            assert_eq!(response.len(), MAX_RESPONSE_BYTES + 1);
            let _ = stream.write_all(&response);
            let _ = stream.write_all(b"\n");
        });
        let client = Client::new(&socket.path, "over");

        let error = client
            .request("limit", json!({}), RetrySafety::Idempotent)
            .expect_err("oversized response");

        assert!(matches!(
            error,
            Error::ResponseTooLarge {
                limit: MAX_RESPONSE_BYTES
            }
        ));
        server.join().expect("mock server");
    }

    #[test]
    fn protocol_errors_are_not_retried() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            write_json(
                &mut stream,
                json!({
                    "id": request["id"],
                    "error": {"code": "invalid_request", "message": "no"}
                }),
            );
            assert_no_connection(&listener);
        });
        let client = Client::new(&socket.path, "no-retry");

        let error = client
            .request("bad", json!({}), RetrySafety::Idempotent)
            .expect_err("protocol error");

        assert_eq!(error.protocol_code(), Some("invalid_request"));
        server.join().expect("mock server");
    }

    #[test]
    fn idempotent_transport_failure_retries_with_same_id() {
        let (socket, server) = spawn_server(|listener| {
            let mut first = accept(&listener);
            let first_request = read_request(&mut first);
            drop(first);

            let mut second = accept(&listener);
            let second_request = read_request(&mut second);
            assert_eq!(first_request["id"], second_request["id"]);
            write_json(
                &mut second,
                json!({"id": second_request["id"], "result": "recovered"}),
            );
        });
        let client = Client::new(&socket.path, "retry");

        let result = client
            .request("read", json!({}), RetrySafety::Idempotent)
            .expect("retry succeeds");

        assert_eq!(result, "recovered");
        server.join().expect("mock server");
    }

    #[test]
    fn unsafe_transport_failure_is_not_retried() {
        let (socket, server) = spawn_server(|listener| {
            let mut first = accept(&listener);
            let _ = read_request(&mut first);
            drop(first);
            assert_no_connection(&listener);
        });
        let client = Client::new(&socket.path, "unsafe");

        let error = client
            .request("write", json!({}), RetrySafety::Never)
            .expect_err("transport failure");

        assert!(matches!(error, Error::Transport(_)));
        server.join().expect("mock server");
    }

    #[test]
    fn connect_probes_then_requests_on_a_fresh_connection() {
        let (socket, server) = spawn_server(|listener| {
            let probe = accept(&listener);
            drop(probe);

            let mut request_stream = accept(&listener);
            let request = read_request(&mut request_stream);
            write_json(
                &mut request_stream,
                json!({"id": request["id"], "result": "fresh"}),
            );
        });

        let client = Client::connect(&socket.path, "connect").expect("preflight connect");
        let result = client
            .request("read", json!({}), RetrySafety::Never)
            .expect("request after probe");

        assert_eq!(result, "fresh");
        server.join().expect("mock server");
    }

    #[test]
    fn connect_preflight_retries_once_deterministically() {
        let attempts = std::cell::Cell::new(0_u8);
        let sleeps = std::cell::Cell::new(Vec::new());

        preflight_with(
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() == 1 {
                    Err(Error::transport(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        "first dial",
                    )))
                } else {
                    Ok(())
                }
            },
            |duration| {
                let mut recorded = sleeps.take();
                recorded.push(duration);
                sleeps.set(recorded);
            },
        )
        .expect("second preflight succeeds");

        assert_eq!(attempts.get(), 2);
        assert_eq!(sleeps.take(), vec![RETRY_BACKOFF]);
    }

    #[test]
    fn connect_failure_names_the_socket_path() {
        let sequence = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "crook-client-missing-{}-{sequence}.sock",
            std::process::id()
        ));

        let error = Client::connect(&path, "missing").expect_err("missing socket");

        assert!(matches!(error, Error::Transport(_)));
        assert!(error.to_string().contains(&path.display().to_string()));
    }
}
