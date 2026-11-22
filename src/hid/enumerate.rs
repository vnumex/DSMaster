use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::ptr;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_Interface_PropertyW, CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetHidGuid,
    HidD_GetPreparsedData, HidP_GetCaps, PHIDP_PREPARSED_DATA,
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

/// Inspects a single HID interface path and returns `Some(DeviceInfo)` if it is a DualSense gamepad.
pub fn inspect_hid_path(device_path: &str) -> Option<DeviceInfo> {
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
        instance_id: query_instance_id(&path_w),
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
