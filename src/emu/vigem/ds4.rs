use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError, WAIT_OBJECT_0};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::WaitForMultipleObjects;

use super::ioctl::{
    Ds4RequestNotification, Ds4SubmitReportEx, IOCTL_DS4_REQUEST_NOTIFICATION,
    IOCTL_DS4_SUBMIT_REPORT, IOCTL_VIGEM_PLUGIN_TARGET, IOCTL_VIGEM_UNPLUG_TARGET,
    IOCTL_VIGEM_WAIT_DEVICE_READY, VigemPluginTarget, VigemTargetType, VigemUnplugTarget,
    VigemWaitDeviceReady,
};
use super::{OwnedEvent, VigemBusHandle, VigemClient};
use crate::emu::mapping::map_to_ds4_ex;
use crate::emu::{EmuError, FeedbackState, VirtualController};
use crate::engine::processing::ProcessedInput;

pub struct DualShock4Target {
    bus: Arc<VigemBusHandle>,
    serial_no: u32,
    submit_event: OwnedEvent,
    submit_ov: Box<OVERLAPPED>,
    submit_req: Box<Ds4SubmitReportEx>,
    feedback_mailbox: Arc<AtomicU64>,
    stop_event: Arc<OwnedEvent>,
    feedback_thread: Option<JoinHandle<()>>,
    touch_counter: u8,
}

unsafe impl Send for DualShock4Target {}

impl DualShock4Target {
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
                target_type: VigemTargetType::DualShock4Wired as u32,
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

        let feedback_mailbox = Arc::new(AtomicU64::new(0));
        let stop_event = Arc::new(OwnedEvent::new_manual_reset()?);

        let bus_clone = Arc::clone(&client.bus);
        let mailbox_clone = Arc::clone(&feedback_mailbox);
        let stop_clone = Arc::clone(&stop_event);

        let feedback_thread = thread::Builder::new()
            .name(format!("vigem-ds4-fb-{serial_no}"))
            .spawn(move || feedback_loop(bus_clone, serial_no, mailbox_clone, stop_clone))
            .map_err(|e| {
                EmuError::Os(
                    "spawn ds4 feedback thread",
                    e.raw_os_error().unwrap_or(0) as u32,
                )
            })?;

        let submit_req = Box::new(Ds4SubmitReportEx {
            size: size_of::<Ds4SubmitReportEx>() as u32,
            serial_no,
            report: [0u8; 63],
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
            touch_counter: 0,
        })
    }
}

impl VirtualController for DualShock4Target {
    #[inline]
    fn submit(&mut self, input: &ProcessedInput) -> Result<(), EmuError> {
        self.touch_counter = self.touch_counter.wrapping_add(1);
        self.submit_req.report = map_to_ds4_ex(input, self.touch_counter);
        self.bus.ioctl_sync(
            IOCTL_DS4_SUBMIT_REPORT,
            &*self.submit_req,
            None,
            &self.submit_event,
            &mut self.submit_ov,
        )
    }

    #[inline]
    fn poll_feedback(&mut self) -> Option<FeedbackState> {
        let packed = self.feedback_mailbox.swap(0, Ordering::AcqRel);
        if (packed & (1u64 << 40)) == 0 {
            return None;
        }
        let r = ((packed >> 16) & 0xFF) as u8;
        let g = ((packed >> 24) & 0xFF) as u8;
        let b = ((packed >> 32) & 0xFF) as u8;
        Some(FeedbackState {
            large_motor: (packed & 0xFF) as u8,
            small_motor: ((packed >> 8) & 0xFF) as u8,
            lightbar_rgb: Some([r, g, b]),
        })
    }
}

impl Drop for DualShock4Target {
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
    mailbox: Arc<AtomicU64>,
    stop_event: Arc<OwnedEvent>,
) {
    let Ok(notif_event) = OwnedEvent::new_manual_reset() else {
        return;
    };
    let mut ov = Box::new(OVERLAPPED::default());
    let mut req = Box::new(Ds4RequestNotification {
        size: size_of::<Ds4RequestNotification>() as u32,
        serial_no,
        ..Default::default()
    });

    loop {
        notif_event.reset();
        *ov = OVERLAPPED {
            hEvent: notif_event.raw(),
            ..Default::default()
        };
        req.size = size_of::<Ds4RequestNotification>() as u32;
        req.serial_no = serial_no;

        let mut returned = 0u32;
        let ptr = &mut *req as *mut Ds4RequestNotification as *mut c_void;
        let len = size_of::<Ds4RequestNotification>() as u32;
        let res = unsafe {
            DeviceIoControl(
                bus.handle,
                IOCTL_DS4_REQUEST_NOTIFICATION,
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

        let packed = (1u64 << 40)
            | ((req.report.lightbar_b as u64) << 32)
            | ((req.report.lightbar_g as u64) << 24)
            | ((req.report.lightbar_r as u64) << 16)
            | ((req.report.small_motor as u64) << 8)
            | (req.report.large_motor as u64);
        mailbox.store(packed, Ordering::Release);
    }
}
