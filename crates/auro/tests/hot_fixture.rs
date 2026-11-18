// SPDX-License-Identifier: MIT OR Apache-2.0

#![cfg(feature = "hot-reload")]

//! Headless fixture for the reload view contract.
//!
//! This does not load a patch library: it swaps the boundary implementation in a mounted view and
//! checks that the host-stable element tree renders the new output while opted-in state survives.
//! The real dx apply path is measured by the platform runs in spec 0004; this test pins the part
//! that can run without a display or a devserver.

use auro::prelude::*;
use auro_ui::gpui::{Context, ParentElement, Render, Styled, TestAppContext, Window, div};

#[derive(Clone, Debug, PartialEq)]
struct CounterState {
    count: u64,
}

/// Framework-free data returned by the boundary.
struct Frame {
    title: &'static str,
}

fn boundary_one() -> Frame {
    Frame { title: "one" }
}

fn boundary_two() -> Frame {
    Frame { title: "two" }
}

struct Fixture {
    boundary: fn() -> Frame,
    state: CounterState,
}

impl Render for Fixture {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let frame = auro::hot::hot_frame(self.boundary);
        auro::hot::rendered();
        let title = frame.title;
        div()
            .id("fixture")
            .debug_selector(move || format!("title-{title}"))
            .flex()
            .flex_col()
            .p_4()
            .child(format!("Clicked {} times", self.state.count))
    }
}

#[test]
fn headless_boundary_change_keeps_state_and_renders_new_output() {
    let mut cx = TestAppContext::single();
    let (view, vcx) = cx.add_window_view(|_, _| Fixture {
        boundary: boundary_one,
        state: CounterState { count: 3 },
    });
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("title-one").is_some(),
        "the first boundary output must be rendered"
    );

    vcx.update(|_, cx| {
        view.update(cx, |this, cx| {
            this.boundary = boundary_two;
            this.state.count += 1;
            cx.notify();
        });
    });
    vcx.run_until_parked();

    assert!(
        vcx.debug_bounds("title-two").is_some(),
        "the changed boundary output must be visible"
    );
    assert!(
        vcx.debug_bounds("title-one").is_none(),
        "the old boundary output must be gone"
    );
    assert_eq!(
        vcx.read(|cx| view.read(cx).state.count),
        4,
        "state must survive the boundary change"
    );
}
