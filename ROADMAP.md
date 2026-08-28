# Roadmap

Order chosen so each step has the most consumers and the clearest contract, and so every
migration can be verified against tests the plugins already have.

## 1. `client` — the socket transport (first, six consumers)

The one module every plugin needs, and the one with the most independent copies.

- [ ] Scaffold the crate: `cargo init --lib`, feature flags per module, MSRV 1.80 to match
      the strictest current CI, minimal deps (serde, serde_json; keep libc/signal-hook
      behind the features that need them).
- [ ] Extract the transport from the two most-hardened copies (shear adapted collide's
      client, so they share lineage): connect via `HERDR_SOCKET_PATH` with XDG/Home
      fallback, one request per connection, string IDs, 15-second I/O deadline, 4 MiB
      response cap, single transport retry, typed split between transport failures and
      protocol errors.
- [ ] Envelope handling: reject a missing nested `snapshot`/`result` loudly (both mature
      copies independently chose fail-loud over treating protocol breakage as idle — keep
      that).
- [ ] Common RPCs: `session.snapshot` returning the raw value (reducers stay in plugins),
      `workspace.report_metadata` with TTL clamp and null-clears (collide and pulse both
      have this; take the union: atomic per-workspace patch + chunked token sets),
      `notification.show`, `server.reload_config`.
- [ ] Plugin environment: `HERDR_PLUGIN_ID`/`HERDR_PLUGIN_STATE_DIR`/`HERDR_PLUGIN_CONFIG_DIR`
      resolution with the empty-string-means-unset rule and XDG/Home fallbacks.
- [ ] Tests: port one repo's `tests/herdr_client.rs` mock-socket harness into crook itself.
- [ ] Proof migration: shear (smallest client, richest client tests). Its
      `tests/herdr_client.rs` and `tests/read_only.rs` must pass unchanged.
- [ ] Second migration: collide (validates the report_metadata surface).
- [ ] Tag v0.1.0 once two plugins consume it.

## 2. `fmt` + `tui` (three-plus consumers each)

- [ ] `fmt`: pick ONE width/truncation implementation wholesale (shear's and collide's
      agree on semantics — path-tail left truncation, label-head right — but differ in
      shape; do not blend, choose and port tests).
- [ ] `tui`: terminal lifecycle guard (raw mode + alternate screen restored on Drop, panic
      hook, SIGINT/SIGTERM), the 50 ms poll/key-repeat-gating event loop, mouse hit-map
      types (`MouseMap`/`HitRow` are already near-verbatim in three repos), and a
      conventions module for the shared visual rules.
- [ ] Migrate shear, collide, standup views onto it. State machines do not move.

## 3. `setup` + `daemon` (the config splicer and badge lifecycle)

- [ ] `setup`: unify the four splicers (collide, pulse, redact sidebar rows; tether's
      keybinding append). Contract: resolve config path, non-clobbering timestamped
      backups (collide's stale-backup refusal was a real UX papercut — rotate instead),
      additive idempotent splice, reload via herdr, byte-for-byte restore when the reload
      does not come back clean, explicit rollback verb.
- [ ] `daemon`: extract the marker/flock lifecycle and token-plan/push/sweep common to
      collide, pulse, redact. Pulse's supervision (systemd/launchd) stays in pulse.
- [ ] New consumer to prove it: shear's planned reclaim badge (shear#36) should be
      buildable almost entirely from `client` + `daemon`.

## 4. Tooling (not Rust, not a crate module)

- [ ] `tooling/` directory in this repo holding the canonical `check_release.py`,
      `test_check_release.py`, `herdr_api_contract.py`, and the three workflow templates.
      Six drifted copies exist today; standup's `--main-ref` ancestry check is the best
      version and becomes canonical.
- [ ] A tiny `sync-tooling` script that copies them into a plugin repo and reports drift,
      so each repo stays self-contained (plugins must build without crook's repo present)
      but drift becomes visible instead of silent.

## Rules that hold for every step

- A migration lands only when the consuming plugin's existing test suite passes unchanged;
  contract tests move into crook, they do not get rewritten around it.
- crook releases are tagged; plugins pin tags, never a branch.
- Anything two plugins would want to behave differently gets deleted from crook, not
  configured into it.
