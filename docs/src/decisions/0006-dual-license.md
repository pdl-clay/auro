# 0006 - Dual MIT/Apache-2.0

**Status:** Accepted

## Context

The README already states dual licensing under MIT or Apache-2.0, and the roadmap lists the licence
as an open question with the same recommendation. Rust's ecosystem, including `cargo` itself and
most of the crates Auro will depend on, is dual-licensed this way. A framework that expects
commercial desktop apps must not create licence friction for them, and Apache-2.0 supplies an
explicit patent grant that MIT alone does not.

## Decision

Auro is licensed under **MIT OR Apache-2.0**, at the user's option. Every crate declares
`license = "MIT OR Apache-2.0"` in its manifest. Contributions are accepted under the same terms.

## Consequences

**Easier**

- Compatible with the Rust ecosystem by default.
- Corporate adopters get a patent grant without asking for an exception.
- No CLA, no per-contributor negotiation.

**Harder**

- Two licence files must be shipped in every crate, and the workspace must not drift.
- Vendored GPUI patches must keep their upstream licence notices.

**Forbidden**

- `license = "MIT"` or `license = "Apache-2.0"` alone on any crate in the workspace.
- Copying code with an incompatible licence into the tree.
- A per-crate licence exception without a superseding decision record.
