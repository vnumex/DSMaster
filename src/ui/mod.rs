mod controller_view;
mod dualsense_paths;
mod icons;
pub mod theme;
mod widgets;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{
    Bounds, Context, Div, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, PathBuilder,
    Pixels, ScrollHandle, SharedString, Task, Window, WindowControlArea, canvas, div, point,
    prelude::*, px, relative,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOW, SetForegroundWindow, ShowWindow};

use self::controller_view::{ControllerPreviewState, HighlightZone, virtual_dualsense};
use self::icons::{
    check_icon, gamepad_icon, gyro_icon, layers_icon, link_icon, plus_icon, settings_icon,
    sidebar_toggle_icon, sliders_icon, stick_icon, sun_icon, swap_icon, trash_icon, tray_hide_icon,
    trigger_icon, win_close_icon, win_max_icon, win_min_icon,
};
use self::theme::{
    ACCENT, ACCENT_FG, ACCENT_SOLID, ACCENT_SOLID_HOVER, BORDER, BORDER_STRONG, CANVAS_BG,
    CLOSE_HOVER, DANGER, DANGER_BORDER, DANGER_SURFACE, DRAWER_BG, FONT_MONO, FONT_SANS, RAIL_BG,
    SUCCESS, SURFACE, SURFACE_ALT, SURFACE_HOVER, TEXT, TEXT_MUTED, TEXT_SECONDARY, WARN,
    WARN_BORDER, WARN_SURFACE, WELL_BG, ZONE_BLUE, ZONE_SECONDARY, color, color_alpha, hsv_to_rgb,
    rgb_to_hsv, rgb_to_u32,
};
use self::widgets::{
    SLIDER_SLOTS, danger_button, deadzone_showcase, drawer_nav_item, driver_status_pill,
    hue_slider_bar, icon_rail_item, latency_sparkline, meter_row, meter_row_with_width,
    pill_button, primary_cta_button, response_curve_chart, rgb_channel_bar, section_card,
    section_card_with_action, segmented_button, setting_row, slider_bar, status_info_pill,
    status_inset_pill, subtle_button, sv_color_pad, text_tooltip, toggle_switch,
    trigger_travel_gauge,
};
use crate::cloak::HidHideManager;
use crate::drivers::{DriverInstallJob, DriverInstallPhase, DriverKind};
use crate::emu::EmulationMode;
use crate::emu::dualsense::is_usbip_win2_installed;
use crate::emu::vigem::is_vigem_installed;
use crate::engine::profile::{
    ButtonPreset, GyroAimMode, ResponseCurve, StickTuning, TriggerPreset,
};
use crate::engine::profiles_store::{
    AppConfig, NamedProfile, config_path, load_config, open_config_dir, save_config,
};
use crate::engine::{Engine, SeqLock, TelemetrySnapshot};
use crate::protocol::{BatteryStatus, Transport};
use crate::startup::{is_run_at_startup_enabled, set_run_at_startup};
use crate::tray::{SystemTray, TrayAction, TrayState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivePage {
    QuickSettings,
    Profiles,
    Sticks,
    Triggers,
    Lighting,
    MotionRemap,
    Settings,
}

impl ActivePage {
    fn highlight_zone(self) -> HighlightZone {
        match self {
            Self::Sticks => HighlightZone::Sticks,
            Self::Triggers => HighlightZone::Triggers,
            Self::Lighting => HighlightZone::Lighting,
            Self::MotionRemap => HighlightZone::Buttons,
            _ => HighlightZone::None,
        }
    }

    fn shows_controller_stage(self) -> bool {
        matches!(
            self,
            Self::QuickSettings
                | Self::Sticks
                | Self::Triggers
                | Self::Lighting
                | Self::MotionRemap
        )
    }

    fn is_controller_section(self) -> bool {
        !matches!(self, Self::Profiles | Self::Settings)
    }
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
    ("Cobalt", [0, 102, 255]),
    ("Ice", [210, 224, 240]),
    ("Cyan", [0, 196, 220]),
    ("Emerald", [32, 200, 96]),
    ("Gold", [242, 208, 107]),
    ("Crimson", [235, 56, 56]),
    ("Violet", [148, 86, 245]),
];

const PLAYER_LED_OPTIONS: [(&str, u8); 5] = [
    ("Off", 0x00),
    ("P1", 0x04),
    ("P2", 0x0A),
    ("P3", 0x15),
    ("P4", 0x1B),
];

const MAX_STICK_DZ: f32 = 0.35;
const MAX_TRIGGER_DZ_PCT: f32 = 35.0;
const LATENCY_HISTORY_CAP: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveSlider {
    LeftInner,
    LeftAnti,
    RightInner,
    RightAnti,
    Trigger,
    HueBar,
    RedChannel,
    GreenChannel,
    BlueChannel,
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
            Self::HueBar => 5,
            Self::RedChannel => 6,
            Self::GreenChannel => 7,
            Self::BlueChannel => 8,
        }
    }
}

const HAIR_TRIGGER_STEPS: [(&str, u8); 4] =
    [("Full", 255), ("75%", 190), ("50%", 128), ("25%", 64)];

const SENSITIVITY_STEPS: [(&str, f32); 4] =
    [("0.5x", 0.5), ("1.0x", 1.0), ("1.5x", 1.5), ("2.0x", 2.0)];

fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

fn button_preset_desc(preset: ButtonPreset) -> &'static str {
    match preset {
        ButtonPreset::Standard => "Default PlayStation to Xbox / DS4 layout",
        ButtonPreset::SwapFace => "Swap Cross ↔ Circle and Square ↔ Triangle",
        ButtonPreset::Tactical => "Swap Circle (crouch) with R3 (melee)",
        ButtonPreset::BumperJumper => "Swap Cross (jump) with L1 bumper",
    }
}

fn trigger_preset_desc(preset: TriggerPreset) -> &'static str {
    match preset {
        TriggerPreset::Off => "No motorized resistance",
        TriggerPreset::Rigid => "Firm continuous spring tension",
        TriggerPreset::Weapon => "Crisp snap-break wall halfway down",
        TriggerPreset::Pulse => "Rapid rhythmic vibration feedback",
    }
}

pub struct DsMasterView {
    engine: Option<Engine>,
    engine_error: Option<String>,
    status_notice: Option<String>,
    vigem_available: bool,
    hidhide_available: bool,
    vds_available: bool,
    driver_install: Option<DriverInstallJob>,
    last_install_phase: Option<DriverInstallPhase>,
    config: AppConfig,
    page: ActivePage,
    drawer_open: bool,
    show_raw_sticks: bool,
    last_topology_version: u64,
    handles: Vec<Arc<SeqLock<TelemetrySnapshot>>>,
    selected_idx: usize,
    snapshots: Vec<TelemetrySnapshot>,
    latency_history: VecDeque<f32>,
    last_packet_count: u64,
    window_ctl: WindowControlState,
    run_at_startup: bool,
    tray_tx: crossbeam_channel::Sender<TrayAction>,
    tray: Option<SystemTray>,
    active_slider: Option<(ActiveSlider, f32, f32)>,
    slider_bounds: Rc<RefCell<[Bounds<Pixels>; SLIDER_SLOTS]>>,
    active_sv_pad: Option<(f32, f32, f32, f32)>,
    sv_pad_bounds: Rc<RefCell<Bounds<Pixels>>>,
    picker_hsv: (f32, f32, f32),
    main_scroll: ScrollHandle,
    _watch_task: Task<()>,
    _tray_task: Task<()>,
}

impl DsMasterView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        engine: Result<Engine, String>,
        vigem_available: bool,
        hidhide_available: bool,
        vds_available: bool,
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
        let init_rgb = config.active_settings().lightbar_rgb;
        let (init_h, init_s, init_v) = rgb_to_hsv(init_rgb);
        let initial_hsv = if init_s < 0.01 || init_v < 0.01 {
            (216.0, init_s, init_v)
        } else {
            (init_h, init_s, init_v)
        };

        let mut this = Self {
            engine,
            engine_error,
            status_notice: None,
            vigem_available,
            hidhide_available,
            vds_available,
            driver_install: None,
            last_install_phase: None,
            config,
            page: ActivePage::QuickSettings,
            drawer_open: true,
            show_raw_sticks: false,
            last_topology_version: 0,
            handles: Vec::new(),
            selected_idx: 0,
            snapshots: Vec::new(),
            latency_history: VecDeque::with_capacity(LATENCY_HISTORY_CAP),
            last_packet_count: 0,
            window_ctl,
            run_at_startup,
            tray_tx,
            tray: None,
            active_slider: None,
            slider_bounds: Rc::new(RefCell::new([Bounds::default(); SLIDER_SLOTS])),
            active_sv_pad: None,
            sv_pad_bounds: Rc::new(RefCell::new(Bounds::default())),
            picker_hsv: initial_hsv,
            main_scroll: ScrollHandle::new(),
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
                    let eng_changed = view.sync_from_engine();
                    let install_changed = view.poll_driver_install();
                    if eng_changed {
                        view.read_snapshots();
                        view.sync_tray_state();
                    }
                    if eng_changed || install_changed {
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
        if self.handles.is_empty() {
            self.latency_history.clear();
            self.last_packet_count = 0;
        }
        let engine_prof = engine.active_profile_name();
        if self.config.active_profile != engine_prof {
            self.config.active_profile = engine_prof;
            self.sync_picker_hsv();
        }
        true
    }

    fn read_snapshots(&mut self) {
        self.snapshots.clear();
        for handle in &self.handles {
            self.snapshots.push(handle.read());
        }
        if let Some(s) = self.snapshots.get(self.selected_idx) {
            if s.packet_count != self.last_packet_count && s.proc_us_ema > 0.0 {
                self.last_packet_count = s.packet_count;
                if self.latency_history.len() >= LATENCY_HISTORY_CAP {
                    self.latency_history.pop_front();
                }
                self.latency_history.push_back(s.proc_us_ema);
            }
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

    fn set_emulation_mode(&mut self, mode: EmulationMode, cx: &mut Context<Self>) {
        self.config.active_settings_mut().emulation = mode;
        self.commit_config(cx);
        match mode {
            EmulationMode::DualSense if !self.vds_available => {
                self.status_notice = Some(
                    "usbip-win2 not detected — install vadimgrn/usbip-win2 for Virtual DualSense"
                        .to_owned(),
                );
                cx.notify();
            }
            EmulationMode::Xbox360 | EmulationMode::DualShock4 if !self.vigem_available => {
                self.status_notice =
                    Some("ViGEmBus not detected — install Nefarius ViGEmBus".to_owned());
                cx.notify();
            }
            _ => {}
        }
    }

    fn toggle_cloak(&mut self, cx: &mut Context<Self>) {
        if !self.hidhide_available {
            self.status_notice = Some("HidHide driver is not installed".to_owned());
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
        let state = self.build_tray_state();
        if let Some(tray) = &self.tray {
            tray.update_state(state);
        } else {
            self.tray = SystemTray::start(self.tray_tx.clone(), state);
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
                self.set_emulation_mode(mode, cx);
            }
            TrayAction::ToggleCloak => self.toggle_cloak(cx),
            TrayAction::Quit => self.quit_app(cx),
        }
    }

    fn recheck_drivers(&mut self) -> (bool, bool, bool) {
        self.vigem_available = is_vigem_installed();
        self.hidhide_available = HidHideManager::is_available();
        self.vds_available = is_usbip_win2_installed();
        (self.vigem_available, self.hidhide_available, self.vds_available)
    }

    fn start_driver_install(&mut self, kind: DriverKind, cx: &mut Context<Self>) {
        if self.driver_install.is_some() {
            return;
        }
        self.driver_install = Some(DriverInstallJob::spawn(kind));
        self.last_install_phase = Some(DriverInstallPhase::Downloading);
        self.status_notice = Some(format!("Downloading {} installer...", kind.display_name()));
        cx.notify();
    }

    fn poll_driver_install(&mut self) -> bool {
        let Some(job) = self.driver_install.as_ref() else {
            return false;
        };
        let phase = job.phase();
        if self.last_install_phase.as_ref() == Some(&phase) {
            return false;
        }

        let kind = job.kind;
        let name = kind.display_name();
        match phase {
            DriverInstallPhase::Downloading => {
                self.last_install_phase = Some(DriverInstallPhase::Downloading);
                self.status_notice = Some(format!("Downloading {name} installer..."));
            }
            DriverInstallPhase::RunningInstaller => {
                self.last_install_phase = Some(DriverInstallPhase::RunningInstaller);
                self.status_notice = Some(format!("Running {name} installer..."));
            }
            DriverInstallPhase::Finished(res) => {
                self.driver_install = None;
                self.last_install_phase = None;
                let was_vigem = self.vigem_available;
                let (vigem, hidhide, vds) = self.recheck_drivers();
                let now_installed = match kind {
                    DriverKind::VigemBus => vigem,
                    DriverKind::HidHide => hidhide,
                    DriverKind::UsbipWin2 => vds,
                };
                self.status_notice = Some(match res {
                    Ok(()) if now_installed => {
                        if kind == DriverKind::VigemBus && !was_vigem {
                            format!(
                                "{name} installed — restart DSMaster to enable Xbox 360 / DS4"
                            )
                        } else {
                            format!("{name} installed")
                        }
                    }
                    Ok(()) => format!(
                        "{name} installer closed — click Recheck or reboot if prompted"
                    ),
                    Err(err) => format!("{name} install failed: {err}"),
                });
                self.sync_tray_state();
            }
        }
        true
    }

    fn selected_snapshot(&self) -> Option<&TelemetrySnapshot> {
        self.snapshots.get(self.selected_idx)
    }

    fn sync_picker_hsv(&mut self) {
        let rgb = self.config.active_settings().lightbar_rgb;
        let (h, s, v) = rgb_to_hsv(rgb);
        let keep_h = if s < 0.01 || v < 0.01 {
            self.picker_hsv.0
        } else {
            h
        };
        self.picker_hsv = (keep_h, s, v);
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

    fn start_sv_drag(&mut self, mouse_x: Pixels, mouse_y: Pixels, cx: &mut Context<Self>) {
        let bounds = *self.sv_pad_bounds.borrow();
        let ox = f32::from(bounds.origin.x);
        let oy = f32::from(bounds.origin.y);
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        if w > 1.0 && h > 1.0 {
            self.active_sv_pad = Some((ox, oy, w, h));
            let sat = ((f32::from(mouse_x) - ox) / w).clamp(0.0, 1.0);
            let val = (1.0 - (f32::from(mouse_y) - oy) / h).clamp(0.0, 1.0);
            self.apply_sv_live(sat, val, cx);
        }
    }

    fn apply_sv_live(&mut self, sat: f32, val: f32, cx: &mut Context<Self>) {
        let sat = sat.clamp(0.0, 1.0);
        let val = val.clamp(0.0, 1.0);
        let h = self.picker_hsv.0;
        self.picker_hsv = (h, sat, val);
        let st = self.config.active_settings_mut();
        st.override_lightbar = true;
        st.lightbar_rgb = hsv_to_rgb(h, sat, val);
        if let Some(engine) = &self.engine {
            engine.apply_config(&self.config);
        }
        cx.notify();
    }

    fn apply_slider_live(&mut self, slider: ActiveSlider, t: f32, cx: &mut Context<Self>) {
        let t = t.clamp(0.0, 1.0);
        match slider {
            ActiveSlider::LeftInner
            | ActiveSlider::LeftAnti
            | ActiveSlider::RightInner
            | ActiveSlider::RightAnti
            | ActiveSlider::Trigger => {
                let stick_val =
                    ((t * MAX_STICK_DZ * 100.0).round() / 100.0).clamp(0.0, MAX_STICK_DZ);
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
                    _ => {}
                }
            }
            ActiveSlider::HueBar => {
                let h = (t * 360.0).clamp(0.0, 359.99);
                let mut sat = self.picker_hsv.1;
                let mut val = self.picker_hsv.2;
                if sat < 0.08 {
                    sat = 0.90;
                }
                if val < 0.12 {
                    val = 1.0;
                }
                self.picker_hsv = (h, sat, val);
                let s = self.config.active_settings_mut();
                s.override_lightbar = true;
                s.lightbar_rgb = hsv_to_rgb(h, sat, val);
            }
            ActiveSlider::RedChannel | ActiveSlider::GreenChannel | ActiveSlider::BlueChannel => {
                let ch = (t * 255.0).round().clamp(0.0, 255.0) as u8;
                let s = self.config.active_settings_mut();
                s.override_lightbar = true;
                match slider {
                    ActiveSlider::RedChannel => s.lightbar_rgb[0] = ch,
                    ActiveSlider::GreenChannel => s.lightbar_rgb[1] = ch,
                    ActiveSlider::BlueChannel => s.lightbar_rgb[2] = ch,
                    _ => {}
                }
                let rgb = s.lightbar_rgb;
                let (h, sat, val) = rgb_to_hsv(rgb);
                let keep_h = if sat < 0.01 || val < 0.01 {
                    self.picker_hsv.0
                } else {
                    h
                };
                self.picker_hsv = (keep_h, sat, val);
            }
        }

        if let Some(engine) = &self.engine {
            engine.apply_config(&self.config);
        }
        cx.notify();
    }

    fn finish_slider_drag(&mut self, cx: &mut Context<Self>) {
        let had_slider = self.active_slider.take().is_some();
        let had_sv = self.active_sv_pad.take().is_some();
        if had_slider || had_sv {
            self.commit_config(cx);
        }
    }

    fn create_profile(&mut self, cx: &mut Context<Self>) {
        let mut n = 2usize;
        while self
            .config
            .profiles
            .iter()
            .any(|p| p.name == format!("Profile {n}"))
        {
            n += 1;
        }
        let name = format!("Profile {n}");
        let settings = self.config.active_settings();
        self.config.profiles.push(NamedProfile {
            name: name.clone(),
            linked_exe: None,
            settings,
        });
        self.config.active_profile = name;
        self.commit_config(cx);
    }

    fn delete_profile(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.config.profiles.len() <= 1 {
            return;
        }
        self.config.profiles.retain(|p| p.name != name);
        if self.config.active_profile == name {
            if let Some(first) = self.config.profiles.first() {
                self.config.active_profile = first.name.clone();
            }
        }
        self.sync_picker_hsv();
        self.commit_config(cx);
    }

    fn unlink_profile_app(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(prof) = self.config.profiles.iter_mut().find(|p| p.name == name) {
            prof.linked_exe = None;
        }
        self.commit_config(cx);
    }

    fn link_profile_to_app(&mut self, name: &str, exe: String, cx: &mut Context<Self>) {
        if let Some(prof) = self.config.profiles.iter_mut().find(|p| p.name == name) {
            prof.linked_exe = Some(exe);
        }
        self.commit_config(cx);
    }

    fn cycle_next_profile(&mut self, cx: &mut Context<Self>) {
        let len = self.config.profiles.len();
        if len <= 1 {
            return;
        }
        let cur = self
            .config
            .profiles
            .iter()
            .position(|p| p.name == self.config.active_profile)
            .unwrap_or(0);
        let next = (cur + 1) % len;
        self.config.active_profile = self.config.profiles[next].name.clone();
        self.sync_picker_hsv();
        self.commit_config(cx);
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
            .gap_1()
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(color(TEXT_SECONDARY))
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap_1()
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
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px_1()
                                    .text_xs()
                                    .font_family(FONT_MONO)
                                    .text_color(color(TEXT_MUTED))
                                    .child("0%")
                                    .child("35%"),
                            ),
                    )
                    .child(
                        div()
                            .w(px(56.0))
                            .h(px(28.0))
                            .rounded(px(6.0))
                            .bg(color(WELL_BG))
                            .border_1()
                            .border_color(color(BORDER))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.0))
                            .font_family(FONT_MONO)
                            .font_weight(FontWeight::NORMAL)
                            .text_color(color(TEXT))
                            .flex_shrink_0()
                            .child(value_label),
                    ),
            )
    }

    /// Custom frameless dark Windows 11 titlebar (`#101214`, 40px tall, zero status dots)
    fn render_titlebar(&self) -> Div {
        let notice = self
            .engine_error
            .as_ref()
            .map(|e| (e.clone(), DANGER, DANGER_SURFACE, DANGER_BORDER))
            .or_else(|| {
                self.status_notice
                    .as_ref()
                    .map(|n| (n.clone(), WARN, WARN_SURFACE, WARN_BORDER))
            });

        let mut bar = div()
            .w_full()
            .h(px(40.0))
            .flex_shrink_0()
            .bg(color(RAIL_BG))
            .border_b_1()
            .border_color(color(BORDER))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("titlebar-drag")
                    .flex_1()
                    .h_full()
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color(TEXT))
                            .child("DSMaster"),
                    ),
            );

        if let Some((msg, col, bg_col, border_col)) = notice {
            bar = bar.child(
                div()
                    .px_3()
                    .h(px(28.0))
                    .mr_3()
                    .rounded(px(6.0))
                    .bg(color(bg_col))
                    .border_1()
                    .border_color(color(border_col))
                    .flex()
                    .items_center()
                    .text_size(px(13.0))
                    .text_color(color(col))
                    .whitespace_nowrap()
                    .child(msg),
            );
        }

        // Native Windows control areas (uniform 48px width)
        bar.child(
            div()
                .flex()
                .items_center()
                .h_full()
                .flex_shrink_0()
                .child(
                    div()
                        .id("win-min")
                        .w(px(48.0))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .window_control_area(WindowControlArea::Min)
                        .cursor_pointer()
                        .hover(|s| s.bg(color(SURFACE_ALT)))
                        .child(win_min_icon(TEXT_SECONDARY)),
                )
                .child(
                    div()
                        .id("win-max")
                        .w(px(48.0))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .window_control_area(WindowControlArea::Max)
                        .cursor_pointer()
                        .hover(|s| s.bg(color(SURFACE_ALT)))
                        .child(win_max_icon(TEXT_SECONDARY)),
                )
                .child(
                    div()
                        .id("win-close")
                        .w(px(48.0))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .window_control_area(WindowControlArea::Close)
                        .cursor_pointer()
                        .hover(|s| s.bg(color(CLOSE_HOVER)))
                        .child(win_close_icon(TEXT)),
                ),
        )
    }

    /// Far-left 72px Icon Rail (`#101214`, quiet neutral selection, icons match text role)
    fn render_icon_rail(&self, cx: &mut Context<Self>) -> Div {
        let on_ctrl = self.page.is_controller_section();
        let on_prof = self.page == ActivePage::Profiles;
        let on_set = self.page == ActivePage::Settings;

        div()
            .w(px(72.0))
            .flex_shrink_0()
            .h_full()
            .bg(color(RAIL_BG))
            .border_r_1()
            .border_color(color(BORDER))
            .py_4()
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        icon_rail_item(
                            "rail-controller",
                            gamepad_icon(20.0, if on_ctrl { TEXT } else { TEXT_MUTED }),
                            "Pad",
                            on_ctrl,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.page.is_controller_section() {
                                this.page = ActivePage::QuickSettings;
                            }
                            this.drawer_open = true;
                            cx.notify();
                        })),
                    )
                    .child(
                        icon_rail_item(
                            "rail-profiles",
                            layers_icon(20.0, if on_prof { TEXT } else { TEXT_MUTED }),
                            "Profiles",
                            on_prof,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.page = ActivePage::Profiles;
                            cx.notify();
                        })),
                    )
                    .child(
                        icon_rail_item(
                            "rail-settings",
                            settings_icon(20.0, if on_set { TEXT } else { TEXT_MUTED }),
                            "Settings",
                            on_set,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.page = ActivePage::Settings;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .id("rail-hide-tray")
                    .size(px(40.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(color(SURFACE)))
                    .tooltip(text_tooltip("Hide to system tray"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.hide_to_tray(cx);
                    }))
                    .child(tray_hide_icon(18.0, TEXT_MUTED)),
            )
    }

    /// Secondary 264px Configuration Drawer (`#1C1F23`, 24px/8px group rhythm, quiet text meta)
    fn render_config_drawer(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied();
        let count = self.handles.len();
        let connected = snap.is_some();

        let model_label = match snap {
            Some(s) if s.is_edge => "DualSense Edge",
            Some(_) => "DualSense",
            None => "No controller",
        };
        let transport_label = match snap {
            Some(s) => match s.transport {
                Transport::Usb => "USB",
                Transport::Bluetooth => "BT",
            },
            None => "Offline",
        };
        let device_sub = match snap {
            Some(s) => {
                let chg = match s.battery_status {
                    BatteryStatus::Charging => "+",
                    _ => "",
                };
                format!("{}%{chg} battery · {:.0} Hz", s.battery_percent, s.polling_hz)
            }
            None => "Connect over USB or Bluetooth".to_owned(),
        };

        let mut device_sub_el = div()
            .text_xs()
            .text_color(color(if connected {
                TEXT_SECONDARY
            } else {
                TEXT_MUTED
            }))
            .whitespace_nowrap()
            .child(device_sub);
        if connected {
            device_sub_el = device_sub_el.font_family(FONT_MONO);
        }

        // Clean text-only Device card (no redundant controller icon)
        let mut device_card = div()
            .w_full()
            .p_3()
            .rounded(px(12.0))
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(BORDER_STRONG))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(color(TEXT_MUTED))
                                    .child(if count > 1 {
                                        format!("Devices ({count})")
                                    } else {
                                        "Device".to_owned()
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(FONT_MONO)
                                    .text_color(color(TEXT_MUTED))
                                    .child(transport_label),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .whitespace_nowrap()
                            .child(model_label),
                    )
                    .child(device_sub_el),
            );

        if count > 1 {
            let mut switcher = div().flex().flex_wrap().gap_2();
            for idx in 0..count {
                let active = idx == self.selected_idx;
                switcher = switcher.child(
                    pill_button(
                        SharedString::from(format!("dev-slot-{idx}")),
                        format!("Pad {}", idx + 1),
                        active,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_idx = idx;
                        this.latency_history.clear();
                        this.last_packet_count = 0;
                        cx.notify();
                    })),
                );
            }
            device_card = device_card.child(switcher);
        }

        let active_meta = || {
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(ACCENT))
                .child("On")
        };

        let trigger_meta = profile.override_triggers.then(active_meta);
        let light_meta = profile.override_lightbar.then(active_meta);
        let gyro_meta =
            (profile.gyro_mode != GyroAimMode::Off || profile.touchpad_mouse).then(active_meta);

        // Icons follow text role color (bright primary when active, muted when resting)
        let nav_icon_col = |p: ActivePage| {
            if self.page == p {
                TEXT
            } else {
                TEXT_MUTED
            }
        };

        div()
            .w(px(264.0))
            .flex_shrink_0()
            .h_full()
            .bg(color(DRAWER_BG))
            .border_r_1()
            .border_color(color(BORDER))
            .p_4()
            .flex()
            .flex_col()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    // Drawer title + collapse button
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .mb_4()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color(TEXT))
                                    .child("DualSense Hub"),
                            )
                            .child(
                                div()
                                    .id("drawer-collapse")
                                    .size(px(28.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(color(SURFACE_ALT)))
                                    .tooltip(text_tooltip("Collapse sidebar"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.drawer_open = false;
                                        cx.notify();
                                    }))
                                    .child(sidebar_toggle_icon(16.0, TEXT_MUTED)),
                            ),
                    )
                    // Device card
                    .child(device_card)
                    // Controller navigation group (Profiles & Settings live exclusively on the left rail)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .pt_6()
                            .child(
                                div()
                                    .px_3()
                                    .pb_2()
                                    .text_xs()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(color(TEXT_MUTED))
                                    .child("Controller"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        drawer_nav_item(
                                            "nav-quick",
                                            sliders_icon(
                                                16.0,
                                                nav_icon_col(ActivePage::QuickSettings),
                                            ),
                                            "Overview",
                                            self.page == ActivePage::QuickSettings,
                                            None,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.page = ActivePage::QuickSettings;
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        drawer_nav_item(
                                            "nav-sticks",
                                            stick_icon(16.0, nav_icon_col(ActivePage::Sticks)),
                                            "Sticks & curves",
                                            self.page == ActivePage::Sticks,
                                            None,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.page = ActivePage::Sticks;
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        drawer_nav_item(
                                            "nav-triggers",
                                            trigger_icon(16.0, nav_icon_col(ActivePage::Triggers)),
                                            "Triggers & haptics",
                                            self.page == ActivePage::Triggers,
                                            trigger_meta,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.page = ActivePage::Triggers;
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        drawer_nav_item(
                                            "nav-lighting",
                                            sun_icon(16.0, nav_icon_col(ActivePage::Lighting)),
                                            "Lighting & LEDs",
                                            self.page == ActivePage::Lighting,
                                            light_meta,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.page = ActivePage::Lighting;
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        drawer_nav_item(
                                            "nav-motion",
                                            gyro_icon(16.0, nav_icon_col(ActivePage::MotionRemap)),
                                            "Motion & remap",
                                            self.page == ActivePage::MotionRemap,
                                            gyro_meta,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.page = ActivePage::MotionRemap;
                                            cx.notify();
                                        })),
                                    ),
                            ),
                    ),
            )
            // Drawer footer
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(color(TEXT_MUTED))
                            .child("DSMaster v0.1.0"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(color(if profile.emulation == EmulationMode::DualSense {
                                if self.vds_available {
                                    TEXT_SECONDARY
                                } else {
                                    DANGER
                                }
                            } else if self.vigem_available {
                                TEXT_SECONDARY
                            } else {
                                DANGER
                            }))
                            .child(if profile.emulation == EmulationMode::DualSense {
                                if self.vds_available {
                                    "usbip-win2 ready"
                                } else {
                                    "usbip-win2 missing"
                                }
                            } else if self.vigem_available {
                                "ViGEm ready"
                            } else {
                                "ViGEm missing"
                            }),
                    ),
            )
    }

    /// Top profile & emulation bar (flat surface, quiet segmented tray, single primary CTA)
    fn render_top_profile_bar(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let active_named = self
            .config
            .profiles
            .iter()
            .find(|p| p.name == self.config.active_profile);
        let linked_exe = active_named.and_then(|p| p.linked_exe.clone());
        let cloak_on = self
            .engine
            .as_ref()
            .map(Engine::is_cloak_enabled)
            .unwrap_or(self.config.cloak_physical);

        let mut left_group = div().flex().items_center().gap_3().min_w(px(0.0));
        if !self.drawer_open {
            left_group = left_group.child(
                div()
                    .id("drawer-expand")
                    .size(px(32.0))
                    .rounded(px(8.0))
                    .bg(color(SURFACE_ALT))
                    .border_1()
                    .border_color(color(BORDER))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(color(SURFACE_HOVER)))
                    .tooltip(text_tooltip("Show sidebar"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.drawer_open = true;
                        cx.notify();
                    }))
                    .child(sidebar_toggle_icon(16.0, TEXT)),
            );
        }

        let subtitle_text = if let Some(exe) = linked_exe {
            format!("Auto: {exe}")
        } else if self.config.profiles.len() == 1 {
            "1 profile".to_owned()
        } else {
            format!("{} profiles", self.config.profiles.len())
        };
        let subtitle_is_exe = active_named.and_then(|p| p.linked_exe.as_ref()).is_some();

        let mut prof_sub = div()
            .text_xs()
            .text_color(color(TEXT_MUTED))
            .whitespace_nowrap()
            .child(subtitle_text);
        if subtitle_is_exe {
            prof_sub = prof_sub.font_family(FONT_MONO);
        }

        let prof_trigger = div()
            .id("topbar-profile-btn")
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .tooltip(text_tooltip("Open profiles"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.page = ActivePage::Profiles;
                cx.notify();
            }))
            .child(
                div()
                    .size(px(36.0))
                    .rounded(px(8.0))
                    .bg(color(SURFACE_ALT))
                    .border_1()
                    .border_color(color(BORDER))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .child(gamepad_icon(16.0, TEXT_SECONDARY)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .whitespace_nowrap()
                            .child(self.config.active_profile.clone()),
                    )
                    .child(prof_sub),
            );

        left_group = left_group.child(prof_trigger);

        if self.config.profiles.len() > 1 {
            left_group = left_group.child(
                div()
                    .id("topbar-cycle-profile")
                    .size(px(32.0))
                    .rounded(px(8.0))
                    .bg(color(SURFACE_ALT))
                    .border_1()
                    .border_color(color(BORDER))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|s| s.bg(color(SURFACE_HOVER)).border_color(color(BORDER_STRONG)))
                    .tooltip(text_tooltip("Switch to next profile"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cycle_next_profile(cx);
                    }))
                    .child(swap_icon(16.0, TEXT_SECONDARY)),
            );
        }

        // Center quiet segmented Emulation Mode switcher (nested 8px tray / 4px pad / 6px segment)
        let emu_group = div()
            .flex()
            .items_center()
            .gap_1()
            .p_1()
            .rounded(px(8.0))
            .bg(color(WELL_BG))
            .border_1()
            .border_color(color(BORDER))
            .child(
                segmented_button(
                    "top-emu-none",
                    "Passthrough",
                    profile.emulation == EmulationMode::None,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_emulation_mode(EmulationMode::None, cx);
                })),
            )
            .child(
                segmented_button(
                    "top-emu-x360",
                    "Xbox 360",
                    profile.emulation == EmulationMode::Xbox360,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_emulation_mode(EmulationMode::Xbox360, cx);
                })),
            )
            .child(
                segmented_button(
                    "top-emu-ds4",
                    "DualShock 4",
                    profile.emulation == EmulationMode::DualShock4,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_emulation_mode(EmulationMode::DualShock4, cx);
                })),
            )
            .child(
                segmented_button(
                    "top-emu-ds5",
                    "DualSense",
                    profile.emulation == EmulationMode::DualSense,
                )
                .tooltip(text_tooltip(if self.vds_available {
                    "Emulate a virtual USB DualSense via usbip-win2"
                } else {
                    "Requires user-installed usbip-win2 driver (vadimgrn/usbip-win2)"
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_emulation_mode(EmulationMode::DualSense, cx);
                })),
            );

        // Right Cloak switch (compact recessed switch matching the rest of the UI)
        let cloak_ctrl = div()
            .h(px(36.0))
            .px_3()
            .rounded(px(8.0))
            .bg(color(WELL_BG))
            .border_1()
            .border_color(color(BORDER))
            .flex()
            .items_center()
            .gap_3()
            .flex_shrink_0()
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(color(if cloak_on { TEXT } else { TEXT_SECONDARY }))
                    .whitespace_nowrap()
                    .child("Cloak"),
            )
            .child(
                toggle_switch("top-cloak-sw", cloak_on)
                    .tooltip(text_tooltip(if cloak_on {
                        "Physical DualSense hidden via HidHide — click to uncloak"
                    } else {
                        "Hide physical DualSense via HidHide to prevent double input"
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_cloak(cx);
                    })),
            );

        div()
            .w_full()
            .px_4()
            .py_3()
            .rounded(px(12.0))
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(BORDER))
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(left_group)
            .child(emu_group)
            .child(cloak_ctrl)
    }

    fn render_raw_output_toggle(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .gap_1()
            .p_1()
            .rounded(px(8.0))
            .bg(color(WELL_BG))
            .border_1()
            .border_color(color(BORDER))
            .child(
                segmented_button("stage-out", "Processed", !self.show_raw_sticks).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.show_raw_sticks = false;
                        cx.notify();
                    }),
                ),
            )
            .child(
                segmented_button("stage-raw", "Raw input", self.show_raw_sticks).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.show_raw_sticks = true;
                        cx.notify();
                    }),
                ),
            )
    }

    /// Open Centerpiece Interactive Two-Tone DualSense Stage floating directly on `#171A1C`
    fn render_controller_stage(&self, compact: bool, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied().unwrap_or_default();
        let connected = !self.handles.is_empty();
        let stage_h = if compact { 164.0 } else { 288.0 };
        // Lock overlay width to the exact 430:228 canvas aspect ratio so hit zones align at every height
        let stage_w = stage_h * (430.0 / 228.0);

        let state = ControllerPreviewState {
            snap,
            override_lightbar: profile.override_lightbar,
            lightbar_rgb: profile.lightbar_rgb,
            player_leds: profile.player_leds,
            show_raw_sticks: self.show_raw_sticks,
            highlight: self.page.highlight_zone(),
            connected,
        };

        let hl_trig = self.page == ActivePage::Triggers;
        let gauge_lbl_col = if hl_trig { ACCENT } else { TEXT_MUTED };

        let stage_interactive = div()
            .relative()
            .w(px(stage_w))
            .h(px(stage_h))
            .flex_shrink_0()
            .child(virtual_dualsense(state, stage_h))
            .child(
                div()
                    .absolute()
                    .left(relative(0.036))
                    .top(relative(0.045))
                    .text_xs()
                    .font_family(FONT_MONO)
                    .text_color(color(gauge_lbl_col))
                    .child("L2"),
            )
            .child(
                div()
                    .absolute()
                    .right(relative(0.034))
                    .top(relative(0.045))
                    .text_xs()
                    .font_family(FONT_MONO)
                    .text_color(color(gauge_lbl_col))
                    .child("R2"),
            )
            .child(
                div()
                    .absolute()
                    .left(relative(0.028))
                    .bottom(relative(0.025))
                    .text_xs()
                    .font_family(FONT_MONO)
                    .text_color(color(TEXT_MUTED))
                    .child("VIB"),
            )
            .child(
                div()
                    .absolute()
                    .right(relative(0.026))
                    .bottom(relative(0.025))
                    .text_xs()
                    .font_family(FONT_MONO)
                    .text_color(color(TEXT_MUTED))
                    .child("VIB"),
            )
            // Aspect-locked interactive hit zones matching the 912×640 DualSense CAD geometry on 430×228 stage
            .child(
                div()
                    .id("zone-l2")
                    .absolute()
                    .left(relative(0.225))
                    .top(relative(0.035))
                    .w(relative(0.110))
                    .h(relative(0.145))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(color_alpha(TEXT, 0x12)))
                    .tooltip(text_tooltip("Triggers & haptics"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = ActivePage::Triggers;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("zone-r2")
                    .absolute()
                    .left(relative(0.665))
                    .top(relative(0.035))
                    .w(relative(0.110))
                    .h(relative(0.145))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(color_alpha(TEXT, 0x12)))
                    .tooltip(text_tooltip("Triggers & haptics"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = ActivePage::Triggers;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("zone-lightbar")
                    .absolute()
                    .left(relative(0.340))
                    .top(relative(0.115))
                    .w(relative(0.320))
                    .h(relative(0.290))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(color_alpha(TEXT, 0x12)))
                    .tooltip(text_tooltip("Lighting & LEDs"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = ActivePage::Lighting;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("zone-sticks")
                    .absolute()
                    .left(relative(0.328))
                    .top(relative(0.418))
                    .w(relative(0.344))
                    .h(relative(0.195))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(color_alpha(TEXT, 0x12)))
                    .tooltip(text_tooltip("Sticks & curves"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = ActivePage::Sticks;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("zone-dpad")
                    .absolute()
                    .left(relative(0.218))
                    .top(relative(0.230))
                    .w(relative(0.112))
                    .h(relative(0.210))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(color_alpha(TEXT, 0x12)))
                    .tooltip(text_tooltip("Motion & remap"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = ActivePage::MotionRemap;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("zone-face")
                    .absolute()
                    .left(relative(0.650))
                    .top(relative(0.200))
                    .w(relative(0.148))
                    .h(relative(0.265))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(color_alpha(TEXT, 0x12)))
                    .tooltip(text_tooltip("Motion & remap"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = ActivePage::MotionRemap;
                        cx.notify();
                    })),
            );

        let ls = if self.show_raw_sticks {
            snap.raw_left_stick
        } else {
            snap.left_stick
        };
        let rs = if self.show_raw_sticks {
            snap.raw_right_stick
        } else {
            snap.right_stick
        };

        let l2_pct = ((snap.l2_analog as f32 / 255.0) * 100.0).round() as u32;
        let r2_pct = ((snap.r2_analog as f32 / 255.0) * 100.0).round() as u32;

        let caption_row = if connected {
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(color(TEXT_MUTED))
                        .child("Select a zone to configure"),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_family(FONT_MONO)
                        .text_color(color(TEXT_SECONDARY))
                        .child(format!(
                            "LS {},{} · RS {},{} · L2 {l2_pct}% · R2 {r2_pct}%",
                            ls[0], ls[1], rs[0], rs[1]
                        )),
                )
        } else {
            div()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(color(TEXT_MUTED))
                        .child("Connect a DualSense over USB or Bluetooth"),
                )
        };

        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(stage_interactive)
            .child(caption_row)
    }

    /// Page 1: Quick settings (Centerpiece DualSense + Asymmetric 3-card deck)
    fn render_quick_settings(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied();
        let s = snap.unwrap_or_default();
        let connected = snap.is_some();

        // Card 1: Adaptive triggers
        let mut l2_pills = div().flex().flex_wrap().items_center().gap_2().child(
            div()
                .w(px(24.0))
                .text_size(px(13.0))
                .font_family(FONT_MONO)
                .text_color(color(TEXT_MUTED))
                .child("L2"),
        );
        for preset in TriggerPreset::ALL {
            let active = profile.override_triggers && profile.left_trigger_preset == preset;
            l2_pills = l2_pills.child(
                pill_button(
                    SharedString::from(format!("qs-l2-{}", preset.label())),
                    preset.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let st = this.config.active_settings_mut();
                    st.override_triggers = true;
                    st.left_trigger_preset = preset;
                    this.commit_config(cx);
                })),
            );
        }

        let mut r2_pills = div().flex().flex_wrap().items_center().gap_2().child(
            div()
                .w(px(24.0))
                .text_size(px(13.0))
                .font_family(FONT_MONO)
                .text_color(color(TEXT_MUTED))
                .child("R2"),
        );
        for preset in TriggerPreset::ALL {
            let active = profile.override_triggers && profile.right_trigger_preset == preset;
            r2_pills = r2_pills.child(
                pill_button(
                    SharedString::from(format!("qs-r2-{}", preset.label())),
                    preset.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let st = this.config.active_settings_mut();
                    st.override_triggers = true;
                    st.right_trigger_preset = preset;
                    this.commit_config(cx);
                })),
            );
        }

        let triggers_card = div()
            .flex()
            .flex_col()
            .justify_between()
            .flex_1()
            .min_w(px(312.0))
            .gap_5()
            .p_5()
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(BORDER))
            .rounded(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
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
                                    .child("Adaptive triggers"),
                            )
                            .child(
                                toggle_switch("qs-trig-override", profile.override_triggers)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let st = this.config.active_settings_mut();
                                        st.override_triggers = !st.override_triggers;
                                        this.commit_config(cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(color(TEXT_MUTED))
                                    .child(if profile.override_triggers {
                                        "Independent motorized resistance on L2 and R2"
                                    } else {
                                        "Motors use native controller behavior"
                                    }),
                            )
                            .child(div().flex().flex_col().gap_2().child(l2_pills).child(r2_pills)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_4()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .child(meter_row_with_width("L2 pull", 56.0, s.l2_analog, ACCENT))
                    .child(meter_row_with_width("R2 pull", 56.0, s.r2_analog, ACCENT)),
            );

        // Card 2: Lightbar & controls
        let mut swatch_chips = div().w_full().flex().items_center().gap_2();
        for (name, rgb_val) in RGB_SWATCHES {
            let active = profile.override_lightbar && profile.lightbar_rgb == rgb_val;
            let hex = rgb_to_u32(rgb_val);
            swatch_chips = swatch_chips.child(
                div()
                    .id(SharedString::from(format!("qs-rgb-{name}")))
                    .flex_1()
                    .h(px(24.0))
                    .rounded(px(6.0))
                    .bg(color(if hex == 0 { WELL_BG } else { hex }))
                    .border_2()
                    .border_color(color(if active { TEXT } else { BORDER_STRONG }))
                    .cursor_pointer()
                    .hover(|st| st.border_color(color(TEXT)))
                    .tooltip(text_tooltip(format!("Lightbar: {name}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let st = this.config.active_settings_mut();
                        st.override_lightbar = true;
                        st.lightbar_rgb = rgb_val;
                        this.sync_picker_hsv();
                        this.commit_config(cx);
                    })),
            );
        }

        let active_lb_rgb = if !profile.override_lightbar {
            [0, 112, 255]
        } else {
            profile.lightbar_rgb
        };
        let active_lb_hex = rgb_to_u32(active_lb_rgb);
        let active_lb_label = if !profile.override_lightbar {
            "Native".to_owned()
        } else if active_lb_hex == 0 {
            "Off".to_owned()
        } else {
            format!(
                "#{:02X}{:02X}{:02X}",
                active_lb_rgb[0], active_lb_rgb[1], active_lb_rgb[2]
            )
        };
        let lb_label_is_hex = profile.override_lightbar && active_lb_hex != 0;
        let mut lb_hex_el = div()
            .text_xs()
            .text_color(color(if lb_label_is_hex {
                TEXT_SECONDARY
            } else {
                TEXT_MUTED
            }))
            .child(active_lb_label);
        if lb_label_is_hex {
            lb_hex_el = lb_hex_el.font_family(FONT_MONO);
        }

        let qs_hue_bar = hue_slider_bar(
            self.picker_hsv.0,
            ActiveSlider::HueBar.slot(),
            Rc::clone(&self.slider_bounds),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.start_slider_drag(ActiveSlider::HueBar, ev.position.x, cx);
            }),
        );

        let mut led_row = div().flex().flex_wrap().items_center().gap_2().child(
            div()
                .w(px(36.0))
                .flex_shrink_0()
                .text_size(px(13.0))
                .text_color(color(TEXT_MUTED))
                .child("LEDs"),
        );
        for (label, mask) in PLAYER_LED_OPTIONS {
            let active = profile.override_lightbar && profile.player_leds == mask;
            led_row = led_row.child(
                pill_button(SharedString::from(format!("qs-led-{label}")), label, active)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let st = this.config.active_settings_mut();
                        st.override_lightbar = true;
                        st.player_leds = mask;
                        this.commit_config(cx);
                    })),
            );
        }

        let lighting_card = div()
            .flex()
            .flex_col()
            .justify_between()
            .flex_1()
            .min_w(px(312.0))
            .gap_5()
            .p_5()
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(BORDER))
            .rounded(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
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
                                    .child("Lightbar & controls"),
                            )
                            .child(
                                toggle_switch("qs-lb-override", profile.override_lightbar)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let st = this.config.active_settings_mut();
                                        st.override_lightbar = !st.override_lightbar;
                                        this.commit_config(cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_size(px(13.0))
                                                            .text_color(color(TEXT_MUTED))
                                                            .child("Color"),
                                                    )
                                                    .child(
                                                        div()
                                                            .size(px(12.0))
                                                            .rounded(px(3.0))
                                                            .bg(color(if active_lb_hex == 0 {
                                                                WELL_BG
                                                            } else {
                                                                active_lb_hex
                                                            }))
                                                            .border_1()
                                                            .border_color(color(BORDER_STRONG)),
                                                    )
                                                    .child(lb_hex_el),
                                            )
                                            .child(
                                                div()
                                                    .id("qs-open-color-picker")
                                                    .px_2()
                                                    .py(px(2.0))
                                                    .rounded(px(6.0))
                                                    .bg(color(WELL_BG))
                                                    .border_1()
                                                    .border_color(color(BORDER))
                                                    .text_xs()
                                                    .text_color(color(TEXT_SECONDARY))
                                                    .cursor_pointer()
                                                    .hover(|st| {
                                                        st.bg(color(SURFACE_ALT))
                                                            .border_color(color(BORDER_STRONG))
                                                            .text_color(color(TEXT))
                                                    })
                                                    .tooltip(text_tooltip(
                                                        "Open full 2D color & RGB picker",
                                                    ))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.page = ActivePage::Lighting;
                                                        cx.notify();
                                                    }))
                                                    .child("Custom"),
                                            ),
                                    )
                                    .child(swatch_chips)
                                    .child(qs_hue_bar),
                            )
                            .child(led_row),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_4()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .child(
                        div()
                            .h(px(28.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(color(TEXT_SECONDARY))
                                    .child("Rumble feedback"),
                            )
                            .child(
                                toggle_switch("qs-rumble", profile.enable_rumble).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        let st = this.config.active_settings_mut();
                                        st.enable_rumble = !st.enable_rumble;
                                        this.commit_config(cx);
                                    }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .h(px(28.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(color(TEXT_SECONDARY))
                                    .child("Touchpad mouse"),
                            )
                            .child(
                                toggle_switch("qs-tpmouse", profile.touchpad_mouse).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        let st = this.config.active_settings_mut();
                                        st.touchpad_mouse = !st.touchpad_mouse;
                                        this.commit_config(cx);
                                    }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .h(px(32.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(color(TEXT_SECONDARY))
                                    .child("Gyro aim"),
                            )
                            .child(
                                pill_button(
                                    "qs-gyro-cycle",
                                    profile.gyro_mode.label(),
                                    profile.gyro_mode != GyroAimMode::Off,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let st = this.config.active_settings_mut();
                                    st.gyro_mode = match st.gyro_mode {
                                        GyroAimMode::Off => GyroAimMode::HoldL2,
                                        GyroAimMode::HoldL2 => GyroAimMode::HoldL1,
                                        GyroAimMode::HoldL1 => GyroAimMode::AlwaysOn,
                                        GyroAimMode::AlwaysOn => GyroAimMode::Off,
                                    };
                                    this.commit_config(cx);
                                })),
                            ),
                    ),
            );

        // Card 3: Polling & latency (wider lead instrument card for sparkline clarity)
        let scan_label = if connected {
            format!("{:.0} Hz", s.polling_hz)
        } else {
            "Idle".to_owned()
        };
        let proc_label = if connected {
            format!("{:.0} µs avg · {} µs max", s.proc_us_ema.round(), s.proc_us_max)
        } else {
            "Waiting for HID report stream".to_owned()
        };
        let pill_text = if let Some(sn) = snap {
            let tr = match sn.transport {
                Transport::Usb => "USB direct",
                Transport::Bluetooth => "Bluetooth",
            };
            let bat = match sn.battery_status {
                BatteryStatus::Charging => format!("{}% charging", sn.battery_percent),
                BatteryStatus::Full => "100% full".to_owned(),
                _ => format!("{}% battery", sn.battery_percent),
            };
            format!("{tr} · {bat}")
        } else {
            "Connect DualSense over USB or BT".to_owned()
        };

        let samples: Vec<f32> = self.latency_history.iter().copied().collect();

        let mut scan_val_el = div()
            .text_size(px(32.0))
            .font_weight(FontWeight::NORMAL)
            .text_color(color(if connected { ACCENT } else { TEXT_MUTED }))
            .child(scan_label);
        if connected {
            scan_val_el = scan_val_el.font_family(FONT_MONO);
        }

        let mut proc_el = div()
            .text_size(px(13.0))
            .text_color(color(TEXT_MUTED))
            .child(proc_label);
        if connected {
            proc_el = proc_el.font_family(FONT_MONO);
        }

        let telemetry_card = div()
            .flex()
            .flex_col()
            .justify_between()
            .flex_1()
            .min_w(px(312.0))
            .gap_5()
            .p_5()
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(BORDER))
            .rounded(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex_shrink_0()
                            .min_h(px(24.0))
                            .flex()
                            .items_center()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .child("Polling & latency"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(color(TEXT_MUTED))
                                                    .child("Polling rate"),
                                            )
                                            .child(scan_val_el),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_family(FONT_MONO)
                                            .text_color(color(TEXT_SECONDARY))
                                            .child(if connected {
                                                format!("{} pkts", fmt_thousands(s.packet_count))
                                            } else {
                                                "0 pkts".to_owned()
                                            }),
                                    ),
                            )
                            .child(proc_el),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(latency_sparkline(samples, connected))
                    .child(status_inset_pill(pill_text, connected)),
            );

        let header_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .text_size(px(20.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color(TEXT))
                    .child("Overview"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(self.render_raw_output_toggle(cx))
                    .child(
                        subtle_button("qs-reset-overrides", "Reset overrides")
                            .tooltip(text_tooltip(
                                "Leave adaptive triggers and lightbar untouched",
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let st = this.config.active_settings_mut();
                                st.override_triggers = false;
                                st.override_lightbar = false;
                                this.commit_config(cx);
                            })),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(header_row)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .child(triggers_card)
                    .child(lighting_card)
                    .child(telemetry_card),
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
            "Radial deadzone",
            format!("{}%", (tuning.deadzone * 100.0).round() as u32),
            tuning.deadzone / MAX_STICK_DZ,
            ACCENT,
            dz_slider,
            cx,
        );

        let adz_row = self.render_slider_row(
            "Anti-deadzone",
            format!("{}%", (tuning.anti_deadzone * 100.0).round() as u32),
            tuning.anti_deadzone / MAX_STICK_DZ,
            ZONE_SECONDARY,
            adz_slider,
            cx,
        );

        let mut curve_pills = div().flex().flex_wrap().items_center().gap_2();
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

        section_card_with_action(
            title,
            subtle_button(SharedString::from(format!("{prefix}-reset")), "Reset").on_click(
                cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    if is_left {
                        s.left_stick = StickTuning::default();
                    } else {
                        s.right_stick = StickTuning::default();
                    }
                    this.commit_config(cx);
                }),
            ),
        )
        .flex_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_around()
                .gap_4()
                .py_1()
                .child(deadzone_showcase(raw, out, tuning, connected))
                .child(response_curve_chart(raw, tuning)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .pt_3()
                .border_t_1()
                .border_color(color(BORDER))
                .child(dz_row)
                .child(adz_row)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(color(TEXT_SECONDARY))
                                .child("Response curve"),
                        )
                        .child(curve_pills),
                ),
        )
    }

    /// Page 2: Sticks & curves
    fn render_sticks_page(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied().unwrap_or_default();
        let connected = !self.handles.is_empty();

        let left_card = self.render_stick_card(
            "Left thumbstick",
            "ls",
            profile.left_stick,
            snap.raw_left_stick,
            snap.left_stick,
            connected,
            true,
            cx,
        );
        let right_card = self.render_stick_card(
            "Right thumbstick",
            "rs",
            profile.right_stick,
            snap.raw_right_stick,
            snap.right_stick,
            connected,
            false,
            cx,
        );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .child("Sticks & response curves"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(color(TEXT_MUTED))
                                    .child("Blue: deadzone · Silver: anti-deadzone"),
                            )
                            .child(self.render_raw_output_toggle(cx)),
                    ),
            )
            .child(div().flex().gap_4().child(left_card).child(right_card))
    }

    /// Page 3: Triggers & haptics
    fn render_triggers_page(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied().unwrap_or_default();

        let l2_preset_lbl = if profile.override_triggers {
            profile.left_trigger_preset.label()
        } else {
            "Native"
        };
        let r2_preset_lbl = if profile.override_triggers {
            profile.right_trigger_preset.label()
        } else {
            "Native"
        };

        let travel_card = section_card("Trigger travel & thresholds").child(
            div()
                .flex()
                .items_center()
                .gap_6()
                .child(trigger_travel_gauge(
                    "L2 trigger",
                    snap.l2_analog,
                    profile.triggers.deadzone,
                    profile.triggers.hair_trigger,
                    l2_preset_lbl,
                    profile.override_triggers,
                ))
                .child(div().w(px(1.0)).h(px(56.0)).bg(color(BORDER)))
                .child(trigger_travel_gauge(
                    "R2 trigger",
                    snap.r2_analog,
                    profile.triggers.deadzone,
                    profile.triggers.hair_trigger,
                    r2_preset_lbl,
                    profile.override_triggers,
                )),
        );

        let mut l2_grid = div().flex().flex_wrap().gap_2();
        for preset in TriggerPreset::ALL {
            let active = profile.override_triggers && profile.left_trigger_preset == preset;
            l2_grid = l2_grid.child(
                pill_button(
                    SharedString::from(format!("tp-l2-{}", preset.label())),
                    preset.label(),
                    active,
                )
                .tooltip(text_tooltip(trigger_preset_desc(preset)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.override_triggers = true;
                    s.left_trigger_preset = preset;
                    this.commit_config(cx);
                })),
            );
        }

        let mut r2_grid = div().flex().flex_wrap().gap_2();
        for preset in TriggerPreset::ALL {
            let active = profile.override_triggers && profile.right_trigger_preset == preset;
            r2_grid = r2_grid.child(
                pill_button(
                    SharedString::from(format!("tp-r2-{}", preset.label())),
                    preset.label(),
                    active,
                )
                .tooltip(text_tooltip(trigger_preset_desc(preset)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.override_triggers = true;
                    s.right_trigger_preset = preset;
                    this.commit_config(cx);
                })),
            );
        }

        let l2_desc = if profile.override_triggers {
            trigger_preset_desc(profile.left_trigger_preset)
        } else {
            "Using native L2 resistance"
        };
        let r2_desc = if profile.override_triggers {
            trigger_preset_desc(profile.right_trigger_preset)
        } else {
            "Using native R2 resistance"
        };

        let adaptive_card = section_card_with_action(
            "Adaptive trigger actuators",
            toggle_switch("tp-trig-override", profile.override_triggers).on_click(cx.listener(
                |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.override_triggers = !s.override_triggers;
                    this.commit_config(cx);
                },
            )),
        )
        .flex_1()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(color(TEXT_SECONDARY))
                        .child("Left trigger (L2)"),
                )
                .child(l2_grid)
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(color(TEXT_MUTED))
                        .child(l2_desc),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .pt_3()
                .border_t_1()
                .border_color(color(BORDER))
                .child(
                    div()
                        .text_sm()
                        .text_color(color(TEXT_SECONDARY))
                        .child("Right trigger (R2)"),
                )
                .child(r2_grid)
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(color(TEXT_MUTED))
                        .child(r2_desc),
                ),
        );

        let trig_dz_pct = (profile.triggers.deadzone as f32 * 100.0 / 255.0).round() as u32;
        let trig_dz_row = self.render_slider_row(
            "Trigger deadzone",
            format!("{trig_dz_pct}%"),
            trig_dz_pct as f32 / MAX_TRIGGER_DZ_PCT,
            ACCENT,
            ActiveSlider::Trigger,
            cx,
        );

        let mut hair_pills = div().flex().flex_wrap().gap_2();
        for (label, threshold) in HAIR_TRIGGER_STEPS {
            let active = profile.triggers.hair_trigger == threshold;
            hair_pills = hair_pills.child(
                pill_button(SharedString::from(format!("tp-ht-{label}")), label, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.config.active_settings_mut().triggers.hair_trigger = threshold;
                        this.commit_config(cx);
                    }),
                ),
            );
        }

        let tuning_card = section_card("Hair trigger & rumble motors")
            .flex_1()
            .child(trig_dz_row)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(color(TEXT_SECONDARY))
                            .child("Hair trigger cap"),
                    )
                    .child(hair_pills),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .child(setting_row(
                        "Rumble feedback",
                        "Route virtual controller rumble back to the physical DualSense",
                        toggle_switch("tp-rumble-sw", profile.enable_rumble).on_click(cx.listener(
                            |this, _, _, cx| {
                                let s = this.config.active_settings_mut();
                                s.enable_rumble = !s.enable_rumble;
                                this.commit_config(cx);
                            },
                        )),
                    ))
                    .child(meter_row("Large motor", snap.rumble_large, WARN))
                    .child(meter_row("Small motor", snap.rumble_small, WARN)),
            );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .child("Triggers & haptics"),
                    )
                    .child(self.render_raw_output_toggle(cx)),
            )
            .child(travel_card)
            .child(div().flex().gap_4().child(adaptive_card).child(tuning_card))
    }

    /// Page 4: Lighting & player LEDs
    fn render_lighting_page(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied().unwrap_or_default();

        let make_rgb_tile = |name: &'static str, rgb_val: [u8; 3], cx: &mut Context<Self>| {
            let active = profile.override_lightbar && profile.lightbar_rgb == rgb_val;
            let hex = rgb_to_u32(rgb_val);
            let hex_text = format!("#{:02X}{:02X}{:02X}", rgb_val[0], rgb_val[1], rgb_val[2]);
            div()
                .id(SharedString::from(format!("lp-rgb-{name}")))
                .flex_1()
                .min_w(px(124.0))
                .px_3()
                .py_2()
                .rounded(px(8.0))
                .bg(color(if active { SURFACE_ALT } else { WELL_BG }))
                .border_1()
                .border_color(color(if active { BORDER_STRONG } else { BORDER }))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .hover(|s| s.border_color(color(BORDER_STRONG)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.override_lightbar = true;
                    s.lightbar_rgb = rgb_val;
                    this.sync_picker_hsv();
                    this.commit_config(cx);
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .size(px(20.0))
                                .rounded(px(5.0))
                                .bg(color(if hex == 0 { SURFACE } else { hex }))
                                .border_1()
                                .border_color(color(BORDER_STRONG))
                                .flex_shrink_0(),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(color(TEXT))
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family(FONT_MONO)
                                        .text_color(color(TEXT_MUTED))
                                        .child(hex_text),
                                ),
                        ),
                )
                .child(if active {
                    check_icon(14.0, ACCENT)
                } else {
                    div()
                })
        };

        let mut row1 = div().flex().gap_2();
        for (name, rgb_val) in &RGB_SWATCHES[..4] {
            row1 = row1.child(make_rgb_tile(name, *rgb_val, cx));
        }
        let mut row2 = div().flex().gap_2();
        for (name, rgb_val) in &RGB_SWATCHES[4..] {
            row2 = row2.child(make_rgb_tile(name, *rgb_val, cx));
        }
        let swatch_grid = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(color(TEXT_MUTED))
                    .child("Quick presets"),
            )
            .child(row1)
            .child(row2);

        let cur_rgb = profile.lightbar_rgb;
        let cur_hex = rgb_to_u32(cur_rgb);
        let game_controlling_lb = !profile.override_lightbar && snap.active_lightbar_rgb.is_some();
        let active_rgb = if profile.override_lightbar {
            cur_rgb
        } else {
            snap.active_lightbar_rgb.unwrap_or([0, 112, 255])
        };
        let active_hex = rgb_to_u32(active_rgb);

        let (hue, sat, val) = self.picker_hsv;

        let sv_pad = sv_color_pad(hue, sat, val, cur_hex, Rc::clone(&self.sv_pad_bounds))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.start_sv_drag(ev.position.x, ev.position.y, cx);
                }),
            );

        let hue_bar = hue_slider_bar(
            hue,
            ActiveSlider::HueBar.slot(),
            Rc::clone(&self.slider_bounds),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.start_slider_drag(ActiveSlider::HueBar, ev.position.x, cx);
            }),
        );

        let make_channel_row = |ch_label: &'static str,
                                ch_val: u8,
                                min_hex: u32,
                                max_hex: u32,
                                slider: ActiveSlider,
                                cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(14.0))
                        .flex_shrink_0()
                        .text_xs()
                        .font_family(FONT_MONO)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(TEXT_MUTED))
                        .child(ch_label),
                )
                .child(
                    rgb_channel_bar(
                        min_hex,
                        max_hex,
                        ch_val as f32 / 255.0,
                        cur_hex,
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
                .child(
                    div()
                        .w(px(40.0))
                        .h(px(24.0))
                        .rounded(px(6.0))
                        .bg(color(SURFACE))
                        .border_1()
                        .border_color(color(BORDER))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .text_xs()
                        .font_family(FONT_MONO)
                        .text_color(color(TEXT))
                        .child(format!("{ch_val}")),
                )
        };

        let r_row = make_channel_row(
            "R",
            cur_rgb[0],
            rgb_to_u32([0, cur_rgb[1], cur_rgb[2]]),
            rgb_to_u32([255, cur_rgb[1], cur_rgb[2]]),
            ActiveSlider::RedChannel,
            cx,
        );
        let g_row = make_channel_row(
            "G",
            cur_rgb[1],
            rgb_to_u32([cur_rgb[0], 0, cur_rgb[2]]),
            rgb_to_u32([cur_rgb[0], 255, cur_rgb[2]]),
            ActiveSlider::GreenChannel,
            cx,
        );
        let b_row = make_channel_row(
            "B",
            cur_rgb[2],
            rgb_to_u32([cur_rgb[0], cur_rgb[1], 0]),
            rgb_to_u32([cur_rgb[0], cur_rgb[1], 255]),
            ActiveSlider::BlueChannel,
            cx,
        );

        let lb_is_numeric = (profile.override_lightbar || game_controlling_lb) && active_hex != 0;
        let mut lb_readout_el = div()
            .text_size(px(13.0))
            .text_color(color(if lb_is_numeric { TEXT } else { TEXT_SECONDARY }))
            .child(if !profile.override_lightbar && !game_controlling_lb {
                "Native or game default".to_owned()
            } else if active_hex == 0 {
                "LED off".to_owned()
            } else {
                format!(
                    "#{:02X}{:02X}{:02X}",
                    active_rgb[0], active_rgb[1], active_rgb[2]
                )
            });
        if lb_is_numeric {
            lb_readout_el = lb_readout_el.font_family(FONT_MONO);
        }

        let color_picker_well = div()
            .p_4()
            .rounded(px(8.0))
            .bg(color(WELL_BG))
            .border_1()
            .border_color(color(BORDER))
            .flex()
            .gap_5()
            .child(sv_pad)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .size(px(28.0))
                                            .rounded(px(6.0))
                                            .bg(if !profile.override_lightbar && !game_controlling_lb {
                                                color(ZONE_BLUE)
                                            } else if active_hex == 0 {
                                                color(SURFACE)
                                            } else {
                                                color(active_hex)
                                            })
                                            .border_1()
                                            .border_color(color(BORDER_STRONG))
                                            .flex_shrink_0(),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(color(TEXT))
                                                    .child("Custom color"),
                                            )
                                            .child(lb_readout_el),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(FONT_MONO)
                                    .text_color(color(TEXT_MUTED))
                                    .child(format!("Hue {:.0}°", hue.round())),
                            ),
                    )
                    .child(hue_bar)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(r_row)
                            .child(g_row)
                            .child(b_row),
                    ),
            );

        let ds4_note = if let Some([r, g, b]) = snap.active_lightbar_rgb {
            format!("Game lightbar active: #{r:02X}{g:02X}{b:02X}")
        } else if !profile.override_lightbar {
            "LEDs follow native or game control".to_owned()
        } else {
            "Custom lightbar color active on connected DualSense".to_owned()
        };

        let rgb_card = section_card_with_action(
            "Lightbar color",
            toggle_switch("lp-lb-override", profile.override_lightbar).on_click(cx.listener(
                |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.override_lightbar = !s.override_lightbar;
                    this.commit_config(cx);
                },
            )),
        )
        .flex_1()
        .child(color_picker_well)
        .child(swatch_grid)
        .child(if profile.override_lightbar {
            status_inset_pill(ds4_note, true)
        } else {
            status_info_pill(ds4_note)
        });

        let mut led_cards = div().flex().flex_col().gap_2();
        for (label, mask) in PLAYER_LED_OPTIONS {
            let active = profile.override_lightbar && profile.player_leds == mask;
            let mut segments = div().flex().items_center().gap_2();
            for i in 0..5u8 {
                let lit = (mask & (1 << i)) != 0;
                segments = segments.child(
                    div()
                        .w(px(12.0))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(color(if lit {
                            if active { ACCENT } else { TEXT }
                        } else {
                            BORDER_STRONG
                        })),
                );
            }
            led_cards = led_cards.child(
                div()
                    .id(SharedString::from(format!("lp-led-{label}")))
                    .h(px(40.0))
                    .px_3()
                    .rounded(px(8.0))
                    .bg(color(if active { SURFACE_ALT } else { WELL_BG }))
                    .border_1()
                    .border_color(color(if active { BORDER_STRONG } else { BORDER }))
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .hover(|s| s.border_color(color(BORDER_STRONG)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.override_lightbar = true;
                        s.player_leds = mask;
                        this.commit_config(cx);
                    }))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(if active {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(color(if active { TEXT } else { TEXT_SECONDARY }))
                            .child(if label == "Off" {
                                "All off".to_owned()
                            } else {
                                format!("Player {}", label.trim_start_matches('P'))
                            }),
                    )
                    .child(segments),
            );
        }

        let leds_card = section_card("Player indicator LEDs")
            .w(px(336.0))
            .flex_shrink_0()
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(color(TEXT_MUTED))
                    .child("Select the 5-LED bar pattern beneath the touchpad"),
            )
            .child(led_cards);

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .child("Lighting & player LEDs"),
                    )
                    .child(self.render_raw_output_toggle(cx)),
            )
            .child(div().flex().gap_4().child(rgb_card).child(leds_card))
    }

    /// Page 5: Motion & button remap
    fn render_motion_remap_page(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();

        let mut preset_tiles = div().flex().flex_wrap().gap_3();
        for preset in ButtonPreset::ALL {
            let active = profile.button_preset == preset;
            preset_tiles = preset_tiles.child(
                div()
                    .id(SharedString::from(format!("mr-bp-{}", preset.label())))
                    .w(px(224.0))
                    .flex_1()
                    .min_w(px(200.0))
                    .p_4()
                    .rounded(px(8.0))
                    .bg(color(if active { SURFACE_ALT } else { WELL_BG }))
                    .border_1()
                    .border_color(color(if active { BORDER_STRONG } else { BORDER }))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.border_color(color(BORDER_STRONG)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.config.active_settings_mut().button_preset = preset;
                        this.commit_config(cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color(if active { TEXT } else { TEXT_SECONDARY }))
                                    .child(preset.label()),
                            )
                            .child(if active {
                                check_icon(16.0, ACCENT)
                            } else {
                                div()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(color(TEXT_MUTED))
                            .child(button_preset_desc(preset)),
                    ),
            );
        }

        let remap_card = section_card("Button layout presets")
            .flex_1()
            .child(preset_tiles);

        let tp_enabled = profile.touchpad_mouse;
        let mut tp_sens = div().flex().flex_wrap().gap_2();
        if !tp_enabled {
            tp_sens = tp_sens.opacity(0.45);
        }
        for (label, sens) in SENSITIVITY_STEPS {
            let active = tp_enabled && (profile.touchpad_sensitivity - sens).abs() < 0.01;
            tp_sens = tp_sens.child(
                pill_button(SharedString::from(format!("mr-tps-{label}")), label, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.touchpad_mouse = true;
                        s.touchpad_sensitivity = sens;
                        this.commit_config(cx);
                    }),
                ),
            );
        }

        let mut gyro_modes = div().flex().flex_wrap().gap_2();
        for mode in GyroAimMode::ALL {
            let active = profile.gyro_mode == mode;
            gyro_modes = gyro_modes.child(
                pill_button(
                    SharedString::from(format!("mr-gm-{}", mode.label())),
                    mode.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.active_settings_mut().gyro_mode = mode;
                    this.commit_config(cx);
                })),
            );
        }

        let gyro_enabled = profile.gyro_mode != GyroAimMode::Off;
        let mut gyro_sens = div().flex().flex_wrap().gap_2();
        if !gyro_enabled {
            gyro_sens = gyro_sens.opacity(0.45);
        }
        for (label, sens) in SENSITIVITY_STEPS {
            let active = gyro_enabled && (profile.gyro_sensitivity - sens).abs() < 0.01;
            gyro_sens = gyro_sens.child(
                pill_button(SharedString::from(format!("mr-gs-{label}")), label, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        if s.gyro_mode == GyroAimMode::Off {
                            s.gyro_mode = GyroAimMode::HoldL2;
                        }
                        s.gyro_sensitivity = sens;
                        this.commit_config(cx);
                    }),
                ),
            );
        }

        let motion_card = section_card("Touchpad & gyro mouse")
            .flex_1()
            .child(setting_row(
                "Touchpad mouse",
                "Use the DualSense touchpad as a desktop pointer",
                toggle_switch("mr-tp-sw", profile.touchpad_mouse).on_click(cx.listener(
                    |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.touchpad_mouse = !s.touchpad_mouse;
                        this.commit_config(cx);
                    },
                )),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(color(if tp_enabled {
                                TEXT_SECONDARY
                            } else {
                                TEXT_MUTED
                            }))
                            .child("Touchpad sensitivity"),
                    )
                    .child(tp_sens),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .pt_3()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color(TEXT))
                                    .child("Gyro aim activation"),
                            )
                            .child(gyro_modes),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(color(if gyro_enabled {
                                        TEXT_SECONDARY
                                    } else {
                                        TEXT_MUTED
                                    }))
                                    .child("Gyro sensitivity"),
                            )
                            .child(gyro_sens),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .child("Motion & button remap"),
                    )
                    .child(self.render_raw_output_toggle(cx)),
            )
            .child(div().flex().gap_4().child(remap_card).child(motion_card))
    }

    /// Page 6: My profiles
    fn render_profiles_page(&self, cx: &mut Context<Self>) -> Div {
        let total = self.config.profiles.len();
        let last_fg_exe = self.engine.as_ref().and_then(|e| e.last_foreground_exe());

        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(22.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .child("My profiles"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(color(TEXT_MUTED))
                            .child("Create per-game configurations and link them to foreground executables"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .h(px(36.0))
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .rounded(px(8.0))
                            .bg(color(SURFACE))
                            .border_1()
                            .border_color(color(BORDER))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(color(TEXT_SECONDARY))
                                    .child("Auto-switch"),
                            )
                            .child(
                                toggle_switch("prof-auto-sw", self.config.auto_switch_profiles)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.config.auto_switch_profiles =
                                            !this.config.auto_switch_profiles;
                                        this.commit_config(cx);
                                    })),
                            ),
                    )
                    .child(
                        primary_cta_button(
                            "prof-create-btn",
                            plus_icon(16.0, ACCENT_FG),
                            "New profile",
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.create_profile(cx);
                        })),
                    ),
            );

        let mut grid = div().flex().flex_wrap().gap_4();
        for (idx, p) in self.config.profiles.iter().enumerate() {
            let active = p.name == self.config.active_profile;
            let name_sel = p.name.clone();
            let name_del = p.name.clone();
            let name_link = p.name.clone();
            let name_unlink = p.name.clone();

            let emu_tag = match p.settings.emulation {
                EmulationMode::Xbox360 => "Xbox 360",
                EmulationMode::DualShock4 => "DualShock 4",
                EmulationMode::DualSense => "DualSense",
                EmulationMode::None => "Passthrough",
            };

            let can_link = last_fg_exe.as_ref().filter(|exe| {
                !p.linked_exe
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(exe))
            });

            let mut card = div()
                .w(px(360.0))
                .p_5()
                .rounded(px(12.0))
                .bg(color(SURFACE))
                .border_1()
                .border_color(color(if active { BORDER_STRONG } else { BORDER }))
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .size(px(36.0))
                                        .rounded(px(8.0))
                                        .bg(color(SURFACE_ALT))
                                        .border_1()
                                        .border_color(color(if active {
                                            BORDER_STRONG
                                        } else {
                                            BORDER
                                        }))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .flex_shrink_0()
                                        .child(gamepad_icon(
                                            16.0,
                                            if active { TEXT } else { TEXT_MUTED },
                                        )),
                                )
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(color(TEXT))
                                        .whitespace_nowrap()
                                        .child(p.name.clone()),
                                ),
                        )
                        .child(if active {
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(check_icon(16.0, ACCENT))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(color(ACCENT))
                                        .child("Active"),
                                )
                        } else {
                            div()
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded(px(6.0))
                                .bg(color(WELL_BG))
                                .text_xs()
                                .text_color(color(TEXT_SECONDARY))
                                .child(emu_tag),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded(px(6.0))
                                .bg(color(WELL_BG))
                                .text_xs()
                                .text_color(color(TEXT_SECONDARY))
                                .child(p.settings.left_stick.curve.label()),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded(px(6.0))
                                .bg(color(WELL_BG))
                                .text_xs()
                                .text_color(color(TEXT_SECONDARY))
                                .child(p.settings.button_preset.label()),
                        ),
                );

            if let Some(exe) = &p.linked_exe {
                card = card.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(px(8.0))
                        .bg(color(WELL_BG))
                        .border_1()
                        .border_color(color(BORDER))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_family(FONT_MONO)
                                .text_color(color(SUCCESS))
                                .child(exe.clone()),
                        )
                        .child(
                            subtle_button(
                                SharedString::from(format!("pc-unlink-{idx}")),
                                "Unlink",
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.unlink_profile_app(&name_unlink, cx);
                            })),
                        ),
                );
            }

            let footer_left: Div = if active {
                div()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .text_size(px(13.0))
                    .text_color(color(TEXT_MUTED))
                    .child("Currently active")
            } else {
                div().child(
                    subtle_button(
                        SharedString::from(format!("pc-activate-{idx}")),
                        "Activate",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.config.active_profile = name_sel.clone();
                        this.sync_picker_hsv();
                        this.commit_config(cx);
                    })),
                )
            };

            let mut right_acts = div().flex().items_center().gap_2();
            if let Some(exe) = can_link {
                let exe_clone = exe.clone();
                right_acts = right_acts.child(
                    div()
                        .id(SharedString::from(format!("pc-link-{idx}")))
                        .h(px(32.0))
                        .px_3()
                        .rounded(px(8.0))
                        .bg(color(SURFACE_ALT))
                        .border_1()
                        .border_color(color(BORDER))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|s| s.bg(color(SURFACE_HOVER)).border_color(color(BORDER_STRONG)))
                        .tooltip(text_tooltip(format!("Auto-switch when {exe} is focused")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.link_profile_to_app(&name_link, exe_clone.clone(), cx);
                        }))
                        .child(link_icon(16.0, TEXT_SECONDARY))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(color(TEXT_SECONDARY))
                                .child(format!("Link {exe}")),
                        ),
                );
            }
            if total > 1 {
                right_acts = right_acts.child(
                    div()
                        .id(SharedString::from(format!("pc-del-{idx}")))
                        .size(px(32.0))
                        .rounded(px(8.0))
                        .bg(color(SURFACE_ALT))
                        .border_1()
                        .border_color(color(BORDER))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| {
                            s.bg(color(DANGER_SURFACE))
                                .border_color(color(DANGER_BORDER))
                        })
                        .tooltip(text_tooltip("Delete profile"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.delete_profile(&name_del, cx);
                        }))
                        .child(trash_icon(16.0, TEXT_SECONDARY)),
                );
            }

            let footer = div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .pt_3()
                .border_t_1()
                .border_color(color(BORDER))
                .child(footer_left)
                .child(right_acts);
            grid = grid.child(card.child(footer));
        }

        let fg_banner = match last_fg_exe {
            Some(exe) => status_inset_pill(
                format!("Last detected foreground app: {exe}"),
                self.config.auto_switch_profiles,
            ),
            None => status_info_pill(
                "Focus any game or application window, then return here to link it to a profile",
            ),
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(header)
            .child(fg_banner)
            .child(grid)
    }

    /// Page 7: Settings (app lifecycle, system drivers, and local config — zero duplicated controls)
    fn render_settings_page(&self, cx: &mut Context<Self>) -> Div {
        let bg_on = self.config.work_in_background;
        let start_min_on = self.config.start_minimized;
        let startup_on = self.run_at_startup;
        let all_drivers_ok =
            self.vigem_available && self.hidhide_available && self.vds_available;

        let hardware_summary = if let Some(s) = self.selected_snapshot() {
            let model = if s.is_edge {
                "DualSense Edge"
            } else {
                "DualSense"
            };
            let tr = match s.transport {
                Transport::Usb => "USB",
                Transport::Bluetooth => "BT",
            };
            if self.handles.len() > 1 {
                format!("{} connected · {model} ({tr})", self.handles.len())
            } else {
                format!("{model} ({tr})")
            }
        } else {
            "None detected".to_owned()
        };

        let startup_card = section_card("Startup & tray")
            .flex_1()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(setting_row(
                        "Work in background",
                        "Keep running in the system tray when closing the window",
                        toggle_switch("set-bg-sw", bg_on).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.config.work_in_background = !this.config.work_in_background;
                                if !this.config.work_in_background {
                                    this.config.start_minimized = false;
                                }
                                this.commit_config(cx);
                            },
                        )),
                    ))
                    .child(setting_row(
                        "Start minimized",
                        "Launch silently into the system tray at startup",
                        toggle_switch("set-min-sw", start_min_on).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.config.start_minimized = !this.config.start_minimized;
                                if this.config.start_minimized {
                                    this.config.work_in_background = true;
                                }
                                this.commit_config(cx);
                            },
                        )),
                    ))
                    .child(setting_row(
                        "Launch at Windows startup",
                        "Start DSMaster automatically when signing in to Windows",
                        toggle_switch("set-startup-sw", startup_on).on_click(cx.listener(
                            |this, _, _, cx| {
                                let next = !this.run_at_startup;
                                match set_run_at_startup(next) {
                                    Ok(()) => {
                                        this.run_at_startup = next;
                                        this.status_notice = None;
                                    }
                                    Err(e) => {
                                        this.status_notice =
                                            Some(format!("Startup registry error: {e}"));
                                    }
                                }
                                cx.notify();
                            },
                        )),
                    )),
            )
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .child(
                        subtle_button("set-hide-tray", "Hide to tray").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.hide_to_tray(cx);
                            },
                        )),
                    )
                    .child(
                        danger_button("set-quit", "Quit DSMaster").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.quit_app(cx);
                            },
                        )),
                    ),
            );

        let active_install = self
            .driver_install
            .as_ref()
            .map(|job| (job.kind, job.phase()));
        let any_installing = active_install.is_some();

        let driver_row_control = |kind: DriverKind,
                                  installed: bool,
                                  btn_id: &'static str|
         -> Div {
            if installed {
                return driver_status_pill(true);
            }
            if let Some((active_kind, phase)) = active_install.as_ref()
                && *active_kind == kind
            {
                let label = match phase {
                    DriverInstallPhase::Downloading => "Downloading…",
                    DriverInstallPhase::RunningInstaller => "Installing…",
                    DriverInstallPhase::Finished(_) => "Finishing…",
                };
                return div()
                    .h(px(28.0))
                    .px_3()
                    .rounded(px(6.0))
                    .bg(color(SURFACE_ALT))
                    .border_1()
                    .border_color(color(BORDER_STRONG))
                    .flex()
                    .items_center()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color(ACCENT))
                    .whitespace_nowrap()
                    .child(label);
            }

            let mut install_btn = div()
                .id(btn_id)
                .h(px(28.0))
                .px_3()
                .rounded(px(6.0))
                .bg(color(if any_installing {
                    SURFACE_ALT
                } else {
                    ACCENT_SOLID
                }))
                .border_1()
                .border_color(color(if any_installing {
                    BORDER
                } else {
                    ACCENT_SOLID
                }))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(if any_installing {
                    TEXT_MUTED
                } else {
                    ACCENT_FG
                }))
                .whitespace_nowrap()
                .child("Install");

            if !any_installing {
                install_btn = install_btn
                    .cursor_pointer()
                    .hover(|s| {
                        s.bg(color(ACCENT_SOLID_HOVER))
                            .border_color(color(ACCENT_SOLID_HOVER))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_driver_install(kind, cx);
                    }));
            }

            div()
                .flex()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .child(driver_status_pill(false))
                .child(install_btn)
        };

        let drivers_footer = if let Some((kind, phase)) = active_install.as_ref() {
            let msg = match phase {
                DriverInstallPhase::Downloading => {
                    format!("Downloading official {} installer...", kind.display_name())
                }
                DriverInstallPhase::RunningInstaller => format!(
                    "Complete the {} installer window to continue",
                    kind.display_name()
                ),
                DriverInstallPhase::Finished(_) => {
                    format!("Refreshing {} status...", kind.display_name())
                }
            };
            status_info_pill(msg)
        } else if all_drivers_ok {
            status_inset_pill("All required drivers are installed", true)
        } else {
            status_info_pill("Click Install on any missing driver to run its official installer")
        };

        let drivers_card = section_card_with_action(
            "System drivers",
            subtle_button("set-recheck-drv", "Recheck").on_click(cx.listener(|this, _, _, cx| {
                this.recheck_drivers();
                this.sync_tray_state();
                this.status_notice = Some("Driver status refreshed".to_owned());
                cx.notify();
            })),
        )
        .flex_1()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(setting_row(
                    "ViGEmBus",
                    "Emulates virtual Xbox 360 and DualShock 4 controllers",
                    driver_row_control(DriverKind::VigemBus, self.vigem_available, "drv-inst-vigem"),
                ))
                .child(setting_row(
                    "HidHide",
                    "Cloaks the physical DualSense to prevent double input",
                    driver_row_control(DriverKind::HidHide, self.hidhide_available, "drv-inst-hidhide"),
                ))
                .child(setting_row(
                    "usbip-win2",
                    "Virtual USB host for DualSense emulation · use Passthrough for strict anti-cheat",
                    driver_row_control(DriverKind::UsbipWin2, self.vds_available, "drv-inst-usbip"),
                ))
                .child(setting_row(
                    "Active hardware",
                    "Controllers currently detected over USB or Bluetooth",
                    div()
                        .h(px(28.0))
                        .px_3()
                        .rounded(px(6.0))
                        .bg(color(WELL_BG))
                        .border_1()
                        .border_color(color(BORDER))
                        .flex()
                        .items_center()
                        .text_size(px(13.0))
                        .font_family(FONT_MONO)
                        .text_color(color(if self.handles.is_empty() {
                            TEXT_MUTED
                        } else {
                            TEXT_SECONDARY
                        }))
                        .child(hardware_summary),
                )),
        )
        .child(
            div()
                .mt_auto()
                .pt_3()
                .border_t_1()
                .border_color(color(BORDER))
                .child(drivers_footer),
        );

        let cfg_path_str = config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "%APPDATA%\\DSMaster\\profiles.toml".to_owned());

        let config_card = section_card("Configuration & data")
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(color(TEXT_MUTED))
                    .child("Profiles and app preferences are saved locally in TOML format"),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(px(8.0))
                    .bg(color(WELL_BG))
                    .border_1()
                    .border_color(color(BORDER))
                    .text_size(px(13.0))
                    .font_family(FONT_MONO)
                    .text_color(color(TEXT_SECONDARY))
                    .child(cfg_path_str),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                subtle_button("set-open-cfg", "Open config folder").on_click(
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
                                subtle_button("set-reload-cfg", "Reload from disk").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.config = load_config();
                                        this.sync_picker_hsv();
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
                            ),
                    )
                    .child(
                        danger_button("set-reset-cfg", "Reset all settings").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.config = AppConfig::default();
                                this.sync_picker_hsv();
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
                            },
                        )),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(22.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(color(TEXT_MUTED))
                            .child(
                                "Application startup, required system drivers, and local configuration",
                            ),
                    ),
            )
            .child(div().flex().gap_4().child(startup_card).child(drivers_card))
            .child(config_card)
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
        if !is_hidden && !self.handles.is_empty() && self.page.shows_controller_stage() {
            window.request_animation_frame();
        }

        let titlebar = self.render_titlebar();
        let icon_rail = self.render_icon_rail(cx);

        let page_content = match self.page {
            ActivePage::QuickSettings => self.render_quick_settings(cx),
            ActivePage::Profiles => self.render_profiles_page(cx),
            ActivePage::Sticks => self.render_sticks_page(cx),
            ActivePage::Triggers => self.render_triggers_page(cx),
            ActivePage::Lighting => self.render_lighting_page(cx),
            ActivePage::MotionRemap => self.render_motion_remap_page(cx),
            ActivePage::Settings => self.render_settings_page(cx),
        };

        let mut canvas_inner = div()
            .id("main-scroll")
            .track_scroll(&self.main_scroll)
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px_6()
            .py_4()
            .gap_4();

        if self.page != ActivePage::Settings {
            canvas_inner = canvas_inner.child(self.render_top_profile_bar(cx));
        }

        if self.page.shows_controller_stage() {
            let compact = self.page != ActivePage::QuickSettings;
            canvas_inner = canvas_inner.child(self.render_controller_stage(compact, cx));
        }

        canvas_inner = canvas_inner.child(page_content);

        // Subtle palette-themed vertical scrollbar overlay (visible only when content overflows viewport)
        let scroll_for_bar = self.main_scroll.clone();
        let scrollbar_overlay = div()
            .absolute()
            .top(px(6.0))
            .bottom(px(6.0))
            .right(px(4.0))
            .w(px(6.0))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let max_y = f32::from(scroll_for_bar.max_offset().height);
                        let view_h = f32::from(scroll_for_bar.bounds().size.height);
                        if max_y <= 1.0 || view_h <= 24.0 {
                            return;
                        }
                        let scroll_y = (-f32::from(scroll_for_bar.offset().y)).clamp(0.0, max_y);
                        let track_x = f32::from(bounds.origin.x);
                        let track_y = f32::from(bounds.origin.y);
                        let track_w = f32::from(bounds.size.width).max(4.0);
                        let track_h = f32::from(bounds.size.height).max(24.0);
                        let total_h = view_h + max_y;
                        let thumb_h = ((view_h / total_h) * track_h).clamp(28.0, track_h);
                        let thumb_y = track_y + (scroll_y / max_y) * (track_h - thumb_h);

                        let paint_capsule = |win: &mut Window, x: f32, y: f32, w: f32, h: f32, col: gpui::Rgba| {
                            let r = (w * 0.5).min(h * 0.5);
                            let mut b = PathBuilder::fill();
                            b.move_to(point(px(x + r), px(y)));
                            b.line_to(point(px(x + w - r), px(y)));
                            b.curve_to(point(px(x + w), px(y + r)), point(px(x + w), px(y)));
                            b.line_to(point(px(x + w), px(y + h - r)));
                            b.curve_to(point(px(x + w - r), px(y + h)), point(px(x + w), px(y + h)));
                            b.line_to(point(px(x + r), px(y + h)));
                            b.curve_to(point(px(x), px(y + h - r)), point(px(x), px(y + h)));
                            b.line_to(point(px(x), px(y + r)));
                            b.curve_to(point(px(x + r), px(y)), point(px(x), px(y)));
                            b.close();
                            if let Ok(path) = b.build() {
                                win.paint_path(path, col);
                            }
                        };

                        paint_capsule(window, track_x, track_y, track_w, track_h, color_alpha(WELL_BG, 0x99));
                        paint_capsule(window, track_x, thumb_y, track_w, thumb_h, color(BORDER_STRONG));
                    },
                )
                .size_full(),
            );

        let mut workspace_row = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(icon_rail);

        if self.drawer_open && self.page != ActivePage::Settings {
            workspace_row = workspace_row.child(self.render_config_drawer(cx));
        }

        workspace_row = workspace_row.child(
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .bg(color(CANVAS_BG))
                .child(canvas_inner)
                .child(scrollbar_overlay),
        );

        div()
            .flex()
            .flex_col()
            .size_full()
            .font_family(FONT_SANS)
            .bg(color(CANVAS_BG))
            .text_color(color(TEXT))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some((slider, origin_x, width)) = this.active_slider {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        let t = ((f32::from(ev.position.x) - origin_x) / width).clamp(0.0, 1.0);
                        this.apply_slider_live(slider, t, cx);
                    } else {
                        this.finish_slider_drag(cx);
                    }
                } else if let Some((ox, oy, w, h)) = this.active_sv_pad {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        let sat = ((f32::from(ev.position.x) - ox) / w).clamp(0.0, 1.0);
                        let val = (1.0 - (f32::from(ev.position.y) - oy) / h).clamp(0.0, 1.0);
                        this.apply_sv_live(sat, val, cx);
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
            .child(titlebar)
            .child(workspace_row)
    }
}
