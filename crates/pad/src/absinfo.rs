//! Absolute axis ranges for sticks (0–255) and triggers (0–255).
//! Applied when creating the uinput device so emulators see DualSense-like ranges.

#[derive(Clone, Copy)]
pub struct AbsRange {
    pub min: i32,
    pub max: i32,
    pub fuzz: i32,
    pub flat: i32,
}

pub const STICK: AbsRange = AbsRange {
    min: 0,
    max: 255,
    fuzz: 0,
    flat: 15,
};

pub const TRIGGER: AbsRange = AbsRange {
    min: 0,
    max: 255,
    fuzz: 0,
    flat: 0,
};
