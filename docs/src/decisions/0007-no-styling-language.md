# 0007 - No styling language in 1.0

**Status:** Accepted

## Context

Every GUI framework that grows a stying language eventually maintains two type systems: one for
what the program computes and one for what the stylesheet says. Web developers arriving at a Rust
UI toolkit will ask for CSS, or for a CSS-like DSL, because that is how they know to express
layout.

GPUI's model is a builder API: `div().flex().flex_col().gap_2().p_4()`. It is Rust all the way
down, which means layout participates in normal tooling - autocomplete, types, refactoring.

The roadmap names scope drift into "CSS for Rust" as a project risk and committed to a hard rule.

## Decision

**Auro ships no styling language in 1.0.** Styling is expressed in Rust against design tokens.
Theming is data (tokens, light/dark palettes, TOML themes), not a syntax to be parsed.

## Consequences

**Easier**

- One type system, one debugger, one set of compile errors.
- Tokens can be validated at compile time and swapped at runtime without a parser.
- The framework cannot drift into building a CSS engine.

**Harder**

- Migration guides from CSS must teach a different mental model ("thinking without CSS"), not just
  a syntax mapping.
- Designers cannot hand over a stylesheet; they hand over tokens and component specs.
- Some things CSS makes trivial (arbitrary selectors, cascade) have no equivalent and must be
  declined explicitly.

**Forbidden**

- A `.css`-like file format, a selector engine, or a cascade in 1.0.
- Adding "just a little" styling DSL behind a feature flag. That is how the rule dies.
- Documenting a styling syntax as "planned"; it is not planned.
