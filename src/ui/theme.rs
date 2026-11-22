use gpui::{Rgba, rgb};

pub const BG: u32 = 0x0D0F13;
pub const SURFACE: u32 = 0x14171E;
pub const SURFACE_ALT: u32 = 0x1A1E27;
pub const SURFACE_HOVER: u32 = 0x212633;
pub const BORDER: u32 = 0x262C3A;
pub const TEXT: u32 = 0xEEF2F6;
pub const TEXT_MUTED: u32 = 0x8A94A6;
pub const ACCENT: u32 = 0x3B82F6;
pub const WARN: u32 = 0xF59E0B;
pub const DANGER: u32 = 0xEF4444;

#[inline]
pub fn color(hex: u32) -> Rgba {
    rgb(hex)
}
