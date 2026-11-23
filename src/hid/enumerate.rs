use std::collections::HashSet;
use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::ptr;
use std::sync::OnceLock;

use parking_lot::Mutex;
use tracing::debug;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CM_Get_Device_Interface_PropertyW, CM_Get_Parent, CM_LOCATE_DEVNODE_NORMAL,
    CM_Locate_DevNodeW, CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetHidGuid,
    HidD_GetPreparsedData, HidD_GetSerialNumberString, HidP_GetCaps, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_InstanceId, DEVPROPTYPE};
use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_ITEMS, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::core::{GUID, PCWSTR};

use super::HidError;
use crate::protocol::{DUALSENSE_EDGE_PID, DUALSENSE_PID, SONY_VID, Transport};

const USAGE_PAGE_GENERIC_DESKTOP: u16 = 0x01;
const USAGE_GAMEPAD: u16 = 0x05;

static VIRTUAL_SERIAL_REGISTRY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn virtual_registry() -> &'static Mutex<HashSet<String>> {
    VIRTUAL_SERIAL_REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Registers an active virtual DualSense serial number or PnP instance ID so DSMaster
/// will never open it as a physical controller or add it to HidHide's cloak blacklist.
#[allow(dead_code)]
pub fn register_virtual_serial(serial: &str) {
    let norm = serial.trim().to_ascii_lowercase();
    if norm.is_empty() {
        return;
    }
    virtual_registry().lock().insert(norm);
}

/// Unregisters a previously registered virtual DualSense serial number or instance ID.
#[allow(dead_code)]
pub fn unregister_virtual_serial(serial: &str) {
    let norm = serial.trim().to_ascii_lowercase();
    if norm.is_empty() {
        return;
    }
    virtual_registry().lock().remove(&norm);
}

/// Returns `true` if `s` matches or contains any dynamically registered virtual DualSense
/// serial number or instance ID (case-insensitive).
pub fn is_registered_virtual_serial(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let norm = s.to_ascii_lowercase();
    let guard = virtual_registry().lock();
    guard.iter().any(|entry| norm.contains(entry.as_str()))
}

/// Returns `true` if a HID path, serial string, or PnP instance/ancestor ID carries a known
/// virtual DualSense marker (`DSMASTER-VDS`, `USBIP` / `USBIP2`, or `VHF\`).
pub fn is_virtual_dualsense_str(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    lower.contains("dsmaster-vds")
        || lower.contains("dsmaster_vds")
        || lower.contains("usbip")
        || lower.contains("usbip2")
        || lower.contains("vhf\\")
        || lower.contains("vhf#")
}

/// Returns `true` if `s` matches either a static virtual DualSense marker or a dynamically
/// registered virtual DualSense serial/instance identifier.
pub fn is_excluded_virtual_identifier(s: &str) -> bool {
    is_virtual_dualsense_str(s) || is_registered_virtual_serial(s)
}

/// Walks up the Windows PnP devnode ancestor chain (up to 8 hops) for `instance_id` and
/// returns `true` if any parent/ancestor devnode belongs to a virtual bus (`USBIP`, `VHF`)
/// or matches a virtual DualSense serial/registry entry.
fn has_virtual_pnp_ancestor(instance_id: &str) -> bool {
    if instance_id.is_empty() {
        return false;
    }
    let id_w = to_wide_null(instance_id);
    let mut devinst = 0u32;
    let cr = unsafe {
        CM_Locate_DevNodeW(
            &mut devinst,
            PCWSTR(id_w.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    };
    if cr != CR_SUCCESS {
        return false;
    }

    let mut current = devinst;
    let mut buf = [0u16; 512];
    for _ in 0..8 {
        let mut parent = 0u32;
        if unsafe { CM_Get_Parent(&mut parent, current, 0) } != CR_SUCCESS || parent == 0 {
            break;
        }
        buf.fill(0);
        if unsafe { CM_Get_Device_IDW(parent, &mut buf, 0) } == CR_SUCCESS {
            let parent_id = wide_to_string(&buf);
            if parent_id.eq_ignore_ascii_case(r"HTREE\ROOT\0") {
                break;
            }
            if is_excluded_virtual_identifier(&parent_id) {
                return true;
            }
        }
        current = parent;
    }
    false
}

/// Returns `true` if `instance_id` identifies a virtual DualSense directly (by static marker
/// or dynamic registry) or via a virtual bus ancestor in the live PnP device tree.
pub fn is_virtual_dualsense_instance(instance_id: &str) -> bool {
    if instance_id.is_empty() {
        return false;
    }
    is_excluded_virtual_identifier(instance_id) || has_virtual_pnp_ancestor(instance_id)
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub device_path: String,
    pub instance_id: String,
    pub product_id: u16,
    pub is_edge: bool,
    pub transport: Transport,
    pub input_report_len: usize,
    pub output_report_len: usize,
    pub feature_report_len: usize,
}

struct DevInfoSet(HDEVINFO);
impl Drop for DevInfoSet {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

struct PreparsedGuard(PHIDP_PREPARSED_DATA);
impl Drop for PreparsedGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = HidD_FreePreparsedData(self.0);
        }
    }
}

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(crate) fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..len])
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn query_instance_id(path_w: &[u16]) -> String {
    let mut prop_type = DEVPROPTYPE::default();
    let mut needed = 0u32;
    let cr = unsafe {
        CM_Get_Device_Interface_PropertyW(
            PCWSTR(path_w.as_ptr()),
            &DEVPKEY_Device_InstanceId,
            &mut prop_type,
            None,
            &mut needed,
            0,
        )
    };
    if needed == 0 && cr != CR_SUCCESS {
        return String::new();
    }
    let mut buf = vec![0u8; needed as usize];
    let cr = unsafe {
        CM_Get_Device_Interface_PropertyW(
            PCWSTR(path_w.as_ptr()),
            &DEVPKEY_Device_InstanceId,
            &mut prop_type,
            Some(buf.as_mut_ptr()),
            &mut needed,
            0,
        )
    };
    if cr != CR_SUCCESS {
        return String::new();
    }
    let mut wide = Vec::with_capacity(buf.len() / 2);
    for chunk in buf.chunks_exact(2) {
        wide.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    wide_to_string(&wide)
}

fn query_serial_number(handle: HANDLE) -> Option<String> {
    let mut buf = [0u16; 128];
    let ok = unsafe {
        HidD_GetSerialNumberString(
            handle,
            buf.as_mut_ptr() as *mut c_void,
            (buf.len() * 2) as u32,
        )
    };
    if !ok {
        return None;
    }
    let serial = wide_to_string(&buf);
    if serial.is_empty() {
        None
    } else {
        Some(serial)
    }
}

/// Inspects a single HID interface path and returns `Some(DeviceInfo)` if it is a physical
/// DualSense gamepad (excluding DSMaster's own virtual DualSense devices).
pub fn inspect_hid_path(device_path: &str) -> Option<DeviceInfo> {
    if is_excluded_virtual_identifier(device_path) {
        debug!(path = %device_path, "Skipping virtual DualSense HID path");
        return None;
    }

    let path_w = to_wide_null(device_path);
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path_w.as_ptr()),
            0, // Query-only open: succeeds even when exclusive or during PnP transitions
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .ok()?;
    let _handle_guard = HandleGuard(handle);

    let mut attrs = HIDD_ATTRIBUTES {
        Size: size_of::<HIDD_ATTRIBUTES>() as u32,
        ..Default::default()
    };
    if !unsafe { HidD_GetAttributes(handle, &mut attrs) } {
        return None;
    }
    if attrs.VendorID != SONY_VID
        || (attrs.ProductID != DUALSENSE_PID && attrs.ProductID != DUALSENSE_EDGE_PID)
    {
        return None;
    }

    let mut preparsed = PHIDP_PREPARSED_DATA::default();
    if !unsafe { HidD_GetPreparsedData(handle, &mut preparsed) } {
        return None;
    }
    let _preparsed_guard = PreparsedGuard(preparsed);

    let mut caps = HIDP_CAPS::default();
    if unsafe { HidP_GetCaps(preparsed, &mut caps) }.is_err() {
        return None;
    }
    if caps.UsagePage != USAGE_PAGE_GENERIC_DESKTOP || caps.Usage != USAGE_GAMEPAD {
        return None;
    }

    if let Some(serial) = query_serial_number(handle) {
        if is_excluded_virtual_identifier(&serial) {
            debug!(
                path = %device_path,
                serial = %serial,
                "Skipping virtual DualSense by HID serial number"
            );
            return None;
        }
    }

    let instance_id = query_instance_id(&path_w);
    if !instance_id.is_empty() && is_virtual_dualsense_instance(&instance_id) {
        debug!(
            path = %device_path,
            instance_id = %instance_id,
            "Skipping virtual DualSense by PnP instance/ancestor"
        );
        return None;
    }

    let lower = device_path.to_ascii_lowercase();
    let transport =
        if caps.InputReportByteLength >= 78 || lower.contains("bth") || lower.contains("{00001124")
        {
            Transport::Bluetooth
        } else {
            Transport::Usb
        };

    Some(DeviceInfo {
        device_path: device_path.to_owned(),
        instance_id,
        product_id: attrs.ProductID,
        is_edge: attrs.ProductID == DUALSENSE_EDGE_PID,
        transport,
        input_report_len: caps.InputReportByteLength as usize,
        output_report_len: caps.OutputReportByteLength as usize,
        feature_report_len: caps.FeatureReportByteLength as usize,
    })
}

/// Enumerates all currently present Sony DualSense and DualSense Edge gamepad HID interfaces.
pub fn enumerate_dualsense_devices() -> Result<Vec<DeviceInfo>, HidError> {
    let hid_guid: GUID = unsafe { HidD_GetHidGuid() };
    let dev_info = unsafe {
        SetupDiGetClassDevsW(
            Some(&hid_guid),
            None,
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    }
    .map_err(|e| HidError::Win32("SetupDiGetClassDevsW", e.code().0 as u32))?;
    let _set_guard = DevInfoSet(dev_info);

    let mut devices = Vec::new();
    let mut index = 0u32;

    loop {
        let mut iface_data = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        if let Err(e) = unsafe {
            SetupDiEnumDeviceInterfaces(dev_info, None, &hid_guid, index, &mut iface_data)
        } {
            if e.code().0 as u32 == (0x80070000 | ERROR_NO_MORE_ITEMS.0) {
                break;
            }
            break;
        }
        index += 1;

        let mut required_size = 0u32;
        let _ = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                dev_info,
                &iface_data,
                None,
                0,
                Some(&mut required_size),
                None,
            )
        };
        if required_size < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
            continue;
        }

        let mut detail_buf = vec![0u8; required_size as usize];
        let detail_ptr = detail_buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        unsafe {
            ptr::write(
                ptr::addr_of_mut!((*detail_ptr).cbSize),
                size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32,
            );
        }

        if unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                dev_info,
                &iface_data,
                Some(detail_ptr),
                required_size,
                None,
                None,
            )
        }
        .is_err()
        {
            continue;
        }

        let str_offset = 4usize; // cbSize is u32 (4 bytes), followed by UTF-16 DevicePath
        let u16_slice = unsafe {
            let ptr = detail_buf.as_ptr().add(str_offset) as *const u16;
            let len = (detail_buf.len() - str_offset) / 2;
            std::slice::from_raw_parts(ptr, len)
        };
        let path = wide_to_string(u16_slice);
        if let Some(info) = inspect_hid_path(&path) {
            devices.push(info);
        }
    }

    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_dualsense_identifiers_are_accepted() {
        let physical_usb = r"USB\VID_054C&PID_0CE6\8&11403F6E&7&0000";
        let physical_hid = r"HID\VID_054C&PID_0CE6&MI_03\8&11403F6E&7&0000";
        let physical_bt = r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0CE6\8&123456&0&A1B2C3D4E5F6_C00000000";
        let physical_mac_serial = "a1:b2:c3:d4:e5:f6";

        assert!(!is_virtual_dualsense_str(physical_usb));
        assert!(!is_virtual_dualsense_str(physical_hid));
        assert!(!is_virtual_dualsense_str(physical_bt));
        assert!(!is_virtual_dualsense_str(physical_mac_serial));

        assert!(!is_virtual_dualsense_instance(physical_usb));
        assert!(!is_virtual_dualsense_instance(physical_bt));
    }

    #[test]
    fn static_virtual_dualsense_identifiers_are_rejected() {
        let cases = [
            "DSMASTER-VDS-01",
            "dsmaster-vds-99",
            r"USB\VID_054C&PID_0CE6\DSMASTER-VDS-01",
            r"USBIP\VID_054C&PID_0CE6\0001",
            r"ROOT\USBIP_WIN2\0000",
            r"USBIP2_UDE\VID_054C&PID_0CE6\0001",
            r"VHF\VID_054C&PID_0CE6\0001",
            r"\\?\hid#vhf#vid_054c&pid_0ce6#0001#{4d1e55b2-f16f-11cf-88cb-001111000030}",
        ];
        for id in cases {
            assert!(
                is_virtual_dualsense_str(id),
                "expected virtual identifier to be rejected: {id}"
            );
            assert!(is_virtual_dualsense_instance(id));
        }
        assert!(
            inspect_hid_path(
                r"\\?\hid#vid_054c&pid_0ce6#dsmaster-vds-01#{4d1e55b2-f16f-11cf-88cb-001111000030}"
            )
            .is_none()
        );
    }

    #[test]
    fn dynamic_virtual_serial_registry_round_trip() {
        let serial = "ENUM-TEST-VIRTUAL-SERIAL-77";
        let instance = r"USB\VID_054C&PID_0CE6\ENUM-TEST-VIRTUAL-SERIAL-77";

        assert!(!is_registered_virtual_serial(serial));
        assert!(!is_excluded_virtual_identifier(instance));

        register_virtual_serial(serial);
        assert!(is_registered_virtual_serial(serial));
        assert!(is_registered_virtual_serial("enum-test-virtual-serial-77"));
        assert!(is_excluded_virtual_identifier(instance));
        assert!(is_virtual_dualsense_instance(instance));

        // Empty/whitespace registration must never match anything.
        register_virtual_serial("   ");
        assert!(!is_registered_virtual_serial(
            r"USB\VID_054C&PID_0CE6\PHYSICAL_UNRELATED"
        ));

        unregister_virtual_serial(serial);
        assert!(!is_registered_virtual_serial(serial));
        assert!(!is_excluded_virtual_identifier(instance));
    }
}
