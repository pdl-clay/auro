# Text input, IME and accessibility

Status: audit of the pinned stack on 2026-11-18. This page records what Auro and GPUI 0.2.2
actually provide. It is not a support claim.

## What Auro exposes

`auro-ui` currently ships one primitive, `Button`, and no text-input primitive. The button:

- is focusable and a tab stop;
- activates on an unmodified Enter or Space when focused;
- exposes no accessibility role, label or action beyond its visible child.

There is no accessibility tree, no screen-reader bridge and no `AccessKit` integration in Auro.
Adding a fake one would mislead users; the gap is upstream.

## What GPUI 0.2.2 provides

A source audit of `gpui 0.2.2` (the pinned version) found:

| Area | Present in GPUI 0.2.2 | Notes |
|---|---|---|
| Accessibility/AccessKit | **No** | No `accesskit` or `accessibility` symbol in the crate source or features |
| Text input handler API | Yes | `platform::InputHandler`, `Window::set_input_handler`/`take_input_handler`, `ElementInputHandler` |
| IME candidate positioning | Yes | `InputHandler::bounds_for_range`/`PlatformWindow::update_ime_position` |
| macOS IME | Yes | `NSTextInputClient` with `insertText:replacementRange:` and `setMarkedText:selectedRange:replacementRange:` |
| Windows IME | Yes | `WM_IME_STARTCOMPOSITION`/`WM_IME_COMPOSITION` handling |
| Wayland IME | Yes | `zwp_text_input_v3` |
| X11 IME | Yes | XIM handler |

That means IME plumbing exists at the toolkit layer on all three target platforms. Auro does not
wrap it in a widget, and no Auro test drives a composition event end to end. An app that uses GPUI's
text elements directly inherits that plumbing; an app that wants an Auro-owned text field does not
have one yet.

## Platform audit

| Platform | IME plumbing | Screen reader | Verified in Auro |
|---|---|---|---|
| Linux (Wayland) | `zwp_text_input_v3` | none | no |
| Linux (X11) | XIM | none | no |
| Windows | `WM_IME_*` | none | no |
| macOS | `NSTextInputClient` | none | no |

"Verified in Auro" means a maintainer ran a composition against an Auro app. No such run exists,
so the row stays `no` even though the upstream code path is present.

## Consequences

- Screen readers cannot inspect an Auro app. The keyboard path is the only accessible interaction
  Auro can claim today, and only for `Button`.
- Complex-script input is expected to work through GPUI where a text element asks for an input
  handler, but it is unmeasured. CJK composition, dead keys, emoji pickers and input-method
  candidate windows are open verification items, not finished features.
- The Phase 2 line "Accessibility (AccessKit), IME and keyboard navigation pass" is partially
  blocked upstream. Keyboard activation is implemented (spec 0023); AccessKit has no upstream API
  to call; IME has upstream plumbing but no Auro surface or verification.

## When this changes

Re-audit when the GPUI pin moves. A row moves to "verified" only after a real desktop session on
that platform exercises composition and, for accessibility, a platform accessibility inspector
reports the expected tree. CI compiling the code is not evidence for either.
