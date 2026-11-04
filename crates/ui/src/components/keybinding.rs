use std::rc::Rc;

use crate::utils::capitalize;
use crate::{h_flex, prelude::*};
use gpui::{
    Action, AnyElement, App, FocusHandle, Global, IntoElement, KeybindingKeystroke, Keystroke,
    Modifiers, Window, relative,
};
use itertools::Itertools;

#[derive(Debug)]
enum Source {
    Action {
        action: Box<dyn Action>,
        focus_handle: Option<FocusHandle>,
    },
    Keystrokes {
        /// A keybinding consists of a set of keystrokes,
        /// where each keystroke is a key and a set of modifier keys.
        /// More than one keystroke produces a chord.
        ///
        /// This should always contain at least one keystroke.
        keystrokes: Rc<[KeybindingKeystroke]>,
    },
}

impl Clone for Source {
    fn clone(&self) -> Self {
        match self {
            Source::Action {
                action,
                focus_handle,
            } => Source::Action {
                action: action.boxed_clone(),
                focus_handle: focus_handle.clone(),
            },
            Source::Keystrokes { keystrokes } => Source::Keystrokes {
                keystrokes: keystrokes.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, IntoElement, RegisterComponent)]
pub struct KeyBinding {
    source: Source,
    size: Option<AbsoluteLength>,
    style: KeyBindingStyle,
    color: Option<Color>,
    /// Indicates whether the keybinding is currently disabled.
    disabled: bool,
}

/// Controls how a [`KeyBinding`] is presented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyBindingStyle {
    /// Render each key using compact key dimensions and muted colors.
    #[default]
    Default,
    /// Render keys and modifier symbols using the same metrics and colors as a [`Label`].
    Label,
}

struct KeyBindingVisibility(bool);
impl Global for KeyBindingVisibility {}

impl Default for KeyBindingVisibility {
    fn default() -> Self {
        Self(true)
    }
}

impl KeyBinding {
    /// Returns the highest precedence keybinding for an action. This is the last binding added to
    /// the keymap. User bindings are added after built-in bindings so that they take precedence.
    pub fn for_action(action: &dyn Action, cx: &App) -> Self {
        Self::new(action, None, cx)
    }

    /// Like `for_action`, but lets you specify the context from which keybindings are matched.
    pub fn for_action_in(action: &dyn Action, focus: &FocusHandle, cx: &App) -> Self {
        Self::new(action, Some(focus.clone()), cx)
    }
    pub fn has_binding(&self, window: &Window) -> bool {
        match &self.source {
            Source::Action {
                action,
                focus_handle: Some(focus),
            } => window
                .highest_precedence_binding_for_action_in(action.as_ref(), focus)
                .or_else(|| window.highest_precedence_binding_for_action(action.as_ref()))
                .is_some(),
            _ => false,
        }
    }

    /// Sets the application-wide visibility of keybindings.
    pub fn set_default_visibility(cx: &mut App, visible: bool) {
        cx.set_global(KeyBindingVisibility(visible));
        cx.refresh_windows();
    }

    /// Returns whether keybindings are visible application-wide.
    pub fn default_visibility(cx: &mut App) -> bool {
        cx.default_global::<KeyBindingVisibility>().0
    }

    pub fn new(action: &dyn Action, focus_handle: Option<FocusHandle>, _cx: &App) -> Self {
        Self {
            source: Source::Action {
                action: action.boxed_clone(),
                focus_handle,
            },
            size: None,
            style: KeyBindingStyle::Default,
            color: None,
            disabled: false,
        }
    }

    pub fn from_keystrokes(keystrokes: Rc<[KeybindingKeystroke]>) -> Self {
        Self {
            source: Source::Keystrokes { keystrokes },
            size: None,
            style: KeyBindingStyle::Default,
            color: None,
            disabled: false,
        }
    }

    /// Sets the size for this [`KeyBinding`].
    pub fn size(mut self, size: impl Into<AbsoluteLength>) -> Self {
        self.size = Some(size.into());
        self
    }

    /// Sets how this keybinding is presented.
    pub fn style(mut self, style: KeyBindingStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets the color used to render the keybinding.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Sets whether this keybinding is currently disabled.
    /// Disabled keybinds will be rendered in a dimmed state.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Resolves this keybinding's keystrokes the same way rendering does
    /// (matching the visible accelerator). Returns `None` when there is no
    /// binding.
    fn resolve_keystrokes(&self, window: &Window, cx: &App) -> Option<Vec<KeybindingKeystroke>> {
        let keystrokes = match &self.source {
            Source::Action {
                action,
                focus_handle,
            } => {
                let binding = focus_handle
                    .clone()
                    .or_else(|| window.focused(cx))
                    .and_then(|focus| {
                        window.highest_precedence_binding_for_action_in(action.as_ref(), &focus)
                    })
                    .or_else(|| window.highest_precedence_binding_for_action(action.as_ref()))?;
                binding.keystrokes().to_vec()
            }
            Source::Keystrokes { keystrokes } => keystrokes.to_vec(),
        };
        (!keystrokes.is_empty()).then_some(keystrokes)
    }

    /// Resolves this keybinding to a human-readable shortcut string in the same
    /// platform format shown to sighted users (e.g. `"Ctrl-S"`, `"Command-S"`,
    /// or `"Ctrl-K Ctrl-S"` for a chord), matching the visible accelerator. This
    /// is the format AccessKit's `keyboard_shortcut` property expects. Returns
    /// `None` when there is no binding.
    pub fn keyboard_shortcut_text(&self, window: &Window, cx: &App) -> Option<SharedString> {
        let keystrokes = self.resolve_keystrokes(window, cx)?;
        let text = keystrokes
            .iter()
            .map(|keystroke| keystroke_text(keystroke.modifiers(), keystroke.key()))
            .join(" ");
        Some(text.into())
    }
}

impl RenderOnce for KeyBinding {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !Self::default_visibility(cx) {
            return gpui::Empty.into_any_element();
        }

        let render_keybinding = |keystrokes: &[KeybindingKeystroke]| {
            let color = if self.disabled {
                Some(Color::Disabled)
            } else {
                self.color.or(match self.style {
                    KeyBindingStyle::Default => None,
                    KeyBindingStyle::Label => Some(Color::Default),
                })
            };

            h_flex()
                .debug_selector(|| {
                    format!(
                        "KEY_BINDING-{}",
                        keystrokes
                            .iter()
                            .map(|k| k.key().to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                })
                .gap(DynamicSpacing::Base04.rems(cx))
                .flex_none()
                .children(keystrokes.iter().map(|keystroke| {
                    h_flex()
                        .flex_none()
                        .when(self.style == KeyBindingStyle::Default, |this| this.py_0p5())
                        .rounded_xs()
                        .text_color(color.unwrap_or(Color::Muted).color(cx))
                        .children(render_keybinding_keystroke_with_style(
                            keystroke, color, self.size, self.style,
                        ))
                }))
                .into_any_element()
        };

        match self.source {
            Source::Action {
                action,
                focus_handle,
            } => focus_handle
                .or_else(|| window.focused(cx))
                .and_then(|focus| {
                    window.highest_precedence_binding_for_action_in(action.as_ref(), &focus)
                })
                .or_else(|| window.highest_precedence_binding_for_action(action.as_ref()))
                .map(|binding| render_keybinding(binding.keystrokes())),
            Source::Keystrokes { keystrokes } => Some(render_keybinding(keystrokes.as_ref())),
        }
        .unwrap_or_else(|| gpui::Empty.into_any_element())
    }
}

pub fn render_keybinding_keystroke(
    keystroke: &KeybindingKeystroke,
    color: Option<Color>,
    size: impl Into<Option<AbsoluteLength>>,
) -> Vec<AnyElement> {
    render_keybinding_keystroke_with_style(keystroke, color, size, KeyBindingStyle::Default)
}

fn render_keybinding_keystroke_with_style(
    keystroke: &KeybindingKeystroke,
    color: Option<Color>,
    size: impl Into<Option<AbsoluteLength>>,
    style: KeyBindingStyle,
) -> Vec<AnyElement> {
    let size = size.into();
    let text = keystroke_text(keystroke.modifiers(), keystroke.key());
    let element = match style {
        KeyBindingStyle::Default => Key::new(text, color).size(size).into_any_element(),
        KeyBindingStyle::Label => LabelKey::new(text, color).size(size).into_any_element(),
    };
    vec![element]
}

pub fn render_modifiers(
    modifiers: &Modifiers,
    color: Option<Color>,
    size: Option<AbsoluteLength>,
    trailing_separator: bool,
) -> impl Iterator<Item = AnyElement> {
    let keys = [
        (modifiers.function, "Fn"),
        (modifiers.control, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.platform, "Super"),
        (modifiers.shift, "Shift"),
    ]
    .into_iter()
    .filter_map(|(enabled, key)| enabled.then_some(Some(key)));

    itertools::intersperse(keys, None)
        .chain((modifiers.modified() && trailing_separator).then_some(None))
        .map(move |key| match key {
            Some(key) => Key::new(key, color).size(size).into_any_element(),
            None => "+".into_any_element(),
        })
}

#[derive(IntoElement)]
pub struct Key {
    key: SharedString,
    color: Option<Color>,
    size: Option<AbsoluteLength>,
}

impl RenderOnce for Key {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let single_char = self.key.len() == 1;
        let size = self
            .size
            .unwrap_or_else(|| TextSize::default().rems(cx).into());

        div()
            .py_0()
            .map(|this| {
                if single_char {
                    this.w(size).flex().flex_none().justify_center()
                } else {
                    this.px_0p5()
                }
            })
            .h(size)
            .text_size(size)
            .line_height(relative(1.))
            .text_color(self.color.unwrap_or(Color::Muted).color(cx))
            .child(self.key)
    }
}

impl Key {
    pub fn new(key: impl Into<SharedString>, color: Option<Color>) -> Self {
        Self {
            key: key.into(),
            color,
            size: None,
        }
    }

    pub fn size(mut self, size: impl Into<Option<AbsoluteLength>>) -> Self {
        self.size = size.into();
        self
    }
}

#[derive(IntoElement)]
struct LabelKey {
    key: SharedString,
    color: Option<Color>,
    size: Option<AbsoluteLength>,
}

impl RenderOnce for LabelKey {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let size = self
            .size
            .map(|size| LabelSize::Custom(size.to_rems(window.rem_size())))
            .unwrap_or_default();

        Label::new(self.key)
            .size(size)
            .color(self.color.unwrap_or(Color::Default))
    }
}

impl LabelKey {
    fn new(key: impl Into<SharedString>, color: Option<Color>) -> Self {
        Self {
            key: key.into(),
            color,
            size: None,
        }
    }

    fn size(mut self, size: impl Into<Option<AbsoluteLength>>) -> Self {
        self.size = size.into();
        self
    }
}

/// Returns a textual representation of the key binding for the given [`Action`].
pub fn text_for_action(action: &dyn Action, window: &Window, cx: &App) -> Option<String> {
    let key_binding = window.highest_precedence_binding_for_action(action)?;
    Some(text_for_keybinding_keystrokes(key_binding.keystrokes(), cx))
}

pub fn text_for_keystrokes(keystrokes: &[Keystroke], _cx: &App) -> String {
    keystrokes
        .iter()
        .map(|keystroke| keystroke_text(&keystroke.modifiers, &keystroke.key))
        .join(" ")
}

pub fn text_for_keybinding_keystrokes(keystrokes: &[KeybindingKeystroke], _cx: &App) -> String {
    keystrokes
        .iter()
        .map(|keystroke| keystroke_text(keystroke.modifiers(), keystroke.key()))
        .join(" ")
}

pub fn text_for_keystroke(modifiers: &Modifiers, key: &str, _cx: &App) -> String {
    keystroke_text(modifiers, key)
}

/// Returns a textual representation of the given [`Keystroke`].
fn keystroke_text(modifiers: &Modifiers, key: &str) -> String {
    let mut text = String::new();
    let delimiter = '-';

    if modifiers.function {
        text.push_str("Fn");
        text.push(delimiter);
    }

    if modifiers.control {
        text.push_str("Ctrl");
        text.push(delimiter);
    }

    if modifiers.platform {
        text.push_str("Super");
        text.push(delimiter);
    }

    if modifiers.alt {
        text.push_str("Alt");
        text.push(delimiter);
    }

    if modifiers.shift {
        text.push_str("Shift");
        text.push(delimiter);
    }

    let key = match key {
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        key => &capitalize(key),
    };
    text.push_str(key);

    text
}

impl Component for KeyBinding {
    fn scope() -> ComponentScope {
        ComponentScope::Typography
    }

    fn name() -> &'static str {
        "KeyBinding"
    }

    fn description() -> &'static str {
        "A component that displays a key binding."
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> AnyElement {
        fn keybinding(input: &str) -> KeyBinding {
            let keystrokes: Rc<[KeybindingKeystroke]> = input
                .split_whitespace()
                .filter_map(|chunk| Keystroke::parse(chunk).ok())
                .map(KeybindingKeystroke::from_keystroke)
                .collect::<Vec<_>>()
                .into();
            KeyBinding::from_keystrokes(keystrokes)
        }

        v_flex()
            .gap_6()
            .children(vec![example_group_with_title(
                "Key Bindings",
                vec![single_example(
                    "Save",
                    keybinding("ctrl-s").into_any_element(),
                )],
            )])
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_for_keystroke() {
        let keystroke = Keystroke::parse("cmd-c").unwrap();
        assert_eq!(
            keystroke_text(&keystroke.modifiers, &keystroke.key),
            "Super-C"
        );

        let keystroke = Keystroke::parse("ctrl-alt-delete").unwrap();
        assert_eq!(
            keystroke_text(&keystroke.modifiers, &keystroke.key),
            "Ctrl-Alt-Delete"
        );

        let keystroke = Keystroke::parse("shift-pageup").unwrap();
        assert_eq!(
            keystroke_text(&keystroke.modifiers, &keystroke.key),
            "Shift-PageUp"
        );
    }
}
