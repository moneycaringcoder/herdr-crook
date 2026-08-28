# Changelog

All notable changes to Crook are recorded here. Crook follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Preserve arbitrarily deep snapshot object views and their exact validation
  paths instead of treating the third nested object as absent.
- Keep explicit-mode temporary files private until staging completes, apply the
  requested mode after writing, and clean up staged files during unwinding.
- Prevent fixture-server shutdown from blocking on accepted sockets whose I/O
  timeouts could not be installed.

### Documentation

- Clarify that newly created parent directories are not separately synced into
  their ancestors.

## [0.2.1] - 2026-08-28

### Added

- Validated structural views and ID joins for `session.snapshot` results.
- Thin wrappers for the four RPCs shared across Herdr plugins, including decoded
  notification delivery verdicts.
- Unix atomic replacement, mode-aware create-new, non-clobbering backup, and
  directory-lock primitives.
- Opt-in `test-support` feature with a scripted Unix-socket server, scoped
  environment guard, request capture, and captured-response loaders.

## [0.2.0] - 2026-08-28

### Added

- Typed validation for Herdr's installed-plugin invocation context.
- Installed plugin-root resolution through `PluginEnv`.

## [0.1.0] - 2026-08-28

### Added

- Bounded one-request-per-connection Herdr Unix-socket client.
- Explicit retry safety for transport failures.
- Request ID and response-envelope validation.
- Plugin ID, socket, state-directory, and config-directory resolution.
- Linux, macOS, and Rust 1.80 CI coverage.

[Semantic Versioning]: https://semver.org/

[Unreleased]: https://github.com/moneycaringcoder/herdr-crook/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/moneycaringcoder/herdr-crook/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/moneycaringcoder/herdr-crook/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/moneycaringcoder/herdr-crook/releases/tag/v0.1.0
