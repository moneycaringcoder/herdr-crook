# crook

Crook is a Rust library for Herdr plugin authors. It provides a bounded
Unix-socket client, validated snapshot views, wrappers for the four common
Herdr RPCs, durable Unix file primitives, and plugin environment resolution.

The lower-level client returns raw `serde_json::Value` results. Snapshot and RPC
helpers validate only shared wire structure; plugins retain their domain types,
reducers, state machines, rendering, and policy.

## Requirements

- Rust 1.80 or newer
- Linux or macOS
- A running Herdr server for socket requests

## Installation

Pin a released tag and commit `Cargo.lock`:

```toml
[dependencies]
crook = { git = "https://github.com/moneycaringcoder/herdr-crook", tag = "v0.2.0" }
serde_json = "1"
```

Crook has no feature flags. It depends on `serde_json` and, on Unix targets,
`libc` for advisory directory locks.

## Quick start

```rust
use crook::client::{Client, RetrySafety};
use crook::env::PluginEnv;
use serde_json::json;

fn load_snapshot() -> Result<serde_json::Value, crook::client::Error> {
    let env = PluginEnv::resolve("example.plugin");
    let client = Client::connect(env.socket_path(), env.plugin_id())?;

    client.request(
        "session.snapshot",
        json!({}),
        RetrySafety::Idempotent,
    )
}
```

`Client::connect` performs a preflight connection probe and retries it once
after 150 ms on a transport failure. The successful probe connection is
discarded. `Client::new` constructs the same client without opening the socket.

## Sending requests

```rust
// After constructing a Client as shown above:
let result = client.request(
    "workspace.report_metadata",
    serde_json::json!({
        "workspace_id": "w1",
        "tokens": {"build": "passing"}
    }),
    RetrySafety::Never,
)?;
```

Request parameters must be a JSON object. Crook:

- opens one Unix-socket connection per request;
- sends one newline-delimited JSON request;
- assigns string IDs using the prefix supplied to `Client`;
- applies 15-second read and write deadlines;
- rejects response lines larger than 4 MiB;
- requires the response ID to match the request ID;
- requires exactly one of `result` or `error`;
- returns the owned `result` value.

### Retry safety

Choose retry behavior for every request:

- `RetrySafety::Never` performs one attempt. Use it when repeating a request
  could repeat a state change.
- `RetrySafety::Idempotent` retries once, after 150 ms, only when the first
  attempt fails at the transport layer.

Protocol errors, invalid response contracts, and oversized responses are never
retried. A retry reuses the original request ID.

## Errors

`crook::client::Error` separates four failure classes:

| Variant | Meaning |
| --- | --- |
| `Transport` | The socket could not be connected to, written to, or read from. |
| `Protocol` | Herdr returned an error code and message. |
| `Contract` | The request or response did not match the wire contract. |
| `ResponseTooLarge` | The response exceeded the 4 MiB limit. |

Use `Error::protocol_code()` when callers need Herdr's stable error code.

## Snapshot views

`Snapshot::from_result` validates the `session_snapshot` result type, nested
`snapshot` object, and its `workspaces`, `panes`, and `agents` arrays. Borrowed
record views provide lenient field access, strict field checks with indexed JSON
paths, whitespace-trimming path access, and ordered workspace/pane/agent ID
joins. They do not parse plugin domain state or apply path normalization policy.

## Common RPCs

`crook::rpc` wraps only `session.snapshot`, `notification.show`,
`workspace.report_metadata`, and `worktree.list`. The wrappers build the request
shapes used by Herdr plugins, select the established retry safety, preserve
protocol error codes, decode notification delivery verdicts, enforce metadata
merge-patch and TTL rules, and leave plugin-specific worktree reduction to
callers.

## Durable files

On Unix, `crook::fs` provides atomic replacement with file and containing
directory sync, an explicit-mode variant for private files, mode-aware durable
create-new publication from in-memory bytes, non-clobbering `0o600` path
backups, and a directory-scoped RAII `flock` guard. A directory-sync error after
publication is returned without rolling back the already-published file.

## Plugin environment

```rust
use crook::env::{PluginContext, PluginEnv};

let env = PluginEnv::resolve("example.plugin");

println!("plugin: {}", env.plugin_id());
println!("socket: {}", env.socket_path().display());
println!("state: {}", env.state_dir().display());
println!("config: {}", env.config_dir().display());

if let Some(context) = PluginContext::resolve()? {
    let invocation_cwd = context
        .focused_pane_cwd()
        .or_else(|| context.workspace_cwd());
}
```

A non-blank UTF-8 `HERDR_PLUGIN_ID` takes precedence over the supplied default.
Non-blank socket, state, and config path variables take precedence and are
preserved unchanged, including relative and non-UTF-8 paths.

| Value | Herdr variable | Fallback |
| --- | --- | --- |
| Plugin ID | `HERDR_PLUGIN_ID` | Default passed to `PluginEnv::resolve` |
| Socket | `HERDR_SOCKET_PATH` | `<config-base>/herdr/herdr.sock` |
| State directory | `HERDR_PLUGIN_STATE_DIR` | `<state-base>/herdr/plugins/<plugin-id>` |
| Config directory | `HERDR_PLUGIN_CONFIG_DIR` | `<config-base>/herdr/plugins/config/<plugin-id>` |
| Invocation context | `HERDR_PLUGIN_CONTEXT_JSON` | No context |
| Installed plugin root | `HERDR_PLUGIN_ROOT` | No plugin root |

Each base is resolved independently. `config-base` uses an absolute
`XDG_CONFIG_HOME`, then an absolute `HOME/.config`. `state-base` uses an
absolute `XDG_STATE_HOME`, then an absolute `HOME/.local/state`. If a base has
neither source, it uses `<system-temp>/herdr-no-home`.

`PluginContext::resolve` validates the known workspace and focused-pane fields
while tolerating unknown fields added by Herdr. A present malformed context,
wrong known-field type, or relative cwd is an error. Installed plugin commands
run from `HERDR_PLUGIN_ROOT`, so consumers must use the invocation context
instead of treating the process cwd as the user's repository. `plugin_root()` is
`Some` only when Herdr supplies an absolute path.

Blank environment values are treated as unset.

## Scope

Crook provides:

- bounded request/response transport for the Herdr Unix socket;
- request ID and response-envelope validation;
- explicit retry safety;
- structural `session.snapshot` validation and ID joins;
- thin wrappers for the four RPCs shared across Herdr plugins;
- Unix atomic replacement, non-clobbering backup, and directory-lock primitives;
- plugin ID, socket, state-directory, config-directory, installed-root, and
  invocation-context resolution.

Crook does not provide:

- plugin domain types, reducers, state machines, rendering, storage schemas, or
  policy;
- wrappers for RPCs outside the four common methods;
- persistent connections or event subscriptions;
- configurable timeout or response-size policies.
