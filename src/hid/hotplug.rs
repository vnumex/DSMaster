use std::ffi::c_void;

use crossbeam_channel::{Receiver, Sender, bounded};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
    CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER,
    CM_NOTIFY_FILTER_0, CM_NOTIFY_FILTER_0_0, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
    CM_Register_Notification, CM_Unregister_Notification, CR_SUCCESS, HCMNOTIFICATION,
};
use windows::Win32::Devices::HumanInterfaceDevice::HidD_GetHidGuid;

use super::HidError;
use super::enumerate::wide_to_string;

#[derive(Debug, Clone)]
pub enum HotPlugEvent {
    Arrived(String),
    Removed(String),
}

struct CallbackContext {
    tx: Sender<HotPlugEvent>,
}

pub struct HotPlugMonitor {
    handle: HCMNOTIFICATION,
    _ctx: Box<CallbackContext>,
    rx: Receiver<HotPlugEvent>,
}

unsafe impl Send for HotPlugMonitor {}

impl HotPlugMonitor {
    pub fn new() -> Result<Self, HidError> {
        let (tx, rx) = bounded(64);
        let ctx = Box::new(CallbackContext { tx });
        let ctx_ptr = &*ctx as *const CallbackContext as *const c_void;

        let hid_guid = unsafe { HidD_GetHidGuid() };
        let filter = CM_NOTIFY_FILTER {
            cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
            Flags: 0,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            Reserved: 0,
            u: CM_NOTIFY_FILTER_0 {
                DeviceInterface: CM_NOTIFY_FILTER_0_0 {
                    ClassGuid: hid_guid,
                },
            },
        };

        let mut handle = HCMNOTIFICATION::default();
        let cr = unsafe {
            CM_Register_Notification(&filter, Some(ctx_ptr), Some(pnp_callback), &mut handle)
        };
        if cr != CR_SUCCESS {
            return Err(HidError::ConfigManager("CM_Register_Notification", cr.0));
        }

        Ok(Self {
            handle,
            _ctx: ctx,
            rx,
        })
    }

    #[inline]
    pub fn receiver(&self) -> &Receiver<HotPlugEvent> {
        &self.rx
    }
}

impl Drop for HotPlugMonitor {
    fn drop(&mut self) {
        unsafe {
            // Waits for any in-flight callback to complete before `_ctx` is dropped.
            let _ = CM_Unregister_Notification(self.handle);
        }
    }
}

fn should_forward_hotplug_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if !lower.contains("vid_054c") && !lower.contains("vid&0002054c") {
        return false;
    }
    !super::enumerate::is_excluded_virtual_identifier(path)
}

unsafe extern "system" fn pnp_callback(
    _hnotify: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    eventdata: *const CM_NOTIFY_EVENT_DATA,
    eventdatasize: u32,
) -> u32 {
    if context.is_null() || eventdata.is_null() {
        return 0;
    }
    if action != CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL
        && action != CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL
    {
        return 0;
    }
    if (eventdatasize as usize) < size_of::<CM_NOTIFY_EVENT_DATA>() {
        return 0;
    }

    let ctx = unsafe { &*(context as *const CallbackContext) };
    let data = unsafe { &*eventdata };
    let base_ptr = data as *const CM_NOTIFY_EVENT_DATA as *const u8;
    let link_offset = std::mem::offset_of!(CM_NOTIFY_EVENT_DATA, u);
    // DeviceInterface.SymbolicLink starts at `u + 16` (after ClassGuid)
    let sym_offset = link_offset + 16;
    if (eventdatasize as usize) <= sym_offset {
        return 0;
    }
    let sym_bytes = (eventdatasize as usize) - sym_offset;
    let sym_u16 = unsafe {
        std::slice::from_raw_parts(base_ptr.add(sym_offset) as *const u16, sym_bytes / 2)
    };
    let path = wide_to_string(sym_u16);
    if !should_forward_hotplug_path(&path) {
        return 0;
    }

    let ev = if action == CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL {
        HotPlugEvent::Arrived(path)
    } else {
        HotPlugEvent::Removed(path)
    };
    let _ = ctx.tx.try_send(ev);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::enumerate::{register_virtual_serial, unregister_virtual_serial};

    #[test]
    fn hotplug_filter_accepts_physical_and_rejects_virtual_dualsense() {
        assert!(should_forward_hotplug_path(
            r"\\?\hid#vid_054c&pid_0ce6&mi_03#8&11403f6e&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        ));
        assert!(should_forward_hotplug_path(
            r"\\?\hid#{00001124-0000-1000-8000-00805f9b34fb}_vid&0002054c_pid&0ce6#9&1234&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        ));
        assert!(!should_forward_hotplug_path(
            r"\\?\hid#vid_054c&pid_0ce6#dsmaster-vds-01#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        ));
        assert!(!should_forward_hotplug_path(
            r"\\?\hid#usbip_vid_054c&pid_0ce6#7&1234#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        ));

        let dyn_serial = "HOTPLUG-DYN-VDS-99";
        register_virtual_serial(dyn_serial);
        assert!(!should_forward_hotplug_path(
            r"\\?\hid#vid_054c&pid_0ce6#hotplug-dyn-vds-99#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        ));
        unregister_virtual_serial(dyn_serial);
    }
}
