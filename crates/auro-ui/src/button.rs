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
    #[cfg(any(test, feature = "test-support"))]
    debug_selector: Option<String>,
}

impl Button {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Button {
            id: id.into(),
            label: label.into(),
            on_click: None,
            #[cfg(any(test, feature = "test-support"))]
            debug_selector: None,
        }
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Sets a debug selector visible to `VisualTestContext::debug_bounds` in tests.
    pub fn debug_selector(mut self, f: impl FnOnce() -> String) -> Self {
        self.debug_selector = Some(f());
        self
    }

    #[cfg(not(any(test, feature = "test-support")))]
    #[inline]
    pub fn debug_selector(self, _: impl FnOnce() -> String) -> Self {
        self
    }
}

impl IntoElement for Button {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        let mut element = div()
            .id(self.id)
            .focusable()
            .tab_stop(true)
            .tab_index(0)
            .px(super::tokens::space_4())
            .py(super::tokens::space_2())
            .rounded_md()
            .child(self.label);

        #[cfg(any(test, feature = "test-support"))]
        if let Some(selector) = self.debug_selector {
            element = element.debug_selector(move || selector.clone());
        }

        if let Some(handler) = self.on_click {
            let click_handler = handler.clone();
            element = element.on_click(move |event, window, cx| click_handler(event, window, cx));
            element = element.on_key_down(move |event, window, cx| {
                if event.is_held {
                    return;
                }
                if event.keystroke.modifiers == gpui::Modifiers::none()
                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                {
                    handler(&ClickEvent::default(), window, cx);
                }
            });
        }

        element.into_any_element()
    }
}
