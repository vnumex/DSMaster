use std::sync::Arc;

use crate::protocol::{
    DS_FEATURE_REPORT_CALIBRATION, DS_FEATURE_REPORT_CALIBRATION_SIZE, ImuCalibration,
    parse_calibration_report,
};
use windows::Win32::Devices::HumanInterfaceDevice::HidD_GetFeature;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_DEVICE_NOT_CONNECTED, ERROR_GEN_FAILURE, ERROR_INVALID_HANDLE,
    ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, GetLastError, HANDLE, WAIT_EVENT, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::PCWSTR;

use super::HidError;
use super::enumerate::{DeviceInfo, to_wide_null};

struct OwnedEvent(HANDLE);

unsafe impl Send for OwnedEvent {}
unsafe impl Sync for OwnedEvent {}

impl OwnedEvent {
    fn new_manual_reset() -> Result<Self, HidError> {
        let h = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|e| HidError::Win32("CreateEventW", e.code().0 as u32))?;
        Ok(Self(h))
    }

    #[inline]
    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedEvent {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Clone)]
pub struct CancelHandle {
    event: Arc<OwnedEvent>,
}

impl CancelHandle {
    #[inline]
    pub fn cancel(&self) {
        unsafe {
            let _ = SetEvent(self.event.raw());
        }
    }
}

pub enum ReadOutcome<'a> {
    Report(&'a [u8]),
    Timeout,
    Cancelled,
    Disconnected,
}

pub struct DualSenseDevice {
    info: DeviceInfo,
    handle: HANDLE,
    read_event: OwnedEvent,
    write_event: OwnedEvent,
    cancel_event: Arc<OwnedEvent>,
    read_ov: Box<OVERLAPPED>,
    write_ov: Box<OVERLAPPED>,
    read_buf: Box<[u8; 128]>,
    write_buf: Box<[u8; 128]>,
}

unsafe impl Send for DualSenseDevice {}

impl DualSenseDevice {
    pub fn open(info: DeviceInfo) -> Result<Self, HidError> {
        if info.input_report_len > 128 || info.output_report_len > 128 {
            return Err(HidError::ReportSizeOutOfBounds {
                input: info.input_report_len,
                output: info.output_report_len,
            });
        }

        let path_w = to_wide_null(&info.device_path);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path_w.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .map_err(|e| HidError::Win32("CreateFileW", e.code().0 as u32))?;

        let read_event = OwnedEvent::new_manual_reset()?;
        let write_event = OwnedEvent::new_manual_reset()?;
        let cancel_event = Arc::new(OwnedEvent::new_manual_reset()?);

        Ok(Self {
            info,
            handle,
            read_event,
            write_event,
            cancel_event,
            read_ov: Box::new(OVERLAPPED::default()),
            write_ov: Box::new(OVERLAPPED::default()),
            read_buf: Box::new([0u8; 128]),
            write_buf: Box::new([0u8; 128]),
        })
    }

    #[inline]
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    #[inline]
    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle {
            event: Arc::clone(&self.cancel_event),
        }
    }

    /// Mutable access to the pre-allocated output report buffer of `output_report_len` bytes.
    #[inline]
    pub fn write_buf_mut(&mut self) -> &mut [u8] {
        let len = self.info.output_report_len;
        &mut self.write_buf[..len]
    }

    /// Queries Feature Report `0x05` (calibration), which also activates full `0x31` Bluetooth reporting.
    pub fn read_feature_calibration(&self) -> Result<Option<ImuCalibration>, HidError> {
        let len = self
            .info
            .feature_report_len
            .max(DS_FEATURE_REPORT_CALIBRATION_SIZE);
        let mut buf = vec![0u8; len];
        buf[0] = DS_FEATURE_REPORT_CALIBRATION;
        let ok = unsafe {
            HidD_GetFeature(
                self.handle,
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                len as u32,
            )
        };
        if !ok {
            let err = unsafe { GetLastError().0 };
            return Err(HidError::Os("HidD_GetFeature(0x05)", err));
        }
        match parse_calibration_report(&buf) {
            Ok(cal) => Ok(Some(cal)),
            Err(_) => Ok(None),
        }
    }

    /// Reads one HID input report using overlapped I/O with zero heap allocations.
    #[inline]
    pub fn read_report(&mut self, timeout_ms: u32) -> Result<ReadOutcome<'_>, HidError> {
        unsafe {
            let _ = ResetEvent(self.read_event.raw());
        }
        *self.read_ov = OVERLAPPED {
            hEvent: self.read_event.raw(),
            ..Default::default()
        };

        let mut bytes_read = 0u32;
        let target_len = self.info.input_report_len;
        let res = unsafe {
            ReadFile(
                self.handle,
                Some(&mut self.read_buf[..target_len]),
                Some(&mut bytes_read),
                Some(&mut *self.read_ov),
            )
        };

        if res.is_ok() {
            return Ok(ReadOutcome::Report(&self.read_buf[..bytes_read as usize]));
        }

        let err = unsafe { GetLastError() };
        if err != ERROR_IO_PENDING {
            if is_disconnect_err(err.0) {
                return Ok(ReadOutcome::Disconnected);
            }
            return Err(HidError::Os("ReadFile", err.0));
        }

        let handles = [self.read_event.raw(), self.cancel_event.raw()];
        let wait = unsafe { WaitForMultipleObjects(&handles, false, timeout_ms) };
        if wait == WAIT_OBJECT_0 {
            let mut completed = 0u32;
            match unsafe { GetOverlappedResult(self.handle, &*self.read_ov, &mut completed, false) }
            {
                Ok(()) => Ok(ReadOutcome::Report(&self.read_buf[..completed as usize])),
                Err(_) => {
                    let gerr = unsafe { GetLastError().0 };
                    if is_disconnect_err(gerr) {
                        Ok(ReadOutcome::Disconnected)
                    } else if gerr == ERROR_OPERATION_ABORTED.0 {
                        Ok(ReadOutcome::Cancelled)
                    } else {
                        Err(HidError::Os("GetOverlappedResult(Read)", gerr))
                    }
                }
            }
        } else {
            // Cancel in-flight overlapped I/O and wait until the kernel releases `read_buf`/`read_ov`.
            unsafe {
                let _ = CancelIoEx(self.handle, Some(&*self.read_ov));
                let mut completed = 0u32;
                let _ = GetOverlappedResult(self.handle, &*self.read_ov, &mut completed, true);
            }
            if wait == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
                Ok(ReadOutcome::Cancelled)
            } else if wait == WAIT_TIMEOUT {
                Ok(ReadOutcome::Timeout)
            } else {
                Err(HidError::Os("WaitForMultipleObjects(Read)", unsafe {
                    GetLastError().0
                }))
            }
        }
    }

    /// Writes `self.info().output_report_len` bytes from `write_buf` using overlapped I/O (zero alloc).
    #[inline]
    pub fn write_report(&mut self, timeout_ms: u32) -> Result<(), HidError> {
        unsafe {
            let _ = ResetEvent(self.write_event.raw());
        }
        *self.write_ov = OVERLAPPED {
            hEvent: self.write_event.raw(),
            ..Default::default()
        };

        let target_len = self.info.output_report_len;
        let mut bytes_written = 0u32;
        let res = unsafe {
            WriteFile(
                self.handle,
                Some(&self.write_buf[..target_len]),
                Some(&mut bytes_written),
                Some(&mut *self.write_ov),
            )
        };

        if res.is_ok() {
            return Ok(());
        }

        let err = unsafe { GetLastError() };
        if err != ERROR_IO_PENDING {
            return Err(HidError::Os("WriteFile", err.0));
        }

        let wait = unsafe { WaitForSingleObject(self.write_event.raw(), timeout_ms) };
        if wait == WAIT_OBJECT_0 {
            let mut completed = 0u32;
            unsafe { GetOverlappedResult(self.handle, &*self.write_ov, &mut completed, false) }
                .map_err(|e| HidError::Win32("GetOverlappedResult(Write)", e.code().0 as u32))?;
            Ok(())
        } else {
            unsafe {
                let _ = CancelIoEx(self.handle, Some(&*self.write_ov));
                let mut completed = 0u32;
                let _ = GetOverlappedResult(self.handle, &*self.write_ov, &mut completed, true);
            }
            Err(HidError::Os("WaitForSingleObject(Write)", wait.0))
        }
    }
}

impl Drop for DualSenseDevice {
    fn drop(&mut self) {
        self.cancel_handle().cancel();
        unsafe {
            let _ = CancelIoEx(self.handle, None);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[inline]
const fn is_disconnect_err(code: u32) -> bool {
    code == ERROR_DEVICE_NOT_CONNECTED.0
        || code == ERROR_GEN_FAILURE.0
        || code == ERROR_INVALID_HANDLE.0
}
