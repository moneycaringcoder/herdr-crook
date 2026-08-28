# crook

Shared infrastructure for Herdr plugins.

Crook owns mechanics that should behave identically across plugins. Plugins
keep their domain reducers, state machines, rendering content, and policy.

## Why this exists

The current plugin repositories repeat several infrastructure layers:

- six Unix-socket clients, with inconsistent limits and envelope validation;
- five matching plugin-environment resolvers, plus Tether's distinct standalone
  path policy;
- three closely related terminal runtimes in shear, collide, and standup;
- three sidebar setup flows and three badge-daemon lifecycles;
- repeated Unicode width, truncation, locking, atomic-write, release, and CI
  tooling.

That drift already matters. Four clients read unbounded response lines and do
not validate response IDs. A transport fix copied into one plugin does not
protect the others.

## v0.1.0

The first release is deliberately narrow:

- `client` — bounded NDJSON request/response transport over the Herdr Unix
  socket. It redials per request, uses string IDs, validates response IDs and
  envelopes, applies 15-second I/O deadlines, caps responses at 4 MiB, and
  distinguishes transport, protocol, contract, and size errors. Callers must
  explicitly mark a request idempotent before Crook will retry it.
- `env` — resolution of `HERDR_PLUGIN_ID`, `HERDR_SOCKET_PATH`,
  `HERDR_PLUGIN_STATE_DIR`, and `HERDR_PLUGIN_CONFIG_DIR`, including the
  empty-means-unset rule and absolute XDG/Home fallbacks used by the first five
  plugins.

No feature flags yet. Both modules are small, always compiled, and depend only
on `serde_json` plus the standard library.

After the release is tagged, plugins consume an exact tag:

```toml
crook = { git = "https://github.com/moneycaringcoder/herdr-crook", tag = "v0.1.0" }
```

Plugins commit `Cargo.lock`. A new Crook tag never changes a plugin build
silently; dependency updates arrive as tested pull requests.

## Boundary

The shared client returns the raw RPC `result`. Each plugin retains:

- snapshot reducers and domain types;
- RPC-specific response validation;
- verdict, severity, and state-machine semantics;
- git plumbing and mutation policy;
- rendered sentences and reports.

The test for extraction: two consumers must require the same observable
behavior. Similar-looking code with different contracts stays local.

Tether does not consume v0.1. Its 5-second/2-MiB unary policy, optional socket
integration, standalone state layout, cloneable threaded client, and persistent
event subscription are materially different contracts. Those must not be
silently widened to fit the first release.

## Later candidates

Only after the client/environment migrations prove the release boundary:

- `fmt` and `tui` from shear, collide, and standup;
- durable locked writes from standup, pulse, and Tether;
- setup transactions from collide, pulse, and redact;
- badge-daemon lifecycle from collide, pulse, and redact.

Python release checks and workflow templates remain repository tooling, not
Rust crate modules.

## Naming

Repository `herdr-crook`, crate `crook` — the same convention as
`herdr-shear`/`shear`.
