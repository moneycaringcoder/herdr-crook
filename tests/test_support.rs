#![cfg(all(feature = "test-support", unix))]

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crook::client::{Client, Error as ClientError, RetrySafety};
use crook::test_support::{CapturedEnvelope, EnvGuard, FixtureReply, FixtureServer};
use serde_json::{json, Value};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> Self {
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "crook-support-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create test scratch directory");
        Self(path)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn result_reply_uses_real_client_and_captures_ndjson_request() {
    let server = FixtureServer::new([FixtureReply::result(json!({"ready": true}))])
        .expect("start fixture server");
    let client = Client::connect(server.socket_path(), "fixture").expect("connect through probe");

    let result = client
        .request(
            "session.snapshot",
            json!({"scope": "all"}),
            RetrySafety::Never,
        )
        .expect("fixture result");

    assert_eq!(result, json!({"ready": true}));
    assert_eq!(
        server.requests(),
        vec![json!({
            "id": "fixture:1",
            "method": "session.snapshot",
            "params": {"scope": "all"}
        })]
    );
    let raw = server.raw_lines();
    assert_eq!(raw.len(), 1);
    assert!(raw[0].ends_with(b"\n"));
    assert_eq!(raw[0].iter().filter(|byte| **byte == b'\n').count(), 1);
}

#[test]
fn scripted_replies_are_consumed_in_order() {
    let server = FixtureServer::new([
        FixtureReply::result(json!("first")),
        FixtureReply::result(json!("second")),
    ])
    .expect("start fixture server");
    let client = Client::new(server.socket_path(), "queue");

    let first = client
        .request("test", json!({}), RetrySafety::Never)
        .expect("first reply");
    let second = client
        .request("test", json!({}), RetrySafety::Never)
        .expect("second reply");

    assert_eq!(first, "first");
    assert_eq!(second, "second");
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn malformed_request_is_captured_and_consumes_its_reply() {
    let server = FixtureServer::new([
        FixtureReply::result(json!("malformed-request-reply")),
        FixtureReply::result(json!("valid-request-reply")),
    ])
    .expect("start fixture server");
    let mut raw_client =
        UnixStream::connect(server.socket_path()).expect("connect malformed fixture client");
    raw_client
        .write_all(b"not-json\n")
        .expect("write malformed request");
    let mut raw_response = Vec::new();
    raw_client
        .read_to_end(&mut raw_response)
        .expect("read malformed request reply");

    assert_eq!(
        serde_json::from_slice::<Value>(&raw_response).expect("parse null-ID fixture reply"),
        json!({"id": null, "result": "malformed-request-reply"})
    );

    let client = Client::new(server.socket_path(), "after-malformed");
    assert_eq!(
        client
            .request("test", json!({}), RetrySafety::Never)
            .expect("second queued reply"),
        "valid-request-reply"
    );
    assert_eq!(server.requests().len(), 1);
    assert_eq!(server.raw_lines()[0], b"not-json\n");
    assert_eq!(server.raw_lines().len(), 2);
}

#[test]
fn raw_reply_bytes_are_verbatim() {
    let expected = b"raw-without-newline";
    let server = FixtureServer::new([FixtureReply::raw(expected)]).expect("start fixture server");
    let mut stream = UnixStream::connect(server.socket_path()).expect("connect raw fixture client");
    stream
        .write_all(b"{\"id\":\"raw:1\",\"method\":\"test\",\"params\":{}}\n")
        .expect("write request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read verbatim response");

    assert_eq!(response, expected);
}

#[test]
fn dropping_server_removes_its_socket_directory() {
    let socket_path = {
        let server = FixtureServer::new([]).expect("start fixture server");
        server.socket_path().to_owned()
    };

    assert!(!socket_path.exists());
    assert!(!socket_path.parent().expect("socket directory").exists());
}

#[test]
fn queued_protocol_error_exposes_code_and_message() {
    let server = FixtureServer::new([FixtureReply::error("workspace_not_found", "gone")])
        .expect("start fixture server");
    let client = Client::new(server.socket_path(), "protocol");

    let error = client
        .request("workspace.get", json!({}), RetrySafety::Never)
        .expect_err("protocol error");

    assert!(matches!(
        error,
        ClientError::Protocol { code, message }
            if code == "workspace_not_found" && message == "gone"
    ));
}

#[test]
fn raw_reply_can_force_an_id_mismatch() {
    let server = FixtureServer::new([FixtureReply::raw(
        br#"{"id":"someone-else:1","result":null}
"#,
    )])
    .expect("start fixture server");
    let client = Client::new(server.socket_path(), "expected");

    let error = client
        .request("test", json!({}), RetrySafety::Never)
        .expect_err("mismatched response ID");

    assert!(matches!(
        error,
        ClientError::Contract(message)
            if message.contains("`id` did not match request `id`")
    ));
}

#[test]
fn oversize_reply_hits_the_real_client_limit() {
    let server = FixtureServer::new([FixtureReply::Oversize]).expect("start fixture server");
    let client = Client::new(server.socket_path(), "oversize");

    let error = client
        .request("test", json!({}), RetrySafety::Never)
        .expect_err("oversized response");

    assert!(matches!(
        error,
        ClientError::ResponseTooLarge { limit } if limit == 4 * 1024 * 1024
    ));
}

#[test]
fn eof_reply_is_a_transport_failure() {
    let server = FixtureServer::new([FixtureReply::Eof]).expect("start fixture server");
    let client = Client::new(server.socket_path(), "eof");

    let error = client
        .request("test", json!({}), RetrySafety::Never)
        .expect_err("empty EOF");

    assert!(matches!(
        error,
        ClientError::Transport(source)
            if source.kind() == std::io::ErrorKind::UnexpectedEof
    ));
}

#[test]
fn malformed_raw_reply_is_a_contract_failure() {
    let server =
        FixtureServer::new([FixtureReply::raw(b"{not-json}\n")]).expect("start fixture server");
    let client = Client::new(server.socket_path(), "malformed");

    let error = client
        .request("test", json!({}), RetrySafety::Never)
        .expect_err("malformed response");

    assert!(matches!(
        error,
        ClientError::Contract(message) if message.contains("malformed JSON response")
    ));
}

#[test]
fn endless_reply_streams_without_a_newline() {
    let server = FixtureServer::new([FixtureReply::Endless]).expect("start fixture server");
    let mut stream = UnixStream::connect(server.socket_path()).expect("connect raw fixture client");
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("set fixture read timeout");
    stream
        .write_all(b"{\"id\":\"endless:1\",\"method\":\"test\",\"params\":{}}\n")
        .expect("write request");

    let mut chunk = [0_u8; 4096];
    let read = stream.read(&mut chunk).expect("read endless bytes");

    assert!(read > 0);
    assert!(!chunk[..read].contains(&b'\n'));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn captured_envelopes_load_from_strings_and_files() {
    let envelope = CapturedEnvelope::from_envelope_str(
        r#"{"id":"captured:9","result":{"ready":true},"unknown":{"kept":1}}"#,
    )
    .expect("parse captured envelope");
    assert_eq!(
        envelope.with_request_id("live:1"),
        json!({
            "id": "live:1",
            "result": {"ready": true},
            "unknown": {"kept": 1}
        })
    );

    let bare = CapturedEnvelope::from_result_str(r#"{"type":"snapshot","future":42}"#)
        .expect("parse captured result");
    assert_eq!(
        bare.with_request_id("live:2"),
        json!({
            "id": "live:2",
            "result": {"type": "snapshot", "future": 42}
        })
    );

    let scratch = ScratchDir::new();
    let envelope_path = scratch.0.join("envelope.json");
    let result_path = scratch.0.join("result.json");
    fs::write(
        &envelope_path,
        r#"{"id":"old","error":{"code":"bad","message":"no"},"extra":true}"#,
    )
    .expect("write envelope capture");
    fs::write(&result_path, r#"["one","two"]"#).expect("write result capture");

    assert_eq!(
        CapturedEnvelope::from_envelope_file(&envelope_path)
            .expect("load envelope capture")
            .with_request_id("file:1"),
        json!({
            "id": "file:1",
            "error": {"code": "bad", "message": "no"},
            "extra": true
        })
    );
    assert_eq!(
        CapturedEnvelope::from_result_file(&result_path)
            .expect("load result capture")
            .with_request_id("file:2"),
        json!({"id": "file:2", "result": ["one", "two"]})
    );

    let server = FixtureServer::new([FixtureReply::Captured(envelope)])
        .expect("start captured fixture server");
    let client = Client::new(server.socket_path(), "captured");
    assert_eq!(
        client
            .request("test", json!({}), RetrySafety::Never)
            .expect("captured response"),
        json!({"ready": true})
    );
}

#[test]
fn env_guard_restores_values_and_unset_state() {
    const PRESENT: &str = "CROOK_TEST_SUPPORT_PRESENT";
    const ABSENT: &str = "CROOK_TEST_SUPPORT_ABSENT";
    const NON_UNICODE: &str = "CROOK_TEST_SUPPORT_NON_UNICODE";
    let original_non_unicode = OsString::from_vec(b"before-\xff".to_vec());
    std::env::set_var(PRESENT, "before");
    std::env::remove_var(ABSENT);
    std::env::set_var(NON_UNICODE, &original_non_unicode);

    {
        let mut guard = EnvGuard::new();
        guard.set(PRESENT, "during");
        guard.set(PRESENT, "after-second-set");
        guard.set(ABSENT, "temporary");
        guard.unset(NON_UNICODE);
        assert_eq!(std::env::var(PRESENT).as_deref(), Ok("after-second-set"));
        assert_eq!(std::env::var(ABSENT).as_deref(), Ok("temporary"));
        assert_eq!(std::env::var_os(NON_UNICODE), None);
    }

    assert_eq!(std::env::var(PRESENT).as_deref(), Ok("before"));
    assert_eq!(std::env::var_os(ABSENT), None);
    assert_eq!(std::env::var_os(NON_UNICODE), Some(original_non_unicode));
    std::env::remove_var(PRESENT);
    std::env::remove_var(NON_UNICODE);
}

#[test]
fn env_guards_serialize_process_environment_mutation() {
    let first = EnvGuard::new();
    let (sender, receiver) = mpsc::channel();
    let contender = std::thread::spawn(move || {
        let _second = EnvGuard::new();
        sender.send(()).expect("report acquired environment lock");
    });

    assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
    drop(first);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("second guard acquires after first drops");
    contender.join().expect("environment contender");
}

#[test]
fn non_object_capture_is_rejected() {
    let error = CapturedEnvelope::from_envelope(Value::Null)
        .expect_err("response envelope must be an object");
    assert_eq!(
        error.to_string(),
        "captured response envelope must be a JSON object"
    );
}
