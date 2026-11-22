pub mod theme;
mod widgets;

use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, FontWeight, SharedString, Task, Window, div, prelude::*, px};

use self::theme::{
    ACCENT, BG, BORDER, DANGER, SURFACE, SURFACE_ALT, SURFACE_HOVER, TEXT, TEXT_MUTED, WARN, color,
};
use self::widgets::{
    button_pill, coming_soon_pill, driver_badge, meter_row, pill_button, section_card, stick_box,
};
use crate::emu::EmulationMode;
use crate::engine::{ControllerProfile, Engine, SeqLock, TelemetrySnapshot};
use crate::protocol::{AdaptiveTriggerEffect, BatteryStatus, DPad, DualSenseButtons, Transport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerPreset {
    Off,
    Rigid,
    Weapon,
    Pulse,
}

impl TriggerPreset {
    const ALL: [Self; 4] = [Self::Off, Self::Rigid, Self::Weapon, Self::Pulse];

    fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Rigid => "Rigid",
            Self::Weapon => "Weapon",
            Self::Pulse => "Pulse",
        }
    }

    fn effect(self) -> AdaptiveTriggerEffect {
        match self {
            Self::Off => AdaptiveTriggerEffect::Off,
            Self::Rigid => AdaptiveTriggerEffect::Feedback {
                start_position: 1,
                strength: 6,
            },
            Self::Weapon => AdaptiveTriggerEffect::Weapon {
                start_position: 2,
                end_position: 6,
                strength: 7,
            },
            Self::Pulse => AdaptiveTriggerEffect::Vibration {
                position: 1,
                amplitude: 5,
                frequency: 35,
            },
        }
    }
}

const RGB_SWATCHES: [(&str, [u8; 3]); 8] = [
    ("Off", [0, 0, 0]),
    ("Blue", [0, 96, 255]),
    ("White", [210, 220, 235]),
    ("Cyan", [0, 196, 220]),
    ("Green", [32, 200, 96]),
    ("Amber", [245, 158, 11]),
    ("Red", [235, 56, 56]),
    ("Violet", [148, 86, 245]),
];

const PLAYER_LED_OPTIONS: [(&str, u8); 5] = [
    ("Off", 0x00),
    ("1", 0x04),
    ("2", 0x0A),
    ("3", 0x15),
    ("4", 0x1B),
];

pub struct DsMasterView {
    engine: Option<Engine>,
    engine_error: Option<String>,
    vigem_available: bool,
    hidhide_available: bool,
    profile: ControllerProfile,
    left_preset: TriggerPreset,
    right_preset: TriggerPreset,
    last_topology_version: u64,
    handles: Vec<Arc<SeqLock<TelemetrySnapshot>>>,
    selected_idx: usize,
    snapshots: Vec<TelemetrySnapshot>,
    _watch_task: Task<()>,
}

impl DsMasterView {
    pub fn new(
        engine: Result<Engine, String>,
        vigem_available: bool,
        hidhide_available: bool,
        profile: ControllerProfile,
        cx: &mut Context<Self>,
    ) -> Self {
        let (engine, engine_error) = match engine {
            Ok(e) => (Some(e), None),
            Err(err) => (None, Some(err)),
        };

        let mut this = Self {
            engine,
            engine_error,
            vigem_available,
            hidhide_available,
            profile,
            left_preset: TriggerPreset::Off,
            right_preset: TriggerPreset::Off,
            last_topology_version: 0,
            handles: Vec::new(),
            selected_idx: 0,
            snapshots: Vec::new(),
            _watch_task: Task::ready(()),
        };
        this.sync_handles_if_changed();
        this.read_snapshots();

        // Low-frequency hot-plug watcher: wakes the UI only when controller count changes.
        this._watch_task = cx.spawn(async move |weak_this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let res = weak_this.update(cx, |view, cx| {
                    if view.sync_handles_if_changed() {
                        view.read_snapshots();
                        cx.notify();
                    }
                });
                if res.is_err() {
                    break;
                }
            }
        });

        this
    }

    fn sync_handles_if_changed(&mut self) -> bool {
        let Some(engine) = &self.engine else {
            return false;
        };
        let ver = engine.topology_version();
        if ver == self.last_topology_version {
            return false;
        }
        self.last_topology_version = ver;
        self.handles = engine.telemetry_handles();
        if self.selected_idx >= self.handles.len() {
            self.selected_idx = 0;
        }
        true
    }

    fn read_snapshots(&mut self) {
        self.snapshots.clear();
        for handle in &self.handles {
            self.snapshots.push(handle.read());
        }
    }

    fn push_profile(&mut self, cx: &mut Context<Self>) {
        if let Some(engine) = &self.engine {
            engine.update_profile(self.profile);
        }
        cx.notify();
    }

    fn selected_snapshot(&self) -> Option<&TelemetrySnapshot> {
        self.snapshots.get(self.selected_idx)
    }
}

impl Drop for DsMasterView {
    fn drop(&mut self) {
        if let Some(mut engine) = self.engine.take() {
            engine.shutdown();
        }
    }
}

impl Render for DsMasterView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_handles_if_changed();
        self.read_snapshots();

        if !self.handles.is_empty() {
            window.request_animation_frame();
        }

        let snap = self.selected_snapshot().copied();
        let count = self.handles.len();

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .px_4()
            .py_2p5()
            .bg(color(SURFACE))
            .border_b_1()
            .border_color(color(BORDER))
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(TEXT))
                    .child("DSMaster"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(driver_badge("ViGEmBus", self.vigem_available))
                    .child(driver_badge("HidHide", self.hidhide_available)),
            );

        let controller_strip = if let Some(err) = &self.engine_error {
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(color(SURFACE))
                .border_1()
                .border_color(color(DANGER))
                .text_xs()
                .text_color(color(DANGER))
                .child(format!("Engine error: {err}"))
        } else if let Some(s) = snap {
            let model_name = if s.is_edge {
                "DualSense Edge"
            } else {
                "DualSense"
            };
            let transport_label = match s.transport {
                Transport::Usb => "USB",
                Transport::Bluetooth => "Bluetooth",
            };
            let emu_label = match s.emulation {
                EmulationMode::Xbox360 => "X360",
                EmulationMode::None => "Passthrough",
            };
            let bat_label = match s.battery_status {
                BatteryStatus::Charging => format!("{}% (Charging)", s.battery_percent),
                BatteryStatus::Full => "100% (Full)".to_owned(),
                _ => format!("{}%", s.battery_percent),
            };

            let mut left = div().flex().items_center().gap_2();
            if count > 1 {
                for idx in 0..count {
                    let active = idx == self.selected_idx;
                    left = left.child(
                        pill_button(
                            SharedString::from(format!("ctrl-{idx}")),
                            format!("Controller {}", idx + 1),
                            active,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_idx = idx;
                            cx.notify();
                        })),
                    );
                }
            }
            left = left
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(TEXT))
                        .child(model_name),
                )
                .child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .bg(color(SURFACE_ALT))
                        .text_xs()
                        .text_color(color(TEXT_MUTED))
                        .child(transport_label),
                )
                .child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .bg(color(SURFACE_ALT))
                        .text_xs()
                        .text_color(color(TEXT_MUTED))
                        .child(emu_label),
                );

            let right = div()
                .flex()
                .items_center()
                .gap_4()
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child(format!("Battery: {bat_label}"))
                .child(format!("Poll: {:.0} Hz", s.polling_hz))
                .child(format!(
                    "Proc: {:.1} us (max {} us)",
                    s.proc_us_ema, s.proc_us_max
                ))
                .child(format!("Pkts: {}", s.packet_count));

            div()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(color(SURFACE))
                .border_1()
                .border_color(color(BORDER))
                .child(left)
                .child(right)
        } else {
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(color(SURFACE))
                .border_1()
                .border_color(color(BORDER))
                .child(
                    div()
                        .text_xs()
                        .text_color(color(TEXT))
                        .child("No DualSense controller connected"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(color(TEXT_MUTED))
                        .child("Connect via USB or Bluetooth; settings apply automatically"),
                )
        };

        let emu_card = section_card("EMULATION MODE").child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    pill_button(
                        "emu-none",
                        "Off (Passthrough)",
                        self.profile.emulation == EmulationMode::None,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.profile.emulation = EmulationMode::None;
                        this.push_profile(cx);
                    })),
                )
                .child(
                    pill_button(
                        "emu-x360",
                        "Xbox 360 (XInput)",
                        self.profile.emulation == EmulationMode::Xbox360,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.profile.emulation = EmulationMode::Xbox360;
                        this.push_profile(cx);
                    })),
                )
                .child(coming_soon_pill("DualShock 4 (Coming Soon)"))
                .child(coming_soon_pill("Virtual DualSense (Coming Soon)")),
        );

        let mut l2_row = div().flex().items_center().gap_1p5().child(
            div()
                .w(px(28.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("L2"),
        );
        for preset in TriggerPreset::ALL {
            let active = self.left_preset == preset;
            l2_row = l2_row.child(
                pill_button(
                    SharedString::from(format!("l2-{}", preset.label())),
                    preset.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.left_preset = preset;
                    this.profile.left_trigger = preset.effect();
                    this.push_profile(cx);
                })),
            );
        }

        let mut r2_row = div().flex().items_center().gap_1p5().child(
            div()
                .w(px(28.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("R2"),
        );
        for preset in TriggerPreset::ALL {
            let active = self.right_preset == preset;
            r2_row = r2_row.child(
                pill_button(
                    SharedString::from(format!("r2-{}", preset.label())),
                    preset.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.right_preset = preset;
                    this.profile.right_trigger = preset.effect();
                    this.push_profile(cx);
                })),
            );
        }

        let triggers_card = section_card("ADAPTIVE TRIGGERS")
            .child(l2_row)
            .child(r2_row);

        let mut swatch_row = div().flex().flex_wrap().gap_1p5();
        for (name, rgb_val) in RGB_SWATCHES {
            let active = self.profile.lightbar_rgb == rgb_val;
            let hex =
                ((rgb_val[0] as u32) << 16) | ((rgb_val[1] as u32) << 8) | (rgb_val[2] as u32);
            swatch_row = swatch_row.child(
                div()
                    .id(SharedString::from(format!("rgb-{name}")))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(color(if active { ACCENT } else { BORDER }))
                    .bg(color(SURFACE_ALT))
                    .cursor_pointer()
                    .hover(|s| s.bg(color(SURFACE_HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.profile.lightbar_rgb = rgb_val;
                        this.push_profile(cx);
                    }))
                    .child(
                        div()
                            .size(px(10.0))
                            .rounded_full()
                            .border_1()
                            .border_color(color(BORDER))
                            .bg(color(hex)),
                    )
                    .child(div().text_xs().text_color(color(TEXT)).child(name)),
            );
        }

        let mut player_row = div().flex().items_center().gap_1p5().child(
            div()
                .w(px(76.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("Player LEDs"),
        );
        for (label, mask) in PLAYER_LED_OPTIONS {
            let active = self.profile.player_leds == mask;
            player_row = player_row.child(
                pill_button(SharedString::from(format!("pled-{label}")), label, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.profile.player_leds = mask;
                        this.push_profile(cx);
                    }),
                ),
            );
        }

        let rumble_row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(76.0))
                    .text_xs()
                    .text_color(color(TEXT_MUTED))
                    .child("Rumble"),
            )
            .child(
                pill_button(
                    "rumble-toggle",
                    if self.profile.enable_rumble {
                        "Enabled"
                    } else {
                        "Muted"
                    },
                    self.profile.enable_rumble,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.profile.enable_rumble = !this.profile.enable_rumble;
                    this.push_profile(cx);
                })),
            );

        let hardware_card = section_card("LIGHTBAR, LEDS & RUMBLE")
            .child(swatch_row)
            .child(player_row)
            .child(rumble_row);

        let s = snap.unwrap_or_default();
        let b = s.buttons;
        let dpad_up = matches!(s.dpad, DPad::North | DPad::NorthEast | DPad::NorthWest);
        let dpad_down = matches!(s.dpad, DPad::South | DPad::SouthEast | DPad::SouthWest);
        let dpad_left = matches!(s.dpad, DPad::West | DPad::NorthWest | DPad::SouthWest);
        let dpad_right = matches!(s.dpad, DPad::East | DPad::NorthEast | DPad::SouthEast);

        let monitor_card = section_card("LIVE INPUT MONITOR")
            .flex_1()
            .child(
                div()
                    .flex()
                    .justify_around()
                    .py_1()
                    .child(stick_box("Left Stick", s.left_stick[0], s.left_stick[1]))
                    .child(stick_box("Right Stick", s.right_stick[0], s.right_stick[1])),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(meter_row("L2", s.l2_analog, ACCENT))
                    .child(meter_row("R2", s.r2_analog, ACCENT))
                    .child(meter_row("Rumble L", s.rumble_large, WARN))
                    .child(meter_row("Rumble R", s.rumble_small, WARN)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1p5()
                    .pt_1()
                    .child(button_pill("Up", dpad_up))
                    .child(button_pill("Down", dpad_down))
                    .child(button_pill("Left", dpad_left))
                    .child(button_pill("Right", dpad_right))
                    .child(button_pill("Cross", b.contains(DualSenseButtons::CROSS)))
                    .child(button_pill("Circle", b.contains(DualSenseButtons::CIRCLE)))
                    .child(button_pill("Square", b.contains(DualSenseButtons::SQUARE)))
                    .child(button_pill(
                        "Triangle",
                        b.contains(DualSenseButtons::TRIANGLE),
                    ))
                    .child(button_pill("L1", b.contains(DualSenseButtons::L1)))
                    .child(button_pill("R1", b.contains(DualSenseButtons::R1)))
                    .child(button_pill("L3", b.contains(DualSenseButtons::L3)))
                    .child(button_pill("R3", b.contains(DualSenseButtons::R3)))
                    .child(button_pill("Create", b.contains(DualSenseButtons::CREATE)))
                    .child(button_pill(
                        "Options",
                        b.contains(DualSenseButtons::OPTIONS),
                    ))
                    .child(button_pill("PS", b.contains(DualSenseButtons::PS_HOME)))
                    .child(button_pill(
                        "Touchpad",
                        b.contains(DualSenseButtons::TOUCHPAD),
                    ))
                    .child(button_pill("Mic", b.contains(DualSenseButtons::MIC_MUTE))),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(color(BG))
            .text_color(color(TEXT))
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_3()
                    .p_4()
                    .child(controller_strip)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .w(px(430.0))
                                    .gap_3()
                                    .child(emu_card)
                                    .child(triggers_card)
                                    .child(hardware_card),
                            )
                            .child(monitor_card),
                    ),
            )
    }
}
