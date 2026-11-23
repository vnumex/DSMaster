use std::collections::HashSet;
use std::ffi::OsString;
use std::ffi::c_void;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;

use parking_lot::Mutex;
use thiserror::Error;
use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, GetLastError, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, QueryDosDeviceW,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::{PCWSTR, w};

const IO_CONTROL_DEVICE_TYPE: u32 = 32769;
const METHOD_BUFFERED: u32 = 0;
const FILE_READ_DATA: u32 = 1;

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

const IOCTL_GET_WHITELIST: u32 = ctl_code(
    IO_CONTROL_DEVICE_TYPE,
    2048,
    METHOD_BUFFERED,
    FILE_READ_DATA,
);
const IOCTL_SET_WHITELIST: u32 = ctl_code(
    IO_CONTROL_DEVICE_TYPE,
    2049,
    METHOD_BUFFERED,
    FILE_READ_DATA,
);
const IOCTL_GET_BLACKLIST: u32 = ctl_code(
    IO_CONTROL_DEVICE_TYPE,
    2050,
    METHOD_BUFFERED,
    FILE_READ_DATA,
);
const IOCTL_SET_BLACKLIST: u32 = ctl_code(
    IO_CONTROL_DEVICE_TYPE,
    2051,
    METHOD_BUFFERED,
    FILE_READ_DATA,
);
const IOCTL_GET_ACTIVE: u32 = ctl_code(
    IO_CONTROL_DEVICE_TYPE,
    2052,
    METHOD_BUFFERED,
    FILE_READ_DATA,
);
const IOCTL_SET_ACTIVE: u32 = ctl_code(
    IO_CONTROL_DEVICE_TYPE,
    2053,
    METHOD_BUFFERED,
    FILE_READ_DATA,
);

#[derive(Debug, Error)]
pub enum CloakError {
    #[error("HidHide driver is not installed or unavailable")]
    DriverNotFound,
    #[error("invalid executable path for HidHide whitelist: {0}")]
    InvalidPath(String),
    #[error("Win32 API {0} failed with code 0x{1:08X}")]
    Win32(&'static str, u32),
    #[error("OS error in {0}: code {1}")]
    Os(&'static str, u32),
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn encode_multi_sz(items: &[String]) -> Vec<u16> {
    if items.is_empty() {
        return vec![0, 0];
    }
    let mut out = Vec::new();
    for item in items {
        if item.is_empty() {
            continue;
        }
        out.extend(item.encode_utf16());
        out.push(0);
    }
    out.push(0);
    if out.len() < 2 {
        out.push(0);
    }
    out
}

pub fn decode_multi_sz(buf: &[u16]) -> Vec<String> {
    let mut result = Vec::new();
    let mut start = 0usize;
    for (idx, &ch) in buf.iter().enumerate() {
        if ch == 0 {
            if idx == start {
                break;
            }
            let s = OsString::from_wide(&buf[start..idx])
                .to_string_lossy()
                .into_owned();
            result.push(s);
            start = idx + 1;
        }
    }
    result
}

pub fn win32_path_to_nt_device_path(path: &Path) -> Result<String, CloakError> {
    let s = path
        .to_str()
        .ok_or_else(|| CloakError::InvalidPath(path.display().to_string()))?;
    let bytes = s.as_bytes();
    if bytes.len() < 3 || bytes[1] != b':' || (bytes[2] != b'\\' && bytes[2] != b'/') {
        return Err(CloakError::InvalidPath(s.to_owned()));
    }
    let drive_wide: [u16; 3] = [bytes[0] as u16, b':' as u16, 0];
    let mut target = vec![0u16; 1024];
    let len = unsafe { QueryDosDeviceW(PCWSTR(drive_wide.as_ptr()), Some(&mut target)) };
    if len == 0 {
        return Err(CloakError::Os("QueryDosDeviceW", unsafe {
            GetLastError().0
        }));
    }
    let end = target.iter().position(|&c| c == 0).unwrap_or(len as usize);
    let nt_root = OsString::from_wide(&target[..end])
        .to_string_lossy()
        .into_owned();
    let rest = &s[2..].replace('/', "\\");
    Ok(format!("{nt_root}{rest}"))
}

fn open_hidhide(desired_access: u32) -> Result<OwnedHandle, CloakError> {
    let handle = unsafe {
        CreateFileW(
            w!(r"\\.\HidHide"),
            desired_access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|_| CloakError::DriverNotFound)?;
    Ok(OwnedHandle(handle))
}

fn ioctl_get_multi_sz(handle: HANDLE, code: u32) -> Result<Vec<String>, CloakError> {
    let mut needed = 0u32;
    let first = unsafe { DeviceIoControl(handle, code, None, 0, None, 0, Some(&mut needed), None) };
    if first.is_err() {
        let err = unsafe { GetLastError() };
        if err != ERROR_INSUFFICIENT_BUFFER && needed == 0 {
            return Err(CloakError::Os("DeviceIoControl(get size)", err.0));
        }
    }
    if needed == 0 {
        return Ok(Vec::new());
    }
    let u16_len = (needed as usize).div_ceil(2);
    let mut buf = vec![0u16; u16_len];
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            code,
            None,
            0,
            Some(buf.as_mut_ptr() as *mut c_void),
            (buf.len() * 2) as u32,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|e| CloakError::Win32("DeviceIoControl(get list)", e.code().0 as u32))?;
    let valid_u16 = (returned as usize / 2).min(buf.len());
    Ok(decode_multi_sz(&buf[..valid_u16]))
}

fn ioctl_set_multi_sz(handle: HANDLE, code: u32, items: &[String]) -> Result<(), CloakError> {
    let mut encoded = encode_multi_sz(items);
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            code,
            Some(encoded.as_mut_ptr() as *mut c_void),
            (encoded.len() * 2) as u32,
            None,
            0,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|e| CloakError::Win32("DeviceIoControl(set list)", e.code().0 as u32))?;
    Ok(())
}

fn ioctl_get_bool(handle: HANDLE, code: u32) -> Result<bool, CloakError> {
    let mut val: u8 = 0;
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            code,
            None,
            0,
            Some(&mut val as *mut u8 as *mut c_void),
            1,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|e| CloakError::Win32("DeviceIoControl(get bool)", e.code().0 as u32))?;
    Ok(val != 0)
}

fn ioctl_set_bool(handle: HANDLE, code: u32, value: bool) -> Result<(), CloakError> {
    let mut val: u8 = u8::from(value);
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            code,
            Some(&mut val as *mut u8 as *mut c_void),
            1,
            None,
            0,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|e| CloakError::Win32("DeviceIoControl(set bool)", e.code().0 as u32))?;
    Ok(())
}

/// Returns `true` if `instance_id` belongs to a virtual controller (ViGEm Xbox 360 / DS4,
/// or DSMaster's own Virtual DualSense via static marker, dynamic registry, or virtual PnP
/// parent bus) and MUST NEVER be added to HidHide's blacklist.
pub fn is_virtual_device_instance(instance_id: &str) -> bool {
    if instance_id.is_empty() {
        return false;
    }
    let upper = instance_id.to_ascii_uppercase();
    upper.contains("VID_045E")
        || upper.contains("PID_05C4")
        || crate::hid::enumerate::is_virtual_dualsense_instance(instance_id)
}

fn filter_cloakable_instance_ids(physical_instance_ids: &[String]) -> HashSet<String> {
    physical_instance_ids
        .iter()
        .filter(|id| !id.is_empty() && !is_virtual_device_instance(id))
        .cloned()
        .collect()
}

fn compute_next_blacklist_enabled(
    current_blacklist: Vec<String>,
    managed_ids: &HashSet<String>,
    desired: &HashSet<String>,
) -> Vec<String> {
    let mut next_blacklist: Vec<String> = current_blacklist
        .into_iter()
        .filter(|id| {
            // Actively strip any stale virtual DualSense entry if one somehow existed.
            if crate::hid::enumerate::is_virtual_dualsense_instance(id) {
                return false;
            }
            let was_managed = managed_ids.iter().any(|m| m.eq_ignore_ascii_case(id));
            let still_desired = desired.iter().any(|d| d.eq_ignore_ascii_case(id));
            !was_managed || still_desired
        })
        .collect();
    for id in desired {
        if !next_blacklist.iter().any(|x| x.eq_ignore_ascii_case(id)) {
            next_blacklist.push(id.clone());
        }
    }
    next_blacklist
}

fn compute_next_blacklist_disabled(
    current_blacklist: Vec<String>,
    managed_ids: &HashSet<String>,
) -> Vec<String> {
    current_blacklist
        .into_iter()
        .filter(|id| {
            !crate::hid::enumerate::is_virtual_dualsense_instance(id)
                && !managed_ids.iter().any(|m| m.eq_ignore_ascii_case(id))
        })
        .collect()
}

#[derive(Default)]
struct CloakState {
    managed_ids: HashSet<String>,
    activated_by_us: bool,
}

pub struct HidHideManager {
    state: Mutex<CloakState>,
}

impl HidHideManager {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(CloakState::default()),
        }
    }

    pub fn is_available() -> bool {
        open_hidhide(0).is_ok()
    }

    pub fn sync_cloaked(
        &self,
        enabled: bool,
        physical_instance_ids: &[String],
    ) -> Result<(), CloakError> {
        let mut guard = self.state.lock();
        if !enabled && guard.managed_ids.is_empty() && !guard.activated_by_us {
            return Ok(());
        }

        let handle = open_hidhide(FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0)?;

        if enabled {
            if let Ok(exe) = std::env::current_exe() {
                let nt_exe = win32_path_to_nt_device_path(&exe)?;
                let mut whitelist = ioctl_get_multi_sz(handle.0, IOCTL_GET_WHITELIST)?;
                if !whitelist.iter().any(|p| p.eq_ignore_ascii_case(&nt_exe)) {
                    whitelist.push(nt_exe);
                    ioctl_set_multi_sz(handle.0, IOCTL_SET_WHITELIST, &whitelist)?;
                }
            }

            let desired = filter_cloakable_instance_ids(physical_instance_ids);
            let current_blacklist = ioctl_get_multi_sz(handle.0, IOCTL_GET_BLACKLIST)?;
            let next_blacklist =
                compute_next_blacklist_enabled(current_blacklist, &guard.managed_ids, &desired);
            ioctl_set_multi_sz(handle.0, IOCTL_SET_BLACKLIST, &next_blacklist)?;
            guard.managed_ids = desired;

            if !ioctl_get_bool(handle.0, IOCTL_GET_ACTIVE)? {
                ioctl_set_bool(handle.0, IOCTL_SET_ACTIVE, true)?;
                guard.activated_by_us = true;
            }
        } else {
            let current_blacklist = ioctl_get_multi_sz(handle.0, IOCTL_GET_BLACKLIST)?;
            let next_blacklist =
                compute_next_blacklist_disabled(current_blacklist, &guard.managed_ids);
            ioctl_set_multi_sz(handle.0, IOCTL_SET_BLACKLIST, &next_blacklist)?;
            guard.managed_ids.clear();

            if guard.activated_by_us && next_blacklist.is_empty() {
                let _ = ioctl_set_bool(handle.0, IOCTL_SET_ACTIVE, false);
            }
            guard.activated_by_us = false;
        }

        Ok(())
    }
}

impl Drop for HidHideManager {
    fn drop(&mut self) {
        let _ = self.sync_cloaked(false, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::enumerate::{register_virtual_serial, unregister_virtual_serial};

    #[test]
    fn multi_sz_round_trip() {
        let items = vec![
            r"\Device\HarddiskVolume3\Games\dsmaster.exe".to_owned(),
            r"HID\VID_054C&PID_0CE6\8&11403F6E&7&0000".to_owned(),
        ];
        let encoded = encode_multi_sz(&items);
        assert_eq!(&encoded[encoded.len() - 2..], &[0, 0]);
        let decoded = decode_multi_sz(&encoded);
        assert_eq!(decoded, items);
        assert!(decode_multi_sz(&encode_multi_sz(&[])).is_empty());
    }

    #[test]
    fn current_exe_converts_to_nt_path() {
        let exe = std::env::current_exe().unwrap();
        let nt = win32_path_to_nt_device_path(&exe).unwrap();
        assert!(nt.starts_with(r"\Device\"));
    }

    #[test]
    fn physical_dualsense_is_allowed_to_be_cloaked() {
        let usb = r"USB\VID_054C&PID_0CE6\8&11403F6E&7&0000";
        let hid_usb = r"HID\VID_054C&PID_0CE6&MI_03\8&11403F6E&7&0000";
        let bt = r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0CE6\8&ABC&0&AABBCCDDEEFF_C00000000";

        assert!(!is_virtual_device_instance(usb));
        assert!(!is_virtual_device_instance(hid_usb));
        assert!(!is_virtual_device_instance(bt));

        let filtered =
            filter_cloakable_instance_ids(&[usb.to_owned(), hid_usb.to_owned(), bt.to_owned()]);
        assert!(filtered.contains(usb));
        assert!(filtered.contains(hid_usb));
        assert!(filtered.contains(bt));
    }

    #[test]
    fn virtual_controllers_are_never_cloaked_and_stale_vds_is_stripped() {
        let phys_usb = r"USB\VID_054C&PID_0CE6\8&11403F6E&7&0000";
        let phys_bt = r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0CE6\8&ABC";
        let vigem_x360 = r"USB\VID_045E&PID_028E\1&1A590E2C&0&01";
        let vigem_ds4 = r"HID\VID_054C&PID_05C4\1&1A590E2C&0&02";
        let vds_serial = r"USB\VID_054C&PID_0CE6\DSMASTER-VDS-01";
        let vds_usbip = r"USBIP\VID_054C&PID_0CE6\0001";
        let vds_vhf = r"VHF\VID_054C&PID_0CE6\0001";

        assert!(is_virtual_device_instance(vigem_x360));
        assert!(is_virtual_device_instance(vigem_ds4));
        assert!(is_virtual_device_instance(vds_serial));
        assert!(is_virtual_device_instance(vds_usbip));
        assert!(is_virtual_device_instance(vds_vhf));

        let dyn_serial = "CLOAK-DYN-VDS-42";
        let vds_dyn = r"USB\VID_054C&PID_0CE6\CLOAK-DYN-VDS-42";
        register_virtual_serial(dyn_serial);
        assert!(is_virtual_device_instance(vds_dyn));

        let desired = filter_cloakable_instance_ids(&[
            phys_usb.to_owned(),
            phys_bt.to_owned(),
            vigem_x360.to_owned(),
            vigem_ds4.to_owned(),
            vds_serial.to_owned(),
            vds_usbip.to_owned(),
            vds_vhf.to_owned(),
            vds_dyn.to_owned(),
        ]);
        assert_eq!(desired.len(), 2);
        assert!(desired.contains(phys_usb));
        assert!(desired.contains(phys_bt));

        // Even if a stale virtual DualSense entry was already present in HidHide's blacklist,
        // syncing strips it while preserving unrelated third-party user rules.
        let user_custom_rule = r"HID\VID_1234&PID_5678\USER_MANUAL";
        let current = vec![
            user_custom_rule.to_owned(),
            vds_serial.to_owned(),
            vds_usbip.to_owned(),
            vds_dyn.to_owned(),
        ];
        let next = compute_next_blacklist_enabled(current, &HashSet::new(), &desired);
        assert!(next.iter().any(|x| x == user_custom_rule));
        assert!(next.iter().any(|x| x == phys_usb));
        assert!(next.iter().any(|x| x == phys_bt));
        assert!(!next.iter().any(|x| x == vds_serial));
        assert!(!next.iter().any(|x| x == vds_usbip));
        assert!(!next.iter().any(|x| x == vds_dyn));

        // Disabling cloak also strips stale virtual DualSense entries and our managed IDs.
        let after_disable = compute_next_blacklist_disabled(
            vec![
                user_custom_rule.to_owned(),
                phys_usb.to_owned(),
                vds_serial.to_owned(),
            ],
            &desired,
        );
        assert_eq!(after_disable, vec![user_custom_rule.to_owned()]);

        unregister_virtual_serial(dyn_serial);
    }
}
