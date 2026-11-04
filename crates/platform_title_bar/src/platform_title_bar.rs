pub mod platforms;

use gpui::{
    Action, AnyElement, App, Context, Decorations, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, StatefulInteractiveElement, Styled, Window, WindowButtonLayout,
    WindowControlArea, div,
};
use smallvec::SmallVec;
use std::mem;
use ui::{prelude::*, utils::platform_title_bar_height};

use crate::platforms::platform_linux;

/// With client-side decorations, the title bar is pulled over the window frame's
/// border to avoid a transparent gap in the rounded corners, and a border in the
/// title bar's own color insets its content back by the same amount. Headers that
/// render window controls in place of the title bar use this to apply the same
/// insets as padding, so the controls stay on the same pixel.
///
/// `pad_bottom` should be false when the header already has a bottom border of
/// its own occupying that space.
pub fn apply_title_bar_insets<E: Styled + FluentBuilder>(
    header: E,
    left_window_controls: bool,
    right_window_controls: bool,
    pad_bottom: bool,
) -> E {
    let inset = theme::CLIENT_SIDE_DECORATION_BORDER;
    header
        .mt(-inset)
        .pt(inset)
        .when(pad_bottom, |header| header.pb(inset))
        .when(left_window_controls, |header| header.pl(inset))
        .when(right_window_controls, |header| header.pr(inset))
}

pub struct PlatformTitleBar {
    id: ElementId,
    children: SmallVec<[AnyElement; 2]>,
    should_move: bool,
    button_layout: Option<WindowButtonLayout>,
}

impl PlatformTitleBar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            children: SmallVec::new(),
            should_move: false,
            button_layout: None,
        }
    }

    pub fn title_bar_color(&self, window: &mut Window, cx: &mut Context<Self>) -> Hsla {
        if window.is_window_active() && !self.should_move {
            cx.theme().colors().title_bar_background
        } else {
            cx.theme().colors().title_bar_inactive_background
        }
    }

    pub fn set_children<T>(&mut self, children: T)
    where
        T: IntoIterator<Item = AnyElement>,
    {
        self.children = children.into_iter().collect();
    }

    pub fn set_button_layout(&mut self, button_layout: Option<WindowButtonLayout>) {
        self.button_layout = button_layout;
    }

    fn effective_button_layout(
        &self,
        decorations: &Decorations,
        cx: &App,
    ) -> Option<WindowButtonLayout> {
        if matches!(decorations, Decorations::Client { .. }) {
            self.button_layout.or_else(|| cx.button_layout())
        } else {
            None
        }
    }
}

/// Renders the platform-appropriate left-side window controls (e.g. Ubuntu/GNOME close button).
///
/// Only relevant on Linux with client-side decorations when the window manager
/// places controls on the left.
pub fn render_left_window_controls(
    button_layout: Option<WindowButtonLayout>,
    close_action: Box<dyn Action>,
    window: &Window,
) -> Option<AnyElement> {
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return None;
    }
    let button_layout = button_layout?;
    if button_layout.left[0].is_none() {
        return None;
    }
    Some(
        platform_linux::LinuxWindowControls::new(
            "left-window-controls",
            button_layout.left,
            close_action,
        )
        .into_any_element(),
    )
}

/// Renders the platform-appropriate right-side window controls (close, minimize, maximize).
///
/// Returns `None` when the platform doesn't need custom controls (server-side decorations).
pub fn render_right_window_controls(
    button_layout: Option<WindowButtonLayout>,
    close_action: Box<dyn Action>,
    window: &Window,
) -> Option<AnyElement> {
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return None;
    }
    let button_layout = button_layout?;
    if button_layout.right[0].is_none() {
        return None;
    }
    Some(
        platform_linux::LinuxWindowControls::new(
            "right-window-controls",
            button_layout.right,
            close_action,
        )
        .into_any_element(),
    )
}

impl Render for PlatformTitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let supported_controls = window.window_controls();
        let decorations = window.window_decorations();
        let height = platform_title_bar_height(window);
        let titlebar_color = self.title_bar_color(window, cx);
        let close_action = Box::new(workspace::CloseWindow);
        let children = mem::take(&mut self.children);

        let button_layout = self.effective_button_layout(&decorations, cx);

        let title_bar = h_flex()
            .window_control_area(WindowControlArea::Drag)
            .w_full()
            .h(height)
            .map(|this| {
                this.on_mouse_down_out(cx.listener(move |this, _ev, _window, _cx| {
                    this.should_move = false;
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _ev, _window, _cx| {
                        this.should_move = false;
                    }),
                )
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _ev, _window, _cx| {
                        this.should_move = true;
                    }),
                )
                .on_mouse_move(cx.listener(move |this, _ev, window, _| {
                    if this.should_move {
                        this.should_move = false;
                        window.start_window_move();
                    }
                }))
            })
            .map(|this| {
                this.id(self.id.clone()).when(
                    supported_controls.maximize && window.is_resizable(),
                    |this| {
                        this.on_click(|event, window, _| {
                            if event.click_count() == 2 {
                                window.zoom_window();
                            }
                        })
                    },
                )
            })
            .map(|this| {
                if window.is_fullscreen() {
                    this.pl_2()
                } else if let Some(controls) = render_left_window_controls(
                    button_layout,
                    close_action.as_ref().boxed_clone(),
                    window,
                ) {
                    this.child(controls)
                } else {
                    this.pl_2()
                }
            })
            .map(|el| match decorations {
                Decorations::Server => el,
                Decorations::Client { tiling, .. } => el
                    .when(!(tiling.top || tiling.right), |el| {
                        el.rounded_tr(theme::CLIENT_SIDE_DECORATION_ROUNDING)
                    })
                    .when(!(tiling.top || tiling.left), |el| {
                        el.rounded_tl(theme::CLIENT_SIDE_DECORATION_ROUNDING)
                    })
                    // Keep in sync with `apply_title_bar_insets`.
                    .mt(-theme::CLIENT_SIDE_DECORATION_BORDER)
                    .mb(-theme::CLIENT_SIDE_DECORATION_BORDER)
                    .border(theme::CLIENT_SIDE_DECORATION_BORDER)
                    .border_color(titlebar_color),
            })
            .bg(titlebar_color)
            .content_stretch()
            .child(
                div()
                    .id(self.id.clone())
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .overflow_x_hidden()
                    .w_full()
                    .children(children),
            )
            .when(!window.is_fullscreen(), |title_bar| {
                let title_bar = title_bar.children(render_right_window_controls(
                    button_layout,
                    close_action.as_ref().boxed_clone(),
                    window,
                ));

                if matches!(decorations, Decorations::Client { .. }) {
                    title_bar.when(supported_controls.window_menu, |titlebar| {
                        titlebar.on_mouse_down(MouseButton::Right, move |ev, window, _| {
                            window.show_window_menu(ev.position)
                        })
                    })
                } else {
                    title_bar
                }
            });

        v_flex().w_full().child(title_bar)
    }
}

impl ParentElement for PlatformTitleBar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}
