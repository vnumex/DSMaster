pub mod app_watcher;
pub mod mouse;
pub mod processing;
pub mod profile;
pub mod profiles_store;
pub mod seqlock;
pub mod session;
pub mod telemetry;

pub use profile::ControllerProfile;
pub use profiles_store::AppConfig;
pub use seqlock::SeqLock;
pub use telemetry::TelemetrySnapshot;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use arc_swap::ArcSwap;
use crossbeam_channel::{Sender, bounded, select};
use parking_lot::Mutex;
use thiserror::Error;
use tracing::{info, warn};

use self::app_watcher::ForegroundAppWatcher;
use self::session::ControllerSessionHandle;
use crate::cloak::HidHideManager;
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
    last_fg_exe: Arc<Mutex<Option<String>>>,
    topology_version: Arc<AtomicU64>,
    stop_tx: Option<Sender<()>>,
    manager_thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(initial_config: AppConfig) -> Result<Self, EngineError> {
        let initial_profile = initial_config.active_settings();
        let cloak_flag = initial_config.cloak_physical;
        let config = Arc::new(Mutex::new(initial_config));
        let profile = Arc::new(ArcSwap::from_pointee(initial_profile));
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        let hidhide = Arc::new(HidHideManager::new());
        let cloak_enabled = Arc::new(AtomicBool::new(cloak_flag));
        let last_fg_exe = Arc::new(Mutex::new(None));
        let topology_version = Arc::new(AtomicU64::new(1));

        let vigem = match VigemClient::connect() {
            Ok(c) => Some(c),
            Err(e) => {
                warn!(error = %e, "ViGEmBus unavailable; running without virtual controller output");
                None
            }
        };

        let initial_devices = enumerate_dualsense_devices()?;
        for info in initial_devices {
            try_attach_device(info, &vigem, &profile, &sessions, &topology_version);
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
        let fg_c = Arc::clone(&last_fg_exe);
        let topo_c = Arc::clone(&topology_version);

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
                                            try_attach_device(info, &vigem, &profile_c, &sessions_c, &topo_c);
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
            last_fg_exe,
            topology_version,
            stop_tx: Some(stop_tx),
            manager_thread: Some(manager_thread),
        })
    }

    pub fn apply_config(&self, next: &AppConfig) {
        *self.config.lock() = next.clone();
        self.profile.store(Arc::new(next.active_settings()));
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

fn try_attach_device(
    info: DeviceInfo,
    vigem: &Option<VigemClient>,
    profile: &Arc<ArcSwap<ControllerProfile>>,
    sessions: &Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
    topology_version: &Arc<AtomicU64>,
) {
    let key = info.device_path.to_ascii_lowercase();
    match DualSenseDevice::open(info) {
        Ok(device) => match ControllerSessionHandle::spawn(
            device,
            vigem.clone(),
            Arc::clone(profile),
            Arc::clone(topology_version),
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
