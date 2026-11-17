// SPDX-License-Identifier: MIT OR Apache-2.0

use auro_ui::Button;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, TestAppContext, Window, div,
};

#[derive(Default)]
struct KeyboardView {
    clicks: u32,
}

impl Render for KeyboardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("root")
            .child(Button::new("activate", "Activate").on_click(cx.listener(
                |this, _event, _window, cx| {
                    this.clicks += 1;
                    cx.notify();
                },
            )))
    }
}

#[test]
fn button_activates_with_enter_and_space() {
    let mut cx = TestAppContext::single();
    let (view, vcx) = cx.add_window_view(|_, _| KeyboardView::default());
    vcx.run_until_parked();
    vcx.update(|window, _| window.focus_next());
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(vcx.read(|cx| view.read(cx).clicks), 1);

    vcx.simulate_keystrokes("space");
    vcx.run_until_parked();
    assert_eq!(vcx.read(|cx| view.read(cx).clicks), 2);
}
