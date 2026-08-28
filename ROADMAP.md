# Roadmap

Crook grows only after existing plugins prove that an infrastructure contract
is genuinely shared. Domain reducers, state machines, and rendering remain in
the plugins.

## v0.1.0 — client and environment

- [x] Create the `crook` crate with MSRV 1.80 and only `serde_json` beyond the
      standard library.
- [x] Add a bounded Unix-socket client: one NDJSON request per connection,
      string request IDs, 15-second I/O deadlines, a 4 MiB response ceiling,
      strict ID/envelope validation, and typed transport/protocol/contract
      errors.
- [x] Make retry safety explicit. Crook retries one transport failure only when
      the caller marks the RPC idempotent; state-changing RPCs never retry.
- [x] Add plugin environment resolution for ID, socket, state, and config paths,
      including blank-as-unset handling and absolute XDG/Home fallbacks.
- [x] Cover framing, limits, retries, contract failures, protocol errors, and
      environment precedence with focused tests.
- [x] Add Linux/macOS CI, formatting, clippy, and Rust 1.80 verification.
- [x] Prove the boundary in standup, collide, shear, pulse, and redact while
      retaining each plugin's public client API and domain reducers.
- [ ] Tag `v0.1.0` after release approval. Until then, migration branches use an
      exact candidate commit revision so builds remain reproducible.

Tether is intentionally outside v0.1. Its 5-second/2-MiB unary policy, optional
socket integration, cloneable threaded client, and persistent event
subscription require configurable transport policy plus a subscription
primitive. Silently forcing those contracts through the first client would be
a regression.

## v0.2 — formatting and terminal runtime

- [ ] Extract one tested Unicode width/truncation implementation. Preserve
      path-tail left truncation and label-head right truncation.
- [ ] Extract the terminal lifecycle guard, 50 ms poll loop, key-repeat gating,
      mouse hit maps, and shared visual conventions from shear, collide, and
      standup.
- [ ] Migrate those three views without moving their state machines into Crook.

## v0.3 — setup and daemon mechanics

- [ ] Unify the additive config splicers from collide, pulse, redact, and
      Tether: non-clobbering backups, idempotent edits, reload, byte-for-byte
      rollback, and an explicit rollback command.
- [ ] Extract marker/lock lifecycle and token plan/push/sweep mechanics shared by
      collide, pulse, and redact. Pulse supervision remains local.
- [ ] Prove the daemon boundary with shear's planned reclaim badge.

## Tooling

- [ ] Keep canonical release checks, API contract checks, and workflow templates
      under `tooling/`; they are repository tooling, not crate modules.
- [ ] Add a drift-checking sync command. Plugin repositories remain
      self-contained and must build without a Crook checkout.

## Release rules

- Releases are immutable tags. Plugin manifests pin a tag; pre-release migration
  branches pin one exact commit revision, never a moving branch.
- A migration lands only after the plugin's complete test suite passes.
- Contract tests move into Crook when the observable behavior is shared. Plugin
  fixtures may change only to represent the strengthened shared contract.
- Behavior that consumers need to differ on stays local rather than becoming a
  configuration switch by default.
