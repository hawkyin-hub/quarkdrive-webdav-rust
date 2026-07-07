# Contributing

Thanks for your interest in making QuarkDrive-WebDAV better!

## Code of conduct

Be respectful. We're a small project; assume good faith. No harassment, no doxxing, no spam.

## Reporting bugs

Open an issue at <https://github.com/hawkyin-hub/quarkdrive-webdav-rust/issues>. Use the **Bug report** template.

For sensitive vulnerabilities, see [SECURITY.md](SECURITY.md) instead.

## Suggesting features

Open an issue with the **Feature request** template.

## Submitting patches

1. **Fork** the repository.
2. Create a feature branch: `git checkout -b fix-bug-N-or-feat-name`
3. Make your change.
4. Run before committing:
   ```bash
   cargo fmt --all
   cargo clippy --all-targets -- -D warnings
   cargo build --release -p quarkdrive-webdav
   bash scripts/build_deploy_test.sh   # full smoke test
   ```
5. Push your branch and open a pull request. The PR template will guide you.

## Coding conventions

- **Edition 2024**, Rust 1.74+.
- Code style: `cargo fmt` default.
- Lint: `cargo clippy -- -D warnings`.
- Tests: when fixing a bug, add a regression test or document a manual repro in the PR.
- Comments in English. User-facing docs may be Chinese or English.
- Don't introduce new dependencies without discussion in the issue tracker.
- Don't modify the `mount.rs` / `webdav.rs` / `proxy.rs` modules without an explicit maintainer ack (they touch the protected §3 surface).

## Commit messages

Recommended format:
```
<scope>: <one-line summary>

<body explaining the why, not the what>

<fixes #issue-number if applicable>
```
Scope examples: `vfs:`, `drive:`, `cache:`, `admin:`, `proxy:`, `docs:`.

## Release process (for maintainers)

1. Pick version per semver; commit version bump + `CHANGELOG.md` update on `main`.
2. Tag `vX.Y.Z`. CI builds the `.app` and the cargo release and pushes to GitHub Releases.
3. Announce in the discussion thread.

## License

By contributing, you agree your contributions are licensed under [MIT](LICENSE).
