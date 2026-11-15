// SPDX-License-Identifier: MIT OR Apache-2.0

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The primitive the vertical slice needs; the component kit arrives in Phase 1.
pub struct Button {
    id: SharedString,
    label: SharedString,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Button {
            id: id.into(),
            label: label.into(),
            on_click: None,
        }
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl IntoElement for Button {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        let mut element = div()
            .id(self.id)
            .px(super::tokens::space_4())
            .py(super::tokens::space_2())
            .rounded_md()
            .child(self.label);

        if let Some(handler) = self.on_click {
            element = element.on_click(move |event, window, cx| handler(event, window, cx));
        }

        element.into_any_element()
    }
}
