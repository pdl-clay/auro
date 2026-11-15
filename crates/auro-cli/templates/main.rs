mod services;
mod views;

use auro::prelude::*;

fn main() {
    App::new().root::<views::Counter>("main").run();
}
