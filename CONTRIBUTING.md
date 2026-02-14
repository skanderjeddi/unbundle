# Contributing to unbundle

Thanks for contributing.

## Development

- Build: `cargo build`
- Build all features: `cargo build --all-features`
- Run tests: `cargo test --all-features`
- Run examples: `cargo run --example <name> -- <media-file>`

## Pull Requests

- Keep changes focused and minimal.
- Add/update tests when behavior changes.
- Update docs/examples for any public API changes.
- Add a changelog entry for user-facing updates.

## Release Checklist

- Bump version in `Cargo.toml` (and lockfile if applicable).
- Update `CHANGELOG.md` with release notes.
- Ensure CI is green on Linux, macOS, and Windows.
- Verify README metadata section matches GitHub About (description/homepage/topics).
- Create/push tag and GitHub release notes.
- Publish crate to crates.io.
- Verify docs.rs and crates.io reflect the new version.
