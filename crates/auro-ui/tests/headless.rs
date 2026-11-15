// SPDX-License-Identifier: MIT OR Apache-2.0

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString, TestAppContext,
    Window, div,
};

struct ReadyView {
    status: SharedString,
}

impl Render for ReadyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("root")
            .debug_selector(|| "root".to_string())
            .child(self.status.clone())
    }
}

#[test]
fn root_view_renders_status_and_bounds() {
    let mut cx = TestAppContext::single();

    let (view, vcx) = cx.add_window_view(|_, _cx| ReadyView {
        status: "Ready".into(),
    });

    vcx.run_until_parked();

    let status = vcx.read(|cx| view.read(cx).status.clone());
    assert_eq!(status.as_ref(), "Ready");
    assert!(
        vcx.debug_bounds("root").is_some(),
        "the root element must be rendered with bounds"
    );
}
