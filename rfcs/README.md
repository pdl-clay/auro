# RFCs

An RFC is a design proposal that needs discussion before it becomes a decision. If the change is
already decided and is expensive to reverse, write a [decision record](../docs/src/decisions/README.md)
instead.

## When an RFC is required

- A new public crate, or a change to the crate layering.
- A change to the IPC contract, the capability model, or the hot-reload mechanism.
- A new platform in the support matrix, or dropping one.
- Anything a reviewer would describe as "that is a design question, not a patch".
- Adding a dependency that the framework's users will inherit.

Not required for: bug fixes, internal refactors, docs, tests, CI, or a new UI component that
follows existing conventions.

## Process

1. **Discuss first.** Open a Discussion in the *RFCs* category and describe the problem. This
   avoids writing a document for a proposal that is already rejected.
2. **Write the RFC.** Copy [`0000-template.md`](0000-template.md) to `NNNN-short-title.md`, where
   `NNNN` is the next free number. Open a pull request.
3. **Review.** The pull request is the review thread. Amendments are pushed as commits, not as
   follow-up comments, so the document stays the source of truth.
4. **Decide.** A maintainer comments with one of:
   - `Accepted` - the design is approved; implementation may start, and an ADR is written once the
     decision lands in code.
   - `Rejected` - with a reason. The document stays in the tree with status `Rejected`; it is
     history, not a parking lot.
   - `Postponed` - with the condition that would unblock it.
5. **Merge.** The file is merged with its status updated. Accepted RFCs are immutable apart from
   status and links; a material change needs a new RFC that supersedes it.

## Numbering

Numbers are assigned in the pull request that creates the file. If two RFCs claim the same number,
the one that merges second renumbers. Gaps are not reused.

## Status values

`Draft` - `Under review` - `Accepted` - `Rejected` - `Postponed` - `Superseded by NNNN`

## Relationship to the roadmap

An accepted RFC that changes scope, sequencing or a stated target must update `ROADMAP.md` in the
same pull request. The roadmap is the plan; the RFC is why the plan changed.

## Index

| # | Title | Status |
|---|---|---|
| [0001](0001-update-metadata-trust.md) | Update metadata trust: freshness, rotation and revocation | Draft |
| [0002](0002-plugin-host.md) | Plugin host: out-of-process RPC and opt-in in-process dylibs | Accepted |
