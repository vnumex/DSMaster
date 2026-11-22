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

            let desired: HashSet<String> = physical_instance_ids
                .iter()
                .filter(|id| {
                    let upper = id.to_ascii_uppercase();
                    !id.is_empty() && !upper.contains("VID_045E") && !upper.contains("PID_05C4")
                })
                .cloned()
                .collect();

            let current_blacklist = ioctl_get_multi_sz(handle.0, IOCTL_GET_BLACKLIST)?;
            let mut next_blacklist: Vec<String> = current_blacklist
                .into_iter()
                .filter(|id| !guard.managed_ids.contains(id) || desired.contains(id))
                .collect();
            for id in &desired {
                if !next_blacklist.iter().any(|x| x.eq_ignore_ascii_case(id)) {
                    next_blacklist.push(id.clone());
                }
            }
            ioctl_set_multi_sz(handle.0, IOCTL_SET_BLACKLIST, &next_blacklist)?;
            guard.managed_ids = desired;

            if !ioctl_get_bool(handle.0, IOCTL_GET_ACTIVE)? {
                ioctl_set_bool(handle.0, IOCTL_SET_ACTIVE, true)?;
                guard.activated_by_us = true;
            }
        } else {
            let current_blacklist = ioctl_get_multi_sz(handle.0, IOCTL_GET_BLACKLIST)?;
            let next_blacklist: Vec<String> = current_blacklist
                .into_iter()
                .filter(|id| !guard.managed_ids.contains(id))
                .collect();
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
}
