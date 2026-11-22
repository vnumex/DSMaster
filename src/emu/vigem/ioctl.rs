use windows::core::GUID;

/// `{96E42B22-F5E9-42F8-B043-ED0F932F014F}`
pub const GUID_DEVINTERFACE_BUSENUM_VIGEM: GUID = GUID::from_values(
    0x96E42B22,
    0xF5E9,
    0x42F8,
    [0xB0, 0x43, 0xED, 0x0F, 0x93, 0x2F, 0x01, 0x4F],
);

pub const VIGEM_COMMON_VERSION: u32 = 0x0001;

const FILE_DEVICE_BUS_EXTENDER: u32 = 0x0000_002A;
const METHOD_BUFFERED: u32 = 0;
const FILE_READ_DATA: u32 = 1;
const FILE_WRITE_DATA: u32 = 2;
const FILE_RW_DATA: u32 = FILE_READ_DATA | FILE_WRITE_DATA;

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

const IOCTL_VIGEM_BASE: u32 = 0x801;

pub const IOCTL_VIGEM_PLUGIN_TARGET: u32 = ctl_code(
    FILE_DEVICE_BUS_EXTENDER,
    IOCTL_VIGEM_BASE,
    METHOD_BUFFERED,
    FILE_WRITE_DATA,
);
pub const IOCTL_VIGEM_UNPLUG_TARGET: u32 = ctl_code(
    FILE_DEVICE_BUS_EXTENDER,
    IOCTL_VIGEM_BASE + 1,
    METHOD_BUFFERED,
    FILE_WRITE_DATA,
);
pub const IOCTL_VIGEM_CHECK_VERSION: u32 = ctl_code(
    FILE_DEVICE_BUS_EXTENDER,
    IOCTL_VIGEM_BASE + 2,
    METHOD_BUFFERED,
    FILE_WRITE_DATA,
);
pub const IOCTL_VIGEM_WAIT_DEVICE_READY: u32 = ctl_code(
    FILE_DEVICE_BUS_EXTENDER,
    IOCTL_VIGEM_BASE + 3,
    METHOD_BUFFERED,
    FILE_WRITE_DATA,
);

pub const IOCTL_XUSB_REQUEST_NOTIFICATION: u32 = ctl_code(
    FILE_DEVICE_BUS_EXTENDER,
    IOCTL_VIGEM_BASE + 0x200,
    METHOD_BUFFERED,
    FILE_RW_DATA,
);
pub const IOCTL_XUSB_SUBMIT_REPORT: u32 = ctl_code(
    FILE_DEVICE_BUS_EXTENDER,
    IOCTL_VIGEM_BASE + 0x201,
    METHOD_BUFFERED,
    FILE_WRITE_DATA,
);

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VigemTargetType {
    Xbox360Wired = 0,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VigemPluginTarget {
    pub size: u32,
    pub serial_no: u32,
    pub target_type: u32,
    pub vendor_id: u16,
    pub product_id: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VigemUnplugTarget {
    pub size: u32,
    pub serial_no: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VigemCheckVersion {
    pub size: u32,
    pub version: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VigemWaitDeviceReady {
    pub size: u32,
    pub serial_no: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct XusbReport {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct XusbSubmitReport {
    pub size: u32,
    pub serial_no: u32,
    pub report: XusbReport,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct XusbRequestNotification {
    pub size: u32,
    pub serial_no: u32,
    pub large_motor: u8,
    pub small_motor: u8,
    pub led_number: u8,
    pub _pad: u8,
}

const _: () = {
    assert!(size_of::<VigemPluginTarget>() == 16);
    assert!(size_of::<VigemUnplugTarget>() == 8);
    assert!(size_of::<VigemCheckVersion>() == 8);
    assert!(size_of::<VigemWaitDeviceReady>() == 8);
    assert!(size_of::<XusbReport>() == 12);
    assert!(size_of::<XusbSubmitReport>() == 20);
    assert!(size_of::<XusbRequestNotification>() == 12);
};
