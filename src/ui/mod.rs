mod controller_view;
mod dualsense_paths;
mod icons;
pub mod theme;
mod widgets;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const NOTICE_TTL: Duration = Duration::from_secs(4);
const NOTICE_ERROR_TTL: Duration = Duration::from_secs(7);

use gpui::{
    Bounds, ClipboardItem, Context, Div, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent,
    PathBuilder, Pixels, ScrollHandle, SharedString, Task, Window, WindowControlArea, canvas, div,
    point, prelude::*, px, relative,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOW, SetForegroundWindow, ShowWindow};

use self::controller_view::{ControllerPreviewState, HighlightZone, virtual_dualsense};
use self::icons::{
    bolt_icon, check_icon, gamepad_icon, gyro_icon, info_icon, layers_icon, link_icon, plus_icon,
    settings_icon, sidebar_toggle_icon, sliders_icon, speaker_icon, stick_icon, sun_icon,
    swap_icon, trash_icon, trigger_icon, win_close_icon, win_max_icon, win_min_icon,
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
    hue_slider_bar, icon_rail_item, info_kv_tile, meter_row,
    meter_row_with_width, pill_button, primary_cta_button, response_curve_chart, rgb_channel_bar,
    section_card, section_card_with_action, segmented_button, setting_row, slider_bar,
    status_info_pill, status_inset_pill, subtle_button, sv_color_pad, text_tooltip, toggle_switch,
    trigger_travel_gauge,
};
use crate::cloak::HidHideManager;
use crate::drivers::{DriverInstallJob, DriverInstallPhase, DriverKind};
use crate::emu::EmulationMode;
use crate::emu::dualsense::is_usbip_win2_installed;
use crate::emu::vigem::is_vigem_installed;
use crate::engine::keyboard::{poll_recorded_hotkey, send_combo_tap};
use crate::engine::profile::{
    ButtonAction, CustomTriggerMode, DeadzoneShape, GyroAimMode, GyroOutputMode, LightbarMode,
    MAX_MACRO_STEPS, MOD_ALT, MOD_CTRL, MOD_SHIFT, MacroStep, MediaKey, MicMuteLedOverride,
    RemappableButton, ResponseCurve, StickTuning, TriggerPreset, format_key_combo,
};
use crate::engine::profiles_store::{
    AppConfig, NamedProfile, config_path, load_config, open_config_dir, save_config,
};
use crate::engine::session::is_low_battery_alert;
use crate::engine::{Engine, SeqLock, TelemetrySnapshot};
use crate::protocol::{BatteryStatus, BtAudioRoute, BtMicRoute, LedBrightness, Transport};
use crate::startup::{is_run_at_startup_enabled, set_run_at_startup};
use crate::tray::{SystemTray, TrayAction, TrayState};
use crate::updater::{
    CURRENT_VERSION, ReleaseInfo, UpdateCheckJob, UpdateCheckOutcome, UpdateInstallJob,
    UpdateInstallPhase, apply_and_relaunch, open_release_url,
};

fn override_toggle_with_label(enabled: bool, switch: impl IntoElement) -> Div {
    let mut row = div().flex().items_center().gap_2();
    if !enabled {
        row = row.child(
            div()
                .text_xs()
                .text_color(color(TEXT_MUTED))
                .whitespace_nowrap()
                .child("Passthrough"),
        );
    }
    row.child(switch)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivePage {
    QuickSettings,
    Profiles,
    Sticks,
    Triggers,
    Audio,
    Lighting,
    MotionRemap,
    DeviceInfo,
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
                | Self::Audio
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
    ("Cyan", [0, 212, 255]),
    ("Emerald", [0, 224, 48]),
    ("Gold", [255, 196, 0]),
    ("Crimson", [255, 0, 0]),
    ("Violet", [148, 0, 255]),
];

const PLAYER_LED_OPTIONS: [(&str, u8); 5] = [
    ("Off", 0x00),
    ("P1", 0x04),
    ("P2", 0x0A),
    ("P3", 0x15),
    ("P4", 0x1B),
];

const MAX_STICK_DZ: f32 = 0.35;
const MAX_OUTER_DZ: f32 = 0.40;
const MAX_TRIGGER_DZ_PCT: f32 = 35.0;
const MAX_GYRO_DZ: f32 = 8.0;

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
    LeftOuter,
    RightOuter,
    RightTriggerDz,
    RumbleStrength,
    LightbarBrightness,
    GyroDeadzone,
    CustomLeftStart,
    CustomLeftEnd,
    CustomLeftForce,
    CustomLeftFreq,
    CustomRightStart,
    CustomRightEnd,
    CustomRightForce,
    CustomRightFreq,
    BtSpeakerVolume,
    BtHapticsGain,
    BtMicGain,
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
            Self::LeftOuter => 9,
            Self::RightOuter => 10,
            Self::RightTriggerDz => 11,
            Self::RumbleStrength => 12,
            Self::LightbarBrightness => 13,
            Self::GyroDeadzone => 14,
            Self::CustomLeftStart => 15,
            Self::CustomLeftEnd => 16,
            Self::CustomLeftForce => 17,
            Self::CustomLeftFreq => 18,
            Self::CustomRightStart => 19,
            Self::CustomRightEnd => 20,
            Self::CustomRightForce => 21,
            Self::CustomRightFreq => 22,
            Self::BtSpeakerVolume => 23,
            Self::BtHapticsGain => 24,
            Self::BtMicGain => 25,
        }
    }
}

const HAIR_TRIGGER_STEPS: [(&str, u8); 4] =
    [("Full", 255), ("75%", 190), ("50%", 128), ("25%", 64)];

const SENSITIVITY_STEPS: [(&str, f32); 4] =
    [("0.5x", 0.5), ("1.0x", 1.0), ("1.5x", 1.5), ("2.0x", 2.0)];

fn trigger_preset_desc(preset: TriggerPreset) -> &'static str {
    match preset {
        TriggerPreset::Off => "No motorized resistance",
        TriggerPreset::Soft => "Light continuous spring tension",
        TriggerPreset::Rigid => "Firm continuous spring tension",
        TriggerPreset::Weapon => "Crisp snap-break wall halfway down",
        TriggerPreset::Bow => "Progressive draw tension with string snap-back",
        TriggerPreset::Slope => "Smoothly ramping resistance across travel",
        TriggerPreset::Pulse => "Rapid rhythmic vibration feedback",
        TriggerPreset::Machine => "Dual-amplitude automatic weapon cycling",
        TriggerPreset::Galloping => "Rhythmic dual-pulse galloping cadence",
        TriggerPreset::Custom => "Custom adaptive trigger parameters",
    }
}

fn fmt_hms(secs: u32) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyRecordTarget {
    SingleKey(RemappableButton),
    MacroStep(RemappableButton),
}

#[derive(Debug, Clone, Copy)]
struct KnownSessionInfo {
    is_edge: bool,
    transport: Transport,
}

pub struct DsMasterView {
    engine: Option<Engine>,
    engine_error: Option<String>,
    status_notice: Option<String>,
    status_notice_expires_at: Option<Instant>,
    vigem_available: bool,
    hidhide_available: bool,
    vds_available: bool,
    driver_install: Option<DriverInstallJob>,
    last_install_phase: Option<DriverInstallPhase>,
    update_check: Option<UpdateCheckJob>,
    update_check_manual: bool,
    available_update: Option<ReleaseInfo>,
    update_banner_dismissed: bool,
    update_install: Option<UpdateInstallJob>,
    last_update_phase: Option<UpdateInstallPhase>,
    update_status_text: Option<String>,
    config: AppConfig,
    page: ActivePage,
    drawer_open: bool,
    show_raw_sticks: bool,
    selected_remap_src: Option<RemappableButton>,
    recording_hotkey: Option<HotkeyRecordTarget>,
    macro_step_delay_ms: u16,
    last_topology_version: u64,
    handles: Vec<Arc<SeqLock<TelemetrySnapshot>>>,
    selected_idx: usize,
    snapshots: Vec<TelemetrySnapshot>,
    known_sessions: HashMap<u64, KnownSessionInfo>,
    low_battery_notified: HashSet<u64>,
    last_tray_battery: Option<(u8, bool)>,
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
    _startup_update_task: Task<()>,
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
            status_notice_expires_at: None,
            vigem_available,
            hidhide_available,
            vds_available,
            driver_install: None,
            last_install_phase: None,
            update_check: None,
            update_check_manual: false,
            available_update: None,
            update_banner_dismissed: false,
            update_install: None,
            last_update_phase: None,
            update_status_text: None,
            config,
            page: ActivePage::QuickSettings,
            drawer_open: true,
            show_raw_sticks: false,
            selected_remap_src: None,
            recording_hotkey: None,
            macro_step_delay_ms: 50,
            last_topology_version: 0,
            handles: Vec::new(),
            selected_idx: 0,
            snapshots: Vec::new(),
            known_sessions: HashMap::new(),
            low_battery_notified: HashSet::new(),
            last_tray_battery: None,
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
            _startup_update_task: Task::ready(()),
        };
        this.sync_from_engine();
        this.read_snapshots();
        this.seed_known_sessions();
        this.sync_tray_state();

        this._watch_task = cx.spawn(async move |weak_this, cx| {
            loop {
                let fast = weak_this
                    .read_with(cx, |view, _| view.recording_hotkey.is_some())
                    .unwrap_or(false);
                cx.background_executor()
                    .timer(Duration::from_millis(if fast { 30 } else { 250 }))
                    .await;
                let res = weak_this.update(cx, |view, cx| {
                    let hotkey_changed = view.poll_hotkey_recording(cx);
                    let eng_changed = view.sync_from_engine();
                    let install_changed = view.poll_driver_install();
                    let update_check_changed = view.poll_update_check();
                    let update_install_changed = view.poll_update_install(cx);
                    let notice_expired = view.expire_status_notice();
                    if eng_changed || !view.handles.is_empty() {
                        view.read_snapshots();
                    }
                    view.check_notifications();
                    let cur_tray_batt = view.current_tray_battery();
                    if eng_changed || cur_tray_batt != view.last_tray_battery {
                        view.last_tray_battery = cur_tray_batt;
                        view.sync_tray_state();
                    }
                    let integrations_live = view.page == ActivePage::Settings
                        && (view.config.dsu_enabled || view.config.dsx_udp_enabled);
                    if hotkey_changed
                        || eng_changed
                        || install_changed
                        || update_check_changed
                        || update_install_changed
                        || notice_expired
                        || integrations_live
                    {
                        cx.notify();
                    }
                });
                if res.is_err() {
                    break;
                }
            }
        });

        if this.config.auto_check_updates {
            this._startup_update_task = cx.spawn(async move |weak_this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(3))
                    .await;
                let _ = weak_this.update(cx, |view, cx| {
                    if view.config.auto_check_updates {
                        view.start_update_check(false, cx);
                    }
                });
            });
        }

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
            self.sync_picker_hsv();
        }
        true
    }

    fn read_snapshots(&mut self) {
        self.snapshots.clear();
        for handle in &self.handles {
            self.snapshots.push(handle.read());
        }
    }

    fn poll_hotkey_recording(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(target) = self.recording_hotkey else {
            return false;
        };
        let Some(res) = poll_recorded_hotkey() else {
            return false;
        };
        self.recording_hotkey = None;
        match res {
            Err(()) => {
                self.set_status_notice("Hotkey recording cancelled", Some(NOTICE_TTL));
                true
            }
            Ok((vk, modifiers)) => {
                let label = format_key_combo(vk, modifiers);
                match target {
                    HotkeyRecordTarget::SingleKey(src) => {
                        let hold = matches!(
                            self.config.active_settings().action_map.get(src),
                            ButtonAction::Key { hold: true, .. }
                        );
                        self.config.active_settings_mut().action_map.set(
                            src,
                            ButtonAction::Key {
                                vk,
                                modifiers,
                                hold,
                            },
                        );
                        self.commit_config(cx);
                        self.set_status_notice(
                            format!("Bound {} → {label}", src.label()),
                            Some(NOTICE_TTL),
                        );
                    }
                    HotkeyRecordTarget::MacroStep(src) => {
                        let cur = self.config.active_settings().action_map.get(src);
                        let (mut steps, mut len) = match cur {
                            ButtonAction::Macro { steps, len } => (steps, len as usize),
                            _ => ([MacroStep::default(); MAX_MACRO_STEPS], 0),
                        };
                        if len < MAX_MACRO_STEPS {
                            steps[len] = MacroStep {
                                vk,
                                modifiers,
                                delay_ms: self.macro_step_delay_ms,
                            };
                            len += 1;
                            self.config
                                .active_settings_mut()
                                .action_map
                                .set(src, ButtonAction::Macro { steps, len: len as u8 });
                            self.commit_config(cx);
                            self.set_status_notice(
                                format!("Added macro step {len}: {label} to {}", src.label()),
                                Some(NOTICE_TTL),
                            );
                        } else {
                            self.set_status_notice(
                                "Macro is full (max 8 steps)",
                                Some(NOTICE_ERROR_TTL),
                            );
                        }
                    }
                }
                true
            }
        }
    }

    fn seed_known_sessions(&mut self) {
        self.known_sessions.clear();
        for snap in &self.snapshots {
            if snap.session_id != 0 {
                self.known_sessions.insert(
                    snap.session_id,
                    KnownSessionInfo {
                        is_edge: snap.is_edge,
                        transport: snap.transport,
                    },
                );
            }
        }
    }

    fn check_notifications(&mut self) {
        let mut current: HashMap<u64, &TelemetrySnapshot> = HashMap::new();
        for snap in &self.snapshots {
            if snap.session_id != 0 {
                current.insert(snap.session_id, snap);
            }
        }

        let notify_all = self.config.notifications_enabled;
        let notify_conn = notify_all && self.config.notify_connect_disconnect;
        let notify_low = notify_all && self.config.notify_low_battery;

        // Detect newly connected controllers.
        for (&sid, snap) in &current {
            if !self.known_sessions.contains_key(&sid) {
                self.known_sessions.insert(
                    sid,
                    KnownSessionInfo {
                        is_edge: snap.is_edge,
                        transport: snap.transport,
                    },
                );
                if notify_conn {
                    if let Some(tray) = &self.tray {
                        let model = if snap.is_edge {
                            "DualSense Edge"
                        } else {
                            "DualSense"
                        };
                        let tr = match snap.transport {
                            Transport::Usb => "USB",
                            Transport::Bluetooth => "Bluetooth",
                        };
                        let body = if snap.battery_status != BatteryStatus::Unknown {
                            format!("{tr} · Battery {}%", snap.battery_percent)
                        } else {
                            tr.to_owned()
                        };
                        tray.show_notification(format!("{model} Connected"), body, false);
                    }
                }
            }
        }

        // Detect disconnected controllers.
        let removed: Vec<(u64, KnownSessionInfo)> = self
            .known_sessions
            .iter()
            .filter(|(sid, _)| !current.contains_key(sid))
            .map(|(&sid, &info)| (sid, info))
            .collect();
        for (sid, info) in removed {
            self.known_sessions.remove(&sid);
            self.low_battery_notified.remove(&sid);
            if notify_conn {
                if let Some(tray) = &self.tray {
                    let model = if info.is_edge {
                        "DualSense Edge"
                    } else {
                        "DualSense"
                    };
                    let tr = match info.transport {
                        Transport::Usb => "USB",
                        Transport::Bluetooth => "Bluetooth",
                    };
                    tray.show_notification(format!("{model} Disconnected"), tr, false);
                }
            }
        }

        // Detect low battery transitions with 5% hysteresis.
        let threshold = self
            .config
            .active_settings()
            .low_battery_threshold
            .clamp(5, 50);
        for (&sid, snap) in &current {
            let is_low = snap.battery_status == BatteryStatus::Discharging
                && snap.battery_percent > 0
                && snap.battery_percent <= threshold;
            let recovered = snap.battery_status != BatteryStatus::Discharging
                || snap.battery_percent > threshold.saturating_add(5);
            if recovered {
                self.low_battery_notified.remove(&sid);
            } else if is_low && !self.low_battery_notified.contains(&sid) {
                self.low_battery_notified.insert(sid);
                if notify_low {
                    if let Some(tray) = &self.tray {
                        let model = if snap.is_edge {
                            "DualSense Edge"
                        } else {
                            "DualSense"
                        };
                        tray.show_notification(
                            format!("{model} Low Battery ({}%)", snap.battery_percent),
                            "Connect USB to charge your controller",
                            true,
                        );
                    }
                }
            }
        }
    }

    fn set_status_notice(&mut self, msg: impl Into<String>, ttl: Option<Duration>) {
        self.status_notice = Some(msg.into());
        self.status_notice_expires_at = ttl.map(|d| Instant::now() + d);
    }

    fn clear_status_notice(&mut self) {
        self.status_notice = None;
        self.status_notice_expires_at = None;
    }

    fn expire_status_notice(&mut self) -> bool {
        if self
            .status_notice_expires_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.clear_status_notice();
            return true;
        }
        false
    }

    fn commit_config(&mut self, cx: &mut Context<Self>) {
        self.clear_status_notice();
        if let Some(engine) = &self.engine {
            engine.apply_config(&self.config);
        }
        if let Err(err) = save_config(&self.config) {
            self.set_status_notice(format!("Failed to save config: {err}"), Some(NOTICE_ERROR_TTL));
        }
        self.sync_tray_state();
        cx.notify();
    }

    fn set_emulation_mode(&mut self, mode: EmulationMode, cx: &mut Context<Self>) {
        self.config.active_settings_mut().emulation = mode;
        self.commit_config(cx);
        match mode {
            EmulationMode::DualSense if !self.vds_available => {
                self.set_status_notice(
                    "usbip-win2 not detected — install vadimgrn/usbip-win2 for Virtual DualSense",
                    Some(NOTICE_ERROR_TTL),
                );
                cx.notify();
            }
            EmulationMode::Xbox360 | EmulationMode::DualShock4 if !self.vigem_available => {
                self.set_status_notice(
                    "ViGEmBus not detected — install Nefarius ViGEmBus",
                    Some(NOTICE_ERROR_TTL),
                );
                cx.notify();
            }
            _ => {}
        }
    }

    fn toggle_cloak(&mut self, cx: &mut Context<Self>) {
        if !self.hidhide_available {
            self.set_status_notice("HidHide driver is not installed", Some(NOTICE_ERROR_TTL));
            cx.notify();
            return;
        }
        let next = !self.config.cloak_physical;
        if let Some(engine) = &self.engine {
            match engine.set_cloak_enabled(next) {
                Ok(()) => {
                    self.config.cloak_physical = next;
                    self.clear_status_notice();
                    let _ = save_config(&self.config);
                }
                Err(e) => {
                    self.set_status_notice(format!("HidHide error: {e}"), Some(NOTICE_ERROR_TTL));
                }
            }
        }
        self.sync_tray_state();
        cx.notify();
    }

    fn current_tray_battery(&self) -> Option<(u8, bool)> {
        let snap = self.selected_snapshot().or_else(|| self.snapshots.first())?;
        if snap.battery_status == BatteryStatus::Unknown {
            return None;
        }
        Some((
            snap.battery_percent,
            snap.battery_status == BatteryStatus::Charging,
        ))
    }

    fn build_tray_state(&self) -> TrayState {
        let cloak_enabled = self
            .engine
            .as_ref()
            .map(Engine::is_cloak_enabled)
            .unwrap_or(self.config.cloak_physical);
        let (battery_percent, battery_charging) = match self.current_tray_battery() {
            Some((pct, chg)) => (Some(pct), chg),
            None => (None, false),
        };
        TrayState {
            window_visible: !self.window_ctl.window_hidden.load(Ordering::Acquire),
            emulation: self.config.active_settings().emulation,
            cloak_enabled,
            hidhide_available: self.hidhide_available,
            connected_count: self.handles.len(),
            battery_percent,
            battery_charging,
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
        self.set_status_notice(format!("Downloading {} installer...", kind.display_name()), None);
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
                self.set_status_notice(format!("Downloading {name} installer..."), None);
            }
            DriverInstallPhase::RunningInstaller => {
                self.last_install_phase = Some(DriverInstallPhase::RunningInstaller);
                self.set_status_notice(format!("Running {name} installer..."), None);
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
                let (msg, ttl) = match res {
                    Ok(()) if now_installed => {
                        if kind == DriverKind::VigemBus && !was_vigem {
                            (
                                format!(
                                    "{name} installed — restart DSMaster to enable Xbox 360 / DS4"
                                ),
                                NOTICE_ERROR_TTL,
                            )
                        } else {
                            (format!("{name} installed"), NOTICE_TTL)
                        }
                    }
                    Ok(()) => (
                        format!(
                            "{name} installer closed — click Recheck or reboot if prompted"
                        ),
                        NOTICE_ERROR_TTL,
                    ),
                    Err(err) => (format!("{name} install failed: {err}"), NOTICE_ERROR_TTL),
                };
                self.set_status_notice(msg, Some(ttl));
                self.sync_tray_state();
            }
        }
        true
    }

    fn start_update_check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.update_check.is_some() || self.update_install.is_some() {
            return;
        }
        self.update_check = Some(UpdateCheckJob::spawn());
        self.update_check_manual = manual;
        if manual {
            self.update_status_text = Some("Checking GitHub Releases…".to_owned());
        }
        cx.notify();
    }

    fn poll_update_check(&mut self) -> bool {
        let Some(job) = self.update_check.as_ref() else {
            return false;
        };
        let Some(outcome) = job.take_result() else {
            return false;
        };
        self.update_check = None;
        let manual = self.update_check_manual;
        self.update_check_manual = false;

        match outcome {
            UpdateCheckOutcome::Available(rel) => {
                let is_new_ver = self
                    .available_update
                    .as_ref()
                    .map(|prev| prev.version.as_str())
                    != Some(rel.version.as_str());
                if is_new_ver {
                    self.update_banner_dismissed = false;
                }
                let ver = rel.version.clone();
                self.available_update = Some(rel);
                self.update_status_text = Some(format!("DSMaster v{ver} is available"));
                if manual {
                    self.set_status_notice(format!("Update available: v{ver}"), Some(NOTICE_TTL));
                }
            }
            UpdateCheckOutcome::UpToDate => {
                self.available_update = None;
                self.update_status_text = Some("Up to date".to_owned());
                if manual {
                    self.set_status_notice(
                        format!("DSMaster v{CURRENT_VERSION} is up to date"),
                        Some(NOTICE_TTL),
                    );
                }
            }
            UpdateCheckOutcome::Failed(err) => {
                if manual {
                    self.update_status_text = Some(err.clone());
                    self.set_status_notice(err, Some(NOTICE_ERROR_TTL));
                }
            }
        }
        true
    }

    fn start_update_install(&mut self, cx: &mut Context<Self>) {
        if self.update_install.is_some() {
            return;
        }
        let Some(release) = self.available_update.clone() else {
            return;
        };
        let ver = release.version.clone();
        self.update_install = Some(UpdateInstallJob::spawn(release));
        self.last_update_phase = Some(UpdateInstallPhase::Downloading);
        self.update_status_text = Some(format!("Downloading DSMaster v{ver}…"));
        self.set_status_notice(format!("Downloading DSMaster v{ver}..."), None);
        cx.notify();
    }

    fn poll_update_install(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(job) = self.update_install.as_ref() else {
            return false;
        };
        let phase = job.phase();
        if self.last_update_phase.as_ref() == Some(&phase) {
            return false;
        }
        let ver = job.release.version.clone();

        match phase {
            UpdateInstallPhase::Downloading => {
                self.last_update_phase = Some(UpdateInstallPhase::Downloading);
                self.update_status_text = Some(format!("Downloading DSMaster v{ver}…"));
                self.set_status_notice(format!("Downloading DSMaster v{ver}..."), None);
            }
            UpdateInstallPhase::Verifying => {
                self.last_update_phase = Some(UpdateInstallPhase::Verifying);
                self.update_status_text = Some(format!("Verifying SHA-256 checksum for v{ver}…"));
                self.set_status_notice(format!("Verifying DSMaster v{ver} checksum..."), None);
            }
            UpdateInstallPhase::Extracting => {
                self.last_update_phase = Some(UpdateInstallPhase::Extracting);
                self.update_status_text = Some(format!("Extracting DSMaster v{ver}…"));
                self.set_status_notice(format!("Extracting DSMaster v{ver}..."), None);
            }
            UpdateInstallPhase::ReadyToRestart(prepared) => {
                self.update_install = None;
                self.last_update_phase = None;
                let mut did_shutdown = false;
                match apply_and_relaunch(&prepared, || {
                    did_shutdown = true;
                    self.shutdown_all();
                }) {
                    Ok(()) => {
                        self.window_ctl.force_quit.store(true, Ordering::Release);
                        if !did_shutdown {
                            self.shutdown_all();
                        }
                        cx.quit();
                    }
                    Err(err) => {
                        self.update_status_text = Some(err.clone());
                        self.set_status_notice(
                            format!("Update failed: {err}"),
                            Some(NOTICE_ERROR_TTL),
                        );
                    }
                }
            }
            UpdateInstallPhase::Failed(err) => {
                self.update_install = None;
                self.last_update_phase = None;
                self.update_status_text = Some(err.clone());
                self.set_status_notice(format!("Update failed: {err}"), Some(NOTICE_ERROR_TTL));
            }
        }
        true
    }

    fn update_install_label(&self) -> Option<&'static str> {
        self.update_install.as_ref().map(|job| match job.phase() {
            UpdateInstallPhase::Downloading => "Downloading…",
            UpdateInstallPhase::Verifying => "Verifying…",
            UpdateInstallPhase::Extracting => "Extracting…",
            UpdateInstallPhase::ReadyToRestart(_) => "Restarting…",
            UpdateInstallPhase::Failed(_) => "Failed",
        })
    }

    fn render_update_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.page == ActivePage::Settings || self.update_banner_dismissed {
            return None;
        }
        let release = self.available_update.as_ref()?;
        let ver = release.version.clone();
        let release_url = release.release_url.clone();
        let active_phase = self.update_install_label();

        let action_btn = if let Some(label) = active_phase {
            div()
                .h(px(32.0))
                .px_3()
                .rounded(px(8.0))
                .bg(color(SURFACE_ALT))
                .border_1()
                .border_color(color(BORDER_STRONG))
                .flex()
                .items_center()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(ACCENT))
                .whitespace_nowrap()
                .child(label)
                .into_any_element()
        } else {
            div()
                .id("upd-banner-install")
                .h(px(32.0))
                .px_3()
                .rounded(px(8.0))
                .bg(color(ACCENT_SOLID))
                .border_1()
                .border_color(color(ACCENT_SOLID))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(ACCENT_FG))
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|s| {
                    s.bg(color(ACCENT_SOLID_HOVER))
                        .border_color(color(ACCENT_SOLID_HOVER))
                })
                .child("Update & restart")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.start_update_install(cx);
                }))
                .into_any_element()
        };

        let mut actions = div()
            .flex()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .child(
                subtle_button("upd-banner-notes", "Release notes").on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let Err(err) = open_release_url(&release_url) {
                            this.set_status_notice(err, Some(NOTICE_ERROR_TTL));
                            cx.notify();
                        }
                    },
                )),
            )
            .child(action_btn);

        if active_phase.is_none() {
            actions = actions.child(
                subtle_button("upd-banner-later", "Later").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.update_banner_dismissed = true;
                        cx.notify();
                    },
                )),
            );
        }

        Some(
            div()
                .px_4()
                .py_3()
                .rounded(px(10.0))
                .bg(color(SURFACE))
                .border_1()
                .border_color(color(BORDER_STRONG))
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .size(px(8.0))
                                .rounded_full()
                                .bg(color(ACCENT))
                                .flex_shrink_0(),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(color(TEXT))
                                        .child(format!("DSMaster v{ver} is available")),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(color(TEXT_MUTED))
                                        .child(format!(
                                            "Installed: v{CURRENT_VERSION} · Verified via GitHub Releases SHA-256"
                                        )),
                                ),
                        ),
                )
                .child(actions),
        )
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
        self.active_sv_pad = None;
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
        self.active_slider = None;
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
        st.set_custom_lightbar_rgb(hsv_to_rgb(h, sat, val));
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
            | ActiveSlider::LeftOuter
            | ActiveSlider::RightOuter
            | ActiveSlider::Trigger
            | ActiveSlider::RightTriggerDz => {
                let stick_val =
                    ((t * MAX_STICK_DZ * 100.0).round() / 100.0).clamp(0.0, MAX_STICK_DZ);
                let outer_val =
                    ((t * MAX_OUTER_DZ * 100.0).round() / 100.0).clamp(0.0, MAX_OUTER_DZ);
                let trig_pct = (t * MAX_TRIGGER_DZ_PCT)
                    .round()
                    .clamp(0.0, MAX_TRIGGER_DZ_PCT);
                let trig_u8 = (trig_pct * 255.0 / 100.0).round() as u8;

                let s = self.config.active_settings_mut();
                match slider {
                    ActiveSlider::LeftInner => s.left_stick.deadzone = stick_val,
                    ActiveSlider::LeftAnti => s.left_stick.anti_deadzone = stick_val,
                    ActiveSlider::LeftOuter => s.left_stick.outer_deadzone = outer_val,
                    ActiveSlider::RightInner => s.right_stick.deadzone = stick_val,
                    ActiveSlider::RightAnti => s.right_stick.anti_deadzone = stick_val,
                    ActiveSlider::RightOuter => s.right_stick.outer_deadzone = outer_val,
                    ActiveSlider::Trigger => s.triggers.deadzone = trig_u8,
                    ActiveSlider::RightTriggerDz => s.right_trigger_tuning.deadzone = trig_u8,
                    _ => {}
                }
            }
            ActiveSlider::RumbleStrength => {
                let val = ((t * 20.0).round() / 20.0).clamp(0.0, 1.0);
                self.config.active_settings_mut().rumble_strength = val;
            }
            ActiveSlider::BtSpeakerVolume => {
                let val = ((t * 20.0).round() / 20.0).clamp(0.0, 1.0);
                self.config.active_settings_mut().bt_speaker_volume = val;
            }
            ActiveSlider::BtHapticsGain => {
                let val = ((t * 20.0).round() / 20.0).clamp(0.0, 1.0);
                self.config.active_settings_mut().bt_haptics_gain = val;
            }
            ActiveSlider::BtMicGain => {
                let val = ((t * 20.0).round() / 10.0).clamp(0.0, 2.0);
                self.config.active_settings_mut().bt_mic_gain = val;
            }
            ActiveSlider::LightbarBrightness => {
                let val = ((t * 100.0).round() / 100.0).clamp(0.0, 1.0);
                let s = self.config.active_settings_mut();
                s.override_lightbar = true;
                s.lightbar_brightness = val;
            }
            ActiveSlider::GyroDeadzone => {
                let val = ((t * MAX_GYRO_DZ * 2.0).round() / 2.0).clamp(0.0, MAX_GYRO_DZ);
                self.config.active_settings_mut().gyro_deadzone = val;
            }
            ActiveSlider::CustomLeftStart
            | ActiveSlider::CustomLeftEnd
            | ActiveSlider::CustomLeftForce
            | ActiveSlider::CustomLeftFreq
            | ActiveSlider::CustomRightStart
            | ActiveSlider::CustomRightEnd
            | ActiveSlider::CustomRightForce
            | ActiveSlider::CustomRightFreq => {
                let s = self.config.active_settings_mut();
                s.override_triggers = true;
                let (cfg, is_start, is_end, is_force, is_freq) = match slider {
                    ActiveSlider::CustomLeftStart => (&mut s.left_custom_trigger, true, false, false, false),
                    ActiveSlider::CustomLeftEnd => (&mut s.left_custom_trigger, false, true, false, false),
                    ActiveSlider::CustomLeftForce => (&mut s.left_custom_trigger, false, false, true, false),
                    ActiveSlider::CustomLeftFreq => (&mut s.left_custom_trigger, false, false, false, true),
                    ActiveSlider::CustomRightStart => (&mut s.right_custom_trigger, true, false, false, false),
                    ActiveSlider::CustomRightEnd => (&mut s.right_custom_trigger, false, true, false, false),
                    ActiveSlider::CustomRightForce => (&mut s.right_custom_trigger, false, false, true, false),
                    _ => (&mut s.right_custom_trigger, false, false, false, true),
                };
                if is_start {
                    cfg.start_position = (t * 8.0).round().clamp(0.0, 8.0) as u8;
                    if cfg.end_position <= cfg.start_position {
                        cfg.end_position = (cfg.start_position + 1).min(9);
                    }
                } else if is_end {
                    cfg.end_position = (1.0 + t * 8.0).round().clamp(1.0, 9.0) as u8;
                    if cfg.end_position <= cfg.start_position {
                        cfg.start_position = cfg.end_position.saturating_sub(1);
                    }
                } else if is_force {
                    cfg.strength = (1.0 + t * 7.0).round().clamp(1.0, 8.0) as u8;
                } else if is_freq {
                    cfg.frequency = (1.0 + t * 99.0).round().clamp(1.0, 100.0) as u8;
                }
            }
            ActiveSlider::HueBar => {
                let h = (t * 360.0).clamp(0.0, 359.99);
                let (sat, val) = if self.page == ActivePage::Lighting {
                    let s = if self.picker_hsv.1 < 0.20 {
                        1.0
                    } else {
                        self.picker_hsv.1
                    };
                    let v = if self.picker_hsv.2 < 0.15 {
                        1.0
                    } else {
                        self.picker_hsv.2
                    };
                    (s, v)
                } else {
                    (1.0, 1.0)
                };
                self.picker_hsv = (h, sat, val);
                let s = self.config.active_settings_mut();
                s.set_custom_lightbar_rgb(hsv_to_rgb(h, sat, val));
            }
            ActiveSlider::RedChannel | ActiveSlider::GreenChannel | ActiveSlider::BlueChannel => {
                let ch = (t * 255.0).round().clamp(0.0, 255.0) as u8;
                let s = self.config.active_settings_mut();
                let mut rgb = s.lightbar_rgb;
                match slider {
                    ActiveSlider::RedChannel => rgb[0] = ch,
                    ActiveSlider::GreenChannel => rgb[1] = ch,
                    ActiveSlider::BlueChannel => rgb[2] = ch,
                    _ => {}
                }
                s.set_custom_lightbar_rgb(rgb);
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
        self.render_slider_row_range(title, value_label, ratio, "0%", "35%", fill_color, slider, cx)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_slider_row_range(
        &self,
        title: &'static str,
        value_label: String,
        ratio: f32,
        min_label: &'static str,
        max_label: &'static str,
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
                                    .child(min_label)
                                    .child(max_label),
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
        let low_batt_msg = self.selected_snapshot().and_then(|s| {
            if !self.handles.is_empty()
                && is_low_battery_alert(&self.config.active_settings(), s.battery_percent, s.battery_status)
            {
                Some(format!("Low battery: {}%", s.battery_percent))
            } else {
                None
            }
        });
        let notice = self
            .engine_error
            .as_ref()
            .map(|e| (e.clone(), DANGER, DANGER_SURFACE, DANGER_BORDER))
            .or_else(|| {
                self.status_notice
                    .as_ref()
                    .map(|n| (n.clone(), WARN, WARN_SURFACE, WARN_BORDER))
            })
            .or_else(|| low_batt_msg.map(|m| (m, WARN, WARN_SURFACE, WARN_BORDER)));

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
                        .child(win_close_icon(TEXT_SECONDARY)),
                ),
        )
    }

    /// Far-left 72px Icon Rail (`#101214`, quiet neutral selection, icons match text role)
    fn render_icon_rail(&self, cx: &mut Context<Self>) -> Div {
        let on_ctrl = self.page.is_controller_section();
        let on_prof = self.page == ActivePage::Profiles;
        let on_set = self.page == ActivePage::Settings;

        div()
            .w(px(78.0))
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
                            "Controller",
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
                    .child({
                        let mut item = div().relative().child(
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
                        );
                        if self.available_update.is_some() {
                            item = item.child(
                                div()
                                    .absolute()
                                    .top(px(8.0))
                                    .right(px(10.0))
                                    .size(px(7.0))
                                    .rounded_full()
                                    .bg(color(ACCENT)),
                            );
                        }
                        item
                    }),
            )
    }

    /// Secondary 264px Configuration Drawer (`#1C1F23`, 24px/8px group rhythm, quiet text meta)
    fn render_config_drawer(&self, cx: &mut Context<Self>) -> Div {
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
        let is_charging = snap.is_some_and(|s| s.battery_status == BatteryStatus::Charging);
        let device_sub = match snap {
            Some(s) => {
                let emu_lbl = match s.emulation {
                    EmulationMode::None => "Passthrough",
                    EmulationMode::Xbox360 => "Xbox 360",
                    EmulationMode::DualShock4 => "DualShock 4",
                    EmulationMode::DualSense => "DualSense",
                };
                format!("{}% battery · {emu_lbl}", s.battery_percent)
            }
            None => "Connect over USB or Bluetooth".to_owned(),
        };

        let mut device_sub_el = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_xs()
            .text_color(color(if connected {
                TEXT_SECONDARY
            } else {
                TEXT_MUTED
            }))
            .whitespace_nowrap();
        if is_charging {
            device_sub_el = device_sub_el.child(bolt_icon(12.0, SUCCESS));
        }
        device_sub_el = device_sub_el.child(device_sub);
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
                        cx.notify();
                    })),
                );
            }
            device_card = device_card.child(switcher);
        }

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
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.page = ActivePage::Triggers;
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        drawer_nav_item(
                                            "nav-audio",
                                            speaker_icon(16.0, nav_icon_col(ActivePage::Audio)),
                                            "Audio & mic",
                                            self.page == ActivePage::Audio,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.page = ActivePage::Audio;
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        drawer_nav_item(
                                            "nav-lighting",
                                            sun_icon(16.0, nav_icon_col(ActivePage::Lighting)),
                                            "Lighting & LEDs",
                                            self.page == ActivePage::Lighting,
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
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.page = ActivePage::MotionRemap;
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        drawer_nav_item(
                                            "nav-devinfo",
                                            info_icon(16.0, nav_icon_col(ActivePage::DeviceInfo)),
                                            "Device info",
                                            self.page == ActivePage::DeviceInfo,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.page = ActivePage::DeviceInfo;
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
                    .child({
                        let mut ver_row = div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(color(TEXT_MUTED))
                                    .child(concat!("DSMaster v", env!("CARGO_PKG_VERSION"))),
                            );
                        if let Some(rel) = self.available_update.as_ref() {
                            ver_row = ver_row.child(
                                div()
                                    .id("drawer-update-pill")
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .bg(color(SURFACE_ALT))
                                    .border_1()
                                    .border_color(color(BORDER_STRONG))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color(ACCENT))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(color(SURFACE_HOVER)))
                                    .child(format!("v{} ready", rel.version))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.page = ActivePage::Settings;
                                        cx.notify();
                                    })),
                            );
                        }
                        ver_row
                    }),
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
        if !self.drawer_open && self.page.is_controller_section() {
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
            .gap_2()
            .child(
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
            .gap_3()
            .p_4()
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(BORDER))
            .rounded(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
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
                            .child(override_toggle_with_label(
                                profile.override_triggers,
                                toggle_switch("qs-trig-override", profile.override_triggers)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let st = this.config.active_settings_mut();
                                        st.override_triggers = !st.override_triggers;
                                        this.commit_config(cx);
                                    })),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
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
                    .pt_3()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .child(meter_row_with_width("L2 pull", 56.0, s.l2_analog, ACCENT))
                    .child(meter_row_with_width("R2 pull", 56.0, s.r2_analog, ACCENT)),
            );

        // Card 2: Lightbar & controls
        let custom_color_active =
            profile.override_lightbar && profile.lightbar_mode.uses_custom_color();
        let mut swatch_chips = div().w_full().flex().items_center().gap_2();
        for (name, rgb_val) in RGB_SWATCHES {
            let active = custom_color_active && profile.lightbar_rgb == rgb_val;
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
                        st.set_custom_lightbar_rgb(rgb_val);
                        this.sync_picker_hsv();
                        this.commit_config(cx);
                    })),
            );
        }

        let active_lb_rgb = if !profile.override_lightbar {
            s.active_lightbar_rgb.unwrap_or([0, 112, 255])
        } else if custom_color_active {
            profile.lightbar_rgb
        } else {
            s.active_lightbar_rgb.unwrap_or(profile.lightbar_rgb)
        };
        let active_lb_hex = rgb_to_u32(active_lb_rgb);
        let active_lb_label = if !profile.override_lightbar {
            "Native".to_owned()
        } else if !custom_color_active {
            profile.lightbar_mode.label().to_owned()
        } else if active_lb_hex == 0 {
            "Off".to_owned()
        } else {
            format!(
                "#{:02X}{:02X}{:02X}",
                active_lb_rgb[0], active_lb_rgb[1], active_lb_rgb[2]
            )
        };
        let lb_label_is_hex = custom_color_active && active_lb_hex != 0;
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

        let mut mode_row = div().flex().flex_wrap().items_center().gap_2().child(
            div()
                .w(px(36.0))
                .flex_shrink_0()
                .text_size(px(13.0))
                .text_color(color(TEXT_MUTED))
                .child("Mode"),
        );
        for mode in LightbarMode::ALL {
            let active = profile.override_lightbar && profile.lightbar_mode == mode;
            mode_row = mode_row.child(
                pill_button(
                    SharedString::from(format!("qs-lb-mode-{}", mode.label())),
                    mode.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let st = this.config.active_settings_mut();
                    st.override_lightbar = true;
                    st.lightbar_mode = mode;
                    this.commit_config(cx);
                })),
            );
        }

        let gyro_on = profile.gyro_mode != GyroAimMode::Off;
        let lighting_card = div()
            .flex()
            .flex_col()
            .justify_between()
            .flex_1()
            .min_w(px(312.0))
            .gap_3()
            .p_4()
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(BORDER))
            .rounded(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
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
                            .child(override_toggle_with_label(
                                profile.override_lightbar,
                                toggle_switch("qs-lb-override", profile.override_lightbar)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let st = this.config.active_settings_mut();
                                        st.override_lightbar = !st.override_lightbar;
                                        this.commit_config(cx);
                                    })),
                            )),
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
                            .child(mode_row),
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
                            .h(px(28.0))
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
                                toggle_switch("qs-gyro-sw", gyro_on).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        let st = this.config.active_settings_mut();
                                        st.gyro_mode = if st.gyro_mode == GyroAimMode::Off {
                                            GyroAimMode::HoldL2
                                        } else {
                                            GyroAimMode::Off
                                        };
                                        this.commit_config(cx);
                                    },
                                )),
                            ),
                    ),
            );

        // Card 3: Controller status (balanced 3-section card matching neighboring cards)
        let scan_label = if connected {
            format!("{:.0} Hz", s.polling_hz)
        } else {
            "Idle".to_owned()
        };
        let interval_label = if connected && s.polling_hz > 0.5 {
            format!("{:.1} ms interval", 1000.0 / s.polling_hz)
        } else if connected {
            "Measuring…".to_owned()
        } else {
            "Waiting for HID".to_owned()
        };
        let interval_is_mono = connected && s.polling_hz > 0.5;

        let mut scan_val_el = div()
            .text_size(px(24.0))
            .font_weight(FontWeight::NORMAL)
            .text_color(color(if connected { ACCENT } else { TEXT_MUTED }))
            .child(scan_label);
        if connected {
            scan_val_el = scan_val_el.font_family(FONT_MONO);
        }

        let mut interval_el = div()
            .text_size(px(13.0))
            .text_color(color(if connected {
                TEXT_SECONDARY
            } else {
                TEXT_MUTED
            }))
            .child(interval_label);
        if interval_is_mono {
            interval_el = interval_el.font_family(FONT_MONO);
        }

        let active_emu = if connected {
            s.emulation
        } else {
            profile.emulation
        };
        let emu_str = match active_emu {
            EmulationMode::None => "Passthrough",
            EmulationMode::Xbox360 => "Xbox 360",
            EmulationMode::DualShock4 => "DualShock 4",
            EmulationMode::DualSense => "DualSense",
        };
        let cloak_on = self
            .engine
            .as_ref()
            .map(Engine::is_cloak_enabled)
            .unwrap_or(self.config.cloak_physical);
        let audio_str = if !connected {
            "—"
        } else if s.bt_audio_streaming || s.bt_haptics_streaming {
            "BT streaming"
        } else if s.headphone_connected {
            if s.mic_muted_hw {
                "Jack · muted"
            } else {
                "Jack in"
            }
        } else if s.mic_muted_hw {
            "Mic muted"
        } else {
            "Internal mic"
        };
        let session_str = if connected {
            fmt_hms(s.connected_secs)
        } else {
            "—".to_owned()
        };

        let status_tile = |label: &'static str, value: String, value_col: u32, mono: bool| {
            let mut val_el = div()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(value_col))
                .whitespace_nowrap()
                .child(value);
            if mono {
                val_el = val_el.font_family(FONT_MONO);
            }
            div()
                .flex_1()
                .min_w(px(0.0))
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
        };

        let (batt_pct, batt_label, batt_col) = if let Some(sn) = snap {
            let pct = sn.battery_percent.min(100);
            let label = match sn.battery_status {
                BatteryStatus::Charging => format!("{pct}% · Charging"),
                BatteryStatus::Full => "100% · Full".to_owned(),
                _ => format!("{pct}%"),
            };
            let col = if matches!(sn.battery_status, BatteryStatus::Charging | BatteryStatus::Full)
                || pct > 25
            {
                SUCCESS
            } else if pct > 10 {
                WARN
            } else {
                DANGER
            };
            (pct, label, col)
        } else {
            (0, "—".to_owned(), TEXT_MUTED)
        };

        let mut batt_val_el = div()
            .text_size(px(13.0))
            .text_color(color(if connected {
                TEXT_SECONDARY
            } else {
                TEXT_MUTED
            }))
            .child(batt_label);
        if connected {
            batt_val_el = batt_val_el.font_family(FONT_MONO);
        }

        let pill_text = if let Some(sn) = snap {
            let model = if sn.is_edge {
                "DualSense Edge"
            } else {
                "DualSense"
            };
            let tr = match sn.transport {
                Transport::Usb => "USB direct",
                Transport::Bluetooth => "Bluetooth",
            };
            format!("{model} · {tr}")
        } else {
            "Connect DualSense over USB or BT".to_owned()
        };

        let telemetry_card = div()
            .flex()
            .flex_col()
            .justify_between()
            .flex_1()
            .min_w(px(312.0))
            .gap_3()
            .p_4()
            .bg(color(SURFACE))
            .border_1()
            .border_color(color(BORDER))
            .rounded(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
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
                                    .child("Controller status"),
                            )
                            .child(
                                div()
                                    .id("qs-open-devinfo")
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
                                        "Open hardware diagnostics & firmware info",
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.page = ActivePage::DeviceInfo;
                                        cx.notify();
                                    }))
                                    .child("Details"),
                            ),
                    )
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
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(color(TEXT_SECONDARY))
                                            .child("Battery"),
                                    )
                                    .child(batt_val_el),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(6.0))
                                    .rounded_full()
                                    .bg(color(WELL_BG))
                                    .border_1()
                                    .border_color(color(BORDER))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(batt_pct as f32 / 100.0))
                                            .rounded_full()
                                            .bg(color(batt_col)),
                                    ),
                            ),
                    )
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
                            .child(interval_el),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(status_tile(
                                        "Emulation",
                                        emu_str.to_owned(),
                                        if connected { TEXT } else { TEXT_MUTED },
                                        false,
                                    ))
                                    .child(status_tile(
                                        "Cloak",
                                        if cloak_on {
                                            "Protected".to_owned()
                                        } else {
                                            "Off".to_owned()
                                        },
                                        if cloak_on { SUCCESS } else { TEXT_MUTED },
                                        false,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(status_tile(
                                        "Audio",
                                        audio_str.to_owned(),
                                        if connected { TEXT } else { TEXT_MUTED },
                                        false,
                                    ))
                                    .child(status_tile(
                                        "Session",
                                        session_str,
                                        if connected { TEXT } else { TEXT_MUTED },
                                        connected,
                                    )),
                            ),
                    ),
            )
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(color(BORDER))
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

        let outer_slider = if is_left {
            ActiveSlider::LeftOuter
        } else {
            ActiveSlider::RightOuter
        };

        let dz_row = self.render_slider_row(
            "Radial deadzone",
            format!("{}%", (tuning.deadzone * 100.0).round() as u32),
            tuning.deadzone / MAX_STICK_DZ,
            ACCENT,
            dz_slider,
            cx,
        );

        let outer_row = self.render_slider_row_range(
            "Outer deadzone",
            format!("{}%", (tuning.outer_deadzone * 100.0).round() as u32),
            tuning.outer_deadzone / MAX_OUTER_DZ,
            "0%",
            "40%",
            SUCCESS,
            outer_slider,
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

        let mut shape_pills = div().flex().flex_wrap().items_center().gap_2();
        for shape in DeadzoneShape::ALL {
            let active = tuning.shape == shape;
            shape_pills = shape_pills.child(
                pill_button(
                    SharedString::from(format!("{prefix}-sh-{}", shape.label())),
                    shape.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    if is_left {
                        s.left_stick.shape = shape;
                    } else {
                        s.right_stick.shape = shape;
                    }
                    this.commit_config(cx);
                })),
            );
        }

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
                .child(outer_row)
                .child(adz_row)
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
                                .child("Deadzone shape"),
                        )
                        .child(shape_pills),
                )
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
                                .child("Axis inversion"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    pill_button(
                                        SharedString::from(format!("{prefix}-invx")),
                                        "Invert X",
                                        tuning.invert_x,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let s = this.config.active_settings_mut();
                                        if is_left {
                                            s.left_stick.invert_x = !s.left_stick.invert_x;
                                        } else {
                                            s.right_stick.invert_x = !s.right_stick.invert_x;
                                        }
                                        this.commit_config(cx);
                                    })),
                                )
                                .child(
                                    pill_button(
                                        SharedString::from(format!("{prefix}-invy")),
                                        "Invert Y",
                                        tuning.invert_y,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let s = this.config.active_settings_mut();
                                        if is_left {
                                            s.left_stick.invert_y = !s.left_stick.invert_y;
                                        } else {
                                            s.right_stick.invert_y = !s.right_stick.invert_y;
                                        }
                                        this.commit_config(cx);
                                    })),
                                ),
                        ),
                )
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
                            .child(
                                pill_button("st-swap-sticks", "Swap L/R sticks", profile.swap_sticks)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let s = this.config.active_settings_mut();
                                        s.swap_sticks = !s.swap_sticks;
                                        this.commit_config(cx);
                                    })),
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

        let left_trig_tuning = profile.left_trigger_tuning();
        let right_trig_tuning = profile.right_trigger_tuning();

        let travel_card = section_card("Trigger travel & thresholds")
        .child(
            div()
                .flex()
                .items_center()
                .gap_6()
                .child(trigger_travel_gauge(
                    "L2 trigger",
                    snap.l2_analog,
                    left_trig_tuning.deadzone,
                    left_trig_tuning.hair_trigger,
                    l2_preset_lbl,
                    profile.override_triggers,
                ))
                .child(div().w(px(1.0)).h(px(56.0)).bg(color(BORDER)))
                .child(trigger_travel_gauge(
                    "R2 trigger",
                    snap.r2_analog,
                    right_trig_tuning.deadzone,
                    right_trig_tuning.hair_trigger,
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

        let mut adaptive_card = section_card_with_action(
            "Adaptive trigger actuators",
            override_toggle_with_label(
                profile.override_triggers,
                toggle_switch("tp-trig-override", profile.override_triggers).on_click(cx.listener(
                    |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.override_triggers = !s.override_triggers;
                        this.commit_config(cx);
                    },
                )),
            ),
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

        let build_custom_block = |is_left: bool,
                                  cfg: crate::engine::profile::CustomTriggerConfig,
                                  cx: &mut Context<Self>|
         -> Div {
            let side_tag = if is_left { "L2" } else { "R2" };
            let mut mode_pills = div().flex().flex_wrap().gap_1();
            for mode in CustomTriggerMode::ALL {
                let active = cfg.mode == mode;
                mode_pills = mode_pills.child(
                    pill_button(
                        SharedString::from(format!("tp-cust-{side_tag}-{}", mode.label())),
                        mode.label(),
                        active,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.override_triggers = true;
                        if is_left {
                            s.left_custom_trigger.mode = mode;
                        } else {
                            s.right_custom_trigger.mode = mode;
                        }
                        this.commit_config(cx);
                    })),
                );
            }
            let (s_sl, e_sl, f_sl, fq_sl) = if is_left {
                (
                    ActiveSlider::CustomLeftStart,
                    ActiveSlider::CustomLeftEnd,
                    ActiveSlider::CustomLeftForce,
                    ActiveSlider::CustomLeftFreq,
                )
            } else {
                (
                    ActiveSlider::CustomRightStart,
                    ActiveSlider::CustomRightEnd,
                    ActiveSlider::CustomRightForce,
                    ActiveSlider::CustomRightFreq,
                )
            };
            div()
                .p_3()
                .rounded(px(8.0))
                .bg(color(WELL_BG))
                .border_1()
                .border_color(color(BORDER))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(ACCENT))
                        .child(format!("{side_tag} custom parameters")),
                )
                .child(mode_pills)
                .child(self.render_slider_row_range(
                    "Start zone",
                    format!("{}", cfg.start_position),
                    cfg.start_position as f32 / 8.0,
                    "0",
                    "8",
                    ACCENT,
                    s_sl,
                    cx,
                ))
                .child(self.render_slider_row_range(
                    "End zone",
                    format!("{}", cfg.end_position),
                    (cfg.end_position.saturating_sub(1)) as f32 / 8.0,
                    "1",
                    "9",
                    ACCENT,
                    e_sl,
                    cx,
                ))
                .child(self.render_slider_row_range(
                    "Strength",
                    format!("{}/8", cfg.strength),
                    (cfg.strength.saturating_sub(1)) as f32 / 7.0,
                    "1",
                    "8",
                    ACCENT,
                    f_sl,
                    cx,
                ))
                .child(self.render_slider_row_range(
                    "Frequency",
                    format!("{} Hz", cfg.frequency),
                    (cfg.frequency.saturating_sub(1)) as f32 / 99.0,
                    "1",
                    "100",
                    ZONE_SECONDARY,
                    fq_sl,
                    cx,
                ))
        };

        if profile.override_triggers && profile.left_trigger_preset == TriggerPreset::Custom {
            adaptive_card = adaptive_card.child(build_custom_block(
                true,
                profile.left_custom_trigger,
                cx,
            ));
        }
        if profile.override_triggers && profile.right_trigger_preset == TriggerPreset::Custom {
            adaptive_card = adaptive_card.child(build_custom_block(
                false,
                profile.right_custom_trigger,
                cx,
            ));
        }

        let trig_dz_pct = (profile.triggers.deadzone as f32 * 100.0 / 255.0).round() as u32;
        let trig_dz_row = self.render_slider_row(
            if profile.link_triggers {
                "Trigger deadzone"
            } else {
                "L2 deadzone"
            },
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

        let rumble_slider_row = self.render_slider_row_range(
            "Rumble intensity",
            format!("{}%", (profile.rumble_strength * 100.0).round() as u32),
            profile.rumble_strength,
            "0%",
            "100%",
            WARN,
            ActiveSlider::RumbleStrength,
            cx,
        );

        let mut tuning_card = section_card_with_action(
            "Hair trigger & rumble motors",
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(color(TEXT_SECONDARY))
                        .child("Link L2 & R2"),
                )
                .child(
                    toggle_switch("tp-link-trig", profile.link_triggers).on_click(cx.listener(
                        |this, _, _, cx| {
                            let s = this.config.active_settings_mut();
                            s.link_triggers = !s.link_triggers;
                            this.commit_config(cx);
                        },
                    )),
                ),
        )
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
                            .child(if profile.link_triggers {
                                "Hair trigger cap"
                            } else {
                                "L2 hair cap"
                            }),
                    )
                    .child(hair_pills),
            );

        if !profile.link_triggers {
            let r2_dz_pct =
                (profile.right_trigger_tuning.deadzone as f32 * 100.0 / 255.0).round() as u32;
            let r2_dz_row = self.render_slider_row(
                "R2 deadzone",
                format!("{r2_dz_pct}%"),
                r2_dz_pct as f32 / MAX_TRIGGER_DZ_PCT,
                ACCENT,
                ActiveSlider::RightTriggerDz,
                cx,
            );
            let mut r2_hair_pills = div().flex().flex_wrap().gap_2();
            for (label, threshold) in HAIR_TRIGGER_STEPS {
                let active = profile.right_trigger_tuning.hair_trigger == threshold;
                r2_hair_pills = r2_hair_pills.child(
                    pill_button(SharedString::from(format!("tp-r2-ht-{label}")), label, active)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.config
                                .active_settings_mut()
                                .right_trigger_tuning
                                .hair_trigger = threshold;
                            this.commit_config(cx);
                        })),
                );
            }
            tuning_card = tuning_card.child(r2_dz_row).child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(color(TEXT_SECONDARY))
                            .child("R2 hair cap"),
                    )
                    .child(r2_hair_pills),
            );
        }

        let tuning_card = tuning_card.child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .pt_3()
                .border_t_1()
                .border_color(color(BORDER))
                .child(setting_row(
                    "Rumble feedback",
                    "Enable standard in-game controller vibration",
                    toggle_switch("tp-rumble-sw", profile.enable_rumble).on_click(cx.listener(
                        |this, _, _, cx| {
                            let s = this.config.active_settings_mut();
                            s.enable_rumble = !s.enable_rumble;
                            this.commit_config(cx);
                        },
                    )),
                ))
                .child(rumble_slider_row)
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
                                .child("Test motors"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(subtle_button("tp-test-left", "Left").on_click(
                                    cx.listener(|this, _, _, _| {
                                        if let Some(eng) = &this.engine {
                                            eng.trigger_test_rumble(200, 0, 400);
                                        }
                                    }),
                                ))
                                .child(subtle_button("tp-test-right", "Right").on_click(
                                    cx.listener(|this, _, _, _| {
                                        if let Some(eng) = &this.engine {
                                            eng.trigger_test_rumble(0, 200, 400);
                                        }
                                    }),
                                ))
                                .child(subtle_button("tp-test-both", "Both").on_click(
                                    cx.listener(|this, _, _, _| {
                                        if let Some(eng) = &this.engine {
                                            eng.trigger_test_rumble(200, 200, 400);
                                        }
                                    }),
                                )),
                        ),
                )
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

    /// Page: Audio & mic (Virtual Windows Audio Device + Bluetooth Speaker, Mic & HD Haptics)
    fn render_audio_page(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied().unwrap_or_default();

        let mic_gain_row = self.render_slider_row_range(
            "Microphone gain",
            format!("{}%", (profile.bt_mic_gain * 100.0).round() as u32),
            (profile.bt_mic_gain / 2.0).clamp(0.0, 1.0),
            "0%",
            "200%",
            SUCCESS,
            ActiveSlider::BtMicGain,
            cx,
        );

        let spk_vol_row = self.render_slider_row_range(
            "Speaker volume",
            format!("{}%", (profile.bt_speaker_volume * 100.0).round() as u32),
            profile.bt_speaker_volume,
            "0%",
            "100%",
            ACCENT,
            ActiveSlider::BtSpeakerVolume,
            cx,
        );

        let hap_gain_row = self.render_slider_row_range(
            "Haptics intensity",
            format!("{}%", (profile.bt_haptics_gain * 100.0).round() as u32),
            profile.bt_haptics_gain,
            "0%",
            "100%",
            WARN,
            ActiveSlider::BtHapticsGain,
            cx,
        );

        let mut route_pills = div().flex().flex_wrap().w_full().gap_2();
        for route in BtAudioRoute::ALL {
            let active = profile.bt_audio_route == route;
            route_pills = route_pills.child(
                pill_button(
                    format!("bt-route-{}", route.label()),
                    route.label(),
                    active,
                )
                .flex_1()
                .min_w(px(0.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.active_settings_mut().bt_audio_route = route;
                    this.commit_config(cx);
                })),
            );
        }

        let mut mic_route_pills = div().flex().flex_wrap().w_full().gap_2();
        for route in BtMicRoute::ALL {
            let active = profile.bt_mic_route == route;
            mic_route_pills = mic_route_pills.child(
                pill_button(
                    format!("bt-mic-route-{}", route.label()),
                    route.label(),
                    active,
                )
                .flex_1()
                .min_w(px(0.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.active_settings_mut().bt_mic_route = route;
                    this.commit_config(cx);
                })),
            );
        }

        let cloak_on = self
            .engine
            .as_ref()
            .is_some_and(Engine::is_cloak_enabled);

        let mut virtual_audio_card = section_card("Windows audio device (Speaker & Mic)");

        if !self.vds_available {
            virtual_audio_card = virtual_audio_card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .p_3()
                    .rounded(px(8.0))
                    .bg(color(WELL_BG))
                    .border_1()
                    .border_color(color(BORDER))
                    .child(
                        div()
                            .text_xs()
                            .text_color(color(TEXT_SECONDARY))
                            .child("usbip-win2 driver is required to register virtual Windows audio endpoints."),
                    )
                    .child(
                        subtle_button("va-open-settings", "Open driver settings").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.page = ActivePage::Settings;
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }

        if profile.virtual_mic_device && !cloak_on {
            let can_cloak_now = self.hidhide_available && profile.emulation != EmulationMode::None;
            let mut cloak_warn = div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .p_3()
                .rounded(px(8.0))
                .bg(color(WARN_SURFACE))
                .border_1()
                .border_color(color(WARN_BORDER))
                .child(
                    div()
                        .text_xs()
                        .text_color(color(WARN))
                        .child("Enable HidHide cloaking (with a virtual controller) to activate the Bluetooth microphone without triggering Steam ghost inputs."),
                );
            if can_cloak_now {
                cloak_warn = cloak_warn.child(
                    subtle_button("va-enable-cloak", "Enable cloaking").on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(eng) = &this.engine {
                                if eng.set_cloak_enabled(true).is_ok() {
                                    this.config.cloak_physical = true;
                                    this.commit_config(cx);
                                }
                            }
                        },
                    )),
                );
            }
            virtual_audio_card = virtual_audio_card.child(cloak_warn);
        }

        virtual_audio_card = virtual_audio_card.child(
            div()
                .flex()
                .gap_6()
                .pt_3()
                .border_t_1()
                .border_color(color(BORDER))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .pb_3()
                                .border_b_1()
                                .border_color(color(BORDER))
                                .child(setting_row(
                                    "Enable DualSense speaker",
                                    "Registers Speakers endpoint in Windows Sound",
                                    toggle_switch(
                                        "va-speaker-toggle",
                                        profile.virtual_speaker_device,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        {
                                            let s = this.config.active_settings_mut();
                                            s.virtual_speaker_device = !s.virtual_speaker_device;
                                            s.virtual_audio_device =
                                                s.virtual_speaker_device || s.virtual_mic_device;
                                        }
                                        this.commit_config(cx);
                                    })),
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(color(TEXT_SECONDARY))
                                        .child("Output route"),
                                )
                                .child(route_pills),
                        )
                        .child(spk_vol_row)
                        .child(meter_row("Speaker output", snap.bt_speaker_level, ACCENT))
                        .child(
                            div().flex().justify_end().child(
                                subtle_button("bt-test-speaker", "Test chime").on_click(
                                    cx.listener(|this, _, _, _| {
                                        if let Some(eng) = &this.engine {
                                            eng.trigger_test_bt_speaker();
                                        }
                                    }),
                                ),
                            ),
                        ),
                )
                .child(div().w(px(1.0)).bg(color(BORDER)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .pb_3()
                                .border_b_1()
                                .border_color(color(BORDER))
                                .child(setting_row(
                                    "Enable DualSense microphone",
                                    "Registers Microphone endpoint in Windows Sound",
                                    toggle_switch("va-mic-toggle", profile.virtual_mic_device)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let next_on = {
                                                let s = this.config.active_settings_mut();
                                                s.virtual_mic_device = !s.virtual_mic_device;
                                                s.virtual_audio_device = s.virtual_speaker_device
                                                    || s.virtual_mic_device;
                                                s.virtual_mic_device
                                            };
                                            if next_on && this.hidhide_available {
                                                let emu = this.config.active_settings().emulation;
                                                if emu != EmulationMode::None {
                                                    if let Some(eng) = &this.engine {
                                                        if !eng.is_cloak_enabled()
                                                            && eng.set_cloak_enabled(true).is_ok()
                                                        {
                                                            this.config.cloak_physical = true;
                                                        }
                                                    }
                                                }
                                            }
                                            this.commit_config(cx);
                                        })),
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(color(TEXT_SECONDARY))
                                        .child("Input route"),
                                )
                                .child(mic_route_pills),
                        )
                        .child(mic_gain_row)
                        .child(meter_row("Microphone input", snap.bt_mic_level, SUCCESS))
                        .child(
                            div()
                                .min_h(px(32.0))
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(color(if snap.mic_muted_hw {
                                            WARN
                                        } else {
                                            TEXT_MUTED
                                        }))
                                        .child(if snap.mic_muted_hw {
                                            "Muted on controller — press the mute button to unmute."
                                        } else if snap.mic_connected {
                                            "3.5 mm headset microphone detected."
                                        } else {
                                            "Opens automatically when an app records from the DualSense microphone."
                                        }),
                                ),
                        ),
                ),
        );

        let haptics_card = section_card("Bluetooth HD haptics")
            .child(setting_row(
                "Audio-to-Haptics",
                "Vibrate the controller in sync with PC bass and sound effects (replaces standard game rumble)",
                toggle_switch("bt-hap-toggle", profile.bt_haptics_enabled).on_click(
                    cx.listener(|this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.bt_haptics_enabled = !s.bt_haptics_enabled;
                        this.commit_config(cx);
                    }),
                ),
            ))
            .child(hap_gain_row)
            .child(meter_row("Haptics output", snap.bt_haptics_level, WARN))
            .child(
                div().flex().justify_end().child(
                    subtle_button("bt-test-haptics", "Test HD haptics").on_click(
                        cx.listener(|this, _, _, _| {
                            if let Some(eng) = &this.engine {
                                eng.trigger_test_bt_haptics();
                            }
                        }),
                    ),
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
                            .child("Audio & mic"),
                    )
                    .child(self.render_raw_output_toggle(cx)),
            )
            .child(virtual_audio_card)
            .child(haptics_card)
    }

    /// Page 4: Lighting & player LEDs
    fn render_lighting_page(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied().unwrap_or_default();

        let custom_color_active =
            profile.override_lightbar && profile.lightbar_mode.uses_custom_color();

        let make_rgb_tile = |name: &'static str, rgb_val: [u8; 3], cx: &mut Context<Self>| {
            let active = custom_color_active && profile.lightbar_rgb == rgb_val;
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
                    s.set_custom_lightbar_rgb(rgb_val);
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
        let active_rgb = if custom_color_active {
            cur_rgb
        } else if profile.override_lightbar {
            snap.active_lightbar_rgb.unwrap_or(cur_rgb)
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

        let lb_is_numeric = (custom_color_active || game_controlling_lb) && active_hex != 0;
        let mut lb_readout_el = div()
            .text_size(px(13.0))
            .text_color(color(if lb_is_numeric { TEXT } else { TEXT_SECONDARY }))
            .child(if !profile.override_lightbar && !game_controlling_lb {
                "Native or game default".to_owned()
            } else if profile.override_lightbar && !custom_color_active {
                format!("{} mode active", profile.lightbar_mode.label())
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

        let mut mode_pills = div().flex().flex_wrap().items_center().gap_2();
        for mode in LightbarMode::ALL {
            let active = profile.override_lightbar && profile.lightbar_mode == mode;
            mode_pills = mode_pills.child(
                pill_button(
                    SharedString::from(format!("lp-mode-{}", mode.label())),
                    mode.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.override_lightbar = true;
                    s.lightbar_mode = mode;
                    this.commit_config(cx);
                })),
            );
        }

        let brightness_row = self.render_slider_row_range(
            "Lightbar brightness",
            format!("{}%", (profile.lightbar_brightness * 100.0).round() as u32),
            profile.lightbar_brightness,
            "0%",
            "100%",
            ACCENT,
            ActiveSlider::LightbarBrightness,
            cx,
        );

        let mut speed_pills = div().flex().flex_wrap().items_center().gap_2();
        for (lbl, spd) in SENSITIVITY_STEPS {
            let active = (profile.lightbar_speed - spd).abs() < 0.05;
            speed_pills = speed_pills.child(
                pill_button(SharedString::from(format!("lp-spd-{lbl}")), lbl, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.override_lightbar = true;
                        s.lightbar_speed = spd;
                        this.commit_config(cx);
                    }),
                ),
            );
        }

        let mut rgb_card = section_card_with_action(
            "Lightbar color",
            override_toggle_with_label(
                profile.override_lightbar,
                toggle_switch("lp-lb-override", profile.override_lightbar).on_click(cx.listener(
                    |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.override_lightbar = !s.override_lightbar;
                        this.commit_config(cx);
                    },
                )),
            ),
        )
        .flex_1()
        .child(color_picker_well)
        .child(swatch_grid)
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
                                .text_size(px(13.0))
                                .text_color(color(TEXT_SECONDARY))
                                .child("Effect mode"),
                        )
                        .child(mode_pills),
                )
                .child(brightness_row),
        );

        if matches!(
            profile.lightbar_mode,
            LightbarMode::Breathing | LightbarMode::Rainbow
        ) {
            rgb_card = rgb_card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(color(TEXT_SECONDARY))
                            .child("Animation speed"),
                    )
                    .child(speed_pills),
            );
        }

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

        let mut led_bright_pills = div().flex().w_full().gap_2();
        for br in LedBrightness::ALL {
            let active = profile.player_led_brightness == br;
            led_bright_pills = led_bright_pills.child(
                pill_button(
                    SharedString::from(format!("lp-bright-{}", br.label())),
                    br.label(),
                    active,
                )
                .flex_1()
                .min_w(px(0.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.override_lightbar = true;
                    s.player_led_brightness = br;
                    this.commit_config(cx);
                })),
            );
        }

        let mut mute_led_pills = div().flex().w_full().gap_2();
        for mode in MicMuteLedOverride::ALL {
            let active = profile.mic_mute_led == mode;
            mute_led_pills = mute_led_pills.child(
                pill_button(
                    SharedString::from(format!("lp-mute-{}", mode.label())),
                    mode.label(),
                    active,
                )
                .flex_1()
                .min_w(px(0.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.active_settings_mut().mic_mute_led = mode;
                    this.commit_config(cx);
                })),
            );
        }

        let mut batt_thresh_pills = div().flex().w_full().gap_2();
        for pct in [15u8, 20, 30] {
            let active = profile.low_battery_threshold == pct;
            batt_thresh_pills = batt_thresh_pills.child(
                pill_button(
                    SharedString::from(format!("lp-bth-{pct}")),
                    format!("{pct}%"),
                    active,
                )
                .flex_1()
                .min_w(px(0.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.low_battery_alert = true;
                    s.low_battery_threshold = pct;
                    this.commit_config(cx);
                })),
            );
        }

        let mut idle_dim_pills = div().flex().w_full().gap_2();
        for (lbl, secs) in [("Off", 0u16), ("1m", 60), ("3m", 180), ("5m", 300)] {
            let active = profile.idle_dim_secs == secs;
            idle_dim_pills = idle_dim_pills.child(
                pill_button(SharedString::from(format!("lp-idle-{lbl}")), lbl, active)
                    .flex_1()
                    .min_w(px(0.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.config.active_settings_mut().idle_dim_secs = secs;
                        this.commit_config(cx);
                    })),
            );
        }

        let leds_card = section_card("Player, mute & power LEDs")
            .w(px(360.0))
            .flex_shrink_0()
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(color(TEXT_MUTED))
                    .child("Select the 5-LED bar pattern beneath the touchpad"),
            )
            .child(led_cards)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .pt_4()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(color(TEXT_SECONDARY))
                                    .child("LED brightness"),
                            )
                            .child(led_bright_pills),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(color(TEXT_SECONDARY))
                                    .child("Mic mute LED"),
                            )
                            .child(mute_led_pills),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .pt_4()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .child(setting_row(
                        "Low-battery flash",
                        "Pulse red lightbar when battery drops below threshold",
                        toggle_switch("lp-lowbatt-sw", profile.low_battery_alert).on_click(
                            cx.listener(|this, _, _, cx| {
                                let s = this.config.active_settings_mut();
                                s.low_battery_alert = !s.low_battery_alert;
                                this.commit_config(cx);
                            }),
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(color(TEXT_SECONDARY))
                                    .child("Alert threshold"),
                            )
                            .child(batt_thresh_pills),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(color(TEXT_SECONDARY))
                                    .child("Idle dim timeout"),
                            )
                            .child(idle_dim_pills),
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
                            .child("Lighting & player LEDs"),
                    )
                    .child(self.render_raw_output_toggle(cx)),
            )
            .child(div().flex().gap_4().child(rgb_card).child(leds_card))
    }

    /// Page 5: Motion & button remap
    fn render_motion_remap_page(&self, cx: &mut Context<Self>) -> Div {
        let profile = self.config.active_settings();
        let snap = self.selected_snapshot().copied().unwrap_or_default();

        let mut source_grid = div().flex().flex_wrap().gap_2();
        for src in RemappableButton::SOURCES {
            let win_mic_locked =
                src == RemappableButton::MicMute && profile.mic_button_controls_windows_mic;
            let dst = profile.button_map.get(src);
            let act = profile.action_map.get(src);
            let has_act = !act.is_none();
            let is_custom = win_mic_locked || dst != src || has_act;
            let is_sel = self.selected_remap_src == Some(src);
            let badge_text = if win_mic_locked {
                "Win Mic".to_owned()
            } else if has_act {
                act.format_label()
            } else if dst != src {
                format!("→ {}", dst.label())
            } else {
                dst.label().to_owned()
            };
            source_grid = source_grid.child(
                div()
                    .id(SharedString::from(format!("mr-src-{}", src.label())))
                    .w(px(148.0))
                    .flex_1()
                    .min_w(px(136.0))
                    .px_3()
                    .py_2()
                    .rounded(px(8.0))
                    .bg(color(if is_sel {
                        SURFACE_HOVER
                    } else if is_custom {
                        SURFACE_ALT
                    } else {
                        WELL_BG
                    }))
                    .border_1()
                    .border_color(color(if is_sel {
                        ACCENT
                    } else if is_custom {
                        BORDER_STRONG
                    } else {
                        BORDER
                    }))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.border_color(color(BORDER_STRONG)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.recording_hotkey = None;
                        this.selected_remap_src = if this.selected_remap_src == Some(src) {
                            None
                        } else {
                            Some(src)
                        };
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(TEXT))
                            .child(src.label()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(FONT_MONO)
                            .text_color(color(if win_mic_locked {
                                SUCCESS
                            } else if !has_act && dst == RemappableButton::None {
                                DANGER
                            } else if is_custom {
                                ACCENT
                            } else {
                                TEXT_MUTED
                            }))
                            .child(badge_text),
                    ),
            );
        }

        let mut turbo_targets = div().flex().flex_wrap().gap_1();
        for tgt in RemappableButton::TURBO_TARGETS {
            let active = profile.turbo.target == tgt;
            turbo_targets = turbo_targets.child(
                pill_button(
                    SharedString::from(format!("mr-tb-{}", tgt.label())),
                    tgt.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.turbo.target = tgt;
                    this.commit_config(cx);
                })),
            );
        }

        let mut turbo_rates = div().flex().flex_wrap().gap_2();
        for rate in [5u8, 10, 15, 20] {
            let active = profile.turbo.rate_hz == rate;
            turbo_rates = turbo_rates.child(
                pill_button(
                    SharedString::from(format!("mr-tbr-{rate}")),
                    format!("{rate} Hz"),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.turbo.enabled = true;
                    s.turbo.rate_hz = rate;
                    this.commit_config(cx);
                })),
            );
        }

        let custom_count = profile.button_map.custom_count() + profile.action_map.custom_count();
        let mut remap_card = section_card_with_action(
            "Button remapping & hotkeys",
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(color(TEXT_MUTED))
                        .child(if custom_count == 0 {
                            "1:1 default".to_owned()
                        } else {
                            format!("{custom_count} customized")
                        }),
                )
                .child(
                    subtle_button("mr-map-reset-all", "Reset all").on_click(cx.listener(
                        |this, _, _, cx| {
                            let s = this.config.active_settings_mut();
                            s.button_map = Default::default();
                            s.action_map = Default::default();
                            this.selected_remap_src = None;
                            this.recording_hotkey = None;
                            this.commit_config(cx);
                        },
                    )),
                ),
        )
        .flex_1()
        .child(
            div()
                .pb_2()
                .border_b_1()
                .border_color(color(BORDER))
                .child(setting_row(
                    "Mic button controls Windows mic",
                    "Toggle default Windows microphone mute & sync the orange Mic LED (disables custom Mic Mute remap)",
                    toggle_switch(
                        "mr-win-mic-sw",
                        profile.mic_button_controls_windows_mic,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.mic_button_controls_windows_mic = !s.mic_button_controls_windows_mic;
                        if s.mic_button_controls_windows_mic
                            && matches!(
                                this.recording_hotkey,
                                Some(
                                    HotkeyRecordTarget::SingleKey(RemappableButton::MicMute)
                                        | HotkeyRecordTarget::MacroStep(RemappableButton::MicMute)
                                )
                            )
                        {
                            this.recording_hotkey = None;
                        }
                        this.commit_config(cx);
                    })),
                )),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(color(TEXT_MUTED))
                .child("Click any button (e.g. Mic Mute) to remap or bind a keyboard hotkey, media key, or macro"),
        )
        .child(source_grid);

        if let Some(src) = self.selected_remap_src {
            if src == RemappableButton::MicMute && profile.mic_button_controls_windows_mic {
                remap_card = remap_card.child(status_info_pill(
                    "Mic Mute is currently controlling your Windows microphone — turn off \"Mic button controls Windows mic\" above to customize its remap or hotkey",
                ));
            } else {
            let cur_dst = profile.button_map.get(src);
            let mut dst_pills = div().flex().flex_wrap().gap_1().child(
                pill_button("mr-dst-off", "Off", cur_dst == RemappableButton::None).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.config
                            .active_settings_mut()
                            .button_map
                            .set(src, RemappableButton::None);
                        this.commit_config(cx);
                    }),
                ),
            );
            for dst in RemappableButton::SOURCES {
                let active = cur_dst == dst;
                dst_pills = dst_pills.child(
                    pill_button(
                        SharedString::from(format!("mr-dst-{}", dst.label())),
                        dst.label(),
                        active,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.config.active_settings_mut().button_map.set(src, dst);
                        this.commit_config(cx);
                    })),
                );
            }

            let mut tray_actions = div().flex().items_center().gap_2().child(
                subtle_button("mr-dst-reset-one", "Reset").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.config.active_settings_mut().button_map.set(src, src);
                        this.commit_config(cx);
                    },
                )),
            );
            if cur_dst != src && cur_dst != RemappableButton::None {
                tray_actions = tray_actions.child(
                    subtle_button("mr-dst-swap", format!("Swap with {}", cur_dst.label()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let map = &mut this.config.active_settings_mut().button_map;
                            map.set(src, src);
                            map.swap(src, cur_dst);
                            this.commit_config(cx);
                        })),
                );
            }

            let cur_act = profile.action_map.get(src);
            let recording_single = self.recording_hotkey == Some(HotkeyRecordTarget::SingleKey(src));
            let recording_macro = self.recording_hotkey == Some(HotkeyRecordTarget::MacroStep(src));

            let mut hotkey_actions = div().flex().flex_wrap().items_center().gap_2().child(
                pill_button(
                    "mr-act-rec",
                    if recording_single {
                        "Press any key… (Esc cancels)"
                    } else {
                        "Record hotkey"
                    },
                    recording_single,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.recording_hotkey = if this.recording_hotkey
                        == Some(HotkeyRecordTarget::SingleKey(src))
                    {
                        None
                    } else {
                        Some(HotkeyRecordTarget::SingleKey(src))
                    };
                    cx.notify();
                })),
            );

            if let ButtonAction::Key {
                vk,
                modifiers,
                hold,
            } = cur_act
            {
                hotkey_actions = hotkey_actions
                    .child(
                        pill_button(
                            "mr-act-hold",
                            if hold { "Mode: Hold" } else { "Mode: Tap" },
                            hold,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.config.active_settings_mut().action_map.set(
                                src,
                                ButtonAction::Key {
                                    vk,
                                    modifiers,
                                    hold: !hold,
                                },
                            );
                            this.commit_config(cx);
                        })),
                    )
                    .child(subtle_button("mr-act-test", "Test").on_click(cx.listener(
                        move |_, _, _, _| {
                            send_combo_tap(vk, modifiers);
                        },
                    )));
            }

            if !cur_act.is_none() {
                hotkey_actions = hotkey_actions.child(
                    subtle_button("mr-act-clear", "Clear").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.config
                                .active_settings_mut()
                                .action_map
                                .set(src, ButtonAction::None);
                            this.recording_hotkey = None;
                            this.commit_config(cx);
                        },
                    )),
                );
            }

            let quick_hotkeys: [(&str, u16, u8); 8] = [
                ("Ctrl+Shift+M", 0x4D, MOD_CTRL | MOD_SHIFT),
                ("Ctrl+Shift+D", 0x44, MOD_CTRL | MOD_SHIFT),
                ("F13", 0x7C, 0),
                ("F14", 0x7D, 0),
                ("F15", 0x7E, 0),
                ("F16", 0x7F, 0),
                ("PrintScr", 0x2C, 0),
                ("Alt+F10", 0x79, MOD_ALT),
            ];
            let mut quick_hk_row = div().flex().flex_wrap().gap_1();
            for (label, vk, mods) in quick_hotkeys {
                let active = matches!(
                    cur_act,
                    ButtonAction::Key {
                        vk: cvk,
                        modifiers: cm,
                        ..
                    } if cvk == vk && cm == mods
                );
                quick_hk_row = quick_hk_row.child(
                    pill_button(SharedString::from(format!("mr-qhk-{label}")), label, active)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.recording_hotkey = None;
                            this.config.active_settings_mut().action_map.set(
                                src,
                                ButtonAction::Key {
                                    vk,
                                    modifiers: mods,
                                    hold: false,
                                },
                            );
                            this.commit_config(cx);
                        })),
                );
            }

            let mut media_row = div().flex().flex_wrap().gap_1();
            for mk in MediaKey::ALL {
                let active = cur_act == ButtonAction::Media(mk);
                media_row = media_row.child(
                    pill_button(
                        SharedString::from(format!("mr-mk-{}", mk.label())),
                        mk.label(),
                        active,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.recording_hotkey = None;
                        this.config
                            .active_settings_mut()
                            .action_map
                            .set(src, ButtonAction::Media(mk));
                        this.commit_config(cx);
                    })),
                );
            }

            let macro_summary = match cur_act {
                ButtonAction::Macro { steps, len } if len > 0 => {
                    let parts: Vec<String> = steps
                        .iter()
                        .take((len as usize).min(MAX_MACRO_STEPS))
                        .map(|st| {
                            format!(
                                "{} ({}ms)",
                                format_key_combo(st.vk, st.modifiers),
                                st.delay_ms
                            )
                        })
                        .collect();
                    parts.join(" → ")
                }
                _ => "No macro steps — click + Record step to chain keys".to_owned(),
            };

            let mut delay_pills = div().flex().items_center().gap_1();
            for ms in [25u16, 50, 100, 250] {
                delay_pills = delay_pills.child(
                    pill_button(
                        SharedString::from(format!("mr-mdel-{ms}")),
                        format!("{ms}ms"),
                        self.macro_step_delay_ms == ms,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.macro_step_delay_ms = ms;
                        cx.notify();
                    })),
                );
            }

            let mut macro_controls = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    pill_button(
                        "mr-macro-add",
                        if recording_macro {
                            "Press step key… (Esc cancels)"
                        } else {
                            "+ Record step"
                        },
                        recording_macro,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.recording_hotkey = if this.recording_hotkey
                            == Some(HotkeyRecordTarget::MacroStep(src))
                        {
                            None
                        } else {
                            Some(HotkeyRecordTarget::MacroStep(src))
                        };
                        cx.notify();
                    })),
                )
                .child(delay_pills);

            if let ButtonAction::Macro { steps, len } = cur_act {
                if len > 0 {
                    macro_controls = macro_controls.child(
                        subtle_button("mr-macro-pop", "Undo step").on_click(cx.listener(
                            move |this, _, _, cx| {
                                let new_len = len.saturating_sub(1);
                                let next = if new_len == 0 {
                                    ButtonAction::None
                                } else {
                                    ButtonAction::Macro {
                                        steps,
                                        len: new_len,
                                    }
                                };
                                this.config.active_settings_mut().action_map.set(src, next);
                                this.commit_config(cx);
                            },
                        )),
                    );
                }
            }

            remap_card = remap_card.child(
                div()
                    .p_3()
                    .rounded(px(8.0))
                    .bg(color(WELL_BG))
                    .border_1()
                    .border_color(color(BORDER_STRONG))
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
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color(ACCENT))
                                    .child(format!("Remap {} → {}", src.label(), cur_dst.label())),
                            )
                            .child(tray_actions),
                    )
                    .child(dst_pills)
                    .child(
                        div()
                            .pt_2()
                            .mt_1()
                            .border_t_1()
                            .border_color(color(BORDER))
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
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(color(TEXT))
                                            .child(format!(
                                                "Keyboard / Media / Macro: {}",
                                                cur_act.format_label()
                                            )),
                                    ),
                            )
                            .child(hotkey_actions)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(color(TEXT_MUTED))
                                    .child("Quick hotkeys (Discord mute/deafen, F13–F16, capture)"),
                            )
                            .child(quick_hk_row)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(color(TEXT_MUTED))
                                    .child("Media keys"),
                            )
                            .child(media_row)
                            .child(
                                div()
                                    .pt_2()
                                    .border_t_1()
                                    .border_color(color(BORDER))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(color(TEXT_MUTED))
                                            .child("Macro sequence"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family(FONT_MONO)
                                            .text_color(color(TEXT_SECONDARY))
                                            .child(macro_summary),
                                    )
                                    .child(macro_controls),
                            ),
                    ),
            );
            }
        }

        let mut turbo_box = div()
            .flex()
            .flex_col()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(color(BORDER))
            .child(setting_row(
                "Turbo rapid-fire",
                "Pulse the selected button at a fixed rate while held",
                toggle_switch("mr-turbo-sw", profile.turbo.enabled).on_click(cx.listener(
                    |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.turbo.enabled = !s.turbo.enabled;
                        this.commit_config(cx);
                    },
                )),
            ))
            .child(turbo_targets)
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
                            .child("Turbo rate"),
                    )
                    .child(turbo_rates),
            );

        if profile.turbo.enabled && profile.emulation == EmulationMode::None {
            turbo_box = turbo_box.child(status_info_pill(
                "Enable Xbox 360, DualShock 4, or DualSense emulation for games to receive Turbo pulses",
            ));
        }

        let remap_card = remap_card.child(turbo_box);

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

        let mut gyro_outputs = div().flex().flex_wrap().gap_2();
        for out_mode in GyroOutputMode::ALL {
            let active = profile.gyro_output == out_mode;
            gyro_outputs = gyro_outputs.child(
                pill_button(
                    SharedString::from(format!("mr-go-{}", out_mode.label())),
                    out_mode.label(),
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.active_settings_mut().gyro_output = out_mode;
                    this.commit_config(cx);
                })),
            );
        }

        let mut gyro_modes = div().flex().flex_wrap().gap_1();
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

        let gyro_dz_row = self.render_slider_row_range(
            "Gyro deadzone",
            format!("{:.1}°/s", profile.gyro_deadzone),
            profile.gyro_deadzone / MAX_GYRO_DZ,
            "0°/s",
            "8°/s",
            ACCENT,
            ActiveSlider::GyroDeadzone,
            cx,
        );

        let imu_readout = div()
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
                    .font_family(FONT_MONO)
                    .text_color(color(TEXT_SECONDARY))
                    .child(format!(
                        "Gyro  P {:+.1} · Y {:+.1} · R {:+.1} °/s",
                        snap.gyro_dps[0], snap.gyro_dps[1], snap.gyro_dps[2]
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(FONT_MONO)
                    .text_color(color(TEXT_MUTED))
                    .child(format!(
                        "Accel X {:+.2} · Y {:+.2} · Z {:+.2} g",
                        snap.accel_g[0], snap.accel_g[1], snap.accel_g[2]
                    )),
            );

        let fs_on = profile.flick_stick_enabled;
        let mut fs_turn_pills = div().flex().flex_wrap().items_center().gap_1();
        if !fs_on {
            fs_turn_pills = fs_turn_pills.opacity(0.45);
        }
        for (label, ppt) in [
            ("2k", 2000.0f32),
            ("4k", 4000.0),
            ("8k", 8000.0),
            ("12k", 12000.0),
            ("16k", 16000.0),
        ] {
            let active = fs_on && (profile.flick_stick_pixels_per_turn - ppt).abs() < 100.0;
            fs_turn_pills = fs_turn_pills.child(
                pill_button(
                    SharedString::from(format!("mr-fs-ppt-{}", ppt as u32)),
                    label,
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.flick_stick_enabled = true;
                    s.flick_stick_pixels_per_turn = ppt;
                    this.commit_config(cx);
                })),
            );
        }
        fs_turn_pills = fs_turn_pills
            .child(subtle_button("mr-fs-dec", "-500").on_click(cx.listener(|this, _, _, cx| {
                let s = this.config.active_settings_mut();
                s.flick_stick_pixels_per_turn = (s.flick_stick_pixels_per_turn - 500.0).max(200.0);
                this.commit_config(cx);
            })))
            .child(subtle_button("mr-fs-inc", "+500").on_click(cx.listener(|this, _, _, cx| {
                let s = this.config.active_settings_mut();
                s.flick_stick_pixels_per_turn =
                    (s.flick_stick_pixels_per_turn + 500.0).min(40_000.0);
                this.commit_config(cx);
            })));

        let mut fs_speed_pills = div().flex().flex_wrap().gap_1();
        if !fs_on {
            fs_speed_pills = fs_speed_pills.opacity(0.45);
        }
        for (label, ms) in [
            ("Snappy (40ms)", 40u16),
            ("Fast (80ms)", 80),
            ("Smooth (130ms)", 130),
        ] {
            let active = fs_on && profile.flick_stick_flick_ms == ms;
            fs_speed_pills = fs_speed_pills.child(
                pill_button(SharedString::from(format!("mr-fs-ms-{ms}")), label, active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.flick_stick_enabled = true;
                        s.flick_stick_flick_ms = ms;
                        this.commit_config(cx);
                    }),
                ),
            );
        }

        let mut fs_thresh_pills = div().flex().flex_wrap().gap_1();
        if !fs_on {
            fs_thresh_pills = fs_thresh_pills.opacity(0.45);
        }
        for (label, th) in [("65%", 0.65f32), ("80%", 0.80), ("90%", 0.90)] {
            let active = fs_on && (profile.flick_stick_threshold - th).abs() < 0.02;
            fs_thresh_pills = fs_thresh_pills.child(
                pill_button(SharedString::from(format!("mr-fs-th-{label}")), label, active)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let s = this.config.active_settings_mut();
                        s.flick_stick_enabled = true;
                        s.flick_stick_threshold = th;
                        this.commit_config(cx);
                    })),
            );
        }

        let mut flick_stick_box = div()
            .flex()
            .flex_col()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(color(BORDER))
            .child(setting_row(
                "Flick Stick (right stick)",
                "Snap-turn camera to stick angle & sweep smoothly via mouse (centers virtual R-stick)",
                toggle_switch("mr-fs-sw", fs_on).on_click(cx.listener(|this, _, _, cx| {
                    let s = this.config.active_settings_mut();
                    s.flick_stick_enabled = !s.flick_stick_enabled;
                    this.commit_config(cx);
                })),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_family(FONT_MONO)
                            .text_color(color(if fs_on { TEXT_SECONDARY } else { TEXT_MUTED }))
                            .child(format!(
                                "360° counts: {} px",
                                profile.flick_stick_pixels_per_turn.round() as u32
                            )),
                    )
                    .child(fs_turn_pills),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(color(if fs_on { TEXT_SECONDARY } else { TEXT_MUTED }))
                            .child("Flick speed"),
                    )
                    .child(fs_speed_pills),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(color(if fs_on { TEXT_SECONDARY } else { TEXT_MUTED }))
                            .child("Engage threshold"),
                    )
                    .child(fs_thresh_pills),
            );

        if fs_on
            && (profile.gyro_output != GyroOutputMode::Mouse
                || profile.gyro_mode == GyroAimMode::Off)
        {
            let warn_msg = if profile.gyro_output != GyroOutputMode::Mouse {
                "Gyro output is not set to Mouse — Flick Stick only turns horizontally; set Gyro to Mouse for vertical aiming"
            } else {
                "Gyro aim is Off — Flick Stick only turns horizontally; enable Gyro (Always On) for vertical aiming"
            };
            flick_stick_box = flick_stick_box.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(px(8.0))
                    .bg(color(WARN_SURFACE))
                    .border_1()
                    .border_color(color(WARN_BORDER))
                    .text_xs()
                    .text_color(color(WARN))
                    .child(warn_msg),
            );
        }

        let motion_card = section_card("Touchpad, gyro & Flick Stick")
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
                    .flex_wrap()
                    .gap_2()
                    .child(
                        pill_button("mr-tp-scroll", "2-finger scroll", profile.touchpad_scroll)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let s = this.config.active_settings_mut();
                                s.touchpad_scroll = !s.touchpad_scroll;
                                this.commit_config(cx);
                            })),
                    )
                    .child(
                        pill_button("mr-tp-tap", "Tap to click", profile.touchpad_tap_to_click)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let s = this.config.active_settings_mut();
                                s.touchpad_tap_to_click = !s.touchpad_tap_to_click;
                                this.commit_config(cx);
                            })),
                    )
                    .child(
                        pill_button("mr-tp-split", "Split L/R click", profile.touchpad_split_click)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let s = this.config.active_settings_mut();
                                s.touchpad_split_click = !s.touchpad_split_click;
                                this.commit_config(cx);
                            })),
                    ),
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
                                    .child("Gyro output"),
                            )
                            .child(gyro_outputs),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
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
                    )
                    .child(gyro_dz_row)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(pill_button("mr-gy-yaw", "Yaw", profile.gyro_use_yaw).on_click(
                                cx.listener(|this, _, _, cx| {
                                    let s = this.config.active_settings_mut();
                                    s.gyro_use_yaw = !s.gyro_use_yaw;
                                    this.commit_config(cx);
                                }),
                            ))
                            .child(
                                pill_button("mr-gy-roll", "Roll", profile.gyro_use_roll).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        let s = this.config.active_settings_mut();
                                        s.gyro_use_roll = !s.gyro_use_roll;
                                        this.commit_config(cx);
                                    }),
                                ),
                            )
                            .child(
                                pill_button("mr-gy-invx", "Invert X", profile.gyro_invert_x)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let s = this.config.active_settings_mut();
                                        s.gyro_invert_x = !s.gyro_invert_x;
                                        this.commit_config(cx);
                                    })),
                            )
                            .child(
                                pill_button("mr-gy-invy", "Invert Y", profile.gyro_invert_y)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let s = this.config.active_settings_mut();
                                        s.gyro_invert_y = !s.gyro_invert_y;
                                        this.commit_config(cx);
                                    })),
                            ),
                    )
                    .child(imu_readout),
            )
            .child(flick_stick_box);

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

    /// Page: Device info & hardware diagnostics
    fn render_device_info_page(&self, cx: &mut Context<Self>) -> Div {
        let connected = !self.handles.is_empty();
        let snap = self.selected_snapshot().copied().unwrap_or_default();

        let model_str = if !connected {
            "Disconnected".to_owned()
        } else if snap.is_edge {
            "DualSense Edge".to_owned()
        } else {
            "DualSense".to_owned()
        };
        let vid_pid_str = if connected && snap.product_id != 0 {
            format!("0x054C:0x{:04X}", snap.product_id)
        } else {
            "—".to_owned()
        };
        let mac_str = if connected && snap.mac_address != [0; 6] {
            let m = snap.mac_address;
            format!(
                "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            )
        } else {
            "Unavailable".to_owned()
        };
        let transport_str = if !connected {
            "—"
        } else {
            match snap.transport {
                Transport::Usb => "USB",
                Transport::Bluetooth => "Bluetooth",
            }
        };
        let fw_str = if connected && snap.firmware_version != 0 {
            format!("0x{:08X}", snap.firmware_version)
        } else {
            "Unavailable".to_owned()
        };
        let hw_str = if connected && snap.hardware_version != 0 {
            format!("0x{:08X}", snap.hardware_version)
        } else {
            "Unavailable".to_owned()
        };
        let fw_date_raw = String::from_utf8_lossy(&snap.firmware_date);
        let fw_date_trimmed = fw_date_raw.trim_matches('\0').trim();
        let fw_date_str = if connected && !fw_date_trimmed.is_empty() {
            fw_date_trimmed.to_owned()
        } else {
            "Unavailable".to_owned()
        };
        let batt_str = if !connected {
            "—".to_owned()
        } else {
            let st = match snap.battery_status {
                BatteryStatus::Charging => "Charging",
                BatteryStatus::Full => "Full",
                BatteryStatus::Discharging => "Discharging",
                BatteryStatus::TempOrVoltageError => "Temp/Volt Err",
                BatteryStatus::Unknown => "Unknown",
            };
            format!("{}% ({st})", snap.battery_percent)
        };
        let poll_str = if connected {
            format!("{:.0} Hz", snap.polling_hz)
        } else {
            "—".to_owned()
        };
        let conn_dur = if connected {
            fmt_hms(snap.connected_secs)
        } else {
            "00:00:00".to_owned()
        };
        let idle_dur = if connected {
            fmt_hms(snap.idle_secs)
        } else {
            "00:00:00".to_owned()
        };
        let audio_str = if !connected {
            "—".to_owned()
        } else {
            let hp = if snap.headphone_connected { "Jack In" } else { "No Jack" };
            let mic = if snap.mic_muted_hw {
                "Mic Muted"
            } else if snap.mic_connected {
                "Mic Ready"
            } else {
                "Internal Mic"
            };
            format!("{hp} · {mic}")
        };

        let copy_summary = format!(
            "Model: {model_str}\nVID:PID: {vid_pid_str}\nMAC: {mac_str}\nTransport: {transport_str}\nFirmware: {fw_str}\nBuild Date: {fw_date_str}\nHardware: {hw_str}\nBattery: {batt_str}\nPolling: {poll_str}\nConnected: {conn_dur}\nIdle: {idle_dur}\nAudio: {audio_str}"
        );

        let info_card = section_card_with_action(
            "Hardware diagnostics",
            subtle_button("di-copy-btn", "Copy info").on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_summary.clone()));
                this.set_status_notice("Device info copied to clipboard", Some(NOTICE_TTL));
                cx.notify();
            })),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .child(info_kv_tile("Controller Model", model_str, false))
                .child(info_kv_tile("VID : PID", vid_pid_str, true))
                .child(info_kv_tile("MAC Address", mac_str, true))
                .child(info_kv_tile("Transport", transport_str, false))
                .child(info_kv_tile("Firmware Version", fw_str, true))
                .child(info_kv_tile("Firmware Build", fw_date_str, true))
                .child(info_kv_tile("Hardware Version", hw_str, true))
                .child(info_kv_tile("Battery", batt_str, true))
                .child(info_kv_tile("Polling Rate", poll_str, true))
                .child(info_kv_tile("Connection Time", conn_dur, true))
                .child(info_kv_tile("Idle Time", idle_dur, true))
                .child(info_kv_tile("Audio / Mic", audio_str, false)),
        );

        let caps = [
            "Adaptive Triggers",
            "Lightbar RGB",
            "Player Indicator LEDs",
            "Mic Mute LED",
            "LED Brightness",
            "Touchpad",
            "Motion IMU",
            "Dual Rumble Motors",
        ];
        let mut caps_grid = div().flex().flex_wrap().gap_2();
        for cap in caps {
            caps_grid = caps_grid.child(status_inset_pill(cap, connected));
        }

        let bt_idle_mins = self.config.bt_idle_disconnect_mins;
        let selected_bt_mac = self
            .selected_snapshot()
            .filter(|s| s.transport == Transport::Bluetooth && s.mac_address != [0; 6])
            .map(|s| s.mac_address);

        let mut bt_idle_pills = div().flex().flex_wrap().gap_2();
        for (mins, label) in [
            (0u16, "Off"),
            (5u16, "5m"),
            (10u16, "10m"),
            (15u16, "15m"),
            (20u16, "20m"),
            (30u16, "30m"),
        ] {
            let active = bt_idle_mins == mins;
            bt_idle_pills = bt_idle_pills.child(
                pill_button(
                    SharedString::from(format!("di-bt-idle-{mins}")),
                    label,
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.bt_idle_disconnect_mins = mins;
                    this.commit_config(cx);
                })),
            );
        }

        let bt_power_card = if let Some(mac) = selected_bt_mac {
            section_card_with_action(
                "Bluetooth power",
                subtle_button("di-bt-disc-now", "Disconnect BT").on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let Some(engine) = &this.engine {
                            match engine.disconnect_bluetooth_mac(mac) {
                                Ok(()) => this.set_status_notice(
                                    "Bluetooth controller disconnected",
                                    Some(NOTICE_TTL),
                                ),
                                Err(e) => this.set_status_notice(
                                    format!("Bluetooth disconnect failed: {e}"),
                                    Some(NOTICE_ERROR_TTL),
                                ),
                            }
                            cx.notify();
                        }
                    },
                )),
            )
        } else {
            section_card("Bluetooth power")
        }
        .child(setting_row(
            "Bluetooth auto-disconnect",
            "Drop Bluetooth after inactivity so the controller powers off",
            bt_idle_pills,
        ));

        let caps_card = section_card("Supported device features").child(caps_grid);

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
                            .child("Device info"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(color(TEXT_MUTED))
                            .child("Live DualSense firmware, hardware identifiers, and capability status"),
                    ),
            )
            .child(info_card)
            .child(bt_power_card)
            .child(caps_card)
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
                        subtle_button("prof-import-btn", "Import clipboard").on_click(cx.listener(
                            |this, _, _, cx| {
                                let Some(raw) = cx.read_from_clipboard().and_then(|i| i.text()) else {
                                    this.set_status_notice(
                                        "Clipboard has no text to import",
                                        Some(NOTICE_TTL),
                                    );
                                    cx.notify();
                                    return;
                                };
                                let imported = toml::from_str::<NamedProfile>(&raw)
                                    .ok()
                                    .or_else(|| {
                                        toml::from_str::<crate::engine::ControllerProfile>(&raw)
                                            .ok()
                                            .map(|settings| NamedProfile {
                                                name: "Imported".to_owned(),
                                                linked_exe: None,
                                                settings,
                                            })
                                    });
                                let Some(mut prof) = imported else {
                                    this.set_status_notice(
                                        "Clipboard is not valid DSMaster profile TOML",
                                        Some(NOTICE_ERROR_TTL),
                                    );
                                    cx.notify();
                                    return;
                                };
                                let base_name = if prof.name.trim().is_empty() {
                                    "Imported".to_owned()
                                } else {
                                    prof.name.trim().to_owned()
                                };
                                let mut unique_name = base_name.clone();
                                let mut suffix = 2usize;
                                while this.config.profiles.iter().any(|p| p.name == unique_name) {
                                    unique_name = format!("{base_name} ({suffix})");
                                    suffix += 1;
                                }
                                prof.name = unique_name.clone();
                                this.config.profiles.push(prof);
                                this.config.active_profile = unique_name.clone();
                                this.sync_picker_hsv();
                                this.commit_config(cx);
                                this.set_status_notice(
                                    format!("Imported profile '{unique_name}'"),
                                    Some(NOTICE_TTL),
                                );
                                cx.notify();
                            },
                        )),
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
            let name_dup = p.name.clone();
            let name_rst = p.name.clone();
            let prof_for_copy = p.clone();

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
                                .child({
                                    let n = p.settings.button_map.custom_count();
                                    if n == 0 {
                                        "Default buttons".to_owned()
                                    } else {
                                        format!("{n} remapped")
                                    }
                                }),
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

            let mut right_acts = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    subtle_button(SharedString::from(format!("pc-dup-{idx}")), "Duplicate")
                        .tooltip(text_tooltip("Duplicate profile"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let Some(src) = this
                                .config
                                .profiles
                                .iter()
                                .find(|pr| pr.name == name_dup)
                                .cloned()
                            else {
                                return;
                            };
                            let mut n = 2usize;
                            let mut cand = format!("{} ({n})", src.name);
                            while this.config.profiles.iter().any(|pr| pr.name == cand) {
                                n += 1;
                                cand = format!("{} ({n})", src.name);
                            }
                            this.config.profiles.push(NamedProfile {
                                name: cand.clone(),
                                linked_exe: None,
                                settings: src.settings,
                            });
                            this.config.active_profile = cand;
                            this.sync_picker_hsv();
                            this.commit_config(cx);
                        })),
                )
                .child(
                    subtle_button(SharedString::from(format!("pc-copy-{idx}")), "Copy TOML")
                        .tooltip(text_tooltip("Copy profile TOML to clipboard"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            match toml::to_string_pretty(&prof_for_copy) {
                                Ok(text) => {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                    this.set_status_notice(
                                        format!("Copied '{}' TOML to clipboard", prof_for_copy.name),
                                        Some(NOTICE_TTL),
                                    );
                                }
                                Err(e) => {
                                    this.set_status_notice(
                                        format!("Export failed: {e}"),
                                        Some(NOTICE_ERROR_TTL),
                                    );
                                }
                            }
                            cx.notify();
                        })),
                )
                .child(
                    subtle_button(SharedString::from(format!("pc-rst-{idx}")), "Reset")
                        .tooltip(text_tooltip("Reset profile settings to defaults"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(pr) =
                                this.config.profiles.iter_mut().find(|pr| pr.name == name_rst)
                            {
                                pr.settings = crate::engine::ControllerProfile::default();
                            }
                            this.sync_picker_hsv();
                            this.commit_config(cx);
                        })),
                );
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
                                        this.clear_status_notice();
                                    }
                                    Err(e) => {
                                        this.set_status_notice(
                                            format!("Startup registry error: {e}"),
                                            Some(NOTICE_ERROR_TTL),
                                        );
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
            Some(status_info_pill(msg))
        } else if all_drivers_ok {
            None
        } else {
            Some(status_info_pill(
                "Click Install on any missing driver to run its official installer",
            ))
        };

        let mut drivers_card = section_card_with_action(
            "System drivers",
            subtle_button("set-recheck-drv", "Recheck").on_click(cx.listener(|this, _, _, cx| {
                this.recheck_drivers();
                this.sync_tray_state();
                this.set_status_notice("Driver status refreshed", Some(NOTICE_TTL));
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
        );
        if let Some(footer) = drivers_footer {
            drivers_card = drivers_card.child(
                div()
                    .mt_auto()
                    .pt_3()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .child(footer),
            );
        }

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
                                            this.set_status_notice(
                                                format!("Failed to open folder: {e}"),
                                                Some(NOTICE_ERROR_TTL),
                                            );
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
                                        this.set_status_notice(
                                            "Configuration reloaded",
                                            Some(NOTICE_TTL),
                                        );
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
                                this.set_status_notice(
                                    "Settings reset to defaults",
                                    Some(NOTICE_TTL),
                                );
                                cx.notify();
                            },
                        )),
                    ),
            );

        let checking_updates = self.update_check.is_some();
        let active_upd_phase = self.update_install_label();
        let updates_busy = checking_updates || active_upd_phase.is_some();

        let check_btn = if updates_busy {
            div()
                .h(px(32.0))
                .px_3()
                .rounded(px(8.0))
                .border_1()
                .border_color(color(BORDER))
                .bg(color(SURFACE_ALT))
                .flex()
                .items_center()
                .text_size(px(13.0))
                .text_color(color(TEXT_MUTED))
                .whitespace_nowrap()
                .child(if checking_updates {
                    "Checking…"
                } else {
                    active_upd_phase.unwrap_or("Updating…")
                })
                .into_any_element()
        } else {
            subtle_button("set-check-updates", "Check now")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.start_update_check(true, cx);
                }))
                .into_any_element()
        };

        let mut version_badges = div().flex().items_center().gap_2();
        if let Some(msg) = self.update_status_text.as_ref() {
            let status_pill = if self.available_update.is_none() && msg.starts_with("Up to date") {
                status_inset_pill(msg.clone(), true)
            } else {
                status_info_pill(msg.clone())
            };
            version_badges = version_badges.child(status_pill);
        }
        version_badges = version_badges.child(
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
                .text_color(color(TEXT_SECONDARY))
                .child(format!("v{CURRENT_VERSION}")),
        );
        if let Some(rel) = self.available_update.as_ref() {
            version_badges = version_badges.child(
                div()
                    .h(px(28.0))
                    .px_3()
                    .rounded(px(6.0))
                    .bg(color(SURFACE_ALT))
                    .border_1()
                    .border_color(color(BORDER_STRONG))
                    .flex()
                    .items_center()
                    .text_size(px(13.0))
                    .font_family(FONT_MONO)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color(ACCENT))
                    .child(format!("→ v{}", rel.version)),
            );
        }

        let mut updates_card = section_card_with_action("Updates", check_btn).child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(setting_row(
                    "Check for updates on startup",
                    "Quietly check GitHub Releases for new DSMaster versions",
                    toggle_switch("set-auto-upd-sw", self.config.auto_check_updates).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.config.auto_check_updates = !this.config.auto_check_updates;
                            this.commit_config(cx);
                        }),
                    ),
                ))
                .child(setting_row(
                    "Installed version",
                    "Source: GitHub Releases (tassak-dozu/DSMaster)",
                    version_badges,
                )),
        );

        if let Some(rel) = self.available_update.as_ref() {
            let release_url = rel.release_url.clone();
            let install_ctl = if let Some(label) = active_upd_phase {
                div()
                    .h(px(32.0))
                    .px_3()
                    .rounded(px(8.0))
                    .bg(color(SURFACE_ALT))
                    .border_1()
                    .border_color(color(BORDER_STRONG))
                    .flex()
                    .items_center()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color(ACCENT))
                    .whitespace_nowrap()
                    .child(label)
                    .into_any_element()
            } else {
                div()
                    .id("set-install-update")
                    .h(px(32.0))
                    .px_3()
                    .rounded(px(8.0))
                    .bg(color(ACCENT_SOLID))
                    .border_1()
                    .border_color(color(ACCENT_SOLID))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color(ACCENT_FG))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|s| {
                        s.bg(color(ACCENT_SOLID_HOVER))
                            .border_color(color(ACCENT_SOLID_HOVER))
                    })
                    .child("Update & restart")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_update_install(cx);
                    }))
                    .into_any_element()
            };

            updates_card = updates_card.child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        subtle_button("set-release-notes", "Release notes").on_click(cx.listener(
                            move |this, _, _, cx| {
                                if let Err(err) = open_release_url(&release_url) {
                                    this.set_status_notice(err, Some(NOTICE_ERROR_TTL));
                                    cx.notify();
                                }
                            },
                        )),
                    )
                    .child(install_ctl),
            );
        }

        let notif_on = self.config.notifications_enabled;
        let notif_conn_on = self.config.notify_connect_disconnect;
        let notif_low_on = self.config.notify_low_battery;

        let power_notif_card = section_card("Notifications")
            .flex_1()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(setting_row(
                        "Desktop notifications",
                        "Show native Windows notifications from the system tray",
                        toggle_switch("set-notif-all-sw", notif_on).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.config.notifications_enabled =
                                    !this.config.notifications_enabled;
                                this.commit_config(cx);
                            },
                        )),
                    ))
                    .child(setting_row(
                        "Connect & disconnect alerts",
                        "Notify when a DualSense connects or disconnects",
                        toggle_switch("set-notif-conn-sw", notif_on && notif_conn_on).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.config.notify_connect_disconnect =
                                    !this.config.notify_connect_disconnect;
                                if this.config.notify_connect_disconnect {
                                    this.config.notifications_enabled = true;
                                }
                                this.commit_config(cx);
                            }),
                        ),
                    ))
                    .child(setting_row(
                        "Low battery alerts",
                        "Warn when battery drops to your low-battery threshold",
                        toggle_switch("set-notif-low-sw", notif_on && notif_low_on).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.config.notify_low_battery = !this.config.notify_low_battery;
                                if this.config.notify_low_battery {
                                    this.config.notifications_enabled = true;
                                }
                                this.commit_config(cx);
                            }),
                        ),
                    )),
            );

        let dsu_on = self.config.dsu_enabled;
        let dsx_on = self.config.dsx_udp_enabled;
        let dsu_status = self
            .engine
            .as_ref()
            .map(Engine::dsu_status)
            .unwrap_or_default();
        let dsx_status = self
            .engine
            .as_ref()
            .map(Engine::dsx_udp_status)
            .unwrap_or_default();

        let dsu_status_text = if !dsu_on {
            "Off".to_owned()
        } else if let Some(err) = dsu_status.last_error.as_ref() {
            err.clone()
        } else if dsu_status.running {
            format!(
                "Listening 127.0.0.1:{} · {} client{}",
                dsu_status.port,
                dsu_status.subscriber_count,
                if dsu_status.subscriber_count == 1 {
                    ""
                } else {
                    "s"
                }
            )
        } else {
            format!("Starting on :{}", self.config.dsu_port)
        };

        let dsx_status_text = if !dsx_on {
            "Off".to_owned()
        } else if let Some(err) = dsx_status.last_error.as_ref() {
            err.clone()
        } else if dsx_status.running {
            let state_label = if dsx_status.override_active {
                "Mod override active"
            } else {
                "Waiting for mods"
            };
            format!(
                "Listening 127.0.0.1:{} · {state_label} ({} pkts)",
                dsx_status.port, dsx_status.packets_received
            )
        } else {
            format!("Starting on :{}", self.config.dsx_udp_port)
        };

        let mut integrations_card = section_card("Integrations (CemuHook & DSX Mods)")
            .flex_1()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(setting_row(
                        "CemuHook / DSU server",
                        "Stream calibrated gyro, accel & touchpad to emulators (Cemu, Dolphin, Ryujinx, RPCS3)",
                        toggle_switch("set-dsu-sw", dsu_on).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.config.dsu_enabled = !this.config.dsu_enabled;
                                this.commit_config(cx);
                            },
                        )),
                    ))
                    .child(if dsu_on && dsu_status.running && dsu_status.last_error.is_none() {
                        status_inset_pill(dsu_status_text, true)
                    } else {
                        status_info_pill(dsu_status_text)
                    })
                    .child(
                        div()
                            .pt_2()
                            .border_t_1()
                            .border_color(color(BORDER))
                            .child(setting_row(
                                "DSX UDP mod server",
                                "Listen on DSX's UDP port (6969) so game mods can drive adaptive triggers & LEDs",
                                toggle_switch("set-dsx-udp-sw", dsx_on).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.config.dsx_udp_enabled = !this.config.dsx_udp_enabled;
                                        this.commit_config(cx);
                                    },
                                )),
                            )),
                    )
                    .child(if dsx_on && dsx_status.running && dsx_status.last_error.is_none() {
                        status_inset_pill(dsx_status_text, true)
                    } else {
                        status_info_pill(dsx_status_text)
                    }),
            );

        if dsx_on && dsx_status.override_active {
            integrations_card = integrations_card.child(
                div()
                    .mt_auto()
                    .pt_3()
                    .border_t_1()
                    .border_color(color(BORDER))
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(
                        subtle_button("set-dsx-reset", "Reset mod override").on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(engine) = &this.engine {
                                    engine.clear_dsx_override();
                                }
                                this.set_status_notice(
                                    "Cleared DSX mod override",
                                    Some(NOTICE_TTL),
                                );
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }

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
                                "Application startup, notifications, integrations, system drivers, and local configuration",
                            ),
                    ),
            )
            .child(div().flex().gap_4().child(startup_card).child(drivers_card))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(power_notif_card)
                    .child(integrations_card),
            )
            .child(updates_card)
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
        if !is_hidden
            && !self.handles.is_empty()
            && (self.page.shows_controller_stage() || self.page == ActivePage::DeviceInfo)
        {
            window.request_animation_frame();
        }

        let titlebar = self.render_titlebar();
        let icon_rail = self.render_icon_rail(cx);

        let page_content = match self.page {
            ActivePage::QuickSettings => self.render_quick_settings(cx),
            ActivePage::Profiles => self.render_profiles_page(cx),
            ActivePage::Sticks => self.render_sticks_page(cx),
            ActivePage::Triggers => self.render_triggers_page(cx),
            ActivePage::Audio => self.render_audio_page(cx),
            ActivePage::Lighting => self.render_lighting_page(cx),
            ActivePage::MotionRemap => self.render_motion_remap_page(cx),
            ActivePage::DeviceInfo => self.render_device_info_page(cx),
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

        if let Some(banner) = self.render_update_banner(cx) {
            canvas_inner = canvas_inner.child(banner);
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

        if self.drawer_open && self.page.is_controller_section() {
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
