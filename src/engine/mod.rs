pub mod profile;
pub mod seqlock;
pub mod session;
pub mod telemetry;

pub use profile::ControllerProfile;
pub use seqlock::SeqLock;
pub use telemetry::TelemetrySnapshot;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use arc_swap::ArcSwap;
use crossbeam_channel::{Sender, bounded, select};
use parking_lot::Mutex;
use thiserror::Error;
use tracing::{info, warn};

use self::session::ControllerSessionHandle;
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
    profile: Arc<ArcSwap<ControllerProfile>>,
    sessions: Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
    topology_version: Arc<AtomicU64>,
    stop_tx: Option<Sender<()>>,
    manager_thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(initial_profile: ControllerProfile) -> Result<Self, EngineError> {
        let profile = Arc::new(ArcSwap::from_pointee(initial_profile));
        let sessions = Arc::new(Mutex::new(HashMap::new()));
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

        let hotplug = HotPlugMonitor::new()?;
        let (stop_tx, stop_rx) = bounded::<()>(1);
        let sessions_c = Arc::clone(&sessions);
        let profile_c = Arc::clone(&profile);
        let topo_c = Arc::clone(&topology_version);

        let manager_thread = thread::Builder::new()
            .name("ds-hotplug-mgr".to_owned())
            .spawn(move || {
                let hp_rx = hotplug.receiver();
                loop {
                    select! {
                        recv(stop_rx) -> _ => break,
                        recv(hp_rx) -> msg => {
                            let Ok(ev) = msg else { break; };
                            match ev {
                                HotPlugEvent::Arrived(path) => {
                                    let key = path.to_ascii_lowercase();
                                    if sessions_c.lock().contains_key(&key) {
                                        continue;
                                    }
                                    if let Some(info) = inspect_hid_path(&path) {
                                        try_attach_device(info, &vigem, &profile_c, &sessions_c, &topo_c);
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
                            let mut guard = sessions_c.lock();
                            let before = guard.len();
                            guard.retain(|_, h| !h.is_finished());
                            if guard.len() != before {
                                topo_c.fetch_add(1, Ordering::Release);
                            }
                        }
                    }
                }
            })?;

        Ok(Self {
            profile,
            sessions,
            topology_version,
            stop_tx: Some(stop_tx),
            manager_thread: Some(manager_thread),
        })
    }

    pub fn update_profile(&self, profile: ControllerProfile) {
        self.profile.store(Arc::new(profile));
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
        self.topology_version.fetch_add(1, Ordering::Release);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
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
