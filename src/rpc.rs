//! Thin conveniences for the Herdr RPCs shared by multiple plugins.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde_json::{Map, Value};

use crate::client::{self, Client, RetrySafety};
use crate::snapshot::{Snapshot, SnapshotError};

const MIN_METADATA_TTL_MS: u64 = 1;
const MAX_METADATA_TTL_MS: u64 = 86_400_000;
const MAX_METADATA_TOKENS: usize = 16;

/// A failure while invoking or structurally decoding a common RPC.
#[derive(Debug)]
pub enum Error {
    /// The underlying socket client failed.
    Client(client::Error),
    /// A successful `session.snapshot` response had an invalid structure.
    Snapshot(SnapshotError),
}

impl Error {
    /// Returns Herdr's stable protocol error code, when the client received one.
    pub fn protocol_code(&self) -> Option<&str> {
        match self {
            Self::Client(error) => error.protocol_code(),
            Self::Snapshot(_) => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => error.fmt(f),
            Self::Snapshot(error) => write!(f, "invalid session.snapshot structure: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::Snapshot(error) => Some(error),
        }
    }
}

impl From<client::Error> for Error {
    fn from(error: client::Error) -> Self {
        Self::Client(error)
    }
}

impl From<SnapshotError> for Error {
    fn from(error: SnapshotError) -> Self {
        Self::Snapshot(error)
    }
}

/// Selects one of the two `worktree.list` request shapes used by Herdr plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeListScope<'a> {
    /// List the repository containing this working directory.
    Cwd(&'a Path),
    /// List worktrees associated with this open workspace.
    WorkspaceId(&'a str),
}

/// Fetches and validates one complete `session.snapshot` result.
pub fn session_snapshot(client: &Client) -> Result<Snapshot, Error> {
    let result = client.request(
        "session.snapshot",
        Value::Object(Map::new()),
        RetrySafety::Idempotent,
    )?;
    Snapshot::from_result(result).map_err(Error::from)
}

/// Shows a notification with the title and body shape used by Herdr plugins.
///
/// The request is never retried because an interrupted successful request could
/// otherwise display the same notification twice. A successful result payload
/// is intentionally ignored.
pub fn notification_show(client: &Client, title: &str, body: &str) -> Result<(), Error> {
    let mut params = Map::new();
    params.insert("title".into(), Value::String(title.to_owned()));
    params.insert("body".into(), Value::String(body.to_owned()));
    client.request(
        "notification.show",
        Value::Object(params),
        RetrySafety::Never,
    )?;
    Ok(())
}

/// Applies a workspace token merge patch through `workspace.report_metadata`.
///
/// `Some(value)` sets a token and `None` serializes as JSON `null` to clear only
/// that token. Empty patches perform no request. Patches larger than Herdr's
/// sixteen-token limit are sent in ordered chunks. A supplied TTL is clamped to
/// Herdr's accepted `1..=86_400_000` millisecond range and is omitted from every
/// pure-clear chunk, because Herdr rejects a TTL that has no set value to govern.
/// Mutating requests are never retried.
pub fn workspace_report_metadata(
    client: &Client,
    workspace_id: &str,
    source: &str,
    tokens: &BTreeMap<String, Option<String>>,
    ttl_ms: Option<u64>,
) -> Result<(), Error> {
    let mut tokens = tokens.iter().peekable();
    while tokens.peek().is_some() {
        let mut patch = Map::new();
        let mut sets_anything = false;
        for _ in 0..MAX_METADATA_TOKENS {
            let Some((name, value)) = tokens.next() else {
                break;
            };
            match value {
                Some(value) => {
                    sets_anything = true;
                    patch.insert(name.clone(), Value::String(value.clone()));
                }
                None => {
                    patch.insert(name.clone(), Value::Null);
                }
            }
        }

        let mut params = Map::new();
        params.insert(
            "workspace_id".into(),
            Value::String(workspace_id.to_owned()),
        );
        params.insert("source".into(), Value::String(source.to_owned()));
        params.insert("tokens".into(), Value::Object(patch));
        if sets_anything {
            if let Some(ttl_ms) = ttl_ms {
                params.insert(
                    "ttl_ms".into(),
                    Value::from(ttl_ms.clamp(MIN_METADATA_TTL_MS, MAX_METADATA_TTL_MS)),
                );
            }
        }

        client.request(
            "workspace.report_metadata",
            Value::Object(params),
            RetrySafety::Never,
        )?;
    }
    Ok(())
}

/// Lists worktrees using either an observed working-directory or workspace-ID
/// selector and returns Herdr's raw result for plugin-specific reduction.
pub fn worktree_list(client: &Client, scope: WorktreeListScope<'_>) -> Result<Value, Error> {
    let mut params = Map::new();
    match scope {
        WorktreeListScope::Cwd(cwd) => {
            params.insert(
                "cwd".into(),
                Value::String(cwd.to_string_lossy().into_owned()),
            );
        }
        WorktreeListScope::WorkspaceId(workspace_id) => {
            params.insert(
                "workspace_id".into(),
                Value::String(workspace_id.to_owned()),
            );
        }
    }
    client
        .request(
            "worktree.list",
            Value::Object(params),
            RetrySafety::Idempotent,
        )
        .map_err(Error::from)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

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
            let dir =
                std::env::temp_dir().join(format!("crook-rpc-{}-{sequence}", std::process::id()));
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
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "timed out awaiting client");
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
        serde_json::from_str(&line).expect("parse mock request")
    }

    fn write_response(stream: &mut UnixStream, response: Value) {
        let mut encoded = serde_json::to_vec(&response).expect("encode mock response");
        encoded.push(b'\n');
        stream.write_all(&encoded).expect("write mock response");
    }

    fn write_result(stream: &mut UnixStream, request: &Value, result: Value) {
        write_response(
            stream,
            json!({"id": request["id"].clone(), "result": result}),
        );
    }

    fn snapshot_result() -> Value {
        json!({
            "type": "session_snapshot",
            "snapshot": {"workspaces": [], "panes": [], "agents": []}
        })
    }

    #[test]
    fn session_snapshot_uses_empty_params_and_structural_validation() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert_eq!(request["method"], "session.snapshot");
            assert_eq!(request["params"], json!({}));
            write_result(&mut stream, &request, snapshot_result());
        });
        let client = Client::new(&socket.path, "rpc-snapshot");

        let snapshot = session_snapshot(&client).expect("snapshot response");

        assert!(snapshot.workspaces().is_empty());
        server.join().expect("mock server");
    }

    #[test]
    fn session_snapshot_preserves_structural_error_type() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            write_result(&mut stream, &request, json!({"type": "changed"}));
        });
        let client = Client::new(&socket.path, "rpc-invalid-snapshot");

        let error = session_snapshot(&client).expect_err("invalid snapshot");

        assert!(matches!(error, Error::Snapshot(_)));
        server.join().expect("mock server");
    }

    #[test]
    fn notification_uses_only_title_and_body() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert_eq!(request["method"], "notification.show");
            assert_eq!(
                request["params"],
                json!({"title": "Build", "body": "Passing"})
            );
            write_result(
                &mut stream,
                &request,
                json!({"type": "notification_show", "shown": true, "reason": "shown"}),
            );
        });
        let client = Client::new(&socket.path, "rpc-notification");

        notification_show(&client, "Build", "Passing").expect("show notification");

        server.join().expect("mock server");
    }

    #[test]
    fn metadata_serializes_mixed_merge_patch_and_clamps_ttl() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert_eq!(request["method"], "workspace.report_metadata");
            assert_eq!(
                request["params"],
                json!({
                    "workspace_id": "w1",
                    "source": "plugin.test",
                    "tokens": {"clear": null, "set": "green"},
                    "ttl_ms": 86_400_000
                })
            );
            write_result(&mut stream, &request, json!({"type": "ok"}));
        });
        let client = Client::new(&socket.path, "rpc-metadata-mixed");
        let tokens = BTreeMap::from([
            ("clear".to_string(), None),
            ("set".to_string(), Some("green".to_string())),
        ]);

        workspace_report_metadata(&client, "w1", "plugin.test", &tokens, Some(u64::MAX))
            .expect("report metadata");

        server.join().expect("mock server");
    }

    #[test]
    fn metadata_omits_ttl_for_pure_clear() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert_eq!(
                request["params"],
                json!({
                    "workspace_id": "w1",
                    "source": "plugin.test",
                    "tokens": {"badge": null}
                })
            );
            write_result(&mut stream, &request, json!({"type": "ok"}));
        });
        let client = Client::new(&socket.path, "rpc-metadata-clear");
        let tokens = BTreeMap::from([("badge".to_string(), None)]);

        workspace_report_metadata(&client, "w1", "plugin.test", &tokens, Some(5_000))
            .expect("clear metadata");

        server.join().expect("mock server");
    }

    #[test]
    fn empty_metadata_patch_performs_no_request() {
        let missing_socket = std::env::temp_dir().join(format!(
            "crook-rpc-missing-{}-{}",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let client = Client::new(missing_socket, "rpc-empty-metadata");

        workspace_report_metadata(&client, "w1", "plugin.test", &BTreeMap::new(), Some(5_000))
            .expect("empty patch is a no-op");
    }

    #[test]
    fn metadata_chunks_at_the_server_limit() {
        let (socket, server) = spawn_server(|listener| {
            let mut first = accept(&listener);
            let first_request = read_request(&mut first);
            assert_eq!(
                first_request["params"]["tokens"]
                    .as_object()
                    .expect("first token patch")
                    .len(),
                16
            );
            write_result(&mut first, &first_request, json!({"type": "ok"}));

            let mut second = accept(&listener);
            let second_request = read_request(&mut second);
            assert_eq!(
                second_request["params"]["tokens"]
                    .as_object()
                    .expect("second token patch")
                    .len(),
                1
            );
            write_result(&mut second, &second_request, json!({"type": "ok"}));
        });
        let client = Client::new(&socket.path, "rpc-metadata-chunks");
        let tokens = (0..17)
            .map(|index| (format!("token-{index:02}"), Some("value".to_string())))
            .collect();

        workspace_report_metadata(&client, "w1", "plugin.test", &tokens, Some(5_000))
            .expect("chunked report");

        server.join().expect("mock server");
    }

    #[test]
    fn worktree_list_supports_both_observed_selectors() {
        let (socket, server) = spawn_server(|listener| {
            let mut cwd_stream = accept(&listener);
            let cwd_request = read_request(&mut cwd_stream);
            assert_eq!(cwd_request["method"], "worktree.list");
            assert_eq!(cwd_request["params"], json!({"cwd": "/repo/app"}));
            write_result(
                &mut cwd_stream,
                &cwd_request,
                json!({"type": "worktree_list", "worktrees": [1]}),
            );

            let mut workspace_stream = accept(&listener);
            let workspace_request = read_request(&mut workspace_stream);
            assert_eq!(workspace_request["params"], json!({"workspace_id": "w1"}));
            write_result(
                &mut workspace_stream,
                &workspace_request,
                json!({"type": "worktree_list", "worktrees": [2]}),
            );
        });
        let client = Client::new(&socket.path, "rpc-worktrees");

        let cwd = worktree_list(&client, WorktreeListScope::Cwd(Path::new("/repo/app")))
            .expect("cwd worktree list");
        let workspace = worktree_list(&client, WorktreeListScope::WorkspaceId("w1"))
            .expect("workspace worktree list");

        assert_eq!(cwd["worktrees"], json!([1]));
        assert_eq!(workspace["worktrees"], json!([2]));
        server.join().expect("mock server");
    }

    #[test]
    fn protocol_codes_remain_inspectable() {
        let (socket, server) = spawn_server(|listener| {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            write_response(
                &mut stream,
                json!({
                    "id": request["id"].clone(),
                    "error": {"code": "not_git_worktree", "message": "not a repository"}
                }),
            );
        });
        let client = Client::new(&socket.path, "rpc-protocol");

        let error = worktree_list(&client, WorktreeListScope::Cwd(Path::new("/tmp")))
            .expect_err("protocol error");

        assert_eq!(error.protocol_code(), Some("not_git_worktree"));
        server.join().expect("mock server");
    }
}
