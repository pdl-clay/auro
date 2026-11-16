// SPDX-License-Identifier: MIT OR Apache-2.0

use std::time::{Duration, Instant};

use auro::prelude::*;
use auro_ui::gpui::{
    Bounds, Context, InteractiveElement, Modifiers, ParentElement, Render, Styled, TestAppContext,
    Window, div,
};

mod services {
    use auro::prelude::*;

    #[auro::command]
    pub async fn increment(current: u64) -> Result<u64, AuroError> {
        Ok(current + 1)
    }
}

#[derive(Default)]
struct Counter {
    count: u64,
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("counter")
            .debug_selector(|| "counter".to_string())
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(format!("Clicked {} times", self.count))
            .child(
                Button::new("increment", "Increment")
                    .debug_selector(|| "increment".to_string())
                    .on_click(cx.listener(|this, _, _window, cx| {
                        let current = this.count;
                        cx.spawn(async move |this, cx| {
                            let next = services::increment(current).await?;
                            this.update(cx, |this, cx| {
                                this.count = next;
                                cx.notify();
                            })
                            .ok();
                            Ok::<(), AuroError>(())
                        })
                        .detach();
                    })),
            )
    }
}

#[test]
fn clicking_button_dispatches_async_command_and_updates_view() {
    let mut cx = TestAppContext::single();

    let (view, vcx) = cx.add_window_view(|_, _cx| Counter::default());
    vcx.run_until_parked();

    assert_eq!(vcx.read(|cx| view.read(cx).count), 0, "initial count");

    let bounds: Bounds<_> = vcx
        .debug_bounds("increment")
        .expect("increment button must be rendered with bounds");
    vcx.simulate_click(bounds.center(), Modifiers::none());

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        vcx.run_until_parked();
        let after = vcx.read(|cx| view.read(cx).count);
        if after == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for command result; count stayed {after}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        vcx.debug_bounds("counter").is_some(),
        "view remains rendered after update"
    );
}
