use gpui::{Rgba, rgb, rgba};

pub const FONT_SANS: &str = "Segoe UI";
pub const FONT_MONO: &str = "Cascadia Mono";

pub const BG: u32 = 0x09090B;
pub const SIDEBAR_BG: u32 = 0x0C0C0E;
pub const SURFACE: u32 = 0x0F0F12;
pub const SURFACE_ALT: u32 = 0x18181B;
pub const SURFACE_HOVER: u32 = 0x27272A;
pub const BORDER: u32 = 0x27272A;
pub const BORDER_STRONG: u32 = 0x3F3F46;
pub const TEXT: u32 = 0xFAFAFA;
pub const TEXT_MUTED: u32 = 0xA1A1AA;
pub const PRIMARY: u32 = 0xFAFAFA;
pub const PRIMARY_FG: u32 = 0x09090B;
pub const PRIMARY_HOVER: u32 = 0xE4E4E7;
pub const WARN: u32 = 0xEAB308;
pub const DANGER: u32 = 0xEF4444;

#[inline]
pub fn color(hex: u32) -> Rgba {
    rgb(hex)
}

#[inline]
pub fn color_alpha(hex: u32, alpha: u8) -> Rgba {
    rgba((hex << 8) | alpha as u32)
}
