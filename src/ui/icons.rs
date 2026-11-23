use gpui::{Div, PathBuilder, Pixels, Point, Window, canvas, div, point, prelude::*, px};

use super::theme::color;

const ICON_STROKE: f32 = 1.6;
const MICRO_STROKE: f32 = 1.9;

#[derive(Clone, Copy)]
struct IconXf {
    ox: f32,
    oy: f32,
    s: f32,
}

impl IconXf {
    #[inline]
    fn from_bounds(bounds: gpui::Bounds<Pixels>, viewbox: f32) -> Self {
        let bw = f32::from(bounds.size.width).max(1.0);
        let bh = f32::from(bounds.size.height).max(1.0);
        let s = (bw / viewbox).min(bh / viewbox);
        let ox = f32::from(bounds.origin.x) + (bw - viewbox * s) * 0.5;
        let oy = f32::from(bounds.origin.y) + (bh - viewbox * s) * 0.5;
        Self { ox, oy, s }
    }

    #[inline]
    fn pt(&self, x: f32, y: f32) -> Point<Pixels> {
        point(px(self.ox + x * self.s), px(self.oy + y * self.s))
    }

    #[inline]
    fn stroke_w(&self, w: f32) -> Pixels {
        px((w * self.s).max(1.0))
    }
}

fn add_circle(b: &mut PathBuilder, xf: &IconXf, cx: f32, cy: f32, r: f32) {
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

fn add_rounded_rect(b: &mut PathBuilder, xf: &IconXf, x: f32, y: f32, w: f32, h: f32, r: f32) {
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

fn stroke_icon(
    size: f32,
    stroke_color: u32,
    stroke_width: f32,
    draw: impl Fn(&mut PathBuilder, &IconXf) + 'static,
) -> Div {
    div().size(px(size)).flex_shrink_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window: &mut Window, _| {
                let xf = IconXf::from_bounds(bounds, 24.0);
                let mut b = PathBuilder::stroke(xf.stroke_w(stroke_width));
                draw(&mut b, &xf);
                if let Ok(path) = b.build() {
                    window.paint_path(path, color(stroke_color));
                }
            },
        )
        .size_full(),
    )
}

pub fn gamepad_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        b.move_to(xf.pt(7.0, 7.0));
        b.line_to(xf.pt(17.0, 7.0));
        b.cubic_bezier_to(xf.pt(21.5, 12.5), xf.pt(20.0, 7.0), xf.pt(21.5, 9.2));
        b.cubic_bezier_to(xf.pt(19.5, 18.0), xf.pt(21.5, 16.0), xf.pt(20.8, 18.0));
        b.line_to(xf.pt(15.6, 14.8));
        b.line_to(xf.pt(8.4, 14.8));
        b.line_to(xf.pt(4.5, 18.0));
        b.cubic_bezier_to(xf.pt(2.5, 12.5), xf.pt(3.2, 18.0), xf.pt(2.5, 16.0));
        b.cubic_bezier_to(xf.pt(7.0, 7.0), xf.pt(2.5, 9.2), xf.pt(4.0, 7.0));
        b.close();
        b.move_to(xf.pt(6.6, 11.0));
        b.line_to(xf.pt(9.6, 11.0));
        b.move_to(xf.pt(8.1, 9.5));
        b.line_to(xf.pt(8.1, 12.5));
        add_circle(b, xf, 15.2, 10.2, 0.7);
        add_circle(b, xf, 17.2, 11.8, 0.7);
    })
}

pub fn sliders_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        b.move_to(xf.pt(4.0, 7.0));
        b.line_to(xf.pt(10.0, 7.0));
        b.move_to(xf.pt(14.0, 7.0));
        b.line_to(xf.pt(20.0, 7.0));
        add_circle(b, xf, 12.0, 7.0, 2.0);

        b.move_to(xf.pt(4.0, 12.0));
        b.line_to(xf.pt(15.0, 12.0));
        b.move_to(xf.pt(19.0, 12.0));
        b.line_to(xf.pt(20.0, 12.0));
        add_circle(b, xf, 17.0, 12.0, 2.0);

        b.move_to(xf.pt(4.0, 17.0));
        b.line_to(xf.pt(6.0, 17.0));
        b.move_to(xf.pt(10.0, 17.0));
        b.line_to(xf.pt(20.0, 17.0));
        add_circle(b, xf, 8.0, 17.0, 2.0);
    })
}

pub fn layers_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        add_rounded_rect(b, xf, 4.0, 4.0, 6.8, 6.8, 1.6);
        add_rounded_rect(b, xf, 13.2, 4.0, 6.8, 6.8, 1.6);
        add_rounded_rect(b, xf, 4.0, 13.2, 6.8, 6.8, 1.6);
        add_rounded_rect(b, xf, 13.2, 13.2, 6.8, 6.8, 1.6);
    })
}

pub fn stick_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        add_circle(b, xf, 12.0, 12.0, 8.2);
        add_circle(b, xf, 12.0, 12.0, 3.2);
        b.move_to(xf.pt(12.0, 3.8));
        b.line_to(xf.pt(12.0, 6.5));
        b.move_to(xf.pt(12.0, 17.5));
        b.line_to(xf.pt(12.0, 20.2));
        b.move_to(xf.pt(3.8, 12.0));
        b.line_to(xf.pt(6.5, 12.0));
        b.move_to(xf.pt(17.5, 12.0));
        b.line_to(xf.pt(20.2, 12.0));
    })
}

pub fn trigger_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        b.move_to(xf.pt(3.0, 12.0));
        b.line_to(xf.pt(7.0, 12.0));
        b.line_to(xf.pt(9.5, 5.5));
        b.line_to(xf.pt(13.0, 18.5));
        b.line_to(xf.pt(16.0, 9.5));
        b.line_to(xf.pt(18.0, 12.0));
        b.line_to(xf.pt(21.0, 12.0));
    })
}

pub fn sun_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        add_circle(b, xf, 12.0, 12.0, 4.0);
        for (x1, y1, x2, y2) in [
            (12.0, 3.0, 12.0, 5.2),
            (12.0, 18.8, 12.0, 21.0),
            (3.0, 12.0, 5.2, 12.0),
            (18.8, 12.0, 21.0, 12.0),
            (5.6, 5.6, 7.2, 7.2),
            (16.8, 16.8, 18.4, 18.4),
            (18.4, 5.6, 16.8, 7.2),
            (7.2, 16.8, 5.6, 18.4),
        ] {
            b.move_to(xf.pt(x1, y1));
            b.line_to(xf.pt(x2, y2));
        }
    })
}

pub fn gyro_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        add_circle(b, xf, 12.0, 12.0, 8.0);
        b.move_to(xf.pt(12.0, 4.0));
        b.cubic_bezier_to(xf.pt(12.0, 20.0), xf.pt(16.5, 6.5), xf.pt(16.5, 17.5));
        b.move_to(xf.pt(4.0, 12.0));
        b.cubic_bezier_to(xf.pt(20.0, 12.0), xf.pt(6.5, 8.5), xf.pt(17.5, 8.5));
    })
}

pub fn settings_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        add_circle(b, xf, 12.0, 12.0, 3.2);
        add_circle(b, xf, 12.0, 12.0, 7.4);
        for (x1, y1, x2, y2) in [
            (12.0, 2.4, 12.0, 4.6),
            (12.0, 19.4, 12.0, 21.6),
            (2.4, 12.0, 4.6, 12.0),
            (19.4, 12.0, 21.6, 12.0),
            (5.2, 5.2, 6.8, 6.8),
            (17.2, 17.2, 18.8, 18.8),
            (18.8, 5.2, 17.2, 6.8),
            (6.8, 17.2, 5.2, 18.8),
        ] {
            b.move_to(xf.pt(x1, y1));
            b.line_to(xf.pt(x2, y2));
        }
    })
}

pub fn check_icon(size: f32, col: u32) -> Div {
    let w = if size <= 13.0 {
        MICRO_STROKE
    } else {
        ICON_STROKE
    };
    stroke_icon(size, col, w, |b, xf| {
        b.move_to(xf.pt(5.5, 12.5));
        b.line_to(xf.pt(10.0, 17.0));
        b.line_to(xf.pt(18.8, 7.2));
    })
}

pub fn close_x_icon(size: f32, col: u32) -> Div {
    let w = if size <= 13.0 {
        MICRO_STROKE
    } else {
        ICON_STROKE
    };
    stroke_icon(size, col, w, |b, xf| {
        b.move_to(xf.pt(6.5, 6.5));
        b.line_to(xf.pt(17.5, 17.5));
        b.move_to(xf.pt(17.5, 6.5));
        b.line_to(xf.pt(6.5, 17.5));
    })
}

pub fn plus_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        b.move_to(xf.pt(5.5, 12.0));
        b.line_to(xf.pt(18.5, 12.0));
        b.move_to(xf.pt(12.0, 5.5));
        b.line_to(xf.pt(12.0, 18.5));
    })
}

pub fn link_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        b.move_to(xf.pt(9.6, 7.0));
        b.line_to(xf.pt(7.0, 7.0));
        b.cubic_bezier_to(xf.pt(2.0, 12.0), xf.pt(4.2, 7.0), xf.pt(2.0, 9.2));
        b.cubic_bezier_to(xf.pt(7.0, 17.0), xf.pt(2.0, 14.8), xf.pt(4.2, 17.0));
        b.line_to(xf.pt(9.6, 17.0));

        b.move_to(xf.pt(14.4, 7.0));
        b.line_to(xf.pt(17.0, 7.0));
        b.cubic_bezier_to(xf.pt(22.0, 12.0), xf.pt(19.8, 7.0), xf.pt(22.0, 9.2));
        b.cubic_bezier_to(xf.pt(17.0, 17.0), xf.pt(22.0, 14.8), xf.pt(19.8, 17.0));
        b.line_to(xf.pt(14.4, 17.0));

        b.move_to(xf.pt(8.4, 12.0));
        b.line_to(xf.pt(15.6, 12.0));
    })
}

pub fn trash_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        b.move_to(xf.pt(4.5, 7.0));
        b.line_to(xf.pt(19.5, 7.0));
        b.move_to(xf.pt(9.0, 7.0));
        b.line_to(xf.pt(9.5, 4.5));
        b.line_to(xf.pt(14.5, 4.5));
        b.line_to(xf.pt(15.0, 7.0));
        b.move_to(xf.pt(6.5, 7.0));
        b.line_to(xf.pt(7.5, 19.5));
        b.line_to(xf.pt(16.5, 19.5));
        b.line_to(xf.pt(17.5, 7.0));
        b.move_to(xf.pt(10.2, 10.5));
        b.line_to(xf.pt(10.2, 16.0));
        b.move_to(xf.pt(13.8, 10.5));
        b.line_to(xf.pt(13.8, 16.0));
    })
}

pub fn sidebar_toggle_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        add_rounded_rect(b, xf, 3.5, 4.5, 17.0, 15.0, 2.2);
        b.move_to(xf.pt(9.0, 4.5));
        b.line_to(xf.pt(9.0, 19.5));
        b.move_to(xf.pt(15.5, 9.5));
        b.line_to(xf.pt(12.5, 12.0));
        b.line_to(xf.pt(15.5, 14.5));
    })
}

/// Window minimize-to-system-tray glyph
pub fn tray_hide_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        b.move_to(xf.pt(11.0, 18.5));
        b.line_to(xf.pt(4.0, 18.5));
        b.line_to(xf.pt(4.0, 5.5));
        b.line_to(xf.pt(20.0, 5.5));
        b.line_to(xf.pt(20.0, 12.0));
        b.move_to(xf.pt(8.5, 9.5));
        b.line_to(xf.pt(15.0, 15.5));
        b.move_to(xf.pt(11.0, 15.5));
        b.line_to(xf.pt(15.2, 15.5));
        b.line_to(xf.pt(15.2, 11.5));
        add_rounded_rect(b, xf, 14.5, 16.5, 5.5, 3.0, 1.0);
    })
}

pub fn swap_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        b.move_to(xf.pt(5.0, 8.5));
        b.line_to(xf.pt(19.0, 8.5));
        b.move_to(xf.pt(15.5, 5.2));
        b.line_to(xf.pt(19.0, 8.5));
        b.line_to(xf.pt(15.5, 11.8));

        b.move_to(xf.pt(19.0, 15.5));
        b.line_to(xf.pt(5.0, 15.5));
        b.move_to(xf.pt(8.5, 12.2));
        b.line_to(xf.pt(5.0, 15.5));
        b.line_to(xf.pt(8.5, 18.8));
    })
}

pub fn info_icon(size: f32, col: u32) -> Div {
    stroke_icon(size, col, ICON_STROKE, |b, xf| {
        add_circle(b, xf, 12.0, 12.0, 8.5);
        b.move_to(xf.pt(12.0, 11.0));
        b.line_to(xf.pt(12.0, 16.5));
        add_circle(b, xf, 12.0, 8.0, 0.7);
    })
}

pub fn win_min_icon(col: u32) -> Div {
    stroke_icon(14.0, col, MICRO_STROKE, |b, xf| {
        b.move_to(xf.pt(4.0, 12.0));
        b.line_to(xf.pt(20.0, 12.0));
    })
}

pub fn win_max_icon(col: u32) -> Div {
    stroke_icon(14.0, col, ICON_STROKE, |b, xf| {
        add_rounded_rect(b, xf, 4.5, 4.5, 15.0, 15.0, 1.5);
    })
}

pub fn win_close_icon(col: u32) -> Div {
    stroke_icon(14.0, col, MICRO_STROKE, |b, xf| {
        b.move_to(xf.pt(5.0, 5.0));
        b.line_to(xf.pt(19.0, 19.0));
        b.move_to(xf.pt(19.0, 5.0));
        b.line_to(xf.pt(5.0, 19.0));
    })
}
