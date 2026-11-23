use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError, WAIT_OBJECT_0};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::WaitForMultipleObjects;

use super::ioctl::{
    IOCTL_VIGEM_PLUGIN_TARGET, IOCTL_VIGEM_UNPLUG_TARGET, IOCTL_VIGEM_WAIT_DEVICE_READY,
    IOCTL_XUSB_REQUEST_NOTIFICATION, IOCTL_XUSB_SUBMIT_REPORT, VigemPluginTarget, VigemTargetType,
    VigemUnplugTarget, VigemWaitDeviceReady, XusbReport, XusbRequestNotification, XusbSubmitReport,
};
use super::{OwnedEvent, VigemBusHandle, VigemClient};
use crate::emu::mapping::map_to_xusb;
use crate::emu::{EmuError, FeedbackState, VirtualController};
use crate::engine::processing::ProcessedInput;

pub struct Xbox360Target {
    bus: Arc<VigemBusHandle>,
    serial_no: u32,
    submit_event: OwnedEvent,
    submit_ov: Box<OVERLAPPED>,
    submit_req: Box<XusbSubmitReport>,
    feedback_mailbox: Arc<AtomicU32>,
    stop_event: Arc<OwnedEvent>,
    feedback_thread: Option<JoinHandle<()>>,
}

unsafe impl Send for Xbox360Target {}

impl Xbox360Target {
    pub fn new(client: &VigemClient) -> Result<Self, EmuError> {
        let submit_event = OwnedEvent::new_manual_reset()?;
        let mut ov = Box::new(OVERLAPPED::default());

        let mut chosen_serial = None;
        let base = client.bus.alloc_serial_hint();
        for offset in 0..64u32 {
            let serial = base.wrapping_add(offset).max(1);
            let req = VigemPluginTarget {
                size: size_of::<VigemPluginTarget>() as u32,
                serial_no: serial,
                target_type: VigemTargetType::Xbox360Wired as u32,
                vendor_id: 0,
                product_id: 0,
            };
            if client
                .bus
                .ioctl_sync(
                    IOCTL_VIGEM_PLUGIN_TARGET,
                    &req,
                    None,
                    &submit_event,
                    &mut ov,
                )
                .is_ok()
            {
                chosen_serial = Some(serial);
                break;
            }
        }
        let serial_no = chosen_serial.ok_or(EmuError::NoFreeSlot)?;

        // Wait for PnP PDO readiness (gracefully ignore if older ViGEmBus doesn't implement it).
        let wait_req = VigemWaitDeviceReady {
            size: size_of::<VigemWaitDeviceReady>() as u32,
            serial_no,
        };
        let _ = client.bus.ioctl_sync(
            IOCTL_VIGEM_WAIT_DEVICE_READY,
            &wait_req,
            None,
            &submit_event,
            &mut ov,
        );

        let feedback_mailbox = Arc::new(AtomicU32::new(0));
        let stop_event = Arc::new(OwnedEvent::new_manual_reset()?);

        let bus_clone = Arc::clone(&client.bus);
        let mailbox_clone = Arc::clone(&feedback_mailbox);
        let stop_clone = Arc::clone(&stop_event);

        let feedback_thread = thread::Builder::new()
            .name(format!("vigem-xusb-fb-{serial_no}"))
            .spawn(move || feedback_loop(bus_clone, serial_no, mailbox_clone, stop_clone))
            .map_err(|e| {
                EmuError::Os(
                    "spawn feedback thread",
                    e.raw_os_error().unwrap_or(0) as u32,
                )
            })?;

        let submit_req = Box::new(XusbSubmitReport {
            size: size_of::<XusbSubmitReport>() as u32,
            serial_no,
            report: XusbReport::default(),
        });

        Ok(Self {
            bus: Arc::clone(&client.bus),
            serial_no,
            submit_event,
            submit_ov: ov,
            submit_req,
            feedback_mailbox,
            stop_event,
            feedback_thread: Some(feedback_thread),
        })
    }

    /// Submits a raw `XusbReport` with zero heap allocations.
    #[inline]
    pub fn submit_raw(&mut self, report: XusbReport) -> Result<(), EmuError> {
        self.submit_req.report = report;
        self.bus.ioctl_sync(
            IOCTL_XUSB_SUBMIT_REPORT,
            &*self.submit_req,
            None,
            &self.submit_event,
            &mut self.submit_ov,
        )
    }
}

impl VirtualController for Xbox360Target {
    #[inline]
    fn submit(&mut self, input: &ProcessedInput) -> Result<(), EmuError> {
        let report = map_to_xusb(input);
        self.submit_raw(report)
    }

    #[inline]
    fn poll_feedback(&mut self) -> Option<FeedbackState> {
        let packed = self.feedback_mailbox.swap(0, Ordering::AcqRel);
        if (packed & (1 << 24)) == 0 {
            return None;
        }
        Some(FeedbackState {
            rumble: Some(((packed & 0xFF) as u8, ((packed >> 8) & 0xFF) as u8)),
            ..Default::default()
        })
    }
}

impl Drop for Xbox360Target {
    fn drop(&mut self) {
        self.stop_event.set();
        if let Some(handle) = self.feedback_thread.take() {
            let _ = handle.join();
        }
        let unplug = VigemUnplugTarget {
            size: size_of::<VigemUnplugTarget>() as u32,
            serial_no: self.serial_no,
        };
        let _ = self.bus.ioctl_sync(
            IOCTL_VIGEM_UNPLUG_TARGET,
            &unplug,
            None,
            &self.submit_event,
            &mut self.submit_ov,
        );
    }
}

fn feedback_loop(
    bus: Arc<VigemBusHandle>,
    serial_no: u32,
    mailbox: Arc<AtomicU32>,
    stop_event: Arc<OwnedEvent>,
) {
    let Ok(notif_event) = OwnedEvent::new_manual_reset() else {
        return;
    };
    let mut ov = Box::new(OVERLAPPED::default());
    let mut req = Box::new(XusbRequestNotification {
        size: size_of::<XusbRequestNotification>() as u32,
        serial_no,
        ..Default::default()
    });

    loop {
        notif_event.reset();
        *ov = OVERLAPPED {
            hEvent: notif_event.raw(),
            ..Default::default()
        };
        req.size = size_of::<XusbRequestNotification>() as u32;
        req.serial_no = serial_no;

        let mut returned = 0u32;
        let ptr = &mut *req as *mut XusbRequestNotification as *mut c_void;
        let len = size_of::<XusbRequestNotification>() as u32;
        let res = unsafe {
            DeviceIoControl(
                bus.handle,
                IOCTL_XUSB_REQUEST_NOTIFICATION,
                Some(ptr),
                len,
                Some(ptr),
                len,
                Some(&mut returned),
                Some(&mut *ov),
            )
        };

        if res.is_err() {
            let err = unsafe { GetLastError() };
            if err != ERROR_IO_PENDING {
                break;
            }
            let handles = [notif_event.raw(), stop_event.raw()];
            let wait = unsafe { WaitForMultipleObjects(&handles, false, u32::MAX) };
            if wait != WAIT_OBJECT_0 {
                unsafe {
                    let _ = CancelIoEx(bus.handle, Some(&*ov));
                    let _ = GetOverlappedResult(bus.handle, &*ov, &mut returned, true);
                }
                break;
            }
            if unsafe { GetOverlappedResult(bus.handle, &*ov, &mut returned, false) }.is_err() {
                break;
            }
        }

        let packed = (1u32 << 24)
            | ((req.led_number as u32) << 16)
            | ((req.small_motor as u32) << 8)
            | (req.large_motor as u32);
        mailbox.store(packed, Ordering::Release);
    }
}
