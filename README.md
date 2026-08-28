# crook

Crook is a Rust library for Herdr plugin authors. It provides a bounded client
for Herdr's Unix-socket API and resolves the environment variables Herdr passes
to plugins.

Crook returns raw `serde_json::Value` results. Plugins remain responsible for
their RPC-specific types, validation, and behavior.

## Requirements

- Rust 1.80 or newer
- Linux or macOS
- A running Herdr server for socket requests

## Installation

Pin a released tag and commit `Cargo.lock`:

```toml
[dependencies]
crook = { git = "https://github.com/moneycaringcoder/herdr-crook", tag = "v0.1.0" }
serde_json = "1"
```

Crook has no feature flags. Its only non-standard-library dependency is
`serde_json`.

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

## Plugin environment

```rust
use crook::env::PluginEnv;

let env = PluginEnv::resolve("example.plugin");

println!("plugin: {}", env.plugin_id());
println!("socket: {}", env.socket_path().display());
println!("state: {}", env.state_dir().display());
println!("config: {}", env.config_dir().display());
```

A non-blank UTF-8 `HERDR_PLUGIN_ID` takes precedence over the supplied default.
Non-blank injected path variables take precedence and are preserved unchanged,
including relative and non-UTF-8 paths.

| Value | Herdr variable | Fallback |
| --- | --- | --- |
| Plugin ID | `HERDR_PLUGIN_ID` | Default passed to `PluginEnv::resolve` |
| Socket | `HERDR_SOCKET_PATH` | `<config-base>/herdr/herdr.sock` |
| State directory | `HERDR_PLUGIN_STATE_DIR` | `<state-base>/herdr/plugins/<plugin-id>` |
| Config directory | `HERDR_PLUGIN_CONFIG_DIR` | `<config-base>/herdr/plugins/config/<plugin-id>` |

Each base is resolved independently. `config-base` uses an absolute
`XDG_CONFIG_HOME`, then an absolute `HOME/.config`. `state-base` uses an
absolute `XDG_STATE_HOME`, then an absolute `HOME/.local/state`. If a base has
neither source, it uses `<system-temp>/herdr-no-home`.

Blank environment values are treated as unset.

## Scope

Crook v0.1 provides:

- bounded request/response transport for the Herdr Unix socket;
- request ID and response-envelope validation;
- explicit retry safety;
- plugin ID, socket, state-directory, and config-directory resolution.

Crook v0.1 does not provide:

- typed wrappers for individual Herdr RPCs;
- snapshot reducers or plugin domain types;
- plugin state machines, rendering, or policy;
- persistent connections or event subscriptions;
- configurable timeout or response-size policies.
