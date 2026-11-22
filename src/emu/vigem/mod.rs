use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows::core::PCWSTR;

use self::ioctl::{
    GUID_DEVINTERFACE_BUSENUM_VIGEM, IOCTL_VIGEM_CHECK_VERSION, VIGEM_COMMON_VERSION,
    VigemCheckVersion,
};
use crate::emu::EmuError;

pub mod ds4;
pub mod ioctl;
pub mod x360;

struct DevInfoSet(HDEVINFO);
impl Drop for DevInfoSet {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

pub(crate) struct OwnedEvent(HANDLE);
unsafe impl Send for OwnedEvent {}
unsafe impl Sync for OwnedEvent {}

impl OwnedEvent {
    pub(crate) fn new_manual_reset() -> Result<Self, EmuError> {
        let h = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|e| EmuError::Win32("CreateEventW", e.code().0 as u32))?;
        Ok(Self(h))
    }

    #[inline]
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }

    #[inline]
    pub(crate) fn set(&self) {
        unsafe {
            let _ = SetEvent(self.0);
        }
    }

    #[inline]
    pub(crate) fn reset(&self) {
        unsafe {
            let _ = ResetEvent(self.0);
        }
    }
}

impl Drop for OwnedEvent {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(crate) struct VigemBusHandle {
    pub(crate) handle: HANDLE,
    next_serial: AtomicU32,
}

unsafe impl Send for VigemBusHandle {}
unsafe impl Sync for VigemBusHandle {}

impl VigemBusHandle {
    #[inline]
    pub(crate) fn alloc_serial_hint(&self) -> u32 {
        self.next_serial.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn ioctl_sync<T>(
        &self,
        code: u32,
        in_buf: &T,
        out_buf: Option<&mut T>,
        event: &OwnedEvent,
        ov: &mut OVERLAPPED,
    ) -> Result<(), EmuError> {
        event.reset();
        *ov = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };

        let in_ptr = in_buf as *const T as *const c_void;
        let in_len = size_of::<T>() as u32;
        let (out_ptr, out_len) = match out_buf {
            Some(b) => (Some(b as *mut T as *mut c_void), size_of::<T>() as u32),
            None => (None, 0),
        };

        let mut returned = 0u32;
        let res = unsafe {
            DeviceIoControl(
                self.handle,
                code,
                Some(in_ptr),
                in_len,
                out_ptr,
                out_len,
                Some(&mut returned),
                Some(ov),
            )
        };
        if res.is_ok() {
            return Ok(());
        }

        let err = unsafe { GetLastError() };
        if err != ERROR_IO_PENDING {
            return Err(EmuError::Os("DeviceIoControl", err.0));
        }

        let wait = unsafe { WaitForSingleObject(event.raw(), INFINITE) };
        if wait != WAIT_OBJECT_0 {
            unsafe {
                let _ = CancelIoEx(self.handle, Some(ov));
                let _ = GetOverlappedResult(self.handle, ov, &mut returned, true);
            }
            return Err(EmuError::Os("WaitForSingleObject(IOCTL)", wait.0));
        }

        unsafe { GetOverlappedResult(self.handle, ov, &mut returned, false) }
            .map_err(|e| EmuError::Win32("GetOverlappedResult(IOCTL)", e.code().0 as u32))?;
        Ok(())
    }
}

impl Drop for VigemBusHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CancelIoEx(self.handle, None);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[derive(Clone)]
pub struct VigemClient {
    pub(crate) bus: Arc<VigemBusHandle>,
}

fn find_vigem_device_path() -> Result<Option<Vec<u16>>, EmuError> {
    let dev_info = unsafe {
        SetupDiGetClassDevsW(
            Some(&GUID_DEVINTERFACE_BUSENUM_VIGEM),
            None,
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    }
    .map_err(|e| EmuError::Win32("SetupDiGetClassDevsW", e.code().0 as u32))?;
    let _guard = DevInfoSet(dev_info);

    let mut iface_data = SP_DEVICE_INTERFACE_DATA {
        cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
        ..Default::default()
    };
    if unsafe {
        SetupDiEnumDeviceInterfaces(
            dev_info,
            None,
            &GUID_DEVINTERFACE_BUSENUM_VIGEM,
            0,
            &mut iface_data,
        )
    }
    .is_err()
    {
        return Ok(None);
    }

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
        return Ok(None);
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
        return Ok(None);
    }

    let str_offset = 4usize;
    let u16_slice = unsafe {
        let ptr = detail_buf.as_ptr().add(str_offset) as *const u16;
        let len = (detail_buf.len() - str_offset) / 2;
        std::slice::from_raw_parts(ptr, len)
    };
    Ok(Some(u16_slice.to_vec()))
}

/// Fast device-interface check (<1 ms) to determine if `ViGEmBus` is installed and active.
pub fn is_vigem_installed() -> bool {
    matches!(find_vigem_device_path(), Ok(Some(_)))
}

impl VigemClient {
    pub fn connect() -> Result<Self, EmuError> {
        let path = find_vigem_device_path()?.ok_or(EmuError::DriverNotFound)?;
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .map_err(|e| EmuError::Win32("CreateFileW(ViGEmBus)", e.code().0 as u32))?;

        let bus = Arc::new(VigemBusHandle {
            handle,
            next_serial: AtomicU32::new(1),
        });

        let event = OwnedEvent::new_manual_reset()?;
        let mut ov = OVERLAPPED::default();
        let req = VigemCheckVersion {
            size: size_of::<VigemCheckVersion>() as u32,
            version: VIGEM_COMMON_VERSION,
        };
        bus.ioctl_sync(IOCTL_VIGEM_CHECK_VERSION, &req, None, &event, &mut ov)
            .map_err(|_| EmuError::IncompatibleVersion)?;

        Ok(Self { bus })
    }
}
