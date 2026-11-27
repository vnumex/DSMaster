pub mod app_watcher;
pub mod bt_audio;
pub mod dsu;
pub mod dsx_udp;
pub mod mouse;
pub mod processing;
pub mod profile;
pub mod profiles_store;
pub mod seqlock;
pub mod session;
pub mod telemetry;

pub use dsu::{DsuServer, DsuStatus};
pub use dsx_udp::{DsxOverrideState, DsxUdpServer, DsxUdpStatus};
pub use profile::ControllerProfile;
pub use profiles_store::AppConfig;
pub use seqlock::SeqLock;
pub use telemetry::TelemetrySnapshot;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use arc_swap::ArcSwap;
use crossbeam_channel::{Sender, bounded, select};
use parking_lot::Mutex;
use thiserror::Error;
use tracing::{info, warn};

use self::app_watcher::ForegroundAppWatcher;
use self::bt_audio::BtAudioTestSignal;
use self::session::{ControllerSessionHandle, TestRumbleSignal};
use crate::cloak::HidHideManager;
use crate::emu::EmuBackends;
use crate::emu::vigem::VigemClient;
use crate::hid::{
    DeviceInfo, DualSenseDevice, HotPlugEvent, HotPlugMonitor, enumerate_dualsense_devices,
    inspect_hid_path,
};

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("HID error: {0}")]
    Hid(#[from] crate::hid::HidError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct Engine {
    config: Arc<Mutex<AppConfig>>,
    profile: Arc<ArcSwap<ControllerProfile>>,
    sessions: Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
    hidhide: Arc<HidHideManager>,
    cloak_enabled: Arc<AtomicBool>,
    bt_idle_disconnect_mins: Arc<AtomicU16>,
    dsx_override: Arc<ArcSwap<DsxOverrideState>>,
    dsu_server: Mutex<Option<DsuServer>>,
    dsx_server: Mutex<Option<DsxUdpServer>>,
    last_fg_exe: Arc<Mutex<Option<String>>>,
    topology_version: Arc<AtomicU64>,
    test_rumble: Arc<TestRumbleSignal>,
    bt_audio_test: Arc<BtAudioTestSignal>,
    stop_tx: Option<Sender<()>>,
    manager_thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(initial_config: AppConfig) -> Result<Self, EngineError> {
        let initial_profile = initial_config.active_settings();
        let cloak_flag = initial_config.cloak_physical;
        let initial_bt_idle_mins = initial_config.bt_idle_disconnect_mins;
        let initial_dsu = (initial_config.dsu_enabled, initial_config.dsu_port);
        let initial_dsx = (initial_config.dsx_udp_enabled, initial_config.dsx_udp_port);
        let config = Arc::new(Mutex::new(initial_config));
        let profile = Arc::new(ArcSwap::from_pointee(initial_profile));
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        let hidhide = Arc::new(HidHideManager::new());
        let cloak_enabled = Arc::new(AtomicBool::new(cloak_flag));
        let bt_idle_disconnect_mins = Arc::new(AtomicU16::new(initial_bt_idle_mins));
        let dsx_override = Arc::new(ArcSwap::from_pointee(DsxOverrideState::default()));
        let last_fg_exe = Arc::new(Mutex::new(None));
        let topology_version = Arc::new(AtomicU64::new(1));
        let test_rumble = Arc::new(TestRumbleSignal::default());
        let bt_audio_test = Arc::new(BtAudioTestSignal::default());

        let dsu_server = Mutex::new(if initial_dsu.0 {
            Some(DsuServer::start(initial_dsu.1, Arc::clone(&sessions)))
        } else {
            None
        });
        let dsx_server = Mutex::new(if initial_dsx.0 {
            Some(DsxUdpServer::start(
                initial_dsx.1,
                Arc::clone(&dsx_override),
                Arc::clone(&sessions),
            ))
        } else {
            None
        });

        let vigem = match VigemClient::connect() {
            Ok(c) => Some(c),
            Err(e) => {
                warn!(error = %e, "ViGEmBus unavailable; Xbox 360 / DualShock 4 emulation unavailable");
                None
            }
        };
        let backends = EmuBackends { vigem };

        let initial_devices = enumerate_dualsense_devices()?;
        for info in initial_devices {
            try_attach_device(
                info,
                &backends,
                &profile,
                &sessions,
                &topology_version,
                &test_rumble,
                &bt_audio_test,
                &cloak_enabled,
                &bt_idle_disconnect_mins,
                &dsx_override,
            );
        }
        if cloak_flag {
            let ids = collect_instance_ids(&sessions);
            if let Err(e) = hidhide.sync_cloaked(true, &ids) {
                warn!(error = %e, "Initial HidHide cloak sync failed");
            }
        }

        let hotplug = HotPlugMonitor::new()?;
        let watcher = ForegroundAppWatcher::start();
        let (stop_tx, stop_rx) = bounded::<()>(1);

        let sessions_c = Arc::clone(&sessions);
        let profile_c = Arc::clone(&profile);
        let config_c = Arc::clone(&config);
        let hidhide_c = Arc::clone(&hidhide);
        let cloak_c = Arc::clone(&cloak_enabled);
        let bt_idle_c = Arc::clone(&bt_idle_disconnect_mins);
        let dsx_override_c = Arc::clone(&dsx_override);
        let fg_c = Arc::clone(&last_fg_exe);
        let topo_c = Arc::clone(&topology_version);
        let test_rumble_c = Arc::clone(&test_rumble);
        let bt_audio_test_c = Arc::clone(&bt_audio_test);

        let manager_thread = thread::Builder::new()
            .name("ds-mgr".to_owned())
            .spawn(move || {
                let hp_rx = hotplug.receiver();
                let fg_rx = watcher.receiver();
                loop {
                    select! {
                        recv(stop_rx) -> _ => break,
                        recv(hp_rx) -> msg => {
                            let Ok(ev) = msg else { break; };
                            match ev {
                                HotPlugEvent::Arrived(path) => {
                                    let key = path.to_ascii_lowercase();
                                    if !sessions_c.lock().contains_key(&key) {
                                        if let Some(info) = inspect_hid_path(&path) {
                                            try_attach_device(
                                                info,
                                                &backends,
                                                &profile_c,
                                                &sessions_c,
                                                &topo_c,
                                                &test_rumble_c,
                                                &bt_audio_test_c,
                                                &cloak_c,
                                                &bt_idle_c,
                                                &dsx_override_c,
                                            );
                                        }
                                    }
                                }
                                HotPlugEvent::Removed(path) => {
                                    let key = path.to_ascii_lowercase();
                                    if let Some(mut handle) = sessions_c.lock().remove(&key) {
                                        handle.stop();
                                        topo_c.fetch_add(1, Ordering::Release);
                                    }
                                }
                            }
                            {
                                let mut guard = sessions_c.lock();
                                let before = guard.len();
                                guard.retain(|_, h| !h.is_finished());
                                if guard.len() != before {
                                    topo_c.fetch_add(1, Ordering::Release);
                                }
                            }
                            if cloak_c.load(Ordering::Acquire) {
                                let ids = collect_instance_ids(&sessions_c);
                                let _ = hidhide_c.sync_cloaked(true, &ids);
                            }
                        }
                        recv(fg_rx) -> msg => {
                            let Ok(exe) = msg else { continue; };
                            if exe.eq_ignore_ascii_case("dsmaster.exe") {
                                continue;
                            }
                            *fg_c.lock() = Some(exe.clone());
                            let mut cfg = config_c.lock();
                            if cfg.auto_switch_profiles {
                                if let Some(found) = cfg.profiles.iter().find(|p| {
                                    p.linked_exe
                                        .as_deref()
                                        .is_some_and(|e| e.eq_ignore_ascii_case(&exe))
                                }) {
                                    let next_name = found.name.clone();
                                    let next_settings = found.settings;
                                    if cfg.active_profile != next_name {
                                        cfg.active_profile = next_name;
                                        profile_c.store(Arc::new(next_settings));
                                        topo_c.fetch_add(1, Ordering::Release);
                                    }
                                }
                            }
                        }
                    }
                }
            })?;

        Ok(Self {
            config,
            profile,
            sessions,
            hidhide,
            cloak_enabled,
            bt_idle_disconnect_mins,
            dsx_override,
            dsu_server,
            dsx_server,
            last_fg_exe,
            topology_version,
            test_rumble,
            bt_audio_test,
            stop_tx: Some(stop_tx),
            manager_thread: Some(manager_thread),
        })
    }

    pub fn trigger_test_rumble(&self, large: u8, small: u8, duration_ms: u16) {
        self.test_rumble.trigger(large, small, duration_ms);
    }

    pub fn trigger_test_bt_speaker(&self) {
        self.bt_audio_test.trigger_speaker();
    }

    pub fn trigger_test_bt_haptics(&self) {
        self.bt_audio_test.trigger_haptics();
    }

    pub fn apply_config(&self, next: &AppConfig) {
        *self.config.lock() = next.clone();
        self.profile.store(Arc::new(next.active_settings()));
        self.bt_idle_disconnect_mins
            .store(next.bt_idle_disconnect_mins, Ordering::Release);
        self.sync_dsu_server(next.dsu_enabled, next.dsu_port);
        self.sync_dsx_server(next.dsx_udp_enabled, next.dsx_udp_port);
    }

    fn sync_dsu_server(&self, enabled: bool, port: u16) {
        let mut guard = self.dsu_server.lock();
        match (enabled, guard.as_ref().map(|s| s.status().port)) {
            (true, Some(cur_port)) if cur_port == port => {}
            (true, _) => {
                *guard = None;
                *guard = Some(DsuServer::start(port, Arc::clone(&self.sessions)));
            }
            (false, _) => {
                *guard = None;
            }
        }
    }

    fn sync_dsx_server(&self, enabled: bool, port: u16) {
        let mut guard = self.dsx_server.lock();
        match (enabled, guard.as_ref().map(|s| s.status().port)) {
            (true, Some(cur_port)) if cur_port == port => {}
            (true, _) => {
                *guard = None;
                *guard = Some(DsxUdpServer::start(
                    port,
                    Arc::clone(&self.dsx_override),
                    Arc::clone(&self.sessions),
                ));
            }
            (false, _) => {
                *guard = None;
            }
        }
    }

    pub fn dsu_status(&self) -> DsuStatus {
        self.dsu_server
            .lock()
            .as_ref()
            .map(DsuServer::status)
            .unwrap_or_default()
    }

    pub fn dsx_udp_status(&self) -> DsxUdpStatus {
        self.dsx_server
            .lock()
            .as_ref()
            .map(DsxUdpServer::status)
            .unwrap_or_default()
    }

    pub fn clear_dsx_override(&self) {
        if let Some(srv) = self.dsx_server.lock().as_ref() {
            srv.clear_override();
        } else {
            let prev = **self.dsx_override.load();
            self.dsx_override.store(Arc::new(DsxOverrideState {
                active: false,
                generation: prev.generation.wrapping_add(1),
                ..Default::default()
            }));
        }
    }

    pub fn disconnect_bluetooth_mac(&self, mac: [u8; 6]) -> Result<(), String> {
        crate::hid::disconnect_bluetooth_device(mac)
    }

    pub fn active_profile_name(&self) -> String {
        self.config.lock().active_profile.clone()
    }

    pub fn last_foreground_exe(&self) -> Option<String> {
        self.last_fg_exe.lock().clone()
    }

    pub fn is_cloak_enabled(&self) -> bool {
        self.cloak_enabled.load(Ordering::Acquire)
    }

    pub fn set_cloak_enabled(&self, enabled: bool) -> Result<(), String> {
        let ids = collect_instance_ids(&self.sessions);
        self.hidhide
            .sync_cloaked(enabled, &ids)
            .map_err(|e| e.to_string())?;
        self.cloak_enabled.store(enabled, Ordering::Release);
        self.config.lock().cloak_physical = enabled;
        Ok(())
    }

    #[inline]
    pub fn topology_version(&self) -> u64 {
        self.topology_version.load(Ordering::Acquire)
    }

    pub fn telemetry_handles(&self) -> Vec<Arc<SeqLock<TelemetrySnapshot>>> {
        let mut guard = self.sessions.lock();
        guard.retain(|_, h| !h.is_finished());
        guard.values().map(|h| Arc::clone(&h.telemetry)).collect()
    }

    pub fn shutdown(&mut self) {
        *self.dsu_server.lock() = None;
        *self.dsx_server.lock() = None;
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.manager_thread.take() {
            let _ = h.join();
        }
        let mut guard = self.sessions.lock();
        for (_, mut session) in guard.drain() {
            session.stop();
        }
        let _ = self.hidhide.sync_cloaked(false, &[]);
        self.topology_version.fetch_add(1, Ordering::Release);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn collect_instance_ids(
    sessions: &Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
) -> Vec<String> {
    sessions
        .lock()
        .values()
        .map(|h| h.instance_id.clone())
        .filter(|id| !id.is_empty())
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn try_attach_device(
    info: DeviceInfo,
    backends: &EmuBackends,
    profile: &Arc<ArcSwap<ControllerProfile>>,
    sessions: &Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
    topology_version: &Arc<AtomicU64>,
    test_rumble: &Arc<TestRumbleSignal>,
    bt_audio_test: &Arc<BtAudioTestSignal>,
    cloak_enabled: &Arc<AtomicBool>,
    bt_idle_disconnect_mins: &Arc<AtomicU16>,
    dsx_override: &Arc<ArcSwap<DsxOverrideState>>,
) {
    let key = info.device_path.to_ascii_lowercase();
    match DualSenseDevice::open(info) {
        Ok(device) => match ControllerSessionHandle::spawn(
            device,
            backends.clone(),
            Arc::clone(profile),
            Arc::clone(topology_version),
            Arc::clone(test_rumble),
            Arc::clone(bt_audio_test),
            Arc::clone(cloak_enabled),
            Arc::clone(bt_idle_disconnect_mins),
            Arc::clone(dsx_override),
        ) {
            Ok(handle) => {
                sessions.lock().insert(key, handle);
                topology_version.fetch_add(1, Ordering::Release);
            }
            Err(e) => warn!(error = %e, "Failed to spawn controller session"),
        },
        Err(e) => info!(error = %e, "Skipping DualSense device open"),
    }
}
