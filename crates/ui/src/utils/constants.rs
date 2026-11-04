use gpui::{Pixels, Window, px};

/// Returns the title bar height: 1.75 times the window's rem size, with a minimum of 34px.
pub fn platform_title_bar_height(window: &Window) -> Pixels {
    (1.75 * window.rem_size()).max(px(34.))
}
