# Releasing Crook

Crook releases are immutable Git tags. Consumers pin exact tags and commit their lockfiles; they never depend on a moving branch.

## Prepare

1. Confirm the intended release commit is on `main` and every required check passes.
2. Run the local release checks:

   ```bash
   cargo fmt --all -- --check
   cargo test --all-targets --locked
   cargo clippy --all-targets --locked -- -D warnings
   cargo +1.80.0 test --all-targets --locked
   ```

3. Move the entries under `Unreleased` in `CHANGELOG.md` into a versioned section with the release date.
4. Confirm `Cargo.toml` contains the release version and correct MSRV.
5. Review the public API, README examples, and error/retry behavior for compatibility with the intended semantic version.

## Publish

1. Create an annotated `v<version>` tag on the verified `main` commit.
2. Push the tag without moving or replacing any existing release tag.
3. Create a GitHub release whose notes match the versioned changelog section.

## Update consumers

For each Herdr plugin:

1. Change the Crook dependency to the new exact tag.
2. Run `cargo update -p crook` and commit `Cargo.lock`.
3. Run that plugin's complete test and lint suite.
4. Open a dependency-update pull request and let the plugin's required CI checks validate the update.

Do not update consumers automatically to the newest Crook release. Every plugin upgrade remains an explicit, tested change.
