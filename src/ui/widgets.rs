use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyView, App, Bounds, Context, Div, FontWeight, IntoElement, PathBuilder, Pixels, Render,
    SharedString, Stateful, Window, canvas, div, linear_color_stop, linear_gradient, point,
    prelude::*, px, relative,
};

use super::icons::{check_icon, close_x_icon, info_icon};
use super::theme::{
    ACCENT, ACCENT_FG, ACCENT_SOLID, ACCENT_SOLID_HOVER, BORDER, BORDER_STRONG, DANGER,
    DANGER_BORDER, DANGER_SURFACE, DANGER_SURFACE_HOVER, DRAWER_BG, FONT_MONO, FONT_SANS, RAIL_BG,
    SUCCESS, SURFACE, SURFACE_ALT, SURFACE_HOVER, TEXT, TEXT_MUTED, TEXT_SECONDARY, WELL_BG,
    ZONE_SECONDARY, color, color_alpha, hsv_to_rgb, overlay_shadow, rgb_to_u32,
};
use crate::engine::profile::{DeadzoneShape, StickTuning};

pub const SLIDER_SLOTS: usize = 28;

/// Flat dark surface card (`#222428`, 12px radius, 20px padding, 16px rhythm, 1px hairline)
pub fn section_card(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w(px(0.0))
        .gap_4()
        .p_5()
        .bg(color(SURFACE))
        .border_1()
        .border_color(color(BORDER))
        .rounded(px(12.0))
        .child(
            div()
                .flex_shrink_0()
                .min_h(px(24.0))
                .flex()
                .items_center()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(TEXT))
                .child(title),
        )
}

pub fn section_card_with_action(title: &'static str, action: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w(px(0.0))
        .gap_4()
        .p_5()
        .bg(color(SURFACE))
        .border_1()
        .border_color(color(BORDER))
        .rounded(px(12.0))
        .child(
            div()
                .flex_shrink_0()
                .min_h(px(24.0))
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(TEXT))
                        .child(title),
                )
                .child(action),
        )
}

/// Hardware pill toggle switch on strict 4px grid (44×24px, 18px thumb)
pub fn toggle_switch(id: impl Into<SharedString>, checked: bool) -> Stateful<Div> {
    let track_bg = if checked { ACCENT_SOLID } else { SURFACE_ALT };
    let track_border = if checked {
        ACCENT_SOLID
    } else {
        BORDER_STRONG
    };
    let thumb_bg = if checked { ACCENT_FG } else { TEXT_SECONDARY };
    let thumb_left = if checked { 22.0 } else { 2.0 };

    let mut track = div()
        .id(id.into())
        .relative()
        .w(px(44.0))
        .h(px(24.0))
        .rounded_full()
        .bg(color(track_bg))
        .border_1()
        .border_color(color(track_border))
        .flex_shrink_0()
        .cursor_pointer()
        .hover(move |s| {
            if checked {
                s.bg(color(ACCENT_SOLID_HOVER))
                    .border_color(color(ACCENT_SOLID_HOVER))
            } else {
                s.bg(color(SURFACE_HOVER))
            }
        });

    if checked {
        track = track.child(
            div()
                .absolute()
                .left(px(6.0))
                .top(px(5.0))
                .child(check_icon(12.0, ACCENT_FG)),
        );
    } else {
        track = track.child(
            div()
                .absolute()
                .right(px(6.0))
                .top(px(5.5))
                .child(close_x_icon(11.0, TEXT_MUTED)),
        );
    }

    track.child(
        div()
            .absolute()
            .top(px(2.0))
            .left(px(thumb_left))
            .size(px(18.0))
            .rounded_full()
            .bg(color(thumb_bg)),
    )
}

/// Quiet segmented tab button sitting flush inside a recessed `#15171A` tray
pub fn segmented_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let bg = if active { SURFACE_ALT } else { WELL_BG };
    let border = if active { BORDER_STRONG } else { WELL_BG };
    let text = if active { TEXT } else { TEXT_MUTED };
    div()
        .id(id.into())
        .h(px(28.0))
        .px_3()
        .rounded(px(6.0))
        .border_1()
        .border_color(color(border))
        .bg(color(bg))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .font_weight(if active {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        })
        .whitespace_nowrap()
        .flex_shrink_0()
        .text_color(color(text))
        .cursor_pointer()
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(color(SURFACE)).text_color(color(TEXT_SECONDARY))
            }
        })
        .child(label.into())
}

/// Quiet selectable option button (machined surface step when active, resting well when inactive)
pub fn pill_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let bg = if active { SURFACE_HOVER } else { WELL_BG };
    let border = if active { BORDER_STRONG } else { BORDER };
    let text = if active { TEXT } else { TEXT_SECONDARY };
    div()
        .id(id.into())
        .h(px(32.0))
        .px_3()
        .rounded(px(8.0))
        .border_1()
        .border_color(color(border))
        .bg(color(bg))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .font_weight(if active {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        })
        .whitespace_nowrap()
        .flex_shrink_0()
        .text_color(color(text))
        .cursor_pointer()
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(color(SURFACE_ALT))
                    .border_color(color(BORDER_STRONG))
                    .text_color(color(TEXT))
            }
        })
        .child(label.into())
}

/// Secondary action button (32px tall, quiet raised surface)
pub fn subtle_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(px(32.0))
        .px_3()
        .rounded(px(8.0))
        .border_1()
        .border_color(color(BORDER))
        .bg(color(SURFACE_ALT))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .font_weight(FontWeight::NORMAL)
        .whitespace_nowrap()
        .flex_shrink_0()
        .text_color(color(TEXT_SECONDARY))
        .cursor_pointer()
        .hover(|s| {
            s.bg(color(SURFACE_HOVER))
                .border_color(color(BORDER_STRONG))
                .text_color(color(TEXT))
        })
        .child(label.into())
}

/// Destructive action button (toned crimson surface + quiet border, zero neon outline)
pub fn danger_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(px(32.0))
        .px_3()
        .rounded(px(8.0))
        .border_1()
        .border_color(color(DANGER_BORDER))
        .bg(color(DANGER_SURFACE))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .flex_shrink_0()
        .text_color(color(DANGER))
        .cursor_pointer()
        .hover(|s| {
            s.bg(color(DANGER_SURFACE_HOVER))
                .border_color(color(DANGER))
        })
        .child(label.into())
}

/// Primary CTA button (solid PlayStation cobalt blue, 36px tall, one primary CTA per view)
pub fn primary_cta_button(
    id: impl Into<SharedString>,
    icon: Div,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let bg = if active { ACCENT_SOLID } else { SURFACE_ALT };
    let border = if active {
        ACCENT_SOLID
    } else {
        BORDER_STRONG
    };
    let fg = if active { ACCENT_FG } else { TEXT };
    div()
        .id(id.into())
        .h(px(36.0))
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .rounded(px(8.0))
        .border_1()
        .border_color(color(border))
        .bg(color(bg))
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .flex_shrink_0()
        .text_color(color(fg))
        .cursor_pointer()
        .hover(move |s| {
            if active {
                s.bg(color(ACCENT_SOLID_HOVER))
                    .border_color(color(ACCENT_SOLID_HOVER))
            } else {
                s.bg(color(SURFACE_HOVER)).border_color(color(BORDER_STRONG))
            }
        })
        .child(icon)
        .child(label.into())
}

/// Far-left 72px icon rail item (quiet neutral selection, icons match text role)
pub fn icon_rail_item(
    id: impl Into<SharedString>,
    icon: Div,
    label: &'static str,
    active: bool,
) -> Stateful<Div> {
    let bg = if active { SURFACE_ALT } else { RAIL_BG };
    let border = if active { BORDER_STRONG } else { RAIL_BG };
    let text = if active { TEXT } else { TEXT_MUTED };
    div()
        .id(id.into())
        .w(px(56.0))
        .py_2()
        .rounded(px(8.0))
        .bg(color(bg))
        .border_1()
        .border_color(color(border))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .cursor_pointer()
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(color(SURFACE))
                    .border_color(color(SURFACE))
                    .text_color(color(TEXT_SECONDARY))
            }
        })
        .child(icon)
        .child(
            div()
                .text_xs()
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .text_color(color(text))
                .child(label),
        )
}

/// Configuration drawer navigation row (strict 36px height, aligned 16px icon column)
pub fn drawer_nav_item(
    id: impl Into<SharedString>,
    icon: Div,
    label: impl Into<SharedString>,
    active: bool,
    meta: Option<Div>,
) -> Stateful<Div> {
    let bg = if active { SURFACE_ALT } else { DRAWER_BG };
    let border = if active { BORDER_STRONG } else { DRAWER_BG };
    let text = if active { TEXT } else { TEXT_SECONDARY };
    let mut row = div()
        .id(id.into())
        .w_full()
        .h(px(36.0))
        .px_3()
        .rounded(px(8.0))
        .bg(color(bg))
        .border_1()
        .border_color(color(border))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .cursor_pointer()
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(color(SURFACE))
                    .border_color(color(SURFACE))
                    .text_color(color(TEXT))
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .min_w(px(0.0))
                .child(icon)
                .child(
                    div()
                        .text_sm()
                        .font_weight(if active {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(color(text))
                        .whitespace_nowrap()
                        .child(label.into()),
                ),
        );

    if let Some(m) = meta {
        row = row.child(m);
    }
    row
}

/// Inset status banner with crisp 16px vector icon (zero dots, zero glowing discs)
pub fn status_inset_pill(label: impl Into<SharedString>, ok: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded(px(8.0))
        .bg(color(WELL_BG))
        .border_1()
        .border_color(color(BORDER))
        .child(if ok {
            check_icon(16.0, SUCCESS)
        } else {
            info_icon(16.0, TEXT_MUTED)
        })
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::NORMAL)
                .text_color(color(if ok { TEXT_SECONDARY } else { TEXT_MUTED }))
                .child(label.into()),
        )
}

/// Neutral informational inset banner (crisp 16px info glyph)
pub fn status_info_pill(label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded(px(8.0))
        .bg(color(WELL_BG))
        .border_1()
        .border_color(color(BORDER))
        .child(info_icon(16.0, TEXT_MUTED))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::NORMAL)
                .text_color(color(TEXT_SECONDARY))
                .child(label.into()),
        )
}

/// Compact driver install status pill for Settings rows (crisp 16px vector check / close icon)
pub fn driver_status_pill(installed: bool) -> Div {
    let status = if installed {
        "Installed"
    } else {
        "Missing"
    };
    div()
        .h(px(28.0))
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .rounded(px(6.0))
        .bg(color(WELL_BG))
        .border_1()
        .border_color(color(if installed { BORDER } else { DANGER_BORDER }))
        .text_size(px(13.0))
        .whitespace_nowrap()
        .flex_shrink_0()
        .child(if installed {
            check_icon(16.0, SUCCESS)
        } else {
            close_x_icon(16.0, DANGER)
        })
        .child(
            div()
                .font_weight(FontWeight::NORMAL)
                .text_color(color(if installed { TEXT_SECONDARY } else { DANGER }))
                .child(status),
        )
}

pub fn meter_row(label: &'static str, value: u8, bar_color: u32) -> Div {
    meter_row_with_width(label, 88.0, value, bar_color)
}

pub fn meter_row_with_width(
    label: &'static str,
    label_w: f32,
    value: u8,
    bar_color: u32,
) -> Div {
    let pct = value as f32 / 255.0;
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(label_w))
                .flex_shrink_0()
                .whitespace_nowrap()
                .text_size(px(13.0))
                .text_color(color(TEXT_SECONDARY))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .h(px(8.0))
                .rounded_full()
                .bg(color(WELL_BG))
                .border_1()
                .border_color(color(BORDER))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(pct))
                        .rounded_full()
                        .bg(color(bar_color)),
                ),
        )
        .child(
            div()
                .w(px(36.0))
                .text_right()
                .text_size(px(13.0))
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
        .gap_4()
        .py_2()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(TEXT))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(13.0))
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
    let r = (field_px - dot_px) * 0.5 - 3.0;
    let left = field_px * 0.5 + cx * r - dot_px * 0.5;
    let top = field_px * 0.5 + cy * r - dot_px * 0.5;
    (left, top)
}

/// Circular deadzone & anti-deadzone visualizer (gold deadzone, muted steel anti-deadzone)
pub fn deadzone_showcase(raw: [u8; 2], out: [u8; 2], tuning: StickTuning, connected: bool) -> Div {
    const FIELD_PX: f32 = 128.0;
    const DOT_PX: f32 = 10.0;

    let (raw_left, raw_top) = stick_point_in_circle(raw[0], raw[1], FIELD_PX, DOT_PX);
    let (out_left, out_top) = stick_point_in_circle(out[0], out[1], FIELD_PX, DOT_PX);

    let mut field = div()
        .relative()
        .size(px(FIELD_PX))
        .rounded_full()
        .bg(color(WELL_BG))
        .border_1()
        .border_color(color(BORDER_STRONG))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .left(px(FIELD_PX * 0.25))
                .top(px(FIELD_PX * 0.25))
                .size(px(FIELD_PX * 0.5))
                .rounded_full()
                .border_1()
                .border_color(color_alpha(BORDER_STRONG, 0x55)),
        )
        .child(
            div()
                .absolute()
                .left(px(FIELD_PX * 0.5))
                .top(px(8.0))
                .w(px(1.0))
                .h(px(FIELD_PX - 16.0))
                .bg(color_alpha(BORDER_STRONG, 0x70)),
        )
        .child(
            div()
                .absolute()
                .top(px(FIELD_PX * 0.5))
                .left(px(8.0))
                .h(px(1.0))
                .w(px(FIELD_PX - 16.0))
                .bg(color_alpha(BORDER_STRONG, 0x70)),
        );

    if tuning.shape != DeadzoneShape::Passthrough && tuning.outer_deadzone > 0.005 {
        let sat_frac = (1.0 - tuning.outer_deadzone.clamp(0.0, 0.40)).clamp(0.4, 1.0);
        let d_outer = FIELD_PX * sat_frac;
        let offset = (FIELD_PX - d_outer) * 0.5;
        field = field.child(
            div()
                .absolute()
                .left(px(offset))
                .top(px(offset))
                .size(px(d_outer))
                .rounded_full()
                .border_1()
                .border_color(color_alpha(SUCCESS, 0x70)),
        );
    }

    if tuning.shape != DeadzoneShape::Passthrough && tuning.anti_deadzone > 0.001 {
        let d_anti = (FIELD_PX * tuning.anti_deadzone.clamp(0.0, 0.45)).max(6.0);
        let offset = (FIELD_PX - d_anti) * 0.5;
        field = field.child(
            div()
                .absolute()
                .left(px(offset))
                .top(px(offset))
                .size(px(d_anti))
                .rounded_full()
                .border_1()
                .border_color(color(ZONE_SECONDARY))
                .bg(color_alpha(ZONE_SECONDARY, 0x24)),
        );
    }

    if tuning.shape == DeadzoneShape::Axial && tuning.deadzone > 0.001 {
        let band = (FIELD_PX * tuning.deadzone.clamp(0.0, 0.49)).max(6.0);
        let offset = (FIELD_PX - band) * 0.5;
        field = field
            .child(
                div()
                    .absolute()
                    .left(px(offset))
                    .top(px(0.0))
                    .w(px(band))
                    .h(px(FIELD_PX))
                    .border_x_1()
                    .border_color(color_alpha(ACCENT, 0x80))
                    .bg(color_alpha(ACCENT, 0x18)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(offset))
                    .w(px(FIELD_PX))
                    .h(px(band))
                    .border_y_1()
                    .border_color(color_alpha(ACCENT, 0x80))
                    .bg(color_alpha(ACCENT, 0x18)),
            );
    } else if tuning.shape == DeadzoneShape::Radial && tuning.deadzone > 0.001 {
        let d_dz = (FIELD_PX * tuning.deadzone.clamp(0.0, 0.49)).max(6.0);
        let offset = (FIELD_PX - d_dz) * 0.5;
        field = field.child(
            div()
                .absolute()
                .left(px(offset))
                .top(px(offset))
                .size(px(d_dz))
                .rounded_full()
                .border_1()
                .border_color(color(ACCENT))
                .bg(color_alpha(ACCENT, 0x1E)),
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
                .border_color(color(TEXT_SECONDARY))
                .bg(color(WELL_BG)),
        )
        .child(
            div()
                .absolute()
                .left(px(out_left))
                .top(px(out_top))
                .size(px(DOT_PX))
                .rounded_full()
                .bg(color(ACCENT))
                .border_1()
                .border_color(color(WELL_BG)),
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
        format!("In {raw_pct}% · Out {out_pct}%")
    } else {
        "Move stick to test".to_owned()
    };

    let mut readout_el = div()
        .text_xs()
        .whitespace_nowrap()
        .text_color(color(TEXT_SECONDARY))
        .child(readout);
    if connected {
        readout_el = readout_el.font_family(FONT_MONO);
    }

    div()
        .w(px(144.0))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .child(field)
        .child(readout_el)
}

/// Single-accent vector Response Curve plot (gold curve & area fill, neutral grid & crosshairs)
pub fn response_curve_chart(raw: [u8; 2], tuning: StickTuning) -> Div {
    let raw_mag = (((raw[0] as f32 - 128.0) / 127.5).hypot((raw[1] as f32 - 128.0) / 127.5))
        .clamp(0.0, 1.0);

    let eval_output = move |x: f32| -> f32 {
        if tuning.shape == DeadzoneShape::Passthrough {
            return x.clamp(0.0, 1.0);
        }
        let dz = tuning.deadzone.clamp(0.0, 0.49);
        if x <= dz {
            return 0.0;
        }
        let outer = tuning.outer_deadzone.clamp(0.0, 0.40);
        let span = (1.0 - dz - outer).max(0.05);
        let norm = ((x - dz) / span).clamp(0.0, 1.0);
        let curved = tuning.curve.apply(norm);
        let adz = tuning.anti_deadzone.clamp(0.0, 0.45);
        (adz + curved * (1.0 - adz)).clamp(0.0, 1.0)
    };

    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .flex_shrink_0()
        .child(
            div()
                .size(px(128.0))
                .rounded(px(8.0))
                .bg(color(WELL_BG))
                .border_1()
                .border_color(color(BORDER_STRONG))
                .overflow_hidden()
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let pad = 12.0;
                            let ox = f32::from(bounds.origin.x) + pad;
                            let oy = f32::from(bounds.origin.y) + pad;
                            let w = (f32::from(bounds.size.width) - pad * 2.0).max(10.0);
                            let h = (f32::from(bounds.size.height) - pad * 2.0).max(10.0);
                            let pt = |u: f32, v: f32| point(px(ox + u * w), px(oy + (1.0 - v) * h));

                            // Subtle 4x4 grid
                            let mut grid = PathBuilder::stroke(px(1.0));
                            for i in 0..=4 {
                                let t = i as f32 / 4.0;
                                grid.move_to(pt(t, 0.0));
                                grid.line_to(pt(t, 1.0));
                                grid.move_to(pt(0.0, t));
                                grid.line_to(pt(1.0, t));
                            }
                            if let Ok(p) = grid.build() {
                                window.paint_path(p, color_alpha(BORDER_STRONG, 0x45));
                            }

                            // Linear reference diagonal
                            let mut diag = PathBuilder::stroke(px(1.0));
                            diag.move_to(pt(0.0, 0.0));
                            diag.line_to(pt(1.0, 1.0));
                            if let Ok(p) = diag.build() {
                                window.paint_path(p, color_alpha(TEXT_MUTED, 0x55));
                            }

                            // Deadzone shaded band on X-axis
                            let dz = tuning.deadzone.clamp(0.0, 0.9);
                            if dz > 0.005 {
                                let mut band = PathBuilder::fill();
                                band.move_to(pt(0.0, 0.0));
                                band.line_to(pt(dz, 0.0));
                                band.line_to(pt(dz, 1.0));
                                band.line_to(pt(0.0, 1.0));
                                band.close();
                                if let Ok(p) = band.build() {
                                    window.paint_path(p, color_alpha(TEXT_MUTED, 0x1A));
                                }
                            }

                            // Single-accent subtle curve area fill
                            let steps = 40usize;
                            let mut area = PathBuilder::fill();
                            area.move_to(pt(0.0, 0.0));
                            for i in 0..=steps {
                                let u = i as f32 / steps as f32;
                                area.line_to(pt(u, eval_output(u)));
                            }
                            area.line_to(pt(1.0, 0.0));
                            area.close();
                            if let Ok(p) = area.build() {
                                window.paint_path(p, color_alpha(ACCENT, 0x14));
                            }

                            // Main transfer curve stroke
                            let mut curve = PathBuilder::stroke(px(2.0));
                            for i in 0..=steps {
                                let u = i as f32 / steps as f32;
                                let p = pt(u, eval_output(u));
                                if i == 0 {
                                    curve.move_to(p);
                                } else {
                                    curve.line_to(p);
                                }
                            }
                            if let Ok(p) = curve.build() {
                                window.paint_path(p, color(ACCENT));
                            }

                            // Neutral crosshair guides + crisp solid position marker (zero halo)
                            let ly = eval_output(raw_mag);
                            let cx = ox + raw_mag * w;
                            let cy = oy + (1.0 - ly) * h;

                            let mut guides = PathBuilder::stroke(px(1.0));
                            guides.move_to(point(px(cx), px(oy + h)));
                            guides.line_to(point(px(cx), px(cy)));
                            guides.line_to(point(px(ox), px(cy)));
                            if let Ok(p) = guides.build() {
                                window.paint_path(p, color_alpha(TEXT_SECONDARY, 0x65));
                            }

                            let add_dot = |b: &mut PathBuilder, r: f32| {
                                let k = 0.552_284_7 * r;
                                b.move_to(point(px(cx), px(cy - r)));
                                b.cubic_bezier_to(
                                    point(px(cx + r), px(cy)),
                                    point(px(cx + k), px(cy - r)),
                                    point(px(cx + r), px(cy - k)),
                                );
                                b.cubic_bezier_to(
                                    point(px(cx), px(cy + r)),
                                    point(px(cx + r), px(cy + k)),
                                    point(px(cx + k), px(cy + r)),
                                );
                                b.cubic_bezier_to(
                                    point(px(cx - r), px(cy)),
                                    point(px(cx - k), px(cy + r)),
                                    point(px(cx - r), px(cy + k)),
                                );
                                b.cubic_bezier_to(
                                    point(px(cx), px(cy - r)),
                                    point(px(cx - r), px(cy - k)),
                                    point(px(cx - k), px(cy - r)),
                                );
                                b.close();
                            };
                            let mut rim = PathBuilder::fill();
                            add_dot(&mut rim, 5.0);
                            if let Ok(p) = rim.build() {
                                window.paint_path(p, color(WELL_BG));
                            }
                            let mut core = PathBuilder::fill();
                            add_dot(&mut core, 3.6);
                            if let Ok(p) = core.build() {
                                window.paint_path(p, color(TEXT));
                            }
                        },
                    )
                    .size_full(),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .w(px(128.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(color(TEXT_MUTED))
                        .child("In → Out"),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(TEXT_SECONDARY))
                        .child(tuning.curve.label()),
                ),
        )
}

/// Flat trigger travel gauge column
pub fn trigger_travel_gauge(
    label: &'static str,
    analog: u8,
    deadzone: u8,
    hair_trigger: u8,
    preset_label: &'static str,
    override_active: bool,
) -> Div {
    let pull_ratio = (analog as f32 / 255.0).clamp(0.0, 1.0);
    let dz_ratio = (deadzone as f32 / 255.0).clamp(0.0, 0.5);
    let hair_ratio = (hair_trigger as f32 / 255.0).clamp(0.15, 1.0);
    let pct = ((analog as f32 / 255.0) * 100.0).round() as u32;

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(color(TEXT))
                                .child(label),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(color(if override_active {
                                    ACCENT
                                } else {
                                    TEXT_MUTED
                                }))
                                .child(preset_label),
                        ),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_family(FONT_MONO)
                        .text_color(color(if analog > 4 { ACCENT } else { TEXT_SECONDARY }))
                        .child(format!("{pct}% · {analog}")),
                ),
        )
        .child(
            div()
                .relative()
                .w_full()
                .h(px(12.0))
                .rounded(px(6.0))
                .bg(color(WELL_BG))
                .border_1()
                .border_color(color(BORDER_STRONG))
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(relative(dz_ratio))
                        .bg(color_alpha(DANGER, 0x3A)),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(relative(pull_ratio))
                        .bg(color(ACCENT)),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(hair_ratio))
                        .ml(px(-1.0))
                        .w(px(2.0))
                        .bg(color(TEXT)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child(format!(
                    "Deadzone {}%",
                    (deadzone as f32 * 100.0 / 255.0).round() as u32
                ))
                .child(format!(
                    "Hair cap {}%",
                    (hair_trigger as f32 * 100.0 / 255.0).round() as u32
                )),
        )
}

/// Graduated tick-marked precision slider on 4px grid (28px tall, 16px thumb)
pub fn slider_bar(
    fill_ratio: f32,
    fill_color: u32,
    bounds_slot: usize,
    bounds_store: Rc<RefCell<[Bounds<Pixels>; SLIDER_SLOTS]>>,
) -> Div {
    let clamped = fill_ratio.clamp(0.0, 1.0);
    div()
        .relative()
        .flex()
        .items_center()
        .flex_1()
        .h(px(28.0))
        .cursor_pointer()
        .child(
            div().absolute().inset_0().child(
                canvas(
                    move |bounds, _, _| {
                        bounds_store.borrow_mut()[bounds_slot] = bounds;
                    },
                    move |bounds, _, window, _| {
                        let ox = f32::from(bounds.origin.x) + 8.0;
                        let w = (f32::from(bounds.size.width) - 16.0).max(10.0);
                        let bottom_y =
                            f32::from(bounds.origin.y) + f32::from(bounds.size.height) - 1.0;

                        let mut minor = PathBuilder::stroke(px(1.0));
                        let mut major = PathBuilder::stroke(px(1.2));
                        let segments = 14usize;
                        for i in 0..=segments {
                            let x = ox + (i as f32 / segments as f32) * w;
                            let is_major = i == 0 || i == segments || i == segments / 2;
                            if is_major {
                                major.move_to(point(px(x), px(bottom_y - 6.0)));
                                major.line_to(point(px(x), px(bottom_y)));
                            } else {
                                minor.move_to(point(px(x), px(bottom_y - 3.5)));
                                minor.line_to(point(px(x), px(bottom_y)));
                            }
                        }
                        if let Ok(p) = minor.build() {
                            window.paint_path(p, color_alpha(BORDER_STRONG, 0x90));
                        }
                        if let Ok(p) = major.build() {
                            window.paint_path(p, color_alpha(TEXT_MUTED, 0xAA));
                        }
                    },
                )
                .size_full(),
            ),
        )
        .child(
            div()
                .w_full()
                .mb(px(4.0))
                .h(px(6.0))
                .rounded_full()
                .bg(color(WELL_BG))
                .border_1()
                .border_color(color(BORDER_STRONG))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(clamped))
                        .rounded_full()
                        .bg(color(fill_color)),
                ),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .bottom(px(4.0))
                .left(px(8.0))
                .right(px(8.0))
                .flex()
                .items_center()
                .child(
                    div()
                        .absolute()
                        .left(relative(clamped))
                        .ml(px(-8.0))
                        .size(px(16.0))
                        .rounded_full()
                        .bg(color(fill_color))
                        .border_2()
                        .border_color(color(WELL_BG)),
                ),
        )
}

/// Interactive 2D HSV Saturation (X) × Brightness/Value (Y) color picker pad
pub fn sv_color_pad(
    hue: f32,
    sat: f32,
    val: f32,
    current_hex: u32,
    bounds_store: Rc<RefCell<Bounds<Pixels>>>,
) -> Div {
    let pure_hue_hex = rgb_to_u32(hsv_to_rgb(hue, 1.0, 1.0));
    let s = sat.clamp(0.0, 1.0);
    let v = val.clamp(0.0, 1.0);

    div()
        .relative()
        .w(px(192.0))
        .h(px(148.0))
        .flex_shrink_0()
        .cursor_pointer()
        .child(
            div()
                .relative()
                .size_full()
                .rounded(px(8.0))
                .border_1()
                .border_color(color(BORDER_STRONG))
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(linear_gradient(
                            90.0,
                            linear_color_stop(color(0xFFFFFF), 0.0),
                            linear_color_stop(color(pure_hue_hex), 1.0),
                        )),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(linear_gradient(
                            180.0,
                            linear_color_stop(color_alpha(0x000000, 0x00), 0.0),
                            linear_color_stop(color_alpha(0x000000, 0xFF), 1.0),
                        )),
                ),
        )
        .child(
            div().absolute().inset_0().child(
                canvas(
                    move |bounds, _, _| {
                        *bounds_store.borrow_mut() = bounds;
                    },
                    |_, _, _, _| {},
                )
                .size_full(),
            ),
        )
        .child(
            div()
                .absolute()
                .left(relative(s))
                .top(relative(1.0 - v))
                .ml(px(-7.0))
                .mt(px(-7.0))
                .size(px(14.0))
                .rounded_full()
                .bg(color(current_hex))
                .border_2()
                .border_color(color(0xFFFFFF))
                .shadow(overlay_shadow()),
        )
}

/// Interactive full-spectrum 0–360° Hue bar with draggable circular thumb
pub fn hue_slider_bar(
    hue: f32,
    bounds_slot: usize,
    bounds_store: Rc<RefCell<[Bounds<Pixels>; SLIDER_SLOTS]>>,
) -> Div {
    let ratio = (hue.rem_euclid(360.0) / 360.0).clamp(0.0, 1.0);
    let thumb_hex = rgb_to_u32(hsv_to_rgb(hue, 1.0, 1.0));
    let segments: [(u32, u32); 6] = [
        (0xFF0000, 0xFFFF00),
        (0xFFFF00, 0x00FF00),
        (0x00FF00, 0x00FFFF),
        (0x00FFFF, 0x0000FF),
        (0x0000FF, 0xFF00FF),
        (0xFF00FF, 0xFF0000),
    ];

    let mut spectrum = div()
        .w_full()
        .h(px(10.0))
        .rounded_full()
        .border_1()
        .border_color(color(BORDER_STRONG))
        .overflow_hidden()
        .flex();
    for (c0, c1) in segments {
        spectrum = spectrum.child(div().flex_1().h_full().bg(linear_gradient(
            90.0,
            linear_color_stop(color(c0), 0.0),
            linear_color_stop(color(c1), 1.0),
        )));
    }

    div()
        .relative()
        .flex()
        .items_center()
        .w_full()
        .h(px(20.0))
        .cursor_pointer()
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
        .child(spectrum)
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(7.0))
                .right(px(7.0))
                .flex()
                .items_center()
                .child(
                    div()
                        .absolute()
                        .left(relative(ratio))
                        .ml(px(-7.0))
                        .size(px(14.0))
                        .rounded_full()
                        .bg(color(thumb_hex))
                        .border_2()
                        .border_color(color(0xFFFFFF)),
                ),
        )
}

/// Interactive single-channel RGB gradient bar (shows exact live color sweep 0..255)
pub fn rgb_channel_bar(
    min_hex: u32,
    max_hex: u32,
    fill_ratio: f32,
    thumb_hex: u32,
    bounds_slot: usize,
    bounds_store: Rc<RefCell<[Bounds<Pixels>; SLIDER_SLOTS]>>,
) -> Div {
    let ratio = fill_ratio.clamp(0.0, 1.0);
    div()
        .relative()
        .flex()
        .items_center()
        .flex_1()
        .h(px(20.0))
        .cursor_pointer()
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
        .child(
            div()
                .w_full()
                .h(px(8.0))
                .rounded_full()
                .border_1()
                .border_color(color(BORDER_STRONG))
                .overflow_hidden()
                .bg(linear_gradient(
                    90.0,
                    linear_color_stop(color(min_hex), 0.0),
                    linear_color_stop(color(max_hex), 1.0),
                )),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(7.0))
                .right(px(7.0))
                .flex()
                .items_center()
                .child(
                    div()
                        .absolute()
                        .left(relative(ratio))
                        .ml(px(-7.0))
                        .size(px(14.0))
                        .rounded_full()
                        .bg(color(thumb_hex))
                        .border_2()
                        .border_color(color(0xFFFFFF)),
                ),
        )
}

struct TextTooltip {
    text: SharedString,
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().pl_2().pt_2().child(
            div()
                .px_3()
                .py_2()
                .rounded(px(8.0))
                .bg(color(RAIL_BG))
                .border_1()
                .border_color(color(BORDER_STRONG))
                .shadow(overlay_shadow())
                .text_size(px(13.0))
                .font_family(FONT_SANS)
                .text_color(color(TEXT))
                .whitespace_nowrap()
                .child(self.text.clone()),
        )
    }
}

pub fn text_tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_window, cx| {
        let text = text.clone();
        cx.new(|_| TextTooltip { text }).into()
    }
}

/// Compact key-value tile for the Device Info diagnostics grid
pub fn info_kv_tile(label: &'static str, value: impl Into<SharedString>, mono: bool) -> Div {
    let mut val_el = div()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(color(TEXT))
        .whitespace_nowrap()
        .child(value.into());
    if mono {
        val_el = val_el.font_family(FONT_MONO);
    }
    div()
        .flex_1()
        .min_w(px(180.0))
        .px_3()
        .py_2()
        .rounded(px(8.0))
        .bg(color(WELL_BG))
        .border_1()
        .border_color(color(BORDER))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child(label),
        )
        .child(val_el)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stick_point_in_circle_centers_and_clamps_diagonals() {
        let field = 128.0;
        let dot = 10.0;
        let (cx, cy) = stick_point_in_circle(128, 128, field, dot);
        let expected_center = (field - dot) * 0.5;
        assert!((cx - expected_center).abs() < 0.01);
        assert!((cy - expected_center).abs() < 0.01);

        let (dx, dy) = stick_point_in_circle(255, 255, field, dot);
        assert!(dx >= 0.0 && dx <= field - dot);
        assert!(dy >= 0.0 && dy <= field - dot);
        let dist_from_center =
            ((dx + dot * 0.5) - field * 0.5).hypot((dy + dot * 0.5) - field * 0.5);
        assert!(dist_from_center <= field * 0.5 - 3.0 + 0.01);
    }
}
