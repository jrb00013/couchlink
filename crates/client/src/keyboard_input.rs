//! Keyboard → PadFrame mapping, so a friend without a DualSense can still play.
//! Fixed layout (not remappable yet — keep it simple until someone asks):
//!
//! WASD          → left stick (digital: full deflection, no analog values)
//! Arrow keys     → D-pad
//! Space          → Cross
//! Left Shift     → Square
//! Left Ctrl      → Circle
//! E              → Triangle
//! Q / R          → L1 / R1
//! 1 / 2          → L2 / R2 (digital: 0 or 255)
//! Enter          → Options
//! Tab            → Create

use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;
use std::collections::HashSet;
use winit::keyboard::KeyCode;

const NEUTRAL: u8 = 127;
const FULL: u8 = 255;
const ZERO: u8 = 0;

#[derive(Default)]
pub struct KeyboardPad {
    held: HashSet<KeyCode>,
}

impl KeyboardPad {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_key(&mut self, code: KeyCode, pressed: bool) {
        if pressed {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
    }

    pub fn any_key_active(&self) -> bool {
        !self.held.is_empty()
    }

    pub fn to_pad_frame(&self, seq: u32) -> PadFrame {
        let h = &self.held;
        let mut buttons_mask = 0u32;

        let mut set = |cond: bool, bit: u32| {
            if cond {
                buttons_mask |= bit;
            }
        };
        set(h.contains(&KeyCode::Space), buttons::CROSS);
        set(h.contains(&KeyCode::ShiftLeft), buttons::SQUARE);
        set(h.contains(&KeyCode::ControlLeft), buttons::CIRCLE);
        set(h.contains(&KeyCode::KeyE), buttons::TRIANGLE);
        set(h.contains(&KeyCode::KeyQ), buttons::L1);
        set(h.contains(&KeyCode::KeyR), buttons::R1);
        set(h.contains(&KeyCode::Enter), buttons::OPTIONS);
        set(h.contains(&KeyCode::Tab), buttons::CREATE);
        set(h.contains(&KeyCode::ArrowUp), buttons::DPAD_UP);
        set(h.contains(&KeyCode::ArrowDown), buttons::DPAD_DOWN);
        set(h.contains(&KeyCode::ArrowLeft), buttons::DPAD_LEFT);
        set(h.contains(&KeyCode::ArrowRight), buttons::DPAD_RIGHT);

        let lx = if h.contains(&KeyCode::KeyA) {
            ZERO
        } else if h.contains(&KeyCode::KeyD) {
            FULL
        } else {
            NEUTRAL
        };
        let ly = if h.contains(&KeyCode::KeyW) {
            ZERO
        } else if h.contains(&KeyCode::KeyS) {
            FULL
        } else {
            NEUTRAL
        };
        let l2 = if h.contains(&KeyCode::Digit1) { FULL } else { ZERO };
        let r2 = if h.contains(&KeyCode::Digit2) { FULL } else { ZERO };

        PadFrame {
            seq,
            buttons: buttons_mask,
            lx,
            ly,
            rx: NEUTRAL,
            ry: NEUTRAL,
            l2,
            r2,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_keys_held_is_neutral() {
        let kp = KeyboardPad::new();
        let f = kp.to_pad_frame(1);
        assert_eq!(f.buttons, 0);
        assert_eq!(f.lx, NEUTRAL);
        assert_eq!(f.ly, NEUTRAL);
        assert!(!kp.any_key_active());
    }

    #[test]
    fn wasd_maps_to_left_stick() {
        let mut kp = KeyboardPad::new();
        kp.set_key(KeyCode::KeyD, true);
        kp.set_key(KeyCode::KeyS, true);
        let f = kp.to_pad_frame(1);
        assert_eq!(f.lx, FULL);
        assert_eq!(f.ly, FULL);
        assert!(kp.any_key_active());
    }

    #[test]
    fn space_maps_to_cross_button() {
        let mut kp = KeyboardPad::new();
        kp.set_key(KeyCode::Space, true);
        let f = kp.to_pad_frame(1);
        assert_eq!(f.buttons & buttons::CROSS, buttons::CROSS);
    }

    #[test]
    fn releasing_a_key_clears_it() {
        let mut kp = KeyboardPad::new();
        kp.set_key(KeyCode::Space, true);
        kp.set_key(KeyCode::Space, false);
        let f = kp.to_pad_frame(1);
        assert_eq!(f.buttons & buttons::CROSS, 0);
    }
}
