<!--
  Keep the title a valid Conventional Commit subject; it becomes the squash-merge commit.
  Example: feat(ipc): derive command schema from the function signature
-->

## What

<!-- The change, in two or three sentences. -->

## Why

<!-- The problem it solves. Link the issue, Discussion or RFC: Fixes #123 / RFC 0004 -->

## How

<!-- Anything a reviewer needs: a design choice, a trade-off, a rejection. Delete if obvious. -->

## Checklist

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `mdbook build docs`
- [ ] `CHANGELOG.md` updated under `[Unreleased]` (user-visible changes only)
- [ ] `README.md` and `README.pt-BR.md` are still in sync (if either changed)
- [ ] New chapters are listed in `docs/src/SUMMARY.md`
- [ ] Commits are signed off (`git commit -s`)
- [ ] The change is licensed under MIT OR Apache-2.0 and every new `.rs` file has the SPDX header
- [ ] An ADR is added or superseded (if this changes an architectural decision)

## Scope

<!-- Delete the ones that do not apply. -->
- [ ] No public API change
- [ ] Public API change, documented and covered by a changelog entry
- [ ] Breaking change (pre-1.0: bumps the minor version; explain the migration)
