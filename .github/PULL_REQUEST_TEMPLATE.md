## Problem

Describe the user-visible problem or shared contract being changed.

## Change

Describe the chosen behavior and why it belongs in Crook rather than one plugin.

## Compatibility

- [ ] Public API compatibility considered
- [ ] Wire behavior and error classification considered
- [ ] `RetrySafety` choice reviewed
- [ ] Rust 1.80 and Linux/macOS support preserved

## Verification

List the focused behavior exercised, then confirm:

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo test --all-targets --locked`
- [ ] `cargo clippy --all-targets --locked -- -D warnings`
- [ ] `cargo +1.80.0 test --all-targets --locked`
- [ ] Public documentation and `CHANGELOG.md` updated when required
