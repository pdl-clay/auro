# Contributing to Auro

Auro is pre-alpha and being designed in public, so the most valuable contribution right now is
an opinion, not a patch.

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## The best ways to help today

1. **Argue with the design.** Read [ROADMAP.md](ROADMAP.md) and the open questions at the end.
   Open a Discussion with your reasoning. A well-argued objection is worth more than a PR.
2. **Tell us what you would ship with it.** Real use cases shape the component kit and the
   plugin boundary.
3. **Report what blocks you** in GPUI for your app. That list becomes our priority order.
4. **Build something small.** Start with [docs/src/start.md](docs/src/start.md); plugin authors
   should read the [plugin author cookbook](docs/src/cookbook/plugin-author.md) for the host
   contract before proposing an index entry.

## Discuss first, code second

Anything larger than a typo starts in [Discussions](https://github.com/pdl-clay/auro/discussions).
A PR that lands without agreement wastes your time.

A typo or a small documentation fix can go straight to a PR. Anything over roughly 50 changed
lines, and every behaviour or API change, starts with an issue or a Discussion first.

Design changes that affect the public API go through a short [RFC](rfcs/README.md). A decision
that has already been made and constrains future code is recorded as an
[ADR](docs/src/decisions/README.md) instead. If you are unsure which applies, ask in the issue
first.

## Development setup

Rust **stable**, with `rustfmt` and `clippy`. `rust-toolchain.toml` pins the channel and the
components, so a plain `rustup` install is enough.

Linux needs native headers for the windowing, audio and TLS stack - the same set CI installs:

```bash
sudo apt-get install -y --no-install-recommends \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
  libvulkan-dev libasound2-dev libfontconfig-dev libssl-dev pkg-config
```

Other distributions need the equivalent packages.

The workspace is at the repository root (`Cargo.toml`, `Cargo.lock`, `crates/`, `vendor/`). Build
and test from the checkout root; `Cargo.lock` is committed and CI runs with `--locked`.

## Gates

CI runs these on every pull request, and a red gate is never merged. Run them locally before
pushing:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
mdbook build docs
```

CI also runs `actionlint` on the workflows, a three-OS build/test matrix, and an `ubuntu-22.04`
floor build. The docs book is built by the `docs` job and deployed to GitHub Pages on `main`.

## Documentation

```bash
cargo install mdbook --version 0.4.40 --locked
mdbook serve docs        # http://localhost:3000
mdbook build docs        # output in docs/book, gitignored
```

Every file listed in `docs/src/SUMMARY.md` must exist (`create-missing = false`). English is the
source of truth; `README.pt-BR.md` must mirror `README.md` section for section - same headings,
same tables, same code samples. A change to one without the other is an incomplete change.

## Code conventions

- `cargo fmt` and `cargo clippy -- -D warnings` before pushing. CI enforces both.
- Every `.rs` file starts with `// SPDX-License-Identifier: MIT OR Apache-2.0`.
- Errors are typed: library code returns `Result<T, AuroError>`. No `unwrap` outside tests, and no
  `unwrap` in a command.
- No blocking I/O on the main thread. GPUI owns the main thread; commands are `async fn`s on the
  `tokio` runtime. See [threading and IPC](docs/src/architecture/threading-and-ipc.md).
- No `println!` for diagnostics in library code; use `tracing`.
- New public API needs a doc comment with an example that compiles, and an entry in
  `CHANGELOG.md` under `## [Unreleased]`.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/), imperative mood, 72-character
subject, no trailing period:

```
feat(ipc): derive command schema from the function signature
fix(ui): keep focus on the input after a reload
docs: explain the capability manifest format
```

Types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.

Sign off every commit (DCO 1.1, no CLA):

```bash
git commit -s -m "docs: add the RFC template"
```

## Pull requests

- One logical change per PR. Do not mix formatting churn with behaviour.
- Fill in the pull request template; it mirrors the gates above.
- Keep the branch rebased on `main`. PRs are squash-merged, so the PR title must be a valid
  Conventional Commit subject.
- A maintainer review is required. Expect questions about scope before style.
- Draft PRs and basic questions are welcome. Nobody needs to be certain before opening one.
- Aim for a first review response within 3 days. If that does not happen, ping the PR: that is
  helpful, not rude. A first response is not a promise of an immediate merge.

## Performance is a feature

Auro's whole pitch is cold start, idle RAM and edit-to-see latency. A PR that improves
ergonomics but regresses those numbers needs to say so and justify it.

## Licensing

By contributing you agree your work is dual-licensed under MIT or Apache-2.0, matching the
project. Do not copy code with an incompatible licence into the tree.

## Conduct

Be technical, be direct, be kind. Disagreement about design is expected; contempt is not.

## Security

Do not open a public issue for a vulnerability. Use [SECURITY.md](SECURITY.md).
