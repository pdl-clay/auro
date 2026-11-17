// SPDX-License-Identifier: MIT OR Apache-2.0

//! The only crate that names GPUI: re-exports, primitives, tokens.

pub use gpui;
pub use gpui::{
    AnyWindowHandle, App, AppContext, Application, Bounds, Context, Entity, Global, IntoElement,
    ParentElement, Pixels, Render, SharedString, Size, Styled, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, div, px, rgb, size,
};

mod button;

pub use button::Button;

/// Spacing and radius, in one place.
pub mod tokens {
    use gpui::{Pixels, px};

    pub fn space_1() -> Pixels {
        px(4.0)
    }

    pub fn space_2() -> Pixels {
        px(8.0)
    }

    pub fn space_4() -> Pixels {
        px(16.0)
    }

    pub const RADIUS_MD: f32 = 6.0;
}

pub mod prelude {
    pub use gpui::prelude::*;
    pub use gpui::{
        Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px, rgb,
        size,
    };

    pub use crate::Button;
}
