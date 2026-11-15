# Contributing to Auro

Auro is pre-alpha and being designed in public, so the most valuable contribution right now is
an opinion, not a patch.

## The best ways to help today

1. **Argue with the design.** Read [ROADMAP.md](ROADMAP.md) and the open questions at the end.
   Open a Discussion with your reasoning. A well-argued objection is worth more than a PR.
2. **Tell us what you would ship with it.** Real use cases shape the component kit and the
   plugin boundary.
3. **Report what blocks you** in GPUI for your app. That list becomes our priority order.

## Discuss first, code second

Anything larger than a typo starts in [Discussions](https://github.com/pdl-clay/auro/discussions).
A PR that lands without agreement wastes your time.

Design changes that affect the public API go through a short RFC in `rfcs/` once that folder
exists (phase 0).

## Code conventions

- `cargo fmt` and `cargo clippy -- -D warnings` before pushing. CI enforces both.
- Commits follow [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`,
  `docs:`, `refactor:`, `perf:`, `test:`, `chore:`.
- One logical change per PR.
- New public API needs a doc comment with an example that compiles.

## Performance is a feature

Auro's whole pitch is cold start, idle RAM and edit-to-see latency. A PR that improves
ergonomics but regresses those numbers needs to say so and justify it.

## Licensing

By contributing you agree your work is dual-licensed under MIT or Apache-2.0, matching the
project.

## Conduct

Be technical, be direct, be kind. Disagreement about design is expected; contempt is not.
