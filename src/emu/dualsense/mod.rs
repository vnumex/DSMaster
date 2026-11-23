//! Virtual Sony DualSense emulation backend over clean-room USB/IP v1.1.1 + user-installed `usbip-win2`.

pub mod driver;
pub mod usbip;

use std::sync::{Arc, OnceLock, Weak};

use parking_lot::Mutex;
use tracing::{debug, warn};

pub use driver::is_usbip_win2_installed;
pub use usbip::{UsbipServer, VirtualDualSenseConfig, VirtualDualSenseSlot};

use crate::hid::enumerate::{register_virtual_serial, unregister_virtual_serial};
use crate::protocol::constants::DS_INPUT_REPORT_USB_SIZE;
use crate::protocol::output::ParsedOutputReport;

static SHARED_SERVER: OnceLock<Mutex<Weak<UsbipServer>>> = OnceLock::new();

fn acquire_shared_server() -> Result<Arc<UsbipServer>, String> {
    let cell = SHARED_SERVER.get_or_init(|| Mutex::new(Weak::new()));
    let mut guard = cell.lock();
    if let Some(existing) = guard.upgrade() {
        return Ok(existing);
    }
    let created = Arc::new(
        UsbipServer::bind_default_or_ephemeral()
            .map_err(|e| format!("failed to bind USB/IP loopback server: {e}"))?,
    );
    *guard = Arc::downgrade(&created);
    Ok(created)
}

/// Active Virtual DualSense target attached to Windows via `usbip-win2`.
///
/// Automatically detaches the `usbip-win2` VHCI port, removes the USB/IP slot, and unregisters
/// its virtual serial tag on [`Drop`] so no ghost virtual USB devices remain.
pub struct UsbipDualSenseTarget {
    server: Arc<UsbipServer>,
    slot: Arc<VirtualDualSenseSlot>,
    vhci_port: Option<u32>,
    serial_tag: String,
}

impl UsbipDualSenseTarget {
    /// Spawns a new Virtual DualSense device on the shared USB/IP loopback server and attaches it
    /// to Windows via `usbip.exe` (`vadimgrn/usbip-win2`).
    pub fn spawn(config: VirtualDualSenseConfig) -> Result<Self, String> {
        if !is_usbip_win2_installed() {
            return Err(
                "usbip-win2 is not installed; install vadimgrn/usbip-win2 to enable Virtual DualSense"
                    .to_string(),
            );
        }

        let server = acquire_shared_server()?;
        let serial_tag = config.serial_tag.clone();
        let slot = server.add_slot(config);

        // Register virtual serial BEFORE attaching so DSMaster's hot-plug listener and
        // HidHide cloak sync immediately ignore this virtual DualSense upon PnP arrival.
        register_virtual_serial(&serial_tag);

        match driver::attach_local(server.port(), slot.bus_id()) {
            Ok(vhci_port) => {
                debug!(
                    tcp_port = server.port(),
                    bus_id = %slot.bus_id(),
                    vhci_port,
                    serial = %serial_tag,
                    "spawned virtual DualSense target"
                );
                Ok(Self {
                    server,
                    slot,
                    vhci_port: Some(vhci_port),
                    serial_tag,
                })
            }
            Err(err) => {
                server.remove_slot(slot.bus_id());
                unregister_virtual_serial(&serial_tag);
                Err(err)
            }
        }
    }

    /// Creates a target bound to a specific [`UsbipServer`] without invoking `usbip.exe`
    /// (useful for loopback unit tests and dry-run harnesses).
    #[cfg(test)]
    pub(crate) fn from_server_detached(
        server: Arc<UsbipServer>,
        config: VirtualDualSenseConfig,
    ) -> Self {
        let serial_tag = config.serial_tag.clone();
        let slot = server.add_slot(config);
        register_virtual_serial(&serial_tag);
        Self {
            server,
            slot,
            vhci_port: None,
            serial_tag,
        }
    }

    /// Submits a 64-byte USB DualSense Input Report (`0x01`) to the virtual device.
    #[inline]
    pub fn submit_report(&mut self, report: &[u8; DS_INPUT_REPORT_USB_SIZE]) {
        self.slot.submit_report(report);
    }

    /// Polls and drains coalesced host feedback (`0x02` Output Report: rumble, lightbar,
    /// player LEDs, and raw 11-byte adaptive trigger payloads).
    #[inline]
    pub fn poll_feedback(&mut self) -> Option<ParsedOutputReport> {
        self.slot.poll_feedback()
    }

    /// Returns the USB/IP bus ID assigned to this target (e.g. `"1-1"`).
    #[allow(dead_code)]
    pub fn bus_id(&self) -> &str {
        self.slot.bus_id()
    }

    /// Returns the virtual USB serial tag exposed by this target.
    #[allow(dead_code)]
    pub fn serial_tag(&self) -> &str {
        &self.serial_tag
    }

    /// Returns the `usbip-win2` VHCI port number if attached.
    #[allow(dead_code)]
    pub fn vhci_port(&self) -> Option<u32> {
        self.vhci_port
    }
}

impl Drop for UsbipDualSenseTarget {
    fn drop(&mut self) {
        if let Some(port) = self.vhci_port.take() {
            if let Err(e) = driver::detach_port(port) {
                warn!(vhci_port = port, err = %e, "failed to detach usbip-win2 port on drop");
            }
        }
        self.server.remove_slot(self.slot.bus_id());
        unregister_virtual_serial(&self.serial_tag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::enumerate::is_registered_virtual_serial;
    use crate::protocol::constants::VIRTUAL_SERIAL_PREFIX;

    #[test]
    fn detached_target_registers_and_unregisters_serial_on_drop() {
        let server = Arc::new(UsbipServer::bind(0).unwrap());
        let serial = format!("{VIRTUAL_SERIAL_PREFIX}-TEST-DROP");
        assert!(!is_registered_virtual_serial(&serial));

        {
            let mut target = UsbipDualSenseTarget::from_server_detached(
                Arc::clone(&server),
                VirtualDualSenseConfig {
                    serial_tag: serial.clone(),
                    ..Default::default()
                },
            );
            assert!(is_registered_virtual_serial(&serial));
            assert_eq!(target.bus_id(), "1-1");
            assert_eq!(target.serial_tag(), serial);
            assert_eq!(target.vhci_port(), None);
            target.submit_report(&[0x01; 64]);
            assert_eq!(target.poll_feedback(), None);
        }

        assert!(!is_registered_virtual_serial(&serial));
    }
}
