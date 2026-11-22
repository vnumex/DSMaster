use gpui::{Div, PathBuilder, Pixels, Point, Window, canvas, div, point, prelude::*, px};

use super::theme::{
    BG, BORDER, BORDER_STRONG, PRIMARY, PRIMARY_FG, SURFACE_ALT, TEXT_MUTED, WARN, color,
    color_alpha,
};
use crate::engine::TelemetrySnapshot;
use crate::protocol::{DPad, DualSenseButtons};

const LOGICAL_W: f32 = 340.0;
const LOGICAL_H: f32 = 220.0;

#[derive(Debug, Clone, Copy)]
pub struct ControllerPreviewState {
    pub snap: TelemetrySnapshot,
    pub override_lightbar: bool,
    pub lightbar_rgb: [u8; 3],
    pub player_leds: u8,
}

#[derive(Debug, Clone, Copy)]
struct CanvasXform {
    ox: f32,
    oy: f32,
    s: f32,
}

impl CanvasXform {
    #[inline]
    fn from_bounds(bounds: gpui::Bounds<Pixels>) -> Self {
        let bw = f32::from(bounds.size.width).max(1.0);
        let bh = f32::from(bounds.size.height).max(1.0);
        let s = (bw / LOGICAL_W).min(bh / LOGICAL_H);
        let ox = f32::from(bounds.origin.x) + (bw - LOGICAL_W * s) * 0.5;
        let oy = f32::from(bounds.origin.y) + (bh - LOGICAL_H * s) * 0.5;
        Self { ox, oy, s }
    }

    #[inline]
    fn pt(&self, x: f32, y: f32) -> Point<Pixels> {
        point(px(self.ox + x * self.s), px(self.oy + y * self.s))
    }

    #[inline]
    fn len(&self, v: f32) -> Pixels {
        px((v * self.s).max(0.75))
    }
}

fn resolve_lightbar_preview(state: &ControllerPreviewState) -> Option<([u8; 3], bool)> {
    if !state.override_lightbar {
        return Some(([0, 102, 255], true));
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

fn add_circle(b: &mut PathBuilder, xf: &CanvasXform, cx: f32, cy: f32, r: f32) {
    let k = 0.552_284_7 * r;
    b.move_to(xf.pt(cx, cy - r));
    b.cubic_bezier_to(
        xf.pt(cx + r, cy),
        xf.pt(cx + k, cy - r),
        xf.pt(cx + r, cy - k),
    );
    b.cubic_bezier_to(
        xf.pt(cx, cy + r),
        xf.pt(cx + r, cy + k),
        xf.pt(cx + k, cy + r),
    );
    b.cubic_bezier_to(
        xf.pt(cx - r, cy),
        xf.pt(cx - k, cy + r),
        xf.pt(cx - r, cy + k),
    );
    b.cubic_bezier_to(
        xf.pt(cx, cy - r),
        xf.pt(cx - r, cy - k),
        xf.pt(cx - k, cy - r),
    );
    b.close();
}

fn add_rounded_rect(b: &mut PathBuilder, xf: &CanvasXform, x: f32, y: f32, w: f32, h: f32, r: f32) {
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

fn add_dualsense_body(b: &mut PathBuilder, xf: &CanvasXform) {
    b.move_to(xf.pt(106.0, 38.0));
    b.line_to(xf.pt(234.0, 38.0));
    b.cubic_bezier_to(xf.pt(294.0, 48.0), xf.pt(258.0, 38.0), xf.pt(280.0, 38.0));
    b.cubic_bezier_to(xf.pt(322.0, 140.0), xf.pt(308.0, 62.0), xf.pt(318.0, 100.0));
    b.cubic_bezier_to(
        xf.pt(310.0, 200.0),
        xf.pt(326.0, 174.0),
        xf.pt(320.0, 194.0),
    );
    b.cubic_bezier_to(
        xf.pt(284.0, 210.0),
        xf.pt(300.0, 208.0),
        xf.pt(292.0, 210.0),
    );
    b.cubic_bezier_to(
        xf.pt(242.0, 166.0),
        xf.pt(268.0, 206.0),
        xf.pt(252.0, 184.0),
    );
    b.cubic_bezier_to(
        xf.pt(216.0, 152.0),
        xf.pt(234.0, 152.0),
        xf.pt(224.0, 152.0),
    );
    b.line_to(xf.pt(124.0, 152.0));
    b.cubic_bezier_to(xf.pt(98.0, 166.0), xf.pt(116.0, 152.0), xf.pt(106.0, 152.0));
    b.cubic_bezier_to(xf.pt(56.0, 210.0), xf.pt(88.0, 184.0), xf.pt(72.0, 206.0));
    b.cubic_bezier_to(xf.pt(30.0, 200.0), xf.pt(48.0, 210.0), xf.pt(40.0, 208.0));
    b.cubic_bezier_to(xf.pt(18.0, 140.0), xf.pt(20.0, 194.0), xf.pt(14.0, 174.0));
    b.cubic_bezier_to(xf.pt(46.0, 48.0), xf.pt(22.0, 100.0), xf.pt(32.0, 62.0));
    b.cubic_bezier_to(xf.pt(106.0, 38.0), xf.pt(60.0, 38.0), xf.pt(82.0, 38.0));
    b.close();
}

fn paint_fill(
    window: &mut Window,
    fill_color: impl Into<gpui::Background>,
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
    stroke_color: impl Into<gpui::Background>,
    draw: impl FnOnce(&mut PathBuilder),
) {
    let mut b = PathBuilder::stroke(width);
    draw(&mut b);
    if let Ok(path) = b.build() {
        window.paint_path(path, stroke_color);
    }
}

fn lerp_rgb(from: u32, to: u32, t: f32) -> u32 {
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
fn mirror_x(x: f32, is_right: bool) -> f32 {
    if is_right { LOGICAL_W - x } else { x }
}

fn add_trigger_shape(b: &mut PathBuilder, xf: &CanvasXform, is_right: bool, dy: f32) {
    let mx = |x: f32| mirror_x(x, is_right);
    b.move_to(xf.pt(mx(57.0), 40.0));
    b.cubic_bezier_to(
        xf.pt(mx(63.0), 17.0 + dy),
        xf.pt(mx(58.0), 29.0 + dy),
        xf.pt(mx(60.0), 21.0 + dy),
    );
    b.cubic_bezier_to(
        xf.pt(mx(89.0), 15.0 + dy),
        xf.pt(mx(69.0), 13.0 + dy),
        xf.pt(mx(83.0), 13.0 + dy),
    );
    b.cubic_bezier_to(
        xf.pt(mx(95.0), 36.0),
        xf.pt(mx(93.0), 18.0 + dy),
        xf.pt(mx(94.5), 27.0 + dy),
    );
    b.close();
}

fn paint_trigger(window: &mut Window, xf: &CanvasXform, is_right: bool, analog: u8) {
    let ratio = (analog as f32 / 255.0).clamp(0.0, 1.0);
    let dy = ratio * 3.5;
    let fill = lerp_rgb(SURFACE_ALT, PRIMARY, ratio);
    let border = lerp_rgb(BORDER_STRONG, PRIMARY, ratio);

    paint_fill(window, color(fill), |b| {
        add_trigger_shape(b, xf, is_right, dy);
    });
    paint_stroke(window, xf.len(1.1), color(border), |b| {
        add_trigger_shape(b, xf, is_right, dy);
    });
}

fn add_bumper_shape(b: &mut PathBuilder, xf: &CanvasXform, is_right: bool, dy: f32) {
    let mx = |x: f32| mirror_x(x, is_right);
    b.move_to(xf.pt(mx(46.0), 50.0));
    b.line_to(xf.pt(mx(48.0), 42.0 + dy));
    b.cubic_bezier_to(
        xf.pt(mx(102.0), 30.0 + dy),
        xf.pt(mx(58.0), 33.0 + dy),
        xf.pt(mx(82.0), 29.5 + dy),
    );
    b.curve_to(xf.pt(mx(106.0), 35.0 + dy), xf.pt(mx(106.0), 30.5 + dy));
    b.line_to(xf.pt(mx(106.0), 43.0));
    b.close();
}

fn paint_bumper(window: &mut Window, xf: &CanvasXform, is_right: bool, pressed: bool) {
    let dy = if pressed { 1.6 } else { 0.0 };
    let fill = if pressed { PRIMARY } else { 0x1C1C21 };
    let border = if pressed { PRIMARY } else { BORDER_STRONG };

    paint_fill(window, color(fill), |b| {
        add_bumper_shape(b, xf, is_right, dy);
    });
    paint_stroke(window, xf.len(1.1), color(border), |b| {
        add_bumper_shape(b, xf, is_right, dy);
    });
}

fn paint_dpad(window: &mut Window, xf: &CanvasXform, dpad: DPad) {
    let up = matches!(dpad, DPad::North | DPad::NorthEast | DPad::NorthWest);
    let down = matches!(dpad, DPad::South | DPad::SouthEast | DPad::SouthWest);
    let left = matches!(dpad, DPad::West | DPad::NorthWest | DPad::SouthWest);
    let right = matches!(dpad, DPad::East | DPad::NorthEast | DPad::SouthEast);

    let cx = 76.0;
    let cy = 86.0;

    let arms: [(bool, [(f32, f32); 5]); 4] = [
        (
            up,
            [
                (cx - 6.5, cy - 19.0),
                (cx + 6.5, cy - 19.0),
                (cx + 6.5, cy - 7.0),
                (cx, cy - 2.0),
                (cx - 6.5, cy - 7.0),
            ],
        ),
        (
            down,
            [
                (cx - 6.5, cy + 7.0),
                (cx, cy + 2.0),
                (cx + 6.5, cy + 7.0),
                (cx + 6.5, cy + 19.0),
                (cx - 6.5, cy + 19.0),
            ],
        ),
        (
            left,
            [
                (cx - 19.0, cy - 6.5),
                (cx - 7.0, cy - 6.5),
                (cx - 2.0, cy),
                (cx - 7.0, cy + 6.5),
                (cx - 19.0, cy + 6.5),
            ],
        ),
        (
            right,
            [
                (cx + 7.0, cy - 6.5),
                (cx + 19.0, cy - 6.5),
                (cx + 19.0, cy + 6.5),
                (cx + 7.0, cy + 6.5),
                (cx + 2.0, cy),
            ],
        ),
    ];

    for (pressed, pts) in arms {
        let fill = if pressed { PRIMARY } else { 0x1F1F24 };
        let stroke = if pressed { PRIMARY } else { BORDER_STRONG };
        paint_fill(window, color(fill), |b| {
            b.move_to(xf.pt(pts[0].0, pts[0].1));
            for &(px_, py_) in &pts[1..] {
                b.line_to(xf.pt(px_, py_));
            }
            b.close();
        });
        paint_stroke(window, xf.len(1.0), color(stroke), |b| {
            b.move_to(xf.pt(pts[0].0, pts[0].1));
            for &(px_, py_) in &pts[1..] {
                b.line_to(xf.pt(px_, py_));
            }
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
    glyph: FaceGlyph,
) {
    let r = 9.5;
    let fill = if pressed { PRIMARY } else { SURFACE_ALT };
    let border = if pressed { PRIMARY } else { BORDER_STRONG };
    let glyph_col = if pressed { PRIMARY_FG } else { TEXT_MUTED };

    paint_fill(window, color(fill), |b| add_circle(b, xf, cx, cy, r));
    paint_stroke(window, xf.len(1.0), color(border), |b| {
        add_circle(b, xf, cx, cy, r)
    });

    paint_stroke(window, xf.len(1.3), color(glyph_col), |b| match glyph {
        FaceGlyph::Triangle => {
            b.move_to(xf.pt(cx, cy - 4.2));
            b.line_to(xf.pt(cx + 4.0, cy + 3.2));
            b.line_to(xf.pt(cx - 4.0, cy + 3.2));
            b.close();
        }
        FaceGlyph::Circle => {
            add_circle(b, xf, cx, cy, 3.8);
        }
        FaceGlyph::Cross => {
            b.move_to(xf.pt(cx - 3.4, cy - 3.4));
            b.line_to(xf.pt(cx + 3.4, cy + 3.4));
            b.move_to(xf.pt(cx + 3.4, cy - 3.4));
            b.line_to(xf.pt(cx - 3.4, cy + 3.4));
        }
        FaceGlyph::Square => {
            b.move_to(xf.pt(cx - 3.3, cy - 3.3));
            b.line_to(xf.pt(cx + 3.3, cy - 3.3));
            b.line_to(xf.pt(cx + 3.3, cy + 3.3));
            b.line_to(xf.pt(cx - 3.3, cy + 3.3));
            b.close();
        }
    });
}

fn paint_stick(
    window: &mut Window,
    xf: &CanvasXform,
    cx: f32,
    cy: f32,
    raw_x: u8,
    raw_y: u8,
    pressed_l3_r3: bool,
) {
    paint_fill(window, color(BG), |b| add_circle(b, xf, cx, cy, 20.5));
    paint_stroke(window, xf.len(1.1), color(BORDER), |b| {
        add_circle(b, xf, cx, cy, 20.5)
    });

    let (dx, dy) = stick_offset(raw_x, raw_y, 7.5);
    let kx = cx + dx;
    let ky = cy + dy;

    let cap_fill = if pressed_l3_r3 { PRIMARY } else { 0x1E1E24 };
    let cap_ring = if pressed_l3_r3 { PRIMARY_FG } else { 0x52525B };

    paint_fill(window, color(cap_fill), |b| add_circle(b, xf, kx, ky, 12.8));
    paint_stroke(window, xf.len(1.3), color(cap_ring), |b| {
        add_circle(b, xf, kx, ky, 12.8)
    });
    paint_stroke(window, xf.len(0.9), color(cap_ring), |b| {
        add_circle(b, xf, kx, ky, 8.2)
    });
}

fn paint_dualsense(
    window: &mut Window,
    bounds: gpui::Bounds<Pixels>,
    state: ControllerPreviewState,
) {
    let xf = CanvasXform::from_bounds(bounds);
    let snap = state.snap;
    let b = snap.buttons;

    // 1. L2 & R2 contoured analog triggers + L1 & R1 contoured shoulder bumpers
    paint_trigger(window, &xf, false, snap.l2_analog);
    paint_trigger(window, &xf, true, snap.r2_analog);
    paint_bumper(window, &xf, false, b.contains(DualSenseButtons::L1));
    paint_bumper(window, &xf, true, b.contains(DualSenseButtons::R1));

    // 2. DualSense shell + lower V-trim
    paint_fill(window, color(0x141418), |p| add_dualsense_body(p, &xf));
    paint_stroke(window, xf.len(1.5), color(BORDER_STRONG), |p| {
        add_dualsense_body(p, &xf)
    });

    paint_fill(window, color(0x0B0B0E), |p| {
        p.move_to(xf.pt(96.0, 126.0));
        p.cubic_bezier_to(
            xf.pt(148.0, 152.0),
            xf.pt(108.0, 150.0),
            xf.pt(130.0, 152.0),
        );
        p.line_to(xf.pt(192.0, 152.0));
        p.cubic_bezier_to(
            xf.pt(244.0, 126.0),
            xf.pt(210.0, 152.0),
            xf.pt(232.0, 150.0),
        );
        p.line_to(xf.pt(226.0, 152.0));
        p.line_to(xf.pt(114.0, 152.0));
        p.close();
    });
    paint_stroke(window, xf.len(1.0), color(BORDER), |p| {
        p.move_to(xf.pt(98.0, 118.0));
        p.cubic_bezier_to(
            xf.pt(128.0, 150.0),
            xf.pt(102.0, 144.0),
            xf.pt(116.0, 150.0),
        );
        p.line_to(xf.pt(224.0, 150.0));
        p.cubic_bezier_to(
            xf.pt(242.0, 118.0),
            xf.pt(234.0, 150.0),
            xf.pt(238.0, 144.0),
        );
    });

    // 3. Rumble arcs on grips when active
    if snap.rumble_large > 0 {
        let alpha = ((snap.rumble_large as f32 / 255.0) * 200.0).round() as u8;
        paint_stroke(window, xf.len(2.2), color_alpha(WARN, alpha.max(40)), |p| {
            p.move_to(xf.pt(26.0, 150.0));
            p.cubic_bezier_to(xf.pt(33.0, 194.0), xf.pt(24.0, 174.0), xf.pt(28.0, 188.0));
        });
    }
    if snap.rumble_small > 0 {
        let alpha = ((snap.rumble_small as f32 / 255.0) * 200.0).round() as u8;
        paint_stroke(window, xf.len(2.2), color_alpha(WARN, alpha.max(40)), |p| {
            p.move_to(xf.pt(314.0, 150.0));
            p.cubic_bezier_to(
                xf.pt(307.0, 194.0),
                xf.pt(316.0, 174.0),
                xf.pt(312.0, 188.0),
            );
        });
    }

    // 4. Trapezoidal Touchpad + live multi-touch dots
    let tp_pressed = b.contains(DualSenseButtons::TOUCHPAD);
    let tp_fill = if tp_pressed { 0x2E2E36 } else { SURFACE_ALT };
    let tp_border = if tp_pressed { PRIMARY } else { BORDER_STRONG };
    paint_fill(window, color(tp_fill), |p| {
        p.move_to(xf.pt(115.0, 38.0));
        p.line_to(xf.pt(225.0, 38.0));
        p.line_to(xf.pt(217.0, 91.0));
        p.line_to(xf.pt(123.0, 91.0));
        p.close();
    });
    paint_stroke(window, xf.len(1.1), color(tp_border), |p| {
        p.move_to(xf.pt(115.0, 38.0));
        p.line_to(xf.pt(225.0, 38.0));
        p.line_to(xf.pt(217.0, 91.0));
        p.line_to(xf.pt(123.0, 91.0));
        p.close();
    });

    for tp in snap.touch {
        if tp.active {
            let tx = 126.0 + (tp.x as f32 / 1920.0).clamp(0.0, 1.0) * 88.0;
            let ty = 42.0 + (tp.y as f32 / 1080.0).clamp(0.0, 1.0) * 45.0;
            paint_fill(window, color(PRIMARY), |p| add_circle(p, &xf, tx, ty, 3.2));
        }
    }

    // 5. Lightbar RGB strips + ambient glow
    match resolve_lightbar_preview(&state) {
        Some(([r, g, b_ch], is_default)) => {
            let hex = ((r as u32) << 16) | ((g as u32) << 8) | (b_ch as u32);
            let (glow_a, mid_a, core_a) = if is_default {
                (0x18, 0x32, 0x96)
            } else {
                (0x2A, 0x5E, 0xFF)
            };
            let draw_strips = |p: &mut PathBuilder| {
                p.move_to(xf.pt(111.5, 41.0));
                p.line_to(xf.pt(119.5, 90.0));
                p.move_to(xf.pt(228.5, 41.0));
                p.line_to(xf.pt(220.5, 90.0));
            };
            paint_stroke(window, xf.len(8.0), color_alpha(hex, glow_a), draw_strips);
            paint_stroke(window, xf.len(4.5), color_alpha(hex, mid_a), draw_strips);
            paint_stroke(window, xf.len(2.2), color_alpha(hex, core_a), draw_strips);
        }
        None => {
            paint_stroke(window, xf.len(2.0), color(BORDER), |p| {
                p.move_to(xf.pt(111.5, 41.0));
                p.line_to(xf.pt(119.5, 90.0));
                p.move_to(xf.pt(228.5, 41.0));
                p.line_to(xf.pt(220.5, 90.0));
            });
        }
    }

    // 6. 5 Player Indicator LEDs
    let led_mask = if state.override_lightbar {
        state.player_leds
    } else {
        0x04
    };
    for i in 0..5u8 {
        let lx = 150.0 + i as f32 * 10.0;
        let ly = 97.0;
        let lit = (led_mask & (1 << i)) != 0;
        if lit {
            paint_fill(window, color_alpha(PRIMARY, 0x38), |p| {
                add_circle(p, &xf, lx, ly, 4.0)
            });
            paint_fill(window, color(PRIMARY), |p| add_circle(p, &xf, lx, ly, 1.9));
        } else {
            paint_fill(window, color(BORDER), |p| add_circle(p, &xf, lx, ly, 1.7));
        }
    }

    // 7. Create & Options buttons
    for (pressed, x) in [
        (b.contains(DualSenseButtons::CREATE), 98.0),
        (b.contains(DualSenseButtons::OPTIONS), 235.0),
    ] {
        let fill = if pressed { PRIMARY } else { BORDER };
        paint_fill(window, color(fill), |p| {
            add_rounded_rect(p, &xf, x, 46.0, 7.0, 13.0, 3.5);
        });
    }

    // 8. Split D-Pad
    paint_dpad(window, &xf, snap.dpad);

    // 9. Face buttons (Triangle, Circle, Cross, Square)
    paint_face_button(
        window,
        &xf,
        264.0,
        66.0,
        b.contains(DualSenseButtons::TRIANGLE),
        FaceGlyph::Triangle,
    );
    paint_face_button(
        window,
        &xf,
        284.0,
        86.0,
        b.contains(DualSenseButtons::CIRCLE),
        FaceGlyph::Circle,
    );
    paint_face_button(
        window,
        &xf,
        264.0,
        106.0,
        b.contains(DualSenseButtons::CROSS),
        FaceGlyph::Cross,
    );
    paint_face_button(
        window,
        &xf,
        244.0,
        86.0,
        b.contains(DualSenseButtons::SQUARE),
        FaceGlyph::Square,
    );

    // 10. Left & Right Analog Sticks
    paint_stick(
        window,
        &xf,
        126.0,
        128.0,
        snap.left_stick[0],
        snap.left_stick[1],
        b.contains(DualSenseButtons::L3),
    );
    paint_stick(
        window,
        &xf,
        214.0,
        128.0,
        snap.right_stick[0],
        snap.right_stick[1],
        b.contains(DualSenseButtons::R3),
    );

    // 11. PS Home Button
    let ps_pressed = b.contains(DualSenseButtons::PS_HOME);
    paint_fill(
        window,
        color(if ps_pressed { PRIMARY } else { SURFACE_ALT }),
        |p| add_circle(p, &xf, 170.0, 121.0, 7.2),
    );
    paint_stroke(
        window,
        xf.len(1.1),
        color(if ps_pressed { PRIMARY } else { BORDER_STRONG }),
        |p| add_circle(p, &xf, 170.0, 121.0, 7.2),
    );

    // 12. Mic Mute Bar
    let mic_pressed = b.contains(DualSenseButtons::MIC_MUTE);
    paint_fill(
        window,
        color(if mic_pressed { WARN } else { BORDER }),
        |p| add_rounded_rect(p, &xf, 163.0, 136.0, 14.0, 3.8, 1.9),
    );
}

pub fn virtual_dualsense(state: ControllerPreviewState) -> Div {
    div().w_full().h(px(230.0)).child(
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
        };
        assert_eq!(resolve_lightbar_preview(&st), Some(([0, 102, 255], true)));

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
        let (cx, cy) = stick_offset(128, 128, 8.0);
        assert!(cx.abs() < 0.05 && cy.abs() < 0.05);

        let (dx, dy) = stick_offset(255, 255, 8.0);
        assert!((dx.hypot(dy) - 8.0).abs() < 0.05);
    }
}
