# Contributing to Crook

Crook is a small Rust library for Herdr plugin infrastructure. Contributions should preserve its bounded transport, explicit retry, and environment-resolution contracts.

## Before starting

- Use Rust 1.80 or newer.
- Develop on Linux or macOS; Crook's client targets Unix sockets.
- Open an issue before changing a public API or wire-level behavior. Small bug fixes and documentation corrections can go directly to a pull request.
- Keep plugin-specific RPC wrappers, reducers, state machines, and rendering outside Crook.

## Development

Clone the repository and run the full local checks:

```bash
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo +1.80.0 test --all-targets --locked
```

Tests that open Unix sockets must pass on both Linux and macOS. Avoid timing assumptions; test observable framing, limits, errors, and retry behavior.

## Compatibility

Crook follows semantic versioning.

- Patch releases preserve the public API and documented behavior.
- Minor releases may add backward-compatible APIs.
- Breaking API or behavior changes require a new major version. While Crook is `0.x`, the minor version carries that breaking-change signal.
- The minimum supported Rust version is part of the compatibility contract and must not change without release notes.

Every request must choose `RetrySafety` explicitly. Never mark a state-changing request idempotent unless Herdr guarantees repeating it is safe after an ambiguous transport failure.

## Pull requests

A pull request should:

- explain the user-visible problem and chosen behavior;
- include focused tests for changed observable behavior;
- update public documentation and `CHANGELOG.md` when required;
- keep `Cargo.lock` committed;
- pass every required GitHub Actions check;
- avoid unrelated refactors.

Use clear commit messages that describe the change rather than the implementation process.

## Reporting security problems

Do not open a public issue for a vulnerability. Follow [SECURITY.md](SECURITY.md) instead.
