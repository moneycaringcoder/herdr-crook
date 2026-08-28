# crook

The shepherd's staff: one shared library for every herdr plugin in this stable.

## Why this exists

Six plugins (shear, collide, standup, pulse, redact, tether) were built independently, and
an inventory of all six found the same infrastructure written over and over. Ranked by how
many copies exist today:

| Concern | Copies | ~LOC per copy |
| --- | --- | --- |
| Herdr socket client — NDJSON over `HERDR_SOCKET_PATH`, redial per call, 15s timeout, 4 MiB response cap, one transport retry, envelope/ID validation, protocol-vs-transport error split | 6/6 | 380–1,800 |
| Plugin environment — `HERDR_PLUGIN_ID` / `HERDR_PLUGIN_STATE_DIR` / `HERDR_PLUGIN_CONFIG_DIR` resolution with XDG/Home fallbacks | 6/6 | 80–170 |
| Release & CI tooling — `check_release.py`, `herdr_api_contract.py`, ci/release/upstream-canary workflows; same filenames in every repo, content drifting apart | 6/6 | (Python) |
| `--setup` config.toml splicer — backup, additive splice, reload through herdr, byte-for-byte restore on failed reload, explicit rollback | 4 | 420–700 |
| Badge/daemon lifecycle — enabled/pid markers, flock ownership, TTL token plans, clear-before-set, disable sweep, startup `--restore` re-arm | 3 (+1 planned) | 1,000–1,400 |
| TUI runtime — raw-mode/alt-screen guard restored on Drop/panic/SIGINT/SIGTERM, crossterm poll/key-gating, mouse hit maps, theme conventions | 3 fresh + 1 older | ~450 |
| Unicode/ANSI display width, ellipsis truncation, wrapping | 4 | 150–450 |
| Atomic state files — flock, tmp+rename, directory sync | 4 | ~150 |

Every copy is a place a bug fix does not reach. crook is where those fixes land once.

## Shape

One crate, feature-gated modules, consumed as a git dependency pinned to tagged releases:

```toml
crook = { git = "https://github.com/moneycaringcoder/herdr-crook", tag = "v0.x.y", features = ["client", "tui"] }
```

- `client` — the socket transport, request/response envelope, and the small set of RPCs
  everyone calls (`session.snapshot` as a raw value, `workspace.report_metadata`,
  notifications, config reload). Plugin environment and socket-path resolution live here.
- `fmt` — display width, truncation, wrapping, byte/age formatting.
- `tui` — terminal lifecycle guard, event loop scaffolding, key/mouse mapping, hit-testing,
  and the visual conventions (theme-inheriting colors, whole-row reversed cursor, bold
  colored tags only, no dim, no background fills off the cursor row).
- `setup` — the config.toml splice/backup/reload/rollback machinery behind every `--setup`.
- `daemon` — marker/lock lifecycle, token planning and push/sweep for badge updaters.
- `fs` — atomic state-file writes and advisory locking.

## What deliberately stays out

Domain logic never moves here. Each plugin keeps its own:

- snapshot *reducers* (what a plugin extracts from `session.snapshot` is its identity),
- state machines and verdict/severity semantics,
- git plumbing policy (what to ask git and what the answers mean),
- rendering content (crook provides width math, not sentences).

The test of belonging: if two plugins would ever legitimately want different behavior from
the same function, it does not belong in crook.

## Naming

Repository `herdr-crook`, crate `crook` — same convention as `herdr-shear`/`shear`.
A crook is the one tool every shepherd carries.
