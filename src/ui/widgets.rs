use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Bounds, Div, FontWeight, IntoElement, Pixels, SharedString, Stateful, canvas, div, prelude::*,
    px, relative,
};

use super::theme::{
    BG, BORDER, BORDER_STRONG, FONT_MONO, PRIMARY, PRIMARY_FG, PRIMARY_HOVER, SIDEBAR_BG, SURFACE,
    SURFACE_ALT, SURFACE_HOVER, TEXT, TEXT_MUTED, WARN, color, color_alpha,
};
use crate::engine::profile::StickTuning;

pub fn section_card(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w(px(0.0))
        .gap_3()
        .p_4()
        .bg(color(SURFACE))
        .border_1()
        .border_color(color(BORDER))
        .rounded(px(8.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(TEXT))
                .child(title),
        )
}

pub fn section_card_with_action(title: &'static str, action: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w(px(0.0))
        .gap_3()
        .p_4()
        .bg(color(SURFACE))
        .border_1()
        .border_color(color(BORDER))
        .rounded(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(TEXT))
                        .child(title),
                )
                .child(action),
        )
}

pub fn pill_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let bg = if active { PRIMARY } else { SURFACE_ALT };
    let border = if active { PRIMARY } else { BORDER };
    let text = if active { PRIMARY_FG } else { TEXT };
    div()
        .id(id.into())
        .px_2p5()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(color(border))
        .bg(color(bg))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .flex_shrink_0()
        .text_color(color(text))
        .cursor_pointer()
        .hover(move |s| {
            if active {
                s.bg(color(PRIMARY_HOVER))
            } else {
                s.bg(color(SURFACE_HOVER))
                    .border_color(color(BORDER_STRONG))
            }
        })
        .child(label.into())
}

pub fn sidebar_item(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let bg = if active { SURFACE_HOVER } else { SIDEBAR_BG };
    let text = if active { TEXT } else { TEXT_MUTED };
    div()
        .id(id.into())
        .w_full()
        .px_2p5()
        .py_1p5()
        .rounded_md()
        .bg(color(bg))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(color(text))
        .cursor_pointer()
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(color(SURFACE_ALT)).text_color(color(TEXT))
            }
        })
        .child(label.into())
}

pub fn coming_soon_pill(label: &'static str) -> Div {
    div()
        .px_2p5()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(color(BORDER))
        .text_xs()
        .whitespace_nowrap()
        .flex_shrink_0()
        .text_color(color(TEXT_MUTED))
        .child(label)
}

pub fn driver_badge(name: &'static str, installed: bool) -> Div {
    let status = if installed {
        "Installed"
    } else {
        "Not Installed"
    };
    let text_color = if installed { TEXT } else { TEXT_MUTED };
    div()
        .px_2p5()
        .py_1()
        .rounded_md()
        .bg(color(SURFACE_ALT))
        .border_1()
        .border_color(color(BORDER))
        .text_xs()
        .whitespace_nowrap()
        .flex_shrink_0()
        .text_color(color(text_color))
        .child(format!("{name}: {status}"))
}

pub fn meter_row(label: &'static str, value: u8, bar_color: u32) -> Div {
    let pct = value as f32 / 255.0;
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(64.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .h(px(6.0))
                .rounded_full()
                .bg(color(SURFACE_ALT))
                .border_1()
                .border_color(color(BORDER))
                .overflow_hidden()
                .child(div().h_full().w(relative(pct)).bg(color(bar_color))),
        )
        .child(
            div()
                .w(px(32.0))
                .text_xs()
                .font_family(FONT_MONO)
                .text_color(color(TEXT))
                .child(format!("{value}")),
        )
}

pub fn setting_row(
    title: &'static str,
    description: &'static str,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .py_1()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(TEXT))
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(color(TEXT_MUTED))
                        .child(description),
                ),
        )
        .child(div().flex_shrink_0().child(control))
}

fn stick_point_in_circle(x: u8, y: u8, field_px: f32, dot_px: f32) -> (f32, f32) {
    let nx = (x as f32 - 128.0) / 127.5;
    let ny = (y as f32 - 128.0) / 127.5;
    let mag = (nx * nx + ny * ny).sqrt();
    let (cx, cy) = if mag > 1.0 {
        (nx / mag, ny / mag)
    } else {
        (nx, ny)
    };
    let r = (field_px - dot_px) * 0.5 - 2.0;
    let left = field_px * 0.5 + cx * r - dot_px * 0.5;
    let top = field_px * 0.5 + cy * r - dot_px * 0.5;
    (left, top)
}

pub fn deadzone_showcase(raw: [u8; 2], out: [u8; 2], tuning: StickTuning, connected: bool) -> Div {
    const FIELD_PX: f32 = 108.0;
    const DOT_PX: f32 = 8.0;

    let (raw_left, raw_top) = stick_point_in_circle(raw[0], raw[1], FIELD_PX, DOT_PX);
    let (out_left, out_top) = stick_point_in_circle(out[0], out[1], FIELD_PX, DOT_PX);

    let mut field = div()
        .relative()
        .size(px(FIELD_PX))
        .rounded_full()
        .bg(color(SURFACE_ALT))
        .border_1()
        .border_color(color(BORDER))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .left(px(FIELD_PX * 0.5))
                .top(px(4.0))
                .w(px(1.0))
                .h(px(FIELD_PX - 8.0))
                .bg(color(BORDER)),
        )
        .child(
            div()
                .absolute()
                .top(px(FIELD_PX * 0.5))
                .left(px(4.0))
                .h(px(1.0))
                .w(px(FIELD_PX - 8.0))
                .bg(color(BORDER)),
        );

    if tuning.anti_deadzone > 0.001 {
        let d_anti = (FIELD_PX * tuning.anti_deadzone.clamp(0.0, 0.45)).max(4.0);
        let offset = (FIELD_PX - d_anti) * 0.5;
        field = field.child(
            div()
                .absolute()
                .left(px(offset))
                .top(px(offset))
                .size(px(d_anti))
                .rounded_full()
                .border_1()
                .border_color(color(WARN))
                .bg(color_alpha(WARN, 0x1A)),
        );
    }

    if tuning.deadzone > 0.001 {
        let d_dz = (FIELD_PX * tuning.deadzone.clamp(0.0, 0.49)).max(4.0);
        let offset = (FIELD_PX - d_dz) * 0.5;
        field = field.child(
            div()
                .absolute()
                .left(px(offset))
                .top(px(offset))
                .size(px(d_dz))
                .rounded_full()
                .border_1()
                .border_color(color(BORDER_STRONG))
                .bg(color_alpha(PRIMARY, 0x22)),
        );
    }

    field = field
        .child(
            div()
                .absolute()
                .left(px(raw_left))
                .top(px(raw_top))
                .size(px(DOT_PX))
                .rounded_full()
                .border_2()
                .border_color(color(TEXT_MUTED))
                .bg(color(BG)),
        )
        .child(
            div()
                .absolute()
                .left(px(out_left))
                .top(px(out_top))
                .size(px(DOT_PX))
                .rounded_full()
                .bg(color(PRIMARY))
                .border_1()
                .border_color(color(PRIMARY_FG)),
        );

    let raw_pct = ((((raw[0] as f32 - 128.0) / 127.5).hypot((raw[1] as f32 - 128.0) / 127.5))
        .min(1.0)
        * 100.0)
        .round() as u32;
    let out_pct = ((((out[0] as f32 - 128.0) / 127.5).hypot((out[1] as f32 - 128.0) / 127.5))
        .min(1.0)
        * 100.0)
        .round() as u32;

    let readout = if connected {
        format!("In {raw_pct:3}% / Out {out_pct:3}%")
    } else {
        "Move stick to test".to_owned()
    };

    div()
        .w(px(140.0))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .child(field)
        .child(
            div()
                .text_xs()
                .font_family(FONT_MONO)
                .whitespace_nowrap()
                .text_color(color(TEXT_MUTED))
                .child(readout),
        )
        .child(
            div()
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("Ring: Raw | Dot: Out"),
        )
}

pub fn slider_bar(
    fill_ratio: f32,
    fill_color: u32,
    bounds_slot: usize,
    bounds_store: Rc<RefCell<[Bounds<Pixels>; 5]>>,
) -> Div {
    let clamped = fill_ratio.clamp(0.0, 1.0);
    div()
        .relative()
        .flex()
        .items_center()
        .flex_1()
        .h(px(20.0))
        .cursor_pointer()
        .child(
            div()
                .w_full()
                .h(px(6.0))
                .rounded_full()
                .bg(color(SURFACE_HOVER))
                .overflow_hidden()
                .child(div().h_full().w(relative(clamped)).bg(color(fill_color))),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(6.0))
                .right(px(6.0))
                .flex()
                .items_center()
                .child(
                    div()
                        .absolute()
                        .left(relative(clamped))
                        .ml(px(-7.0))
                        .size(px(14.0))
                        .rounded_full()
                        .bg(color(PRIMARY))
                        .border_2()
                        .border_color(color(BG)),
                ),
        )
        .child(
            div().absolute().inset_0().child(
                canvas(
                    move |bounds, _, _| {
                        bounds_store.borrow_mut()[bounds_slot] = bounds;
                    },
                    |_, _, _, _| {},
                )
                .size_full(),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stick_point_in_circle_centers_and_clamps_diagonals() {
        let field = 120.0;
        let dot = 8.0;
        let (cx, cy) = stick_point_in_circle(128, 128, field, dot);
        let expected_center = (field - dot) * 0.5;
        assert!((cx - expected_center).abs() < 0.01);
        assert!((cy - expected_center).abs() < 0.01);

        let (dx, dy) = stick_point_in_circle(255, 255, field, dot);
        assert!(dx >= 0.0 && dx <= field - dot);
        assert!(dy >= 0.0 && dy <= field - dot);
        let dist_from_center =
            ((dx + dot * 0.5) - field * 0.5).hypot((dy + dot * 0.5) - field * 0.5);
        assert!(dist_from_center <= field * 0.5 - 2.0 + 0.01);
    }
}
