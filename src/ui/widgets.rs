use gpui::{Div, FontWeight, SharedString, Stateful, div, prelude::*, px, relative};

use super::theme::{ACCENT, BORDER, SURFACE, SURFACE_ALT, SURFACE_HOVER, TEXT, TEXT_MUTED, color};

pub fn section_card(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_3()
        .bg(color(SURFACE))
        .border_1()
        .border_color(color(BORDER))
        .rounded_md()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(TEXT_MUTED))
                .child(title),
        )
}

pub fn pill_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let bg = if active { ACCENT } else { SURFACE_ALT };
    let border = if active { ACCENT } else { BORDER };
    let text = if active { 0xFFFFFF } else { TEXT };
    div()
        .id(id.into())
        .px_3()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(color(border))
        .bg(color(bg))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(color(text))
        .cursor_pointer()
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(color(SURFACE_HOVER))
            }
        })
        .child(label.into())
}

pub fn coming_soon_pill(label: &'static str) -> Div {
    div()
        .px_2p5()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(color(BORDER))
        .text_xs()
        .text_color(color(TEXT_MUTED))
        .child(label)
}

pub fn driver_badge(name: &'static str, ready: bool) -> Div {
    let status = if ready { "Ready" } else { "Missing" };
    let text_color = if ready { TEXT } else { TEXT_MUTED };
    div()
        .px_2()
        .py_0p5()
        .rounded_sm()
        .bg(color(SURFACE_ALT))
        .border_1()
        .border_color(color(BORDER))
        .text_xs()
        .text_color(color(text_color))
        .child(format!("{name}: {status}"))
}

pub fn stick_box(title: &'static str, x: u8, y: u8) -> Div {
    let box_px = 116.0f32;
    let dot_px = 10.0f32;
    let pad = 6.0f32;
    let span = box_px - dot_px - pad * 2.0;
    let left_px = pad + (x as f32 / 255.0) * span;
    let top_px = pad + (y as f32 / 255.0) * span;

    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .items_center()
        .child(
            div()
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child(format!("{title} ({x}, {y})")),
        )
        .child(
            div()
                .relative()
                .size(px(box_px))
                .bg(color(SURFACE_ALT))
                .border_1()
                .border_color(color(BORDER))
                .rounded_sm()
                .child(
                    div()
                        .absolute()
                        .left(px(box_px / 2.0))
                        .top(px(4.0))
                        .w(px(1.0))
                        .h(px(box_px - 8.0))
                        .bg(color(BORDER)),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(box_px / 2.0))
                        .left(px(4.0))
                        .h(px(1.0))
                        .w(px(box_px - 8.0))
                        .bg(color(BORDER)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(left_px))
                        .top(px(top_px))
                        .size(px(dot_px))
                        .rounded_full()
                        .bg(color(ACCENT)),
                ),
        )
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
                .h(px(8.0))
                .rounded_sm()
                .bg(color(SURFACE_ALT))
                .border_1()
                .border_color(color(BORDER))
                .overflow_hidden()
                .child(div().h_full().w(relative(pct)).bg(color(bar_color))),
        )
        .child(
            div()
                .w(px(30.0))
                .text_xs()
                .text_color(color(TEXT))
                .child(format!("{value}")),
        )
}

pub fn button_pill(label: &'static str, pressed: bool) -> Div {
    div()
        .px_2()
        .py_0p5()
        .rounded_sm()
        .border_1()
        .border_color(color(if pressed { ACCENT } else { BORDER }))
        .bg(color(if pressed { ACCENT } else { SURFACE_ALT }))
        .text_xs()
        .text_color(color(if pressed { 0xFFFFFF } else { TEXT_MUTED }))
        .child(label)
}
