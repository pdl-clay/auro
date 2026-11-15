use auro::prelude::*;

pub struct Counter {
    count: u64,
}

impl Default for Counter {
    fn default() -> Self {
        Counter { count: 0 }
    }
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(format!("Clicked {} times", self.count))
            .child(Button::new("increment", "Increment").on_click(cx.listener(
                |this, _, _window, cx| {
                    let current = this.count;
                    cx.spawn(async move |this, cx| {
                        let next = crate::services::increment(current).await?;
                        this.update(cx, |this, cx| {
                            this.count = next;
                            cx.notify();
                        })
                        .ok();
                        Ok::<(), AuroError>(())
                    })
                    .detach();
                },
            )))
    }
}
