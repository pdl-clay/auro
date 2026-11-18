use auro::prelude::*;

#[derive(Default)]
struct Hello;

impl Render for Hello {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().gap_2().p_4().child("Hello from Auro")
    }
}

fn main() {
    App::new().root::<Hello>("main").run();
}
