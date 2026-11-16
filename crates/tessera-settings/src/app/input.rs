//! Wayland seat events folded into one Lens input snapshot per frame.

use lens::{Input, MouseButton};
use smithay_client_toolkit::seat::keyboard::{KeyEvent, Keysym, Modifiers};
use smithay_client_toolkit::seat::pointer::{AxisScroll, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};

/// Pixels one discrete wheel notch scrolls when the device reports only
/// notches.
const LINE_PIXELS: f32 = 48.0;

/// Accumulated seat state between two frames. Levels (cursor, held buttons,
/// modifiers) persist; edges (presses, releases, scroll, text, keys) are
/// consumed by [`InputState::take_frame`].
#[derive(Default)]
pub struct InputState {
    cursor: Option<(f32, f32)>,
    down: [bool; 3],
    pressed: [bool; 3],
    released: [bool; 3],
    scroll: (f32, f32),
    text: String,
    keys: Vec<(i32, bool, bool)>,
    mods: u32,
}

fn button_index(button: u32) -> Option<usize> {
    match button {
        BTN_LEFT => Some(0),
        BTN_RIGHT => Some(1),
        BTN_MIDDLE => Some(2),
        _ => None,
    }
}

const BUTTONS: [MouseButton; 3] = [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

impl InputState {
    pub fn pointer_moved(&mut self, position: (f64, f64)) {
        self.cursor = Some((position.0 as f32, position.1 as f32));
    }

    pub fn pointer_left(&mut self) {
        self.cursor = None;
        // A button held across the window edge must not stay latched.
        for index in 0..3 {
            if self.down[index] {
                self.down[index] = false;
                self.released[index] = true;
            }
        }
    }

    pub fn button(&mut self, button: u32, pressed: bool) {
        let Some(index) = button_index(button) else {
            return;
        };
        self.down[index] = pressed;
        if pressed {
            self.pressed[index] = true;
        } else {
            self.released[index] = true;
        }
    }

    pub fn axis(&mut self, horizontal: AxisScroll, vertical: AxisScroll) {
        let pixels = |axis: AxisScroll| {
            if axis.absolute != 0.0 {
                axis.absolute as f32
            } else {
                axis.discrete as f32 * LINE_PIXELS
            }
        };
        self.scroll.0 += pixels(horizontal);
        self.scroll.1 += pixels(vertical);
    }

    pub fn modifiers(&mut self, modifiers: Modifiers) {
        let mut mask = 0;
        if modifiers.shift {
            mask |= lens::mods::SHIFT;
        }
        if modifiers.ctrl {
            mask |= lens::mods::CTRL;
        }
        if modifiers.alt {
            mask |= lens::mods::ALT;
        }
        if modifiers.logo {
            mask |= lens::mods::SUPER;
        }
        self.mods = mask;
    }

    pub fn ctrl(&self) -> bool {
        self.mods & lens::mods::CTRL != 0
    }

    pub fn key(&mut self, event: &KeyEvent, pressed: bool, repeat: bool) {
        if let Some(key) = lens_key(event.keysym) {
            self.keys.push((key, pressed, repeat));
        }
        // Text enters through the layout commit path, never as shortcuts.
        if pressed
            && self.mods & (lens::mods::CTRL | lens::mods::ALT | lens::mods::SUPER) == 0
            && let Some(text) = &event.utf8
            && text.chars().all(|c| !c.is_control())
        {
            self.text.push_str(text);
        }
    }

    /// Whether anything happened that a frame must answer.
    pub fn has_edges(&self) -> bool {
        self.pressed.iter().any(|&p| p)
            || self.released.iter().any(|&r| r)
            || self.scroll != (0.0, 0.0)
            || !self.text.is_empty()
            || !self.keys.is_empty()
    }

    /// Build this frame's Lens input and reset the edges.
    pub fn take_frame(&mut self, size: (f32, f32), dt: f32) -> Input {
        let mut input = Input::new(size, dt);
        let (x, y) = self.cursor.unwrap_or((-10_000.0, -10_000.0));
        input.set_cursor(x, y);
        for (index, button) in BUTTONS.into_iter().enumerate() {
            input.set_mouse_down(button, self.down[index]);
            input.set_mouse_pressed(button, self.pressed[index]);
            input.set_mouse_released(button, self.released[index]);
        }
        if self.scroll != (0.0, 0.0) {
            input.set_scroll_pixels(self.scroll.0, self.scroll.1);
        }
        if !self.text.is_empty() {
            input.set_text(&self.text);
        }
        for &(key, pressed, repeat) in &self.keys {
            input.push_key(key, pressed, repeat);
        }
        input.set_mods(self.mods);
        self.pressed = [false; 3];
        self.released = [false; 3];
        self.scroll = (0.0, 0.0);
        self.text.clear();
        self.keys.clear();
        input
    }
}

/// Map a keysym onto Lens's key vocabulary: portable sentinels for editing
/// and navigation keys, unshifted ASCII for printable keys (so `ctrl+a`
/// arrives as `'a'`), nothing for the rest.
pub fn lens_key(keysym: Keysym) -> Option<i32> {
    use lens::key;
    let special = match keysym {
        Keysym::Escape => key::ESCAPE,
        Keysym::Tab | Keysym::ISO_Left_Tab => key::TAB,
        Keysym::Return | Keysym::KP_Enter => key::RETURN,
        Keysym::BackSpace => key::BACKSPACE,
        Keysym::Delete | Keysym::KP_Delete => key::DELETE,
        Keysym::Left | Keysym::KP_Left => key::LEFT,
        Keysym::Right | Keysym::KP_Right => key::RIGHT,
        Keysym::Up | Keysym::KP_Up => key::UP,
        Keysym::Down | Keysym::KP_Down => key::DOWN,
        Keysym::Home | Keysym::KP_Home => key::HOME,
        Keysym::End | Keysym::KP_End => key::END,
        Keysym::F1 => key::F1,
        Keysym::F2 => key::F2,
        Keysym::F3 => key::F3,
        Keysym::F4 => key::F4,
        Keysym::F5 => key::F5,
        Keysym::F6 => key::F6,
        Keysym::F7 => key::F7,
        Keysym::F8 => key::F8,
        Keysym::F9 => key::F9,
        Keysym::F10 => key::F10,
        Keysym::F11 => key::F11,
        Keysym::F12 => key::F12,
        other => {
            let raw = other.raw();
            return (0x20..=0x7e)
                .contains(&raw)
                .then(|| i32::from((raw as u8).to_ascii_lowercase()));
        }
    };
    Some(special)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_keys_map_to_portable_sentinels() {
        assert_eq!(lens_key(Keysym::Escape), Some(lens::key::ESCAPE));
        assert_eq!(lens_key(Keysym::KP_Enter), Some(lens::key::RETURN));
        assert_eq!(lens_key(Keysym::ISO_Left_Tab), Some(lens::key::TAB));
    }

    #[test]
    fn printable_keys_arrive_as_unshifted_ascii() {
        assert_eq!(lens_key(Keysym::a), Some(i32::from(b'a')));
        assert_eq!(lens_key(Keysym::A), Some(i32::from(b'a')));
        assert_eq!(lens_key(Keysym::comma), Some(i32::from(b',')));
        assert_eq!(lens_key(Keysym::Shift_L), None);
    }

    #[test]
    fn edges_are_consumed_and_levels_persist() {
        let mut state = InputState::default();
        state.pointer_moved((10.0, 20.0));
        state.button(BTN_LEFT, true);
        assert!(state.has_edges());
        let first = state.take_frame((100.0, 100.0), 1.0 / 60.0);
        assert!(first.as_raw().mouse_down[0]);
        assert!(!state.has_edges());
        let second = state.take_frame((100.0, 100.0), 1.0 / 60.0);
        assert!(second.as_raw().mouse_down[0], "a held button is a level");
        state.pointer_left();
        let third = state.take_frame((100.0, 100.0), 1.0 / 60.0);
        assert!(!third.as_raw().mouse_down[0]);
    }
}
