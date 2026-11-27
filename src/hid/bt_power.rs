use std::ffi::c_void;

use tracing::{info, warn};
use windows::Win32::Devices::Bluetooth::{
    BLUETOOTH_FIND_RADIO_PARAMS, BluetoothFindFirstRadio, BluetoothFindNextRadio,
    BluetoothFindRadioClose,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::IO::DeviceIoControl;

/// `CTL_CODE(FILE_DEVICE_BLUETOOTH, 0x003, METHOD_BUFFERED, FILE_ANY_ACCESS)`
const IOCTL_BTH_DISCONNECT_DEVICE: u32 = 0x0041_000C;

/// Converts a standard MSB-first 6-byte MAC address (`[b0, b1, b2, b3, b4, b5]`)
/// into Windows' `BTH_ADDR` (`u64`) representation.
#[inline]
pub fn mac_to_bth_addr(mac: [u8; 6]) -> u64 {
    ((mac[0] as u64) << 40)
        | ((mac[1] as u64) << 32)
        | ((mac[2] as u64) << 24)
        | ((mac[3] as u64) << 16)
        | ((mac[4] as u64) << 8)
        | (mac[5] as u64)
}

/// Disconnects a Bluetooth device by its 6-byte MAC address via `IOCTL_BTH_DISCONNECT_DEVICE`
/// on every local Windows Bluetooth radio.
///
/// Dropping the ACL link causes a DualSense / DualSense Edge controller to power itself off
/// immediately to conserve battery.
pub fn disconnect_bluetooth_device(mac: [u8; 6]) -> Result<(), String> {
    if mac == [0; 6] {
        return Err("Bluetooth MAC address unavailable".to_owned());
    }
    let mut bth_addr = mac_to_bth_addr(mac);
    let params = BLUETOOTH_FIND_RADIO_PARAMS {
        dwSize: std::mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
    };

    let mut radio = HANDLE::default();
    let find_handle = unsafe { BluetoothFindFirstRadio(&params, &mut radio) }
        .map_err(|e| format!("No Bluetooth radio found: {e}"))?;

    let mut disconnected = false;
    loop {
        if !radio.is_invalid() {
            let mut bytes_returned: u32 = 0;
            let ok = unsafe {
                DeviceIoControl(
                    radio,
                    IOCTL_BTH_DISCONNECT_DEVICE,
                    Some((&mut bth_addr as *mut u64).cast::<c_void>()),
                    std::mem::size_of::<u64>() as u32,
                    None,
                    0,
                    Some(&mut bytes_returned),
                    None,
                )
            };
            unsafe {
                let _ = CloseHandle(radio);
            }
            if ok.is_ok() {
                disconnected = true;
                info!(
                    mac = format!(
                        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                    ),
                    "Disconnected Bluetooth controller via IOCTL_BTH_DISCONNECT_DEVICE"
                );
                break;
            }
        }

        radio = HANDLE::default();
        if unsafe { BluetoothFindNextRadio(find_handle, &mut radio) }.is_err() {
            break;
        }
    }

    unsafe {
        let _ = BluetoothFindRadioClose(find_handle);
    }

    if disconnected {
        Ok(())
    } else {
        warn!(
            mac = format!(
                "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            ),
            "IOCTL_BTH_DISCONNECT_DEVICE did not succeed on any local Bluetooth radio"
        );
        Err("Failed to disconnect Bluetooth device".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_to_bth_addr_packs_msb_first() {
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        assert_eq!(mac_to_bth_addr(mac), 0x0000_AABB_CCDD_EEFF);
    }

    #[test]
    fn zero_mac_is_rejected() {
        assert!(disconnect_bluetooth_device([0; 6]).is_err());
    }
}
