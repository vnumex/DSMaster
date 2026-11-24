use gpui::{
    Background, Div, PathBuilder, Pixels, Point, Window, canvas, div, linear_color_stop,
    linear_gradient, point, prelude::*, px,
};

use super::dualsense_paths::{
    BODY_OUTER, BUMPER_L1, BUMPER_R1, CHASSIS_CENTER, CREATE_BTN, DPAD_DOWN, DPAD_LEFT, DPAD_RIGHT,
    DPAD_UP, DS_CENTER_X, DS_HEIGHT, DS_MIN_Y, MUTE_BTN, OPTIONS_BTN, Seg, TOUCHPAD, TRIGGER_L2,
    TRIGGER_R2, WING_LEFT, WING_RIGHT,
};
use super::theme::{
    ACCENT, ACCENT_SOLID, BORDER_STRONG, TEXT, TEXT_SECONDARY, WARN, WELL_BG, ZONE_BLUE,
    color, color_alpha, lerp_rgb,
};
use crate::engine::TelemetrySnapshot;
use crate::protocol::{DPad, DualSenseButtons};

const LOGICAL_W: f32 = 430.0;
const LOGICAL_H: f32 = 228.0;
const STAGE_CX: f32 = LOGICAL_W * 0.5;
const STAGE_TOP_PAD: f32 = 10.0;
const DS_STAGE_H: f32 = 208.0;
const DS_SCALE: f32 = DS_STAGE_H / DS_HEIGHT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightZone {
    None,
    Sticks,
    Triggers,
    Lighting,
    Buttons,
}

#[derive(Debug, Clone, Copy)]
pub struct ControllerPreviewState {
    pub snap: TelemetrySnapshot,
    pub override_lightbar: bool,
    pub lightbar_rgb: [u8; 3],
    pub player_leds: u8,
    pub show_raw_sticks: bool,
    pub highlight: HighlightZone,
    pub connected: bool,
}

#[derive(Debug, Clone, Copy)]
struct CanvasXform {
    ox: f32,
    oy: f32,
    s: f32,
}

impl CanvasXform {
    #[inline]
    fn from_bounds(bounds: gpui::Bounds<Pixels>, logical_w: f32, logical_h: f32) -> Self {
        let bw = f32::from(bounds.size.width).max(1.0);
        let bh = f32::from(bounds.size.height).max(1.0);
        let s = (bw / logical_w).min(bh / logical_h);
        let ox = f32::from(bounds.origin.x) + (bw - logical_w * s) * 0.5;
        let oy = f32::from(bounds.origin.y) + (bh - logical_h * s) * 0.5;
        Self { ox, oy, s }
    }

    /// Stage coordinates (`0..430`, `0..228`).
    #[inline]
    fn pt(&self, x: f32, y: f32) -> Point<Pixels> {
        point(px(self.ox + x * self.s), px(self.oy + y * self.s))
    }

    /// DualSense CAD coordinates (`0..912`, `92..732`) centered cleanly on the stage.
    #[inline]
    fn ds(&self, x: f32, y: f32) -> Point<Pixels> {
        let sx = STAGE_CX + (x - DS_CENTER_X) * DS_SCALE;
        let sy = STAGE_TOP_PAD + (y - DS_MIN_Y) * DS_SCALE;
        self.pt(sx, sy)
    }

    #[inline]
    fn len(&self, v: f32) -> Pixels {
        px((v * self.s).max(0.75))
    }

    #[inline]
    fn ds_len(&self, v: f32) -> Pixels {
        px((v * DS_SCALE * self.s).max(0.75))
    }
}

#[inline]
fn grad(angle_deg: f32, top_hex: u32, bottom_hex: u32) -> Background {
    linear_gradient(
        angle_deg,
        linear_color_stop(color(top_hex), 0.0),
        linear_color_stop(color(bottom_hex), 1.0),
    )
}

pub fn resolve_lightbar_preview(state: &ControllerPreviewState) -> Option<([u8; 3], bool)> {
    if !state.override_lightbar {
        return Some(([0, 112, 255], true));
    }
    let rgb = state.snap.active_lightbar_rgb.unwrap_or(state.lightbar_rgb);
    if rgb == [0, 0, 0] {
        None
    } else {
        Some((rgb, false))
    }
}

fn stick_offset(x: u8, y: u8, max_travel: f32) -> (f32, f32) {
    let nx = (x as f32 - 128.0) / 127.5;
    let ny = (y as f32 - 128.0) / 127.5;
    let mag = (nx * nx + ny * ny).sqrt();
    let (cx, cy) = if mag > 1.0 {
        (nx / mag, ny / mag)
    } else {
        (nx, ny)
    };
    (cx * max_travel, cy * max_travel)
}

fn add_segs(b: &mut PathBuilder, xf: &CanvasXform, segs: &[Seg], dx: f32, dy: f32) {
    for &seg in segs {
        match seg {
            Seg::M(x, y) => b.move_to(xf.ds(x + dx, y + dy)),
            Seg::L(x, y) => b.line_to(xf.ds(x + dx, y + dy)),
            Seg::C(c1x, c1y, c2x, c2y, ex, ey) => b.cubic_bezier_to(
                xf.ds(ex + dx, ey + dy),
                xf.ds(c1x + dx, c1y + dy),
                xf.ds(c2x + dx, c2y + dy),
            ),
            Seg::Z => b.close(),
        }
    }
}

fn add_circle_ds(b: &mut PathBuilder, xf: &CanvasXform, cx: f32, cy: f32, r: f32) {
    let k = 0.552_284_7 * r;
    b.move_to(xf.ds(cx, cy - r));
    b.cubic_bezier_to(
        xf.ds(cx + r, cy),
        xf.ds(cx + k, cy - r),
        xf.ds(cx + r, cy - k),
    );
    b.cubic_bezier_to(
        xf.ds(cx, cy + r),
        xf.ds(cx + r, cy + k),
        xf.ds(cx + k, cy + r),
    );
    b.cubic_bezier_to(
        xf.ds(cx - r, cy),
        xf.ds(cx - k, cy + r),
        xf.ds(cx - r, cy + k),
    );
    b.cubic_bezier_to(
        xf.ds(cx, cy - r),
        xf.ds(cx - r, cy - k),
        xf.ds(cx - k, cy - r),
    );
    b.close();
}

fn add_rounded_rect_abs(
    b: &mut PathBuilder,
    xf: &CanvasXform,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
) {
    let r = r.min(w * 0.5).min(h * 0.5);
    b.move_to(xf.pt(x + r, y));
    b.line_to(xf.pt(x + w - r, y));
    b.curve_to(xf.pt(x + w, y + r), xf.pt(x + w, y));
    b.line_to(xf.pt(x + w, y + h - r));
    b.curve_to(xf.pt(x + w - r, y + h), xf.pt(x + w, y + h));
    b.line_to(xf.pt(x + r, y + h));
    b.curve_to(xf.pt(x, y + h - r), xf.pt(x, y + h));
    b.line_to(xf.pt(x, y + r));
    b.curve_to(xf.pt(x + r, y), xf.pt(x, y));
    b.close();
}

fn add_rounded_rect_ds(
    b: &mut PathBuilder,
    xf: &CanvasXform,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
) {
    let r = r.min(w * 0.5).min(h * 0.5);
    b.move_to(xf.ds(x + r, y));
    b.line_to(xf.ds(x + w - r, y));
    b.curve_to(xf.ds(x + w, y + r), xf.ds(x + w, y));
    b.line_to(xf.ds(x + w, y + h - r));
    b.curve_to(xf.ds(x + w - r, y + h), xf.ds(x + w, y + h));
    b.line_to(xf.ds(x + r, y + h));
    b.curve_to(xf.ds(x, y + h - r), xf.ds(x, y + h));
    b.line_to(xf.ds(x, y + r));
    b.curve_to(xf.ds(x + r, y), xf.ds(x, y));
    b.close();
}

fn paint_fill(
    window: &mut Window,
    fill_color: impl Into<Background>,
    draw: impl FnOnce(&mut PathBuilder),
) {
    let mut b = PathBuilder::fill();
    draw(&mut b);
    if let Ok(path) = b.build() {
        window.paint_path(path, fill_color);
    }
}

fn paint_stroke(
    window: &mut Window,
    width: Pixels,
    stroke_color: impl Into<Background>,
    draw: impl FnOnce(&mut PathBuilder),
) {
    let mut b = PathBuilder::stroke(width);
    draw(&mut b);
    if let Ok(path) = b.build() {
        window.paint_path(path, stroke_color);
    }
}

/// Flanking vertical L2/R2 analog travel gauges.
fn paint_side_gauge(
    window: &mut Window,
    xf: &CanvasXform,
    is_right: bool,
    trigger_val: u8,
    highlighted: bool,
) {
    let x = if is_right { 398.0 } else { 20.0 };
    let y = 34.0;
    let w = 12.0;
    let h = 148.0;

    paint_fill(window, color(WELL_BG), |b| {
        add_rounded_rect_abs(b, xf, x, y, w, h, 5.0);
    });
    paint_stroke(
        window,
        xf.len(if highlighted { 1.4 } else { 1.0 }),
        color(if highlighted { ACCENT } else { BORDER_STRONG }),
        |b| add_rounded_rect_abs(b, xf, x, y, w, h, 5.0),
    );

    let tick_x1 = if is_right { x - 5.0 } else { x + w + 2.0 };
    let tick_x2 = tick_x1 + 3.0;
    paint_stroke(window, xf.len(0.9), color(BORDER_STRONG), |b| {
        for i in 0..=4 {
            let ty = y + 6.0 + (h - 12.0) * (i as f32 / 4.0);
            b.move_to(xf.pt(tick_x1, ty));
            b.line_to(xf.pt(tick_x2, ty));
        }
    });

    let ratio = (trigger_val as f32 / 255.0).clamp(0.0, 1.0);
    if ratio > 0.01 {
        let fh = (h - 4.0) * ratio;
        let fy = y + h - 2.0 - fh;
        paint_fill(window, color(ACCENT), |b| {
            add_rounded_rect_abs(b, xf, x + 2.0, fy, w - 4.0, fh.max(3.0), 3.0);
        });
    }
}

fn paint_trigger(
    window: &mut Window,
    xf: &CanvasXform,
    segs: &[Seg],
    analog: u8,
    highlighted: bool,
) {
    let ratio = (analog as f32 / 255.0).clamp(0.0, 1.0);
    // Physically depress trigger smoothly down into the shoulder housing as L2/R2 is pulled.
    let dy = ratio * 15.0;
    let top = lerp_rgb(0x363C48, ACCENT, ratio);
    let bot = lerp_rgb(0x181B22, ACCENT_SOLID, ratio);
    let stroke = if highlighted {
        ACCENT
    } else {
        lerp_rgb(0x4E5666, ACCENT, ratio)
    };

    paint_fill(window, grad(180.0, top, bot), |b| {
        add_segs(b, xf, segs, 0.0, dy);
    });
    paint_stroke(
        window,
        xf.ds_len(if highlighted { 3.2 } else { 2.2 }),
        color(stroke),
        |b| add_segs(b, xf, segs, 0.0, dy),
    );
}

fn paint_bumper(
    window: &mut Window,
    xf: &CanvasXform,
    segs: &[Seg],
    pressed: bool,
    highlighted: bool,
) {
    let dy = if pressed { 4.0 } else { 0.0 };
    let (top, bot) = if pressed {
        (ACCENT, ACCENT_SOLID)
    } else {
        (0x2F3440, 0x161920)
    };
    let border = if pressed || highlighted {
        ACCENT
    } else {
        0x4A5262
    };

    paint_fill(window, grad(180.0, top, bot), |b| {
        add_segs(b, xf, segs, 0.0, dy);
    });
    paint_stroke(
        window,
        xf.ds_len(if highlighted || pressed { 2.8 } else { 2.0 }),
        color(border),
        |b| add_segs(b, xf, segs, 0.0, dy),
    );
}

fn paint_dpad(
    window: &mut Window,
    xf: &CanvasXform,
    dpad: DPad,
    connected: bool,
    highlighted: bool,
) {
    let up = matches!(dpad, DPad::North | DPad::NorthEast | DPad::NorthWest);
    let down = matches!(dpad, DPad::South | DPad::SouthEast | DPad::SouthWest);
    let left = matches!(dpad, DPad::West | DPad::NorthWest | DPad::SouthWest);
    let right = matches!(dpad, DPad::East | DPad::NorthEast | DPad::SouthEast);

    let cx = 158.7;
    let cy = 295.5;

    // Subtle circular recessed dish in the satin shell around the D-pad
    paint_fill(window, color_alpha(0x0E1117, 0x10), |b| {
        add_circle_ds(b, xf, cx, cy, 69.0);
    });
    if highlighted {
        paint_stroke(window, xf.ds_len(3.2), color_alpha(ACCENT, 0x95), |b| {
            add_circle_ds(b, xf, cx, cy, 72.0);
        });
    }

    let (cap_top, cap_bot, cap_edge, arrow_rest) = if connected {
        (0xF1F4FA, 0xC6CFDE, 0x8F99AB, 0x626C7E)
    } else {
        (0xE2E7F0, 0xBCC5D5, 0x858FA1, 0x5B6576)
    };

    let arms: [(bool, &[Seg], [(f32, f32); 3]); 4] = [
        (
            up,
            DPAD_UP,
            [(158.7, 244.0), (164.8, 253.8), (152.6, 253.8)],
        ),
        (
            down,
            DPAD_DOWN,
            [(158.7, 347.0), (164.8, 337.2), (152.6, 337.2)],
        ),
        (
            left,
            DPAD_LEFT,
            [(107.2, 295.5), (117.0, 289.4), (117.0, 301.6)],
        ),
        (
            right,
            DPAD_RIGHT,
            [(210.2, 295.5), (200.4, 289.4), (200.4, 301.6)],
        ),
    ];

    for (pressed, segs, tri) in arms {
        let press_dy = if pressed { 1.8 } else { 0.0 };
        if !pressed {
            paint_fill(window, color_alpha(0x000000, 0x24), |b| {
                add_segs(b, xf, segs, 0.0, 2.8);
            });
        }
        let bg = if pressed {
            grad(170.0, ACCENT, ACCENT_SOLID)
        } else {
            grad(165.0, cap_top, cap_bot)
        };
        let border = if pressed { ACCENT } else { cap_edge };
        paint_fill(window, bg, |b| add_segs(b, xf, segs, 0.0, press_dy));
        paint_stroke(window, xf.ds_len(2.0), color(border), |b| {
            add_segs(b, xf, segs, 0.0, press_dy);
        });
        let arrow_col = if pressed { 0xFFFFFF } else { arrow_rest };
        paint_fill(window, color(arrow_col), |b| {
            b.move_to(xf.ds(tri[0].0, tri[0].1 + press_dy));
            b.line_to(xf.ds(tri[1].0, tri[1].1 + press_dy));
            b.line_to(xf.ds(tri[2].0, tri[2].1 + press_dy));
            b.close();
        });
    }
}

#[derive(Clone, Copy)]
enum FaceGlyph {
    Triangle,
    Circle,
    Cross,
    Square,
}

fn paint_face_button(
    window: &mut Window,
    xf: &CanvasXform,
    cx: f32,
    cy: f32,
    pressed: bool,
    connected: bool,
    glyph: FaceGlyph,
) {
    let r = 26.2;
    let press_dy = if pressed { 1.6 } else { 0.0 };
    let (cap_top, cap_bot, cap_edge, glyph_rest) = if connected {
        (0xF1F5FA, 0xC5CEDE, 0x8C96A8, 0x4C5566)
    } else {
        (0xE3E8F1, 0xBCC5D5, 0x848EA0, 0x525B6C)
    };

    if !pressed {
        paint_fill(window, color_alpha(0x000000, 0x26), |b| {
            add_circle_ds(b, xf, cx, cy + 2.8, r);
        });
    }

    let bg = if pressed {
        grad(165.0, ACCENT, ACCENT_SOLID)
    } else {
        grad(160.0, cap_top, cap_bot)
    };
    let border = if pressed { ACCENT } else { cap_edge };
    let glyph_col = if pressed { 0xFFFFFF } else { glyph_rest };

    paint_fill(window, bg, |b| add_circle_ds(b, xf, cx, cy + press_dy, r));
    // Crisp subtle top specular crescent on translucent crystal cap
    if !pressed {
        paint_stroke(window, xf.ds_len(1.8), color_alpha(0xFFFFFF, 0x85), |b| {
            b.move_to(xf.ds(cx - 14.5, cy - 15.5));
            b.cubic_bezier_to(
                xf.ds(cx + 14.5, cy - 15.5),
                xf.ds(cx - 6.0, cy - 23.0),
                xf.ds(cx + 6.0, cy - 23.0),
            );
        });
    }
    paint_stroke(window, xf.ds_len(2.1), color(border), |b| {
        add_circle_ds(b, xf, cx, cy + press_dy, r);
    });

    let gy = cy + press_dy;
    paint_stroke(window, xf.ds_len(3.5), color(glyph_col), |b| match glyph {
        FaceGlyph::Triangle => {
            b.move_to(xf.ds(cx, gy - 11.2));
            b.line_to(xf.ds(cx + 11.0, gy + 8.4));
            b.line_to(xf.ds(cx - 11.0, gy + 8.4));
            b.close();
        }
        FaceGlyph::Circle => {
            add_circle_ds(b, xf, cx, gy, 10.2);
        }
        FaceGlyph::Cross => {
            b.move_to(xf.ds(cx - 9.2, gy - 9.2));
            b.line_to(xf.ds(cx + 9.2, gy + 9.2));
            b.move_to(xf.ds(cx + 9.2, gy - 9.2));
            b.line_to(xf.ds(cx - 9.2, gy + 9.2));
        }
        FaceGlyph::Square => {
            b.move_to(xf.ds(cx - 8.8, gy - 8.8));
            b.line_to(xf.ds(cx + 8.8, gy - 8.8));
            b.line_to(xf.ds(cx + 8.8, gy + 8.8));
            b.line_to(xf.ds(cx - 8.8, gy + 8.8));
            b.close();
        }
    });
}

fn paint_stick(
    window: &mut Window,
    xf: &CanvasXform,
    cx: f32,
    cy: f32,
    stick_x: u8,
    stick_y: u8,
    pressed_l3_r3: bool,
    highlighted: bool,
) {
    let (dx, dy) = stick_offset(stick_x, stick_y, 20.0);
    let deflected = dx.hypot(dy) > 2.2;

    // Deep recessed circular socket well + machined concentric bezel ring
    paint_fill(window, color(0x0B0D11), |b| {
        add_circle_ds(b, xf, cx, cy, 59.5);
    });
    let well_ring = if highlighted {
        ACCENT
    } else if deflected {
        TEXT_SECONDARY
    } else {
        0x333946
    };
    paint_stroke(
        window,
        xf.ds_len(if highlighted { 3.6 } else { 2.4 }),
        color(well_ring),
        |b| add_circle_ds(b, xf, cx, cy, 59.5),
    );
    paint_stroke(window, xf.ds_len(1.8), color(0x1D212A), |b| {
        add_circle_ds(b, xf, cx, cy, 51.0);
    });

    let kx = cx + dx;
    let ky = cy + dy;

    // Sculpted shadow cast into socket cavity
    paint_fill(window, color_alpha(0x000000, 0x85), |b| {
        add_circle_ds(b, xf, kx + 2.0, ky + 6.0, 42.5);
    });

    // Outer convex matte-rubber thumbstick shoulder (top-lit gradient)
    let outer_bg = if pressed_l3_r3 {
        grad(165.0, ACCENT, ACCENT_SOLID)
    } else if deflected {
        grad(165.0, 0x454D5E, 0x20252F)
    } else {
        grad(165.0, 0x3D4352, 0x1B1F27)
    };
    let outer_ring = if pressed_l3_r3 || deflected || highlighted {
        ACCENT
    } else {
        0x596274
    };

    paint_fill(window, outer_bg, |b| add_circle_ds(b, xf, kx, ky, 42.5));
    paint_stroke(window, xf.ds_len(2.4), color(outer_ring), |b| {
        add_circle_ds(b, xf, kx, ky, 42.5);
    });

    // Concave inner thumb dish (inverted bottom-lit gradient creates authentic concave 3D depth)
    let inner_bg = if pressed_l3_r3 {
        grad(165.0, ACCENT_SOLID, ACCENT)
    } else {
        grad(165.0, 0x1D2129, 0x323847)
    };
    paint_fill(window, inner_bg, |b| add_circle_ds(b, xf, kx, ky, 27.5));
    paint_stroke(
        window,
        xf.ds_len(2.0),
        color(if pressed_l3_r3 { 0x8BB8FF } else { 0x181B22 }),
        |b| add_circle_ds(b, xf, kx, ky, 27.5),
    );
    // Subtle top specular highlight along the raised thumbstick lip
    if !pressed_l3_r3 {
        paint_stroke(window, xf.ds_len(1.8), color_alpha(0xFFFFFF, 0x36), |b| {
            b.move_to(xf.ds(kx - 22.0, ky - 28.0));
            b.cubic_bezier_to(
                xf.ds(kx + 22.0, ky - 28.0),
                xf.ds(kx - 9.0, ky - 38.0),
                xf.ds(kx + 9.0, ky - 38.0),
            );
        });
    }
}

/// Authentic sculpted PlayStation home emblem embedded directly in the obsidian center bridge.
fn paint_ps_emblem(window: &mut Window, xf: &CanvasXform, pressed: bool) {
    let cx = 455.5;
    let cy = 417.0;
    let col = if pressed { ACCENT } else { 0x7A8394 };

    if pressed {
        paint_fill(window, color_alpha(ACCENT, 0x26), |b| {
            add_circle_ds(b, xf, cx, cy, 24.0);
        });
    }

    // Upright sculpted P stem + angled top lobe
    paint_fill(window, color(col), |b| {
        b.move_to(xf.ds(cx - 4.0, cy - 17.5));
        b.line_to(xf.ds(cx + 3.2, cy - 15.2));
        b.cubic_bezier_to(
            xf.ds(cx + 15.0, cy - 3.5),
            xf.ds(cx + 13.0, cy - 12.2),
            xf.ds(cx + 15.8, cy - 8.2),
        );
        b.cubic_bezier_to(
            xf.ds(cx + 4.5, cy + 4.0),
            xf.ds(cx + 14.2, cy + 1.8),
            xf.ds(cx + 9.5, cy + 4.4),
        );
        b.line_to(xf.ds(cx + 4.5, cy - 2.2));
        b.cubic_bezier_to(
            xf.ds(cx + 8.5, cy - 5.5),
            xf.ds(cx + 7.2, cy - 2.2),
            xf.ds(cx + 8.8, cy - 3.6),
        );
        b.cubic_bezier_to(
            xf.ds(cx + 3.2, cy - 9.2),
            xf.ds(cx + 8.2, cy - 7.6),
            xf.ds(cx + 6.2, cy - 8.8),
        );
        b.line_to(xf.ds(cx + 3.2, cy + 16.0));
        b.line_to(xf.ds(cx - 4.0, cy + 13.8));
        b.close();
    });

    // Left sweeping perspective ribbon
    paint_fill(window, color(col), |b| {
        b.move_to(xf.ds(cx - 6.8, cy + 1.8));
        b.line_to(xf.ds(cx - 19.5, cy + 6.4));
        b.cubic_bezier_to(
            xf.ds(cx - 19.2, cy + 14.2),
            xf.ds(cx - 24.8, cy + 8.5),
            xf.ds(cx - 24.5, cy + 12.8),
        );
        b.cubic_bezier_to(
            xf.ds(cx - 6.8, cy + 11.4),
            xf.ds(cx - 14.6, cy + 15.2),
            xf.ds(cx - 10.2, cy + 13.0),
        );
        b.line_to(xf.ds(cx - 6.8, cy + 7.4));
        b.line_to(xf.ds(cx - 15.0, cy + 10.2));
        b.cubic_bezier_to(
            xf.ds(cx - 15.4, cy + 8.2),
            xf.ds(cx - 17.5, cy + 10.6),
            xf.ds(cx - 17.6, cy + 9.0),
        );
        b.line_to(xf.ds(cx - 6.8, cy + 5.2));
        b.close();
    });

    // Right sweeping perspective ribbon
    paint_fill(window, color(col), |b| {
        b.move_to(xf.ds(cx + 6.2, cy + 6.5));
        b.line_to(xf.ds(cx + 15.2, cy + 3.4));
        b.cubic_bezier_to(
            xf.ds(cx + 21.8, cy + 6.6),
            xf.ds(cx + 19.6, cy + 2.2),
            xf.ds(cx + 23.2, cy + 4.0),
        );
        b.cubic_bezier_to(
            xf.ds(cx + 15.8, cy + 11.6),
            xf.ds(cx + 20.6, cy + 9.0),
            xf.ds(cx + 18.6, cy + 10.6),
        );
        b.line_to(xf.ds(cx + 6.2, cy + 15.6));
        b.line_to(xf.ds(cx + 6.2, cy + 11.2));
        b.line_to(xf.ds(cx + 14.8, cy + 8.0));
        b.cubic_bezier_to(
            xf.ds(cx + 14.6, cy + 6.4),
            xf.ds(cx + 17.0, cy + 7.2),
            xf.ds(cx + 16.8, cy + 6.2),
        );
        b.line_to(xf.ds(cx + 6.2, cy + 9.8));
        b.close();
    });
}

fn paint_dualsense(
    window: &mut Window,
    bounds: gpui::Bounds<Pixels>,
    state: ControllerPreviewState,
) {
    let xf = CanvasXform::from_bounds(bounds, LOGICAL_W, LOGICAL_H);
    let snap = state.snap;
    let b = snap.buttons;
    let connected = state.connected;
    let is_edge = snap.is_edge;

    let hl_trig = state.highlight == HighlightZone::Triggers;
    let hl_sticks = state.highlight == HighlightZone::Sticks;
    let hl_light = state.highlight == HighlightZone::Lighting;
    let hl_btns = state.highlight == HighlightZone::Buttons;

    // 0. Flanking L2 / R2 analog travel gauges
    paint_side_gauge(window, &xf, false, snap.l2_analog, hl_trig);
    paint_side_gauge(window, &xf, true, snap.r2_analog, hl_trig);

    // 1. Multi-pass studio grounding shadow beneath DualSense silhouette
    for (dy, alpha) in [(26.0, 0x0A), (18.0, 0x10), (11.0, 0x17), (5.0, 0x1E)] {
        paint_fill(window, color_alpha(0x000000, alpha), |p| {
            add_segs(p, &xf, BODY_OUTER, 0.0, dy);
        });
    }

    // 2. Tucked contoured L2/R2 analog triggers + L1/R1 bumpers
    paint_trigger(window, &xf, TRIGGER_L2, snap.l2_analog, hl_trig);
    paint_trigger(window, &xf, TRIGGER_R2, snap.r2_analog, hl_trig);
    paint_bumper(
        window,
        &xf,
        BUMPER_L1,
        b.contains(DualSenseButtons::L1),
        hl_trig,
    );
    paint_bumper(
        window,
        &xf,
        BUMPER_R1,
        b.contains(DualSenseButtons::R1),
        hl_trig,
    );

    // 3. Deep matte obsidian structural underlayer (renders physical recessed panel seams!)
    paint_fill(window, color(0x111419), |p| {
        add_segs(p, &xf, BODY_OUTER, 0.0, 0.0);
    });

    // 4. Sweeping Matte Obsidian Center Chassis & Inner Grip Collar
    paint_fill(window, grad(180.0, 0x252933, 0x13161C), |p| {
        add_segs(p, &xf, CHASSIS_CENTER, 0.0, 0.0);
    });
    paint_stroke(window, xf.ds_len(1.8), color(0x2D333F), |p| {
        add_segs(p, &xf, CHASSIS_CENTER, 0.0, 0.0);
    });

    // 5. Iconic Two-Tone Satin Pearl Outer Wings (studio key light top-to-bottom gradient)
    let (wing_top, wing_bot, wing_edge) = if connected {
        (0xEAEFF6, 0xBFC7D6, 0x8D97AA)
    } else {
        (0xD8DFE9, 0xB2BBCB, 0x818B9E)
    };
    paint_fill(window, grad(165.0, wing_top, wing_bot), |p| {
        add_segs(p, &xf, WING_LEFT, 0.0, 0.0);
    });
    paint_fill(window, grad(195.0, wing_top, wing_bot), |p| {
        add_segs(p, &xf, WING_RIGHT, 0.0, 0.0);
    });

    // Subtle upper-shoulder specular rim highlight on both wings
    paint_stroke(
        window,
        xf.ds_len(2.2),
        color_alpha(0xFFFFFF, if connected { 0x75 } else { 0x4A }),
        |p| {
            p.move_to(xf.ds(104.0, 179.0));
            p.cubic_bezier_to(
                xf.ds(218.0, 162.0),
                xf.ds(140.0, 172.0),
                xf.ds(180.0, 165.0),
            );
            p.move_to(xf.ds(694.0, 162.0));
            p.cubic_bezier_to(
                xf.ds(808.0, 179.0),
                xf.ds(732.0, 165.0),
                xf.ds(772.0, 172.0),
            );
        },
    );

    paint_stroke(window, xf.ds_len(2.0), color(wing_edge), |p| {
        add_segs(p, &xf, WING_LEFT, 0.0, 0.0);
        add_segs(p, &xf, WING_RIGHT, 0.0, 0.0);
    });

    // 6. Live Grip Rumble Ripple Arcs outside vibrating handles
    if snap.rumble_large > 0 {
        let alpha = ((snap.rumble_large as f32 / 255.0) * 225.0).round() as u8;
        paint_stroke(window, xf.ds_len(5.5), color_alpha(WARN, alpha.max(70)), |p| {
            p.move_to(xf.ds(-14.0, 440.0));
            p.cubic_bezier_to(
                xf.ds(6.0, 680.0),
                xf.ds(-20.0, 535.0),
                xf.ds(-12.0, 622.0),
            );
        });
    }
    if snap.rumble_small > 0 {
        let alpha = ((snap.rumble_small as f32 / 255.0) * 225.0).round() as u8;
        paint_stroke(window, xf.ds_len(5.5), color_alpha(WARN, alpha.max(70)), |p| {
            p.move_to(xf.ds(926.0, 440.0));
            p.cubic_bezier_to(
                xf.ds(906.0, 680.0),
                xf.ds(932.0, 535.0),
                xf.ds(924.0, 622.0),
            );
        });
    }

    // 7. Recessed Lightbar Channels flanking the Touchpad (contained strictly inside bevel gap)
    let draw_lightbar_channels = |p: &mut PathBuilder| {
        p.move_to(xf.ds(249.5, 166.0));
        p.line_to(xf.ds(275.0, 296.0));
        p.cubic_bezier_to(
            xf.ds(310.0, 326.5),
            xf.ds(279.0, 314.0),
            xf.ds(291.0, 324.0),
        );
        p.move_to(xf.ds(656.5, 166.0));
        p.line_to(xf.ds(631.0, 296.0));
        p.cubic_bezier_to(
            xf.ds(596.0, 326.5),
            xf.ds(627.0, 314.0),
            xf.ds(615.0, 324.0),
        );
    };
    match resolve_lightbar_preview(&state) {
        Some(([r, g, b_ch], is_default)) => {
            let hex = ((r as u32) << 16) | ((g as u32) << 8) | (b_ch as u32);
            let strip_col = if is_default { ZONE_BLUE } else { hex };
            let outer_a = if connected { 0xC8 } else { 0x98 };
            let core_a = if connected { 0xFF } else { 0xD8 };
            // Diffused indirect strip strictly inside the recessed touchpad bevel gap
            paint_stroke(
                window,
                xf.ds_len(10.0),
                color_alpha(strip_col, outer_a),
                draw_lightbar_channels,
            );
            paint_stroke(
                window,
                xf.ds_len(4.8),
                color_alpha(strip_col, core_a),
                draw_lightbar_channels,
            );
        }
        None => {
            paint_stroke(window, xf.ds_len(5.0), color(0x363D4A), draw_lightbar_channels);
        }
    }

    // 8. Sculpted Wide Touchpad (Satin Pearl on Standard DualSense, Glossy Obsidian on DualSense Edge)
    let tp_pressed = b.contains(DualSenseButtons::TOUCHPAD);
    let (tp_top, tp_bot, tp_edge) = if is_edge {
        if tp_pressed {
            (0x313847, 0x1D222D, ACCENT)
        } else {
            (0x282D38, 0x161920, if hl_light { ACCENT } else { 0x4B5364 })
        }
    } else if tp_pressed {
        (0xD3DDF0, 0xB8C7E2, ACCENT)
    } else if connected {
        (0xE8EDF5, 0xCFD7E5, if hl_light { ACCENT } else { 0x97A1B3 })
    } else {
        (0xD7DEE8, 0xBEC7D6, if hl_light { ACCENT } else { 0x8791A4 })
    };

    paint_fill(window, grad(180.0, tp_top, tp_bot), |p| {
        add_segs(p, &xf, TOUCHPAD, 0.0, if tp_pressed { 1.4 } else { 0.0 });
    });
    paint_stroke(
        window,
        xf.ds_len(if hl_light || tp_pressed { 3.2 } else { 2.1 }),
        color(tp_edge),
        |p| add_segs(p, &xf, TOUCHPAD, 0.0, if tp_pressed { 1.4 } else { 0.0 }),
    );

    // Subtle micro-texture dot matrix across touchpad
    let dot_col = if is_edge {
        color_alpha(0xB8C4DA, 0x26)
    } else {
        color_alpha(0x475266, 0x1D)
    };
    paint_fill(window, dot_col, |p| {
        for row in 0..4 {
            for col in 0..9 {
                let gx = 324.0 + col as f32 * 33.0;
                let gy = 186.0 + row as f32 * 32.0;
                add_circle_ds(p, &xf, gx, gy, 1.5);
            }
        }
    });

    // Live multi-touch finger markers
    for tp in snap.touch {
        if tp.active {
            let tx = 286.0 + (tp.x as f32 / 1920.0).clamp(0.0, 1.0) * 336.0;
            let ty = 166.0 + (tp.y as f32 / 1080.0).clamp(0.0, 1.0) * 144.0;
            paint_stroke(window, xf.ds_len(3.2), color(ACCENT), |p| {
                add_circle_ds(p, &xf, tx, ty, 14.0);
            });
            paint_fill(window, color(ACCENT), |p| {
                add_circle_ds(p, &xf, tx, ty, 7.0);
            });
        }
    }

    // 9. 5 Player Indicator LEDs nestled right under the touchpad lower lip
    let led_mask = if state.override_lightbar {
        state.player_leds
    } else {
        0x04
    };
    for i in 0..5u8 {
        let lx = 405.5 + i as f32 * 25.0;
        let ly = 337.0;
        let lit = (led_mask & (1 << i)) != 0;
        if lit {
            paint_fill(
                window,
                color_alpha(TEXT, if connected { 0xFF } else { 0xCC }),
                |p| add_circle_ds(p, &xf, lx, ly, 4.2),
            );
        } else {
            paint_fill(window, color(0x3A414F), |p| {
                add_circle_ds(p, &xf, lx, ly, 3.1);
            });
        }
    }

    // Staggered 2-row speaker pinhole grille
    paint_fill(window, color(0x0A0C10), |p| {
        for i in 0..5 {
            add_circle_ds(p, &xf, 415.5 + i as f32 * 20.0, 357.0, 3.2);
        }
        for i in 0..4 {
            add_circle_ds(p, &xf, 425.5 + i as f32 * 20.0, 371.0, 3.2);
        }
    });
    paint_stroke(window, xf.ds_len(1.2), color(0x323846), |p| {
        for i in 0..5 {
            add_circle_ds(p, &xf, 415.5 + i as f32 * 20.0, 357.0, 3.2);
        }
        for i in 0..4 {
            add_circle_ds(p, &xf, 425.5 + i as f32 * 20.0, 371.0, 3.2);
        }
    });

    // 10. Angled Create & Options Pill Buttons + micro markings
    let (pill_top, pill_bot, pill_edge, mark_col) = if connected {
        (0xDFE5EF, 0xB5BFCE, 0x838E9F, 0x5A6476)
    } else {
        (0xD0D7E3, 0xAAB4C5, 0x7A8496, 0x555F70)
    };
    for (pressed, segs) in [
        (b.contains(DualSenseButtons::CREATE), CREATE_BTN),
        (b.contains(DualSenseButtons::OPTIONS), OPTIONS_BTN),
    ] {
        let bg = if pressed {
            grad(180.0, ACCENT, ACCENT_SOLID)
        } else {
            grad(180.0, pill_top, pill_bot)
        };
        let border = if pressed { ACCENT } else { pill_edge };
        paint_fill(window, bg, |p| add_segs(p, &xf, segs, 0.0, 0.0));
        paint_stroke(window, xf.ds_len(2.0), color(border), |p| {
            add_segs(p, &xf, segs, 0.0, 0.0);
        });
    }
    paint_stroke(window, xf.ds_len(2.3), color(mark_col), |p| {
        // Create 3-ray sunburst mark
        p.move_to(xf.ds(220.0, 182.0));
        p.line_to(xf.ds(216.5, 173.5));
        p.move_to(xf.ds(227.5, 180.5));
        p.line_to(xf.ds(227.5, 171.5));
        p.move_to(xf.ds(235.0, 182.0));
        p.line_to(xf.ds(238.5, 173.5));
        // Options 3-bar stack mark
        for dy in [172.5, 178.5, 184.5] {
            p.move_to(xf.ds(679.0, dy));
            p.line_to(xf.ds(693.0, dy));
        }
    });

    // 11. Crystal-Faceted Split D-Pad
    paint_dpad(window, &xf, snap.dpad, connected, hl_btns);

    // 12. PlayStation Crystal Face Buttons (Triangle, Circle, Cross, Square)
    if hl_btns {
        paint_stroke(window, xf.ds_len(3.2), color_alpha(ACCENT, 0x95), |p| {
            add_circle_ds(p, &xf, 751.6, 293.6, 90.0);
        });
    }
    paint_face_button(
        window,
        &xf,
        751.5,
        234.5,
        b.contains(DualSenseButtons::TRIANGLE),
        connected,
        FaceGlyph::Triangle,
    );
    paint_face_button(
        window,
        &xf,
        813.8,
        293.7,
        b.contains(DualSenseButtons::CIRCLE),
        connected,
        FaceGlyph::Circle,
    );
    paint_face_button(
        window,
        &xf,
        751.5,
        352.6,
        b.contains(DualSenseButtons::CROSS),
        connected,
        FaceGlyph::Cross,
    );
    paint_face_button(
        window,
        &xf,
        689.5,
        293.7,
        b.contains(DualSenseButtons::SQUARE),
        connected,
        FaceGlyph::Square,
    );

    // 13. Sculpted Concave Matte-Obsidian Analog Thumbsticks (respects Raw vs Processed toggle)
    let ls = if state.show_raw_sticks {
        snap.raw_left_stick
    } else {
        snap.left_stick
    };
    let rs = if state.show_raw_sticks {
        snap.raw_right_stick
    } else {
        snap.right_stick
    };
    paint_stick(
        window,
        &xf,
        295.5,
        420.0,
        ls[0],
        ls[1],
        b.contains(DualSenseButtons::L3),
        hl_sticks,
    );
    paint_stick(
        window,
        &xf,
        616.5,
        420.0,
        rs[0],
        rs[1],
        b.contains(DualSenseButtons::R3),
        hl_sticks,
    );

    // Subtle metallic Fn paddles beneath stick sockets when DualSense Edge is detected
    if is_edge {
        for (x, pressed) in [
            (276.0, b.contains(DualSenseButtons::EDGE_FN1) || b.contains(DualSenseButtons::EDGE_PADDLE_L)),
            (597.0, b.contains(DualSenseButtons::EDGE_FN2) || b.contains(DualSenseButtons::EDGE_PADDLE_R)),
        ] {
            let fill = if pressed {
                grad(180.0, ACCENT, ACCENT_SOLID)
            } else {
                grad(180.0, 0x454D5C, 0x242933)
            };
            let stroke_col = if pressed { ACCENT } else { 0x636C7E };
            paint_fill(window, fill, |p| {
                add_rounded_rect_ds(p, &xf, x, 502.0, 39.0, 14.0, 5.0);
            });
            paint_stroke(window, xf.ds_len(1.8), color(stroke_col), |p| {
                add_rounded_rect_ds(p, &xf, x, 502.0, 39.0, 14.0, 5.0);
            });
        }
    }

    // 14. Sculpted PlayStation Home Emblem + Mic Mute Bar
    paint_ps_emblem(window, &xf, b.contains(DualSenseButtons::PS_HOME));

    let mic_pressed = b.contains(DualSenseButtons::MIC_MUTE);
    paint_fill(
        window,
        color(if mic_pressed { WARN } else { 0x333946 }),
        |p| add_segs(p, &xf, MUTE_BTN, 0.0, 0.0),
    );
    paint_stroke(
        window,
        xf.ds_len(1.8),
        color(if mic_pressed { WARN } else { 0x525A6B }),
        |p| add_segs(p, &xf, MUTE_BTN, 0.0, 0.0),
    );
}

pub fn virtual_dualsense(state: ControllerPreviewState, height_px: f32) -> Div {
    div().w_full().h(px(height_px)).child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                paint_dualsense(window, bounds, state);
            },
        )
        .size_full(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lightbar_preview_respects_untouched_off_and_ds4_override() {
        let mut st = ControllerPreviewState {
            snap: TelemetrySnapshot::default(),
            override_lightbar: false,
            lightbar_rgb: [235, 56, 56],
            player_leds: 0x04,
            show_raw_sticks: false,
            highlight: HighlightZone::None,
            connected: true,
        };
        assert_eq!(resolve_lightbar_preview(&st), Some(([0, 112, 255], true)));

        st.override_lightbar = true;
        assert_eq!(resolve_lightbar_preview(&st), Some(([235, 56, 56], false)));

        st.snap.active_lightbar_rgb = Some([32, 200, 96]);
        assert_eq!(resolve_lightbar_preview(&st), Some(([32, 200, 96], false)));

        st.snap.active_lightbar_rgb = None;
        st.lightbar_rgb = [0, 0, 0];
        assert_eq!(resolve_lightbar_preview(&st), None);
    }

    #[test]
    fn stick_offset_clamps_radial_travel() {
        let (cx, cy) = stick_offset(128, 128, 20.0);
        assert!(cx.abs() < 0.05 && cy.abs() < 0.05);

        let (dx, dy) = stick_offset(255, 255, 20.0);
        assert!((dx.hypot(dy) - 20.0).abs() < 0.05);
    }
}
