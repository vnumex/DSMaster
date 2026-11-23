use gpui::{BoxShadow, Hsla, Rgba, point, px, rgb, rgba};

pub const FONT_SANS: &str = "DM Sans";
pub const FONT_MONO: &str = "JetBrains Mono";

// Quiet cool-graphite neutral ramp (surface steps + 7% hairlines, zero glow)
pub const RAIL_BG: u32 = 0x101214; // Far-left icon rail & custom frameless titlebar
pub const WELL_BG: u32 = 0x15171A; // Recessed instrument wells (numeric pills, graphs, trays)
pub const CANVAS_BG: u32 = 0x171A1C; // Open workspace canvas behind controller stage & cards
pub const DRAWER_BG: u32 = 0x1C1F23; // Left configuration drawer
pub const SURFACE: u32 = 0x222428; // Top bar & section cards
pub const SURFACE_ALT: u32 = 0x2A2D33; // Elevated buttons, device card, active nav row
pub const SURFACE_HOVER: u32 = 0x333740; // Hover & active segmented control fill
pub const BORDER: u32 = 0x2B2E36; // Quiet structural hairline (~7% contrast step)
pub const BORDER_STRONG: u32 = 0x3E434E; // Active control & rim hairline

// Three text roles from the graphite ramp (all >= 4.5:1 WCAG AA)
pub const TEXT: u32 = 0xF0F2F5;
pub const TEXT_SECONDARY: u32 = 0xA8B0BC;
pub const TEXT_MUTED: u32 = 0x828B99;

// Single primary hardware accent: PlayStation Cobalt Blue (WCAG AA on dark surfaces & solid CTAs)
pub const ACCENT: u32 = 0x5898FA; // Luminous cobalt for telemetry numerals, curves, gauges, checkmarks (5.5:1 on cards)
pub const ACCENT_SOLID: u32 = 0x2667D4; // Rich cobalt for solid primary CTA & enabled toggle track (5.3:1 with white ink)
pub const ACCENT_SOLID_HOVER: u32 = 0x3274E2;
pub const ACCENT_FG: u32 = 0xFFFFFF; // Crisp white ink on solid cobalt surfaces

// Calm secondary calibration tone (quiet silver-slate for anti-deadzone vs blue deadzone)
pub const ZONE_SECONDARY: u32 = 0x94A0B4;
pub const ZONE_BLUE: u32 = 0x3B82F6; // Native PlayStation lightbar default color

// Toned semantic colors (solid muted surfaces + quiet borders, zero neon rims)
pub const SUCCESS: u32 = 0x4CC38A;
pub const SUCCESS_SURFACE: u32 = 0x17241E;
pub const SUCCESS_SURFACE_HOVER: u32 = 0x1D2F27;
pub const SUCCESS_BORDER: u32 = 0x294B3B;

pub const WARN: u32 = 0xE5A83B;
pub const WARN_SURFACE: u32 = 0x272219;
pub const WARN_BORDER: u32 = 0x4D4026;

pub const DANGER: u32 = 0xE56B6F;
pub const DANGER_SURFACE: u32 = 0x251A1C;
pub const DANGER_SURFACE_HOVER: u32 = 0x332124;
pub const DANGER_BORDER: u32 = 0x4E2C30;

pub const CLOSE_HOVER: u32 = 0xC42B1C; // Native Windows close button hover

#[inline]
pub fn color(hex: u32) -> Rgba {
    rgb(hex)
}

#[inline]
pub fn color_alpha(hex: u32, alpha: u8) -> Rgba {
    rgba((hex << 8) | alpha as u32)
}

#[inline]
pub fn hsla_alpha(hex: u32, alpha: u8) -> Hsla {
    color_alpha(hex, alpha).into()
}

/// Subtle neutral elevation shadow reserved strictly for floating tooltip overlays
pub fn overlay_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla_alpha(0x000000, 0x48),
        offset: point(px(0.0), px(4.0)),
        blur_radius: px(12.0),
        spread_radius: px(0.0),
    }]
}

#[inline]
pub fn lerp_rgb(from: u32, to: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let r0 = ((from >> 16) & 0xFF) as f32;
    let g0 = ((from >> 8) & 0xFF) as f32;
    let b0 = (from & 0xFF) as f32;
    let r1 = ((to >> 16) & 0xFF) as f32;
    let g1 = ((to >> 8) & 0xFF) as f32;
    let b1 = (to & 0xFF) as f32;
    let r = (r0 + (r1 - r0) * t).round() as u32;
    let g = (g0 + (g1 - g0) * t).round() as u32;
    let b = (b0 + (b1 - b0) * t).round() as u32;
    (r << 16) | (g << 8) | b
}

#[inline]
pub fn rgb_to_u32(rgb: [u8; 3]) -> u32 {
    ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | (rgb[2] as u32)
}

pub fn rgb_to_hsv(rgb: [u8; 3]) -> (f32, f32, f32) {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let h = if delta < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if (max - g).abs() < 1e-6 {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let s = if max < 1e-6 { 0.0 } else { delta / max };
    (h.rem_euclid(360.0), s.clamp(0.0, 1.0), max.clamp(0.0, 1.0))
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(360.0);
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_rgb_round_trip_for_primaries_and_presets() {
        let samples = [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [0, 102, 255],
            [242, 208, 107],
            [148, 86, 245],
            [0, 0, 0],
            [255, 255, 255],
        ];
        for rgb in samples {
            let (h, s, v) = rgb_to_hsv(rgb);
            let back = hsv_to_rgb(h, s, v);
            for i in 0..3 {
                assert!(
                    (rgb[i] as i32 - back[i] as i32).abs() <= 1,
                    "round-trip mismatch for {rgb:?}: got {back:?}"
                );
            }
        }
    }
}
