# Changelog

All notable changes to Crook are recorded here. Crook follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/moneycaringcoder/herdr-crook/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/moneycaringcoder/herdr-crook/releases/tag/v0.1.0
