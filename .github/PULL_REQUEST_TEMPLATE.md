## Summary

<!-- One paragraph. -->

## Linked issues

Closes #

## Type of change

- [ ] Bug fix (non-breaking, fixes #)
- [ ] New feature (non-breaking)
- [ ] Breaking change (please describe below)
- [ ] Documentation only

## Test plan

<!-- How did you verify this works? Include `cargo build` results + manual smoke test if applicable. -->

- [ ] `cargo fmt --all` clean
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo build --release -p quarkdrive-webdav` succeeds
- [ ] `bash scripts/build_deploy_test.sh` end-to-end passes

## Notes for reviewers

<!-- Anything subtle or out of scope. -->
