//! Clean-room USB 2.0 and USB-IF HID 1.11 descriptors for DSMaster's Virtual DualSense device.
//!
//! Authored strictly from the USB 2.0 Specification (Chapter 9), USB-IF HID 1.11 Specification,
//! and USB-IF HID Usage Tables. Uses honest non-trademark manufacturer/product strings and an
//! identifiable virtual serial string (`DSMASTER-VDS-*`) for self-exclusion safety.

#![allow(dead_code)]

use super::constants::{DUALSENSE_PID, SONY_VID};

pub const USB_STRING_MANUFACTURER: &str = "DSMaster";
pub const USB_STRING_PRODUCT: &str = "Virtual DualSense Controller";

/// Clean-room USB-IF HID 1.11 Report Descriptor for a Generic Desktop Gamepad matching the
/// DualSense USB wire layout:
/// - Input Report `0x01`: 63 payload bytes (504 bits; 64 bytes total with Report ID)
/// - Output Report `0x02`: 47 payload bytes (376 bits; 48 bytes total with Report ID)
/// - Feature Reports: `0x05` (40B), `0x08` (47B), `0x09` (19B), `0x20` (63B), `0x22` (63B)
pub const HID_REPORT_DESCRIPTOR: &[u8] = &[
    // Usage Page (Generic Desktop), Usage (Game Pad), Collection (Application)
    0x05, 0x01,
    0x09, 0x05,
    0xA1, 0x01,
    // --- Input Report 0x01 (63 payload bytes = 504 bits) ---
    0x85, 0x01, // Report ID (1)
    // Bytes 0..6 (48 bits): X (LX), Y (LY), Z (RX), Rz (RY), Rx (L2), Ry (R2)
    0x09, 0x30, // Usage (X)
    0x09, 0x31, // Usage (Y)
    0x09, 0x32, // Usage (Z)
    0x09, 0x35, // Usage (Rz)
    0x09, 0x33, // Usage (Rx)
    0x09, 0x34, // Usage (Ry)
    0x15, 0x00, // Logical Minimum (0)
    0x26, 0xFF, 0x00, // Logical Maximum (255)
    0x75, 0x08, // Report Size (8)
    0x95, 0x06, // Report Count (6)
    0x81, 0x02, // Input (Data, Var, Abs)
    // Byte 6 (8 bits): Monotonic report sequence counter (Vendor Defined)
    0x06, 0x00, 0xFF, // Usage Page (Vendor Defined 0xFF00)
    0x09, 0x20, // Usage (0x20)
    0x75, 0x08, // Report Size (8)
    0x95, 0x01, // Report Count (1)
    0x81, 0x02, // Input (Data, Var, Abs)
    // Byte 7 low nibble (4 bits): D-Pad Hat Switch (0..7 = N..NW, 8 = Null)
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x39, // Usage (Hat Switch)
    0x15, 0x00, // Logical Minimum (0)
    0x25, 0x07, // Logical Maximum (7)
    0x35, 0x00, // Physical Minimum (0)
    0x46, 0x3B, 0x01, // Physical Maximum (315)
    0x65, 0x14, // Unit (English Rotation: Degrees)
    0x75, 0x04, // Report Size (4)
    0x95, 0x01, // Report Count (1)
    0x81, 0x42, // Input (Data, Var, Abs, Null State)
    // Reset Unit & Physical bounds so they do not apply to subsequent fields
    0x65, 0x00, // Unit (None)
    0x35, 0x00, // Physical Minimum (0)
    0x45, 0x00, // Physical Maximum (0)
    // Byte 7 high nibble (4b) + Byte 8 (8b) + Byte 9 bits 0..1 (2b) = 14 Buttons
    0x05, 0x09, // Usage Page (Button)
    0x19, 0x01, // Usage Minimum (1)
    0x29, 0x0E, // Usage Maximum (14)
    0x15, 0x00, // Logical Minimum (0)
    0x25, 0x01, // Logical Maximum (1)
    0x75, 0x01, // Report Size (1)
    0x95, 0x0E, // Report Count (14)
    0x81, 0x02, // Input (Data, Var, Abs)
    // Byte 9 remaining 6 bits (Mic Mute + Edge/reserved bits; 4 + 14 + 6 = 24 bits)
    0x06, 0x00, 0xFF, // Usage Page (Vendor Defined 0xFF00)
    0x09, 0x21, // Usage (0x21)
    0x15, 0x00, // Logical Minimum (0)
    0x25, 0x3F, // Logical Maximum (63)
    0x75, 0x06, // Report Size (6)
    0x95, 0x01, // Report Count (1)
    0x81, 0x02, // Input (Data, Var, Abs)
    // Bytes 10..63 (53 bytes = 424 bits): IMU, timestamp, touchpad, battery/status payload
    0x09, 0x22, // Usage (0x22)
    0x15, 0x00, // Logical Minimum (0)
    0x26, 0xFF, 0x00, // Logical Maximum (255)
    0x75, 0x08, // Report Size (8)
    0x95, 0x35, // Report Count (53)
    0x81, 0x02, // Input (Data, Var, Abs)
    // --- Output Report 0x02 (47 payload bytes -> 48 bytes total) ---
    0x85, 0x02, // Report ID (2)
    0x09, 0x23, // Usage (0x23)
    0x75, 0x08, // Report Size (8)
    0x95, 0x2F, // Report Count (47)
    0x91, 0x02, // Output (Data, Var, Abs)
    // --- Feature Report 0x05: IMU Calibration (40 payload bytes -> 41 bytes total) ---
    0x85, 0x05, // Report ID (5)
    0x09, 0x24, // Usage (0x24)
    0x95, 0x28, // Report Count (40)
    0xB1, 0x02, // Feature (Data, Var, Abs)
    // --- Feature Report 0x08: Host Config Stub (47 payload bytes -> 48 bytes total) ---
    0x85, 0x08, // Report ID (8)
    0x09, 0x25, // Usage (0x25)
    0x95, 0x2F, // Report Count (47)
    0xB1, 0x02, // Feature (Data, Var, Abs)
    // --- Feature Report 0x09: Pairing / MAC Address (19 payload bytes -> 20 bytes total) ---
    0x85, 0x09, // Report ID (9)
    0x09, 0x26, // Usage (0x26)
    0x95, 0x13, // Report Count (19)
    0xB1, 0x02, // Feature (Data, Var, Abs)
    // --- Feature Report 0x20: Firmware & Hardware Info (63 payload bytes -> 64 bytes total) ---
    0x85, 0x20, // Report ID (0x20)
    0x09, 0x27, // Usage (0x27)
    0x95, 0x3F, // Report Count (63)
    0xB1, 0x02, // Feature (Data, Var, Abs)
    // --- Feature Report 0x22: Extended Info Stub (63 payload bytes -> 64 bytes total) ---
    0x85, 0x22, // Report ID (0x22)
    0x09, 0x28, // Usage (0x28)
    0x95, 0x3F, // Report Count (63)
    0xB1, 0x02, // Feature (Data, Var, Abs)
    // End Collection
    0xC0,
];

const HID_REPORT_LEN_LO: u8 = (HID_REPORT_DESCRIPTOR.len() & 0xFF) as u8;
const HID_REPORT_LEN_HI: u8 = ((HID_REPORT_DESCRIPTOR.len() >> 8) & 0xFF) as u8;

/// Clean-room USB 2.0 Device Descriptor (18 bytes).
pub const DEVICE_DESCRIPTOR: [u8; 18] = [
    18,                          // bLength
    0x01,                        // bDescriptorType (Device)
    0x00,                        // bcdUSB lo (2.00)
    0x02,                        // bcdUSB hi
    0x00,                        // bDeviceClass (defined at interface level)
    0x00,                        // bDeviceSubClass
    0x00,                        // bDeviceProtocol
    64,                          // bMaxPacketSize0
    (SONY_VID & 0xFF) as u8,     // idVendor lo (0x054C)
    (SONY_VID >> 8) as u8,       // idVendor hi
    (DUALSENSE_PID & 0xFF) as u8, // idProduct lo (0x0CE6)
    (DUALSENSE_PID >> 8) as u8,  // idProduct hi
    0x00,                        // bcdDevice lo (1.00)
    0x01,                        // bcdDevice hi
    1,                           // iManufacturer
    2,                           // iProduct
    3,                           // iSerialNumber
    1,                           // bNumConfigurations
];

/// Clean-room USB HID Class Descriptor (9 bytes, `bDescriptorType = 0x21`).
pub const HID_DESCRIPTOR: [u8; 9] = [
    9,                 // bLength
    0x21,              // bDescriptorType (HID)
    0x11,              // bcdHID lo (1.11)
    0x01,              // bcdHID hi
    0x00,              // bCountryCode
    0x01,              // bNumDescriptors
    0x22,              // bDescriptorType (Report)
    HID_REPORT_LEN_LO, // wDescriptorLength lo
    HID_REPORT_LEN_HI, // wDescriptorLength hi
];

/// Clean-room USB 2.0 Configuration Descriptor bundle (41 bytes):
/// Config (9B) + HID Interface 0 (9B) + HID Descriptor (9B) + EP 0x84 IN (7B) + EP 0x03 OUT (7B).
pub const CONFIG_DESCRIPTOR: [u8; 41] = [
    // Configuration Descriptor (9 bytes)
    9,    // bLength
    0x02, // bDescriptorType (Configuration)
    41,   // wTotalLength lo
    0,    // wTotalLength hi
    1,    // bNumInterfaces
    1,    // bConfigurationValue
    0,    // iConfiguration
    0x80, // bmAttributes (Bus-powered)
    0xFA, // bMaxPower (500 mA)
    // Interface Descriptor 0: HID (9 bytes)
    9,    // bLength
    0x04, // bDescriptorType (Interface)
    0,    // bInterfaceNumber
    0,    // bAlternateSetting
    2,    // bNumEndpoints
    0x03, // bInterfaceClass (HID)
    0x00, // bInterfaceSubClass (None)
    0x00, // bInterfaceProtocol (None)
    0,    // iInterface
    // HID Class Descriptor (9 bytes)
    HID_DESCRIPTOR[0],
    HID_DESCRIPTOR[1],
    HID_DESCRIPTOR[2],
    HID_DESCRIPTOR[3],
    HID_DESCRIPTOR[4],
    HID_DESCRIPTOR[5],
    HID_DESCRIPTOR[6],
    HID_DESCRIPTOR[7],
    HID_DESCRIPTOR[8],
    // Endpoint Descriptor: Interrupt IN 0x84 (7 bytes)
    7,    // bLength
    0x05, // bDescriptorType (Endpoint)
    0x84, // bEndpointAddress (EP 4 IN)
    0x03, // bmAttributes (Interrupt)
    64,   // wMaxPacketSize lo
    0,    // wMaxPacketSize hi
    1,    // bInterval (1 ms)
    // Endpoint Descriptor: Interrupt OUT 0x03 (7 bytes)
    7,    // bLength
    0x05, // bDescriptorType (Endpoint)
    0x03, // bEndpointAddress (EP 3 OUT)
    0x03, // bmAttributes (Interrupt)
    64,   // wMaxPacketSize lo
    0,    // wMaxPacketSize hi
    1,    // bInterval (1 ms)
];

fn encode_utf16_string_descriptor(s: &str) -> Vec<u8> {
    // USB string descriptor bLength is u8 (max 254 bytes => max 126 UTF-16 code units).
    let units: Vec<u16> = s.encode_utf16().take(126).collect();
    let total_len = 2 + units.len() * 2;
    let mut out = Vec::with_capacity(total_len);
    out.push(total_len as u8);
    out.push(0x03); // USB_DT_STRING
    for u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// Builds a USB 2.0 String Descriptor for `index`:
/// - `0`: Supported Language IDs (`0x0409` English US)
/// - `1`: Manufacturer (`"DSMaster"`)
/// - `2`: Product (`"Virtual DualSense Controller"`)
/// - `3`: Serial Number (`serial`, e.g. `"DSMASTER-VDS-01"`)
pub fn build_string_descriptor(index: u8, serial: &str) -> Option<Vec<u8>> {
    match index {
        0 => Some(vec![4, 0x03, 0x09, 0x04]),
        1 => Some(encode_utf16_string_descriptor(USB_STRING_MANUFACTURER)),
        2 => Some(encode_utf16_string_descriptor(USB_STRING_PRODUCT)),
        3 => Some(encode_utf16_string_descriptor(serial)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn device_and_config_descriptors_are_internally_consistent() {
        assert_eq!(DEVICE_DESCRIPTOR[0] as usize, DEVICE_DESCRIPTOR.len());
        assert_eq!(DEVICE_DESCRIPTOR[1], 0x01);
        assert_eq!(
            u16::from_le_bytes([DEVICE_DESCRIPTOR[8], DEVICE_DESCRIPTOR[9]]),
            SONY_VID
        );
        assert_eq!(
            u16::from_le_bytes([DEVICE_DESCRIPTOR[10], DEVICE_DESCRIPTOR[11]]),
            DUALSENSE_PID
        );

        assert_eq!(CONFIG_DESCRIPTOR[0], 9);
        assert_eq!(CONFIG_DESCRIPTOR[1], 0x02);
        let w_total = u16::from_le_bytes([CONFIG_DESCRIPTOR[2], CONFIG_DESCRIPTOR[3]]) as usize;
        assert_eq!(w_total, CONFIG_DESCRIPTOR.len());
        assert_eq!(&CONFIG_DESCRIPTOR[18..27], &HID_DESCRIPTOR);

        let hid_rep_len = u16::from_le_bytes([HID_DESCRIPTOR[7], HID_DESCRIPTOR[8]]) as usize;
        assert_eq!(hid_rep_len, HID_REPORT_DESCRIPTOR.len());
    }

    #[test]
    fn hid_report_descriptor_bit_counts_and_structure() {
        let mut idx = 0usize;
        let mut report_id: u8 = 0;
        let mut report_size: usize = 0;
        let mut report_count: usize = 0;
        let mut collection_depth: i32 = 0;

        let mut input_bits: HashMap<u8, usize> = HashMap::new();
        let mut output_bits: HashMap<u8, usize> = HashMap::new();
        let mut feature_bits: HashMap<u8, usize> = HashMap::new();

        while idx < HID_REPORT_DESCRIPTOR.len() {
            let prefix = HID_REPORT_DESCRIPTOR[idx];
            assert_ne!(prefix, 0xFE, "unexpected long HID item");
            let size_code = prefix & 0x03;
            let data_len = match size_code {
                0 => 0,
                1 => 1,
                2 => 2,
                3 => 4,
                _ => unreachable!(),
            };
            idx += 1;
            assert!(idx + data_len <= HID_REPORT_DESCRIPTOR.len());
            let data = &HID_REPORT_DESCRIPTOR[idx..idx + data_len];
            idx += data_len;

            let val: usize = match data_len {
                0 => 0,
                1 => data[0] as usize,
                2 => u16::from_le_bytes([data[0], data[1]]) as usize,
                4 => u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize,
                _ => unreachable!(),
            };

            let item_type = (prefix >> 2) & 0x03;
            let item_tag = (prefix >> 4) & 0x0F;

            match (item_type, item_tag) {
                // Global items
                (1, 0x7) => report_size = val,
                (1, 0x8) => report_id = val as u8,
                (1, 0x9) => report_count = val,
                // Main items
                (0, 0x8) => *input_bits.entry(report_id).or_default() += report_size * report_count,
                (0, 0x9) => {
                    *output_bits.entry(report_id).or_default() += report_size * report_count
                }
                (0, 0xB) => {
                    *feature_bits.entry(report_id).or_default() += report_size * report_count
                }
                (0, 0xA) => collection_depth += 1,
                (0, 0xC) => collection_depth -= 1,
                _ => {}
            }
        }

        assert_eq!(collection_depth, 0, "unbalanced HID collection depth");
        assert_eq!(input_bits.get(&0x01), Some(&(63 * 8))); // 504 bits -> 64B with ID
        assert_eq!(output_bits.get(&0x02), Some(&(47 * 8))); // 376 bits -> 48B with ID
        assert_eq!(feature_bits.get(&0x05), Some(&(40 * 8))); // 41B with ID
        assert_eq!(feature_bits.get(&0x08), Some(&(47 * 8))); // 48B with ID
        assert_eq!(feature_bits.get(&0x09), Some(&(19 * 8))); // 20B with ID
        assert_eq!(feature_bits.get(&0x20), Some(&(63 * 8))); // 64B with ID
        assert_eq!(feature_bits.get(&0x22), Some(&(63 * 8))); // 64B with ID
    }

    #[test]
    fn string_descriptors_encode_langid_and_utf16() {
        assert_eq!(
            build_string_descriptor(0, "DSMASTER-VDS-01"),
            Some(vec![4, 0x03, 0x09, 0x04])
        );

        let mfg = build_string_descriptor(1, "DSMASTER-VDS-01").unwrap();
        assert_eq!(mfg[0] as usize, mfg.len());
        assert_eq!(mfg[1], 0x03);
        let mfg_u16: Vec<u16> = mfg[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(String::from_utf16(&mfg_u16).unwrap(), "DSMaster");

        let prod = build_string_descriptor(2, "DSMASTER-VDS-01").unwrap();
        let prod_u16: Vec<u16> = prod[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(
            String::from_utf16(&prod_u16).unwrap(),
            "Virtual DualSense Controller"
        );

        let ser = build_string_descriptor(3, "DSMASTER-VDS-01").unwrap();
        let ser_u16: Vec<u16> = ser[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(String::from_utf16(&ser_u16).unwrap(), "DSMASTER-VDS-01");

        assert_eq!(build_string_descriptor(4, "DSMASTER-VDS-01"), None);
    }
}
