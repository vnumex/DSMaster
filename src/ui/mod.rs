pub mod theme;
mod widgets;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{
    Bounds, Context, Div, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    SharedString, Task, Window, div, prelude::*, px,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOW, SetForegroundWindow, ShowWindow};

use self::theme::{
    ACCENT, BG, BORDER, DANGER, SURFACE, SURFACE_ALT, SURFACE_HOVER, TEXT, TEXT_MUTED, WARN, color,
};
use self::widgets::{
    button_pill, coming_soon_pill, deadzone_showcase, driver_badge, meter_row, pill_button,
    section_card, setting_row, slider_bar, stick_box,
};
use crate::emu::EmulationMode;
use crate::engine::profile::{
    ButtonPreset, GyroAimMode, ResponseCurve, StickTuning, TriggerPreset,
};
use crate::engine::profiles_store::{
    AppConfig, NamedProfile, config_path, load_config, open_config_dir, save_config,
};
use crate::engine::{Engine, SeqLock, TelemetrySnapshot};
use crate::protocol::{BatteryStatus, DPad, DualSenseButtons, Transport};
use crate::startup::{is_run_at_startup_enabled, set_run_at_startup};
use crate::tray::{SystemTray, TrayAction, TrayState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    Overview,
    Tuning,
    Profiles,
    Settings,
}

#[derive(Clone)]
pub struct WindowControlState {
    pub raw_hwnd: isize,
    pub work_in_background: Arc<AtomicBool>,
    pub window_hidden: Arc<AtomicBool>,
    pub force_quit: Arc<AtomicBool>,
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

const MAX_STICK_DZ: f32 = 0.35;
const MAX_TRIGGER_DZ_PCT: f32 = 35.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveSlider {
    LeftInner,
    LeftAnti,
    RightInner,
    RightAnti,
    Trigger,
}

impl ActiveSlider {
    #[inline]
    fn slot(self) -> usize {
        match self {
            Self::LeftInner => 0,
            Self::LeftAnti => 1,
            Self::RightInner => 2,
            Self::RightAnti => 3,
            Self::Trigger => 4,
        }
    }
}

const HAIR_TRIGGER_STEPS: [(&str, u8); 4] =
    [("Full", 255), ("75%", 190), ("50%", 128), ("25%", 64)];

const SENSITIVITY_STEPS: [(&str, f32); 4] =
    [("0.5x", 0.5), ("1.0x", 1.0), ("1.5x", 1.5), ("2.0x", 2.0)];

pub struct DsMasterView {
    engine: Option<Engine>,
    engine_error: Option<String>,
    status_notice: Option<String>,
    vigem_available: bool,
    hidhide_available: bool,
    config: AppConfig,
    tab: ActiveTab,
    last_topology_version: u64,
    handles: Vec<Arc<SeqLock<TelemetrySnapshot>>>,
    selected_idx: usize,
    snapshots: Vec<TelemetrySnapshot>,
    window_ctl: WindowControlState,
    run_at_startup: bool,
    tray_tx: crossbeam_channel::Sender<TrayAction>,
    tray: Option<SystemTray>,
    active_slider: Option<(ActiveSlider, f32, f32)>,
    slider_bounds: Rc<RefCell<[Bounds<Pixels>; 5]>>,
    _watch_task: Task<()>,
    _tray_task: Task<()>,
}

impl DsMasterView {
    pub fn new(
        engine: Result<Engine, String>,
        vigem_available: bool,
        hidhide_available: bool,
        config: AppConfig,
        window_ctl: WindowControlState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (engine, engine_error) = match engine {
            Ok(e) => (Some(e), None),
            Err(err) => (None, Some(err)),
        };

        let (tray_tx, tray_rx) = crossbeam_channel::bounded::<TrayAction>(16);
        let run_at_startup = is_run_at_startup_enabled();

        let mut this = Self {
            engine,
            engine_error,
            status_notice: None,
            vigem_available,
            hidhide_available,
            config,
            tab: ActiveTab::Overview,
            last_topology_version: 0,
            handles: Vec::new(),
            selected_idx: 0,
            snapshots: Vec::new(),
            window_ctl,
            run_at_startup,
            tray_tx,
            tray: None,
            active_slider: None,
            slider_bounds: Rc::new(RefCell::new([Bounds::default(); 5])),
            _watch_task: Task::ready(()),
            _tray_task: Task::ready(()),
        };
        this.sync_from_engine();
        this.read_snapshots();
        this.sync_tray_state();

        this._watch_task = cx.spawn(async move |weak_this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let res = weak_this.update(cx, |view, cx| {
                    if view.sync_from_engine() {
                        view.read_snapshots();
                        view.sync_tray_state();
                        cx.notify();
                    }
                });
                if res.is_err() {
                    break;
                }
            }
        });

        this._tray_task = cx.spawn_in(window, async move |weak_this, cx| {
            loop {
                let rx_clone = tray_rx.clone();
                let maybe_action = cx
                    .background_executor()
                    .spawn(async move { rx_clone.recv().ok() })
                    .await;
                let Some(action) = maybe_action else {
                    break;
                };
                let res = weak_this.update_in(cx, |view, window, cx| {
                    view.handle_tray_action(action, window, cx);
                });
                if res.is_err() {
                    break;
                }
            }
        });

        this
    }

    fn sync_from_engine(&mut self) -> bool {
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
        let engine_prof = engine.active_profile_name();
        if self.config.active_profile != engine_prof {
            self.config.active_profile = engine_prof;
        }
        true
    }

    fn read_snapshots(&mut self) {
        self.snapshots.clear();
        for handle in &self.handles {
            self.snapshots.push(handle.read());
        }
    }

    fn commit_config(&mut self, cx: &mut Context<Self>) {
        self.status_notice = None;
        if let Some(engine) = &self.engine {
            engine.apply_config(&self.config);
        }
        if let Err(err) = save_config(&self.config) {
            self.status_notice = Some(format!("Failed to save config: {err}"));
        }
        self.sync_tray_state();
        cx.notify();
    }

    fn toggle_cloak(&mut self, cx: &mut Context<Self>) {
        if !self.hidhide_available {
            self.status_notice = Some("HidHide driver not installed".to_owned());
            cx.notify();
            return;
        }
        let next = !self.config.cloak_physical;
        if let Some(engine) = &self.engine {
            match engine.set_cloak_enabled(next) {
                Ok(()) => {
                    self.config.cloak_physical = next;
                    self.status_notice = None;
                    let _ = save_config(&self.config);
                }
                Err(e) => {
                    self.status_notice = Some(format!("HidHide error: {e}"));
                }
            }
        }
        self.sync_tray_state();
        cx.notify();
    }

    fn build_tray_state(&self) -> TrayState {
        let cloak_enabled = self
            .engine
            .as_ref()
            .map(Engine::is_cloak_enabled)
            .unwrap_or(self.config.cloak_physical);
        TrayState {
            window_visible: !self.window_ctl.window_hidden.load(Ordering::Acquire),
            emulation: self.config.active_settings().emulation,
            cloak_enabled,
            hidhide_available: self.hidhide_available,
            connected_count: self.handles.len(),
            active_profile: self.config.active_profile.clone(),
        }
    }

    fn sync_tray_state(&mut self) {
        self.window_ctl
            .work_in_background
            .store(self.config.work_in_background, Ordering::Release);
        if self.config.work_in_background {
            let state = self.build_tray_state();
            if let Some(tray) = &self.tray {
                tray.update_state(state);
            } else {
                self.tray = SystemTray::start(self.tray_tx.clone(), state);
            }
        } else {
            self.tray = None;
        }
    }

    pub fn hide_to_tray(&mut self, cx: &mut Context<Self>) {
        self.window_ctl.window_hidden.store(true, Ordering::Release);
        if self.window_ctl.raw_hwnd != 0 {
            let hwnd = HWND(self.window_ctl.raw_hwnd as *mut core::ffi::c_void);
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
        self.sync_tray_state();
        cx.notify();
    }

    pub fn show_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_ctl
            .window_hidden
            .store(false, Ordering::Release);
        if self.window_ctl.raw_hwnd != 0 {
            let hwnd = HWND(self.window_ctl.raw_hwnd as *mut core::ffi::c_void);
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
        }
        window.activate_window();
        self.sync_tray_state();
        cx.notify();
    }

    pub fn shutdown_all(&mut self) {
        self.tray = None;
        if let Some(mut engine) = self.engine.take() {
            engine.shutdown();
        }
    }

    fn quit_app(&mut self, cx: &mut Context<Self>) {
        self.window_ctl.force_quit.store(true, Ordering::Release);
        self.shutdown_all();
        cx.quit();
    }

    fn handle_tray_action(
        &mut self,
        action: TrayAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            TrayAction::ShowWindow => self.show_window(window, cx),
            TrayAction::ToggleVisibility => {
                if self.window_ctl.window_hidden.load(Ordering::Acquire) {
                    self.show_window(window, cx);
                } else {
                    self.hide_to_tray(cx);
                }
            }
            TrayAction::SetEmulation(mode) => {
                self.config.active_settings_mut().emulation = mode;
                self.commit_config(cx);
            }
            TrayAction::ToggleCloak => self.toggle_cloak(cx),
            TrayAction::Quit => self.quit_app(cx),
        }
    }

    fn selected_snapshot(&self) -> Option<&TelemetrySnapshot> {
        self.snapshots.get(self.selected_idx)
    }

    fn start_slider_drag(&mut self, slider: ActiveSlider, mouse_x: Pixels, cx: &mut Context<Self>) {
        let bounds = self.slider_bounds.borrow()[slider.slot()];
        let origin_x = f32::from(bounds.origin.x);
        let width = f32::from(bounds.size.width);
        if width > 1.0 {
            self.active_slider = Some((slider, origin_x, width));
            let t = ((f32::from(mouse_x) - origin_x) / width).clamp(0.0, 1.0);
            self.apply_slider_live(slider, t, cx);
        }
    }

    fn apply_slider_live(&mut self, slider: ActiveSlider, t: f32, cx: &mut Context<Self>) {
        let t = t.clamp(0.0, 1.0);
        let stick_val = ((t * MAX_STICK_DZ * 100.0).round() / 100.0).clamp(0.0, MAX_STICK_DZ);
        let trig_pct = (t * MAX_TRIGGER_DZ_PCT)
            .round()
            .clamp(0.0, MAX_TRIGGER_DZ_PCT);
        let trig_u8 = (trig_pct * 255.0 / 100.0).round() as u8;

        let s = self.config.active_settings_mut();
        match slider {
            ActiveSlider::LeftInner => s.left_stick.deadzone = stick_val,
            ActiveSlider::LeftAnti => s.left_stick.anti_deadzone = stick_val,
            ActiveSlider::RightInner => s.right_stick.deadzone = stick_val,
            ActiveSlider::RightAnti => s.right_stick.anti_deadzone = stick_val,
            ActiveSlider::Trigger => s.triggers.deadzone = trig_u8,
        }

        if let Some(engine) = &self.engine {
            engine.apply_config(&self.config);
        }
        cx.notify();
    }

    fn finish_slider_drag(&mut self, cx: &mut Context<Self>) {
        if self.active_slider.take().is_some() {
            self.commit_config(cx);
        }
    }

    fn render_slider_row(
        &self,
        title: &'static str,
        value_label: String,
        ratio: f32,
        fill_color: u32,
        slider: ActiveSlider,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_xs().text_color(color(TEXT_MUTED)).child(title))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color(TEXT))
                            .child(value_label),
                    ),
            )
            .child(
                slider_bar(
                    ratio,
                    fill_color,
                    slider.slot(),
                    Rc::clone(&self.slider_bounds),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.start_slider_drag(slider, ev.position.x, cx);
                    }),
                ),
            )
    }

    fn render_overview(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied();
        let count = self.handles.len();

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
                EmulationMode::DualShock4 => "DS4",
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
                        profile.emulation == EmulationMode::None,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.config.active_settings_mut().emulation = EmulationMode::None;
                        this.commit_config(cx);
                    })),
                )
                .child(
                    pill_button(
                        "emu-x360",
                        "Xbox 360 (XInput)",
                        profile.emulation == EmulationMode::Xbox360,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.config.active_settings_mut().emulation = EmulationMode::Xbox360;
                        this.commit_config(cx);
                    })),
                )
                .child(
                    pill_button(
                        "emu-ds4",
                        "DualShock 4",
                        profile.emulation == EmulationMode::DualShock4,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.config.active_settings_mut().emulation = EmulationMode::DualShock4;
                        this.commit_config(cx);
                    })),
                )
                .child(coming_soon_pill("Virtual DualSense (Coming Soon)")),
        );

        let trig_mode_row = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(
                div()
                    .w(px(36.0))
                    .text_xs()
                    .text_color(color(TEXT_MUTED))
                    .child("Mode"),
            )
            .child(
                pill_button(
                    "trig-untouched",
                    "Default (Untouched)",
                    !profile.override_triggers,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.config.active_settings_mut().override_triggers = false;
                    this.commit_config(cx);
                })),
            )
            .child(
                pill_button("trig-custom", "Custom", profile.override_triggers).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.config.active_settings_mut().override_triggers = true;
                        this.commit_config(cx);
                    }),
                ),
            );

        let mut l2_row = div().flex().items_center().gap_1p5().child(
            div()
                .w(px(36.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("L2"),
        );
        for preset in TriggerPreset::ALL {
            let active = profile.override_triggers && profile.left_trigger_preset == preset;
            l2_row = l2_row.child(
                pill_button(
                    SharedString::from(format!("l2-{}", preset.label())),
                    preset.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.override_triggers = true;
                    s.left_trigger_preset = preset;
                    this.commit_config(cx);
                })),
            );
        }

        let mut r2_row = div().flex().items_center().gap_1p5().child(
            div()
                .w(px(36.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("R2"),
        );
        for preset in TriggerPreset::ALL {
            let active = profile.override_triggers && profile.right_trigger_preset == preset;
            r2_row = r2_row.child(
                pill_button(
                    SharedString::from(format!("r2-{}", preset.label())),
                    preset.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.override_triggers = true;
                    s.right_trigger_preset = preset;
                    this.commit_config(cx);
                })),
            );
        }

        let triggers_card = section_card("ADAPTIVE TRIGGERS")
            .child(trig_mode_row)
            .child(l2_row)
            .child(r2_row);

        let mut swatch_row = div().flex().flex_wrap().gap_1p5().child(
            pill_button(
                "rgb-untouched",
                "Default (Untouched)",
                !profile.override_lightbar,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.config.active_settings_mut().override_lightbar = false;
                this.commit_config(cx);
            })),
        );
        for (name, rgb_val) in RGB_SWATCHES {
            let active = profile.override_lightbar && profile.lightbar_rgb == rgb_val;
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
                        let s = this.config.active_settings_mut();
                        s.override_lightbar = true;
                        s.lightbar_rgb = rgb_val;
                        this.commit_config(cx);
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
            let active = profile.override_lightbar && profile.player_leds == mask;
            player_row = player_row.child(
                pill_button(SharedString::from(format!("pled-{label}")), label, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.override_lightbar = true;
                        s.player_leds = mask;
                        this.commit_config(cx);
                    }),
                ),
            );
        }

        let quick_toggles = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                pill_button(
                    "rumble-toggle",
                    if profile.enable_rumble {
                        "Rumble: On"
                    } else {
                        "Rumble: Off"
                    },
                    profile.enable_rumble,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.enable_rumble = !s.enable_rumble;
                    this.commit_config(cx);
                })),
            )
            .child(
                pill_button(
                    "tp-mouse-quick",
                    if profile.touchpad_mouse {
                        "Touchpad Mouse: On"
                    } else {
                        "Touchpad Mouse: Off"
                    },
                    profile.touchpad_mouse,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.touchpad_mouse = !s.touchpad_mouse;
                    this.commit_config(cx);
                })),
            )
            .child(
                pill_button(
                    "gyro-quick",
                    format!("Gyro: {}", profile.gyro_mode.label()),
                    profile.gyro_mode != GyroAimMode::Off,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.gyro_mode = match s.gyro_mode {
                        GyroAimMode::Off => GyroAimMode::HoldL2,
                        GyroAimMode::HoldL2 => GyroAimMode::HoldL1,
                        GyroAimMode::HoldL1 => GyroAimMode::AlwaysOn,
                        GyroAimMode::AlwaysOn => GyroAimMode::Off,
                    };
                    this.commit_config(cx);
                })),
            );

        let hardware_card = section_card("LIGHTBAR, LEDS & QUICK TOGGLES")
            .child(swatch_row)
            .child(player_row)
            .child(quick_toggles);

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
            .flex_1()
            .gap_3()
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
                            .w(px(450.0))
                            .gap_3()
                            .child(emu_card)
                            .child(triggers_card)
                            .child(hardware_card),
                    )
                    .child(monitor_card),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_stick_card(
        &self,
        title: &'static str,
        prefix: &'static str,
        tuning: StickTuning,
        raw: [u8; 2],
        out: [u8; 2],
        connected: bool,
        is_left: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let dz_slider = if is_left {
            ActiveSlider::LeftInner
        } else {
            ActiveSlider::RightInner
        };
        let adz_slider = if is_left {
            ActiveSlider::LeftAnti
        } else {
            ActiveSlider::RightAnti
        };

        let dz_row = self.render_slider_row(
            "Deadzone",
            format!("{}%", (tuning.deadzone * 100.0).round() as u32),
            tuning.deadzone / MAX_STICK_DZ,
            ACCENT,
            dz_slider,
            cx,
        );

        let adz_row = self.render_slider_row(
            "Anti-Deadzone",
            format!("{}%", (tuning.anti_deadzone * 100.0).round() as u32),
            tuning.anti_deadzone / MAX_STICK_DZ,
            WARN,
            adz_slider,
            cx,
        );

        let mut curve_pills = div().flex().flex_wrap().items_center().gap_1();
        for curve in ResponseCurve::ALL {
            let active = tuning.curve == curve;
            curve_pills = curve_pills.child(
                pill_button(
                    SharedString::from(format!("{prefix}-cv-{}", curve.label())),
                    curve.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    if is_left {
                        s.left_stick.curve = curve;
                    } else {
                        s.right_stick.curve = curve;
                    }
                    this.commit_config(cx);
                })),
            );
        }

        let curve_row = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(color(TEXT_MUTED)).child("Curve"))
            .child(curve_pills);

        section_card(title).flex_1().child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(deadzone_showcase(raw, out, tuning, connected))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap_2()
                        .child(dz_row)
                        .child(adz_row)
                        .child(curve_row),
                ),
        )
    }

    fn render_tuning(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied().unwrap_or_default();
        let connected = !self.handles.is_empty();

        let left_card = self.render_stick_card(
            "LEFT STICK TUNING",
            "ls",
            profile.left_stick,
            snap.raw_left_stick,
            snap.left_stick,
            connected,
            true,
            cx,
        );
        let right_card = self.render_stick_card(
            "RIGHT STICK TUNING",
            "rs",
            profile.right_stick,
            snap.raw_right_stick,
            snap.right_stick,
            connected,
            false,
            cx,
        );

        let trig_dz_pct = (profile.triggers.deadzone as f32 * 100.0 / 255.0).round() as u32;
        let trig_dz_row = self.render_slider_row(
            "Trigger Deadzone",
            format!("{trig_dz_pct}%"),
            trig_dz_pct as f32 / MAX_TRIGGER_DZ_PCT,
            ACCENT,
            ActiveSlider::Trigger,
            cx,
        );

        let mut hair_row = div().flex().items_center().gap_1p5().child(
            div()
                .w(px(96.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("Hair Trigger"),
        );
        for (label, threshold) in HAIR_TRIGGER_STEPS {
            let active = profile.triggers.hair_trigger == threshold;
            hair_row = hair_row.child(
                pill_button(SharedString::from(format!("ht-{label}")), label, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.config.active_settings_mut().triggers.hair_trigger = threshold;
                        this.commit_config(cx);
                    }),
                ),
            );
        }

        let mut remap_row = div().flex().items_center().gap_1p5().child(
            div()
                .w(px(96.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("Button Preset"),
        );
        for preset in ButtonPreset::ALL {
            let active = profile.button_preset == preset;
            remap_row = remap_row.child(
                pill_button(
                    SharedString::from(format!("bp-{}", preset.label())),
                    preset.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.active_settings_mut().button_preset = preset;
                    this.commit_config(cx);
                })),
            );
        }

        let trig_remap_card = section_card("TRIGGERS & BUTTON REMAPPING")
            .child(trig_dz_row)
            .child(hair_row)
            .child(remap_row);

        let mut tp_row = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(
                div()
                    .w(px(110.0))
                    .text_xs()
                    .text_color(color(TEXT_MUTED))
                    .child("Touchpad Mouse"),
            )
            .child(
                pill_button(
                    "tp-toggle",
                    if profile.touchpad_mouse {
                        "Enabled"
                    } else {
                        "Disabled"
                    },
                    profile.touchpad_mouse,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.touchpad_mouse = !s.touchpad_mouse;
                    this.commit_config(cx);
                })),
            );
        for (label, sens) in SENSITIVITY_STEPS {
            let active = (profile.touchpad_sensitivity - sens).abs() < 0.01;
            tp_row = tp_row.child(
                pill_button(SharedString::from(format!("tps-{label}")), label, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.config.active_settings_mut().touchpad_sensitivity = sens;
                        this.commit_config(cx);
                    }),
                ),
            );
        }

        let mut gyro_mode_row = div().flex().items_center().gap_1p5().child(
            div()
                .w(px(110.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("Gyro Aim Mode"),
        );
        for mode in GyroAimMode::ALL {
            let active = profile.gyro_mode == mode;
            gyro_mode_row = gyro_mode_row.child(
                pill_button(
                    SharedString::from(format!("gm-{}", mode.label())),
                    mode.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.active_settings_mut().gyro_mode = mode;
                    this.commit_config(cx);
                })),
            );
        }

        let mut gyro_sens_row = div().flex().items_center().gap_1p5().child(
            div()
                .w(px(110.0))
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .child("Gyro Sensitivity"),
        );
        for (label, sens) in SENSITIVITY_STEPS {
            let active = (profile.gyro_sensitivity - sens).abs() < 0.01;
            gyro_sens_row = gyro_sens_row.child(
                pill_button(SharedString::from(format!("gs-{label}")), label, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.config.active_settings_mut().gyro_sensitivity = sens;
                        this.commit_config(cx);
                    }),
                ),
            );
        }

        let motion_card = section_card("TOUCHPAD & GYRO MOUSE")
            .child(tp_row)
            .child(gyro_mode_row)
            .child(gyro_sens_row);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap_3()
            .child(div().flex().gap_3().child(left_card).child(right_card))
            .child(trig_remap_card)
            .child(motion_card)
    }

    fn render_profiles(&self, cx: &mut Context<Self>) -> Div {
        let last_exe = self.engine.as_ref().and_then(|e| e.last_foreground_exe());

        let mut controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                pill_button("new-profile", "+ New Profile", false).on_click(cx.listener(
                    |this, _, _, cx| {
                        let idx = this.config.profiles.len() + 1;
                        let name = format!("Profile {idx}");
                        let settings = this.config.active_settings();
                        this.config.profiles.push(NamedProfile {
                            name: name.clone(),
                            linked_exe: None,
                            settings,
                        });
                        this.config.active_profile = name;
                        this.commit_config(cx);
                    },
                )),
            )
            .child(
                pill_button(
                    "auto-switch-toggle",
                    if self.config.auto_switch_profiles {
                        "Auto-Switch by App: On"
                    } else {
                        "Auto-Switch by App: Off"
                    },
                    self.config.auto_switch_profiles,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.config.auto_switch_profiles = !this.config.auto_switch_profiles;
                    this.commit_config(cx);
                })),
            );

        if let Some(exe) = &last_exe {
            let exe_clone = exe.clone();
            controls = controls.child(
                pill_button("link-last-app", format!("Link Active to {exe}"), false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let active = this.config.active_profile.clone();
                        if let Some(p) = this.config.profiles.iter_mut().find(|p| p.name == active)
                        {
                            p.linked_exe = Some(exe_clone.clone());
                        }
                        this.commit_config(cx);
                    }),
                ),
            );
        } else {
            controls = controls.child(
                div()
                    .text_xs()
                    .text_color(color(TEXT_MUTED))
                    .child("Focus any game/app window to bind it to the active profile"),
            );
        }

        let mut list = div().flex().flex_col().gap_2();
        let total = self.config.profiles.len();
        for (idx, p) in self.config.profiles.iter().enumerate() {
            let is_active = p.name == self.config.active_profile;
            let name_sel = p.name.clone();
            let name_del = p.name.clone();
            let name_unlink = p.name.clone();
            let linked = p.linked_exe.as_deref().unwrap_or("No linked app");

            let mut row = div()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(color(SURFACE))
                .border_1()
                .border_color(color(if is_active { ACCENT } else { BORDER }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            pill_button(
                                SharedString::from(format!("prof-sel-{idx}")),
                                if is_active { "Active" } else { "Activate" },
                                is_active,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.config.active_profile = name_sel.clone();
                                    this.commit_config(cx);
                                },
                            )),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(color(TEXT))
                                .child(p.name.clone()),
                        )
                        .child(div().text_xs().text_color(color(TEXT_MUTED)).child(format!(
                            "{:?} | {} | {}",
                            p.settings.emulation,
                            p.settings.button_preset.label(),
                            linked
                        ))),
                );

            let mut actions = div().flex().items_center().gap_1p5();
            if p.linked_exe.is_some() {
                actions = actions.child(
                    pill_button(
                        SharedString::from(format!("prof-unlink-{idx}")),
                        "Unlink App",
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(prof) = this
                            .config
                            .profiles
                            .iter_mut()
                            .find(|x| x.name == name_unlink)
                        {
                            prof.linked_exe = None;
                        }
                        this.commit_config(cx);
                    })),
                );
            }
            if total > 1 {
                actions = actions.child(
                    pill_button(
                        SharedString::from(format!("prof-del-{idx}")),
                        "Delete",
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.config.profiles.retain(|x| x.name != name_del);
                        if this.config.active_profile == name_del {
                            if let Some(first) = this.config.profiles.first() {
                                this.config.active_profile = first.name.clone();
                            }
                        }
                        this.commit_config(cx);
                    })),
                );
            }
            row = row.child(actions);
            list = list.child(row);
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap_3()
            .child(section_card("PROFILE CONTROLS").child(controls))
            .child(list)
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> Div {
        let bg_on = self.config.work_in_background;
        let start_min_on = self.config.start_minimized;
        let startup_on = self.run_at_startup;
        let cloak_on = self
            .engine
            .as_ref()
            .map(Engine::is_cloak_enabled)
            .unwrap_or(self.config.cloak_physical);
        let auto_switch_on = self.config.auto_switch_profiles;
        let profile = self.config.active_settings();

        let bg_card =
            section_card("BACKGROUND & TRAY")
                .flex_1()
                .child(setting_row(
                    "Work in Background",
                    "Keep running in system tray when closing the window",
                    div()
                        .flex()
                        .gap_1()
                        .child(pill_button("set-bg-on", "On", bg_on).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.config.work_in_background = true;
                                this.commit_config(cx);
                            },
                        )))
                        .child(
                            pill_button("set-bg-off", "Off", !bg_on).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.config.work_in_background = false;
                                    this.config.start_minimized = false;
                                    this.commit_config(cx);
                                },
                            )),
                        ),
                ))
                .child(setting_row(
                    "Start Minimized",
                    "Launch directly into the system tray on startup",
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            pill_button("set-min-on", "On", start_min_on).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.config.work_in_background = true;
                                    this.config.start_minimized = true;
                                    this.commit_config(cx);
                                },
                            )),
                        )
                        .child(pill_button("set-min-off", "Off", !start_min_on).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.config.start_minimized = false;
                                this.commit_config(cx);
                            }),
                        )),
                ))
                .child(setting_row(
                    "Launch at Startup",
                    "Start DSMaster automatically when signing in to Windows",
                    div()
                        .flex()
                        .gap_1()
                        .child(pill_button("set-startup-on", "On", startup_on).on_click(
                            cx.listener(|this, _, _, cx| {
                                match set_run_at_startup(true) {
                                    Ok(()) => {
                                        this.run_at_startup = true;
                                        this.status_notice = None;
                                    }
                                    Err(e) => {
                                        this.status_notice =
                                            Some(format!("Startup registry error: {e}"));
                                    }
                                }
                                cx.notify();
                            }),
                        ))
                        .child(pill_button("set-startup-off", "Off", !startup_on).on_click(
                            cx.listener(|this, _, _, cx| {
                                match set_run_at_startup(false) {
                                    Ok(()) => {
                                        this.run_at_startup = false;
                                        this.status_notice = None;
                                    }
                                    Err(e) => {
                                        this.status_notice =
                                            Some(format!("Startup registry error: {e}"));
                                    }
                                }
                                cx.notify();
                            }),
                        )),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pt_1()
                        .child(if bg_on {
                            pill_button("set-hide-tray", "Hide to Tray", false)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.hide_to_tray(cx);
                                }))
                                .into_any_element()
                        } else {
                            coming_soon_pill("Background Mode Off").into_any_element()
                        })
                        .child(pill_button("set-quit", "Quit DSMaster", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.quit_app(cx);
                            }),
                        )),
                );

        let controller_card =
            section_card("CONTROLLER & PROFILES")
                .flex_1()
                .child(setting_row(
                    "Cloak Physical Controller",
                    "Hide DualSense via HidHide to prevent double input",
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            pill_button("set-cloak-on", "On", cloak_on).on_click(cx.listener(
                                |this, _, _, cx| {
                                    if !this.config.cloak_physical {
                                        this.toggle_cloak(cx);
                                    }
                                },
                            )),
                        )
                        .child(pill_button("set-cloak-off", "Off", !cloak_on).on_click(
                            cx.listener(|this, _, _, cx| {
                                if this.config.cloak_physical {
                                    this.toggle_cloak(cx);
                                }
                            }),
                        )),
                ))
                .child(setting_row(
                    "Auto-Switch Profiles",
                    "Activate linked profile when a game window focuses",
                    div()
                        .flex()
                        .gap_1()
                        .child(pill_button("set-auto-on", "On", auto_switch_on).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.config.auto_switch_profiles = true;
                                this.commit_config(cx);
                            }),
                        ))
                        .child(
                            pill_button("set-auto-off", "Off", !auto_switch_on).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.config.auto_switch_profiles = false;
                                    this.commit_config(cx);
                                }),
                            ),
                        ),
                ))
                .child(setting_row(
                    "Adaptive Triggers",
                    "Leave trigger actuators untouched or apply profile presets",
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            pill_button("set-trig-def", "Default", !profile.override_triggers)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.config.active_settings_mut().override_triggers = false;
                                    this.commit_config(cx);
                                })),
                        )
                        .child(
                            pill_button("set-trig-cust", "Custom", profile.override_triggers)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.config.active_settings_mut().override_triggers = true;
                                    this.commit_config(cx);
                                })),
                        ),
                ))
                .child(setting_row(
                    "Lightbar & LEDs",
                    "Leave lightbar and player LEDs untouched or override them",
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            pill_button("set-lb-def", "Default", !profile.override_lightbar)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.config.active_settings_mut().override_lightbar = false;
                                    this.commit_config(cx);
                                })),
                        )
                        .child(
                            pill_button("set-lb-cust", "Custom", profile.override_lightbar)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.config.active_settings_mut().override_lightbar = true;
                                    this.commit_config(cx);
                                })),
                        ),
                ))
                .child(setting_row(
                    "Active Profile",
                    "Manage profiles and app bindings in the Profiles tab",
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(color(TEXT))
                                .child(self.config.active_profile.clone()),
                        )
                        .child(
                            pill_button("set-open-profiles", "Open Profiles", false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.tab = ActiveTab::Profiles;
                                    cx.notify();
                                }),
                            ),
                        ),
                ));

        let cfg_path_str = config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "%APPDATA%\\DSMaster\\profiles.toml".to_owned());

        let system_card = section_card("SYSTEM & CONFIGURATION")
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(driver_badge("ViGEmBus", self.vigem_available))
                    .child(driver_badge("HidHide", self.hidhide_available))
                    .child(coming_soon_pill("Virtual DualSense (Coming Soon)")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(color(TEXT_MUTED))
                            .child(format!("Config file: {cfg_path_str}")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                pill_button("set-open-cfg", "Open Config Folder", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        if let Err(e) = open_config_dir() {
                                            this.status_notice =
                                                Some(format!("Failed to open folder: {e}"));
                                            cx.notify();
                                        }
                                    }),
                                ),
                            )
                            .child(
                                pill_button("set-reload-cfg", "Reload Config", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.config = load_config();
                                        if let Some(engine) = &this.engine {
                                            engine.apply_config(&this.config);
                                            if this.hidhide_available {
                                                let _ = engine
                                                    .set_cloak_enabled(this.config.cloak_physical);
                                            }
                                        }
                                        this.sync_tray_state();
                                        this.status_notice =
                                            Some("Configuration reloaded".to_owned());
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                pill_button("set-reset-cfg", "Reset to Defaults", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.config = AppConfig::default();
                                        if let Some(engine) = &this.engine {
                                            engine.apply_config(&this.config);
                                            if this.hidhide_available {
                                                let _ = engine.set_cloak_enabled(false);
                                            }
                                        }
                                        let _ = save_config(&this.config);
                                        this.sync_tray_state();
                                        this.status_notice =
                                            Some("Settings reset to defaults".to_owned());
                                        cx.notify();
                                    }),
                                ),
                            ),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap_3()
            .child(div().flex().gap_3().child(bg_card).child(controller_card))
            .child(system_card)
    }
}

impl Drop for DsMasterView {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

impl Render for DsMasterView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_from_engine();
        self.read_snapshots();

        let is_hidden = self.window_ctl.window_hidden.load(Ordering::Acquire);
        if !is_hidden
            && !self.handles.is_empty()
            && matches!(self.tab, ActiveTab::Overview | ActiveTab::Tuning)
        {
            window.request_animation_frame();
        }

        let cloak_on = self
            .engine
            .as_ref()
            .map(Engine::is_cloak_enabled)
            .unwrap_or(self.config.cloak_physical);

        let mut profile_switcher = div().flex().items_center().gap_1();
        for (idx, p) in self.config.profiles.iter().take(4).enumerate() {
            let active = p.name == self.config.active_profile;
            let name = p.name.clone();
            profile_switcher = profile_switcher.child(
                pill_button(
                    SharedString::from(format!("hdr-prof-{idx}")),
                    p.name.clone(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.active_profile = name.clone();
                    this.commit_config(cx);
                })),
            );
        }

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
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(TEXT))
                            .child("DSMaster"),
                    )
                    .child(profile_switcher),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        pill_button(
                            "cloak-toggle",
                            if cloak_on { "Cloak: On" } else { "Cloak: Off" },
                            cloak_on,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_cloak(cx);
                        })),
                    )
                    .child(driver_badge("ViGEmBus", self.vigem_available))
                    .child(driver_badge("HidHide", self.hidhide_available)),
            );

        let nav_bar = div()
            .flex()
            .items_center()
            .justify_between()
            .px_4()
            .py_1p5()
            .bg(color(BG))
            .border_b_1()
            .border_color(color(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        pill_button("tab-overview", "Overview", self.tab == ActiveTab::Overview)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tab = ActiveTab::Overview;
                                cx.notify();
                            })),
                    )
                    .child(
                        pill_button(
                            "tab-tuning",
                            "Tuning & Remap",
                            self.tab == ActiveTab::Tuning,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tab = ActiveTab::Tuning;
                            cx.notify();
                        })),
                    )
                    .child(
                        pill_button("tab-profiles", "Profiles", self.tab == ActiveTab::Profiles)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tab = ActiveTab::Profiles;
                                cx.notify();
                            })),
                    )
                    .child(
                        pill_button("tab-settings", "Settings", self.tab == ActiveTab::Settings)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tab = ActiveTab::Settings;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(color(WARN))
                    .child(self.status_notice.clone().unwrap_or_default()),
            );

        let body = match self.tab {
            ActiveTab::Overview => self.render_overview(cx),
            ActiveTab::Tuning => self.render_tuning(cx),
            ActiveTab::Profiles => self.render_profiles(cx),
            ActiveTab::Settings => self.render_settings(cx),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(color(BG))
            .text_color(color(TEXT))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some((slider, origin_x, width)) = this.active_slider {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        let t = ((f32::from(ev.position.x) - origin_x) / width).clamp(0.0, 1.0);
                        this.apply_slider_live(slider, t, cx);
                    } else {
                        this.finish_slider_drag(cx);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.finish_slider_drag(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.finish_slider_drag(cx);
                }),
            )
            .child(header)
            .child(nav_bar)
            .child(div().flex().flex_col().flex_1().p_4().child(body))
    }
}
