# 0023 - Keyboard activation

**Status:** Implemented (2026-11-17; keyboard activation only; AccessKit and IME audit pending)

Phase 2 needs keyboard-operable controls. This spec fixes the first slice: the primitive `Button`
participates in the tab order and activates from the keyboard. It does not claim a full
accessibility pass.

## Scope

- `auro_ui::Button` is focusable and a tab stop.
- Enter and Space with no modifiers invoke the same handler as a mouse click.
- A held key does not repeat activation.
- Headless test proving tab focus and keyboard activation.

## Out of scope

- AccessKit or any screen-reader tree. GPUI 0.2.2 does not expose one.
- IME/composition behavior.
- Focus-ring styling and tokens.
- Full tab order across every primitive.
- Shortcut/menu navigation outside the focused control.

## Decisions

### D1 - Tab focus is automatic

`Button` renders with `focusable().tab_stop(true).tab_index(0)`. GPUI stores a focus handle in the
element state, so Auro does not need a second, app-owned handle for this primitive.

### D2 - Enter and Space share the click path

The keyboard handler calls the existing `on_click` handler with `ClickEvent::default()` (the
keyboard variant). A control has one activation path, not two subtly different ones.

*Rejected:* a separate `on_activate` callback. It doubles the API surface before there is a second
control that needs it.

### D3 - Ignore modifiers and held keys

Only unmodified Enter/Space activates. A held key does not fire repeatedly; key repeat is not a
click. Modal shortcuts belong to the app's key bindings, not to the primitive.

## Interfaces

```rust
pub struct Button { /* id, label, on_click */ }

impl Button {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self;
    pub fn on_click(
        self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self;
}
```

Behavior: after the button is focused (for example with Tab), Enter or Space invokes `on_click`.

## Acceptance

1. A focused button activates on Enter and then Space, and the click count advances
   (`button_activates_with_enter_and_space`).
2. Existing mouse activation still passes (`clicking_button_dispatches_async_command_and_updates_view`).
3. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| AccessKit tree and screen-reader parity | Upstream GPUI / accessibility spec |
| IME and composition behavior | Platform input audit |
| Focus-ring styling and tokens | Design-system follow-up |
| Full tab order across primitives | Accessibility follow-up |
