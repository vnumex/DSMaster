//! Clean-room Rust USB/IP v1.1.1 (`0x0111`) loopback server for Virtual DualSense emulation.
//!
//! Speaks the open USB/IP v1.1.1 wire protocol over `127.0.0.1` (`TCP_NODELAY`) so a
//! user-installed `vadimgrn/usbip-win2` UDE driver can attach DSMaster's virtual DualSense
//! device cleanly without any custom kernel driver.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use parking_lot::Mutex;
use tracing::{debug, trace, warn};

use super::driver::DEFAULT_USBIP_PORT;
use crate::protocol::constants::{
    DS_FEATURE_REPORT_CALIBRATION, DS_INPUT_REPORT_USB_SIZE, DS_OUTPUT_REPORT_USB,
    DS_OUTPUT_REPORT_USB_MIN_SIZE, DUALSENSE_PID, FEATURE_REPORT_FIRMWARE_ID,
    FEATURE_REPORT_PAIRING_ID, SONY_VID, VIRTUAL_SERIAL_PREFIX,
};
use crate::protocol::descriptors::{
    CONFIG_DESCRIPTOR, DEVICE_DESCRIPTOR, HID_DESCRIPTOR, HID_REPORT_DESCRIPTOR,
    build_string_descriptor,
};
use crate::protocol::features::{
    build_calibration_feature_05, build_firmware_feature_20, build_pairing_feature_09,
};
use crate::protocol::input::{BatteryStatus, DPad, RawInputState, encode_usb_input_report};
use crate::protocol::output::{ParsedOutputReport, parse_usb_output_report};

pub const USBIP_VERSION: u16 = 0x0111;

pub const OP_REQ_DEVLIST: u16 = 0x8005;
pub const OP_REP_DEVLIST: u16 = 0x0005;
pub const OP_REQ_IMPORT: u16 = 0x8003;
pub const OP_REP_IMPORT: u16 = 0x0003;

pub const USBIP_CMD_SUBMIT: u32 = 0x0000_0001;
pub const USBIP_CMD_UNLINK: u32 = 0x0000_0002;
pub const USBIP_RET_SUBMIT: u32 = 0x0000_0003;
pub const USBIP_RET_UNLINK: u32 = 0x0000_0004;

pub const USBIP_DIR_OUT: u32 = 0;
pub const USBIP_DIR_IN: u32 = 1;

/// USB 2.0 Full Speed (12 Mbps, 64-byte interrupt packets).
pub const USB_SPEED_FULL: u32 = 2;

const EP_INTERRUPT_IN: u32 = 4;
const EP_INTERRUPT_OUT: u32 = 3;

const ERR_EPIPE: i32 = -32;
const ERR_ECONNRESET: i32 = -104;

const USBIP_HEADER_SIZE: usize = 48;
const MAX_OUT_TRANSFER_BYTES: usize = 65_536;

/// Configuration for a single virtual DualSense USB device slot.
#[derive(Debug, Clone)]
pub struct VirtualDualSenseConfig {
    /// Optional raw 41-byte `0x05` IMU calibration cloned from the physical DualSense.
    pub raw_calibration_05: Option<[u8; 41]>,
    /// Virtual Bluetooth MAC address (MSB-first) returned in Feature Report `0x09`.
    pub virtual_mac: [u8; 6],
    /// USB string serial number (must carry `VIRTUAL_SERIAL_PREFIX` for self-exclusion).
    pub serial_tag: String,
}

impl Default for VirtualDualSenseConfig {
    fn default() -> Self {
        Self {
            raw_calibration_05: None,
            // Locally-administered unicast MAC ("DSM\0\x01").
            virtual_mac: [0x02, 0x44, 0x53, 0x4D, 0x00, 0x01],
            serial_tag: format!("{VIRTUAL_SERIAL_PREFIX}-01"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingInUrb {
    seqnum: u32,
    max_len: usize,
}

struct InPipeState {
    report: [u8; DS_INPUT_REPORT_USB_SIZE],
    generation: u64,
    last_sent_gen: u64,
    pending: VecDeque<PendingInUrb>,
}

struct ControlState {
    configuration: u8,
    alt_setting: u8,
    idle_rate: u8,
    protocol: u8,
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            configuration: 1,
            alt_setting: 0,
            idle_rate: 0,
            protocol: 1, // Report protocol
        }
    }
}

/// Builds a valid centered 64-byte USB Input Report `0x01` for initial enumeration.
fn neutral_usb_input_report() -> [u8; DS_INPUT_REPORT_USB_SIZE] {
    let neutral = RawInputState {
        left_x: 0x80,
        left_y: 0x80,
        right_x: 0x80,
        right_y: 0x80,
        dpad: DPad::Center,
        battery_percent: 100,
        battery_status: BatteryStatus::Full,
        ..Default::default()
    };
    let mut buf = [0u8; DS_INPUT_REPORT_USB_SIZE];
    encode_usb_input_report(&neutral, &mut buf);
    buf
}

/// Merges non-`None` feedback fields into a coalescing mailbox so rapid rumble packets
/// never overwrite pending adaptive trigger or LED updates before the session polls them.
pub(crate) fn merge_parsed_output(
    mailbox: &mut Option<ParsedOutputReport>,
    incoming: ParsedOutputReport,
) {
    if incoming.rumble.is_none()
        && incoming.lightbar_rgb.is_none()
        && incoming.player_leds.is_none()
        && incoming.left_trigger_raw.is_none()
        && incoming.right_trigger_raw.is_none()
    {
        return;
    }

    match mailbox {
        Some(existing) => {
            if incoming.rumble.is_some() {
                existing.rumble = incoming.rumble;
            }
            if incoming.lightbar_rgb.is_some() {
                existing.lightbar_rgb = incoming.lightbar_rgb;
            }
            if incoming.player_leds.is_some() {
                existing.player_leds = incoming.player_leds;
            }
            if incoming.left_trigger_raw.is_some() {
                existing.left_trigger_raw = incoming.left_trigger_raw;
            }
            if incoming.right_trigger_raw.is_some() {
                existing.right_trigger_raw = incoming.right_trigger_raw;
            }
        }
        None => {
            *mailbox = Some(incoming);
        }
    }
}

/// Handle to one exported Virtual DualSense slot inside a [`UsbipServer`].
pub struct VirtualDualSenseSlot {
    bus_id: String,
    busnum: u32,
    devnum: u32,
    #[allow(dead_code)]
    dev_id: u32,
    config: VirtualDualSenseConfig,
    in_pipe: Mutex<InPipeState>,
    feedback: Mutex<Option<ParsedOutputReport>>,
    control: Mutex<ControlState>,
    writer: Mutex<Option<TcpStream>>,
    imported: AtomicBool,
}

impl VirtualDualSenseSlot {
    fn new(slot_index: u32, config: VirtualDualSenseConfig) -> Self {
        let busnum = 1u32;
        let devnum = slot_index + 1;
        let dev_id = (busnum << 16) | (devnum & 0xFFFF);
        let bus_id = format!("{busnum}-{devnum}");

        Self {
            bus_id,
            busnum,
            devnum,
            dev_id,
            config,
            in_pipe: Mutex::new(InPipeState {
                report: neutral_usb_input_report(),
                generation: 1,
                last_sent_gen: 0,
                pending: VecDeque::with_capacity(8),
            }),
            feedback: Mutex::new(None),
            control: Mutex::new(ControlState::default()),
            writer: Mutex::new(None),
            imported: AtomicBool::new(false),
        }
    }

    /// USB/IP bus identifier (e.g. `"1-1"`).
    pub fn bus_id(&self) -> &str {
        &self.bus_id
    }

    /// USB/IP device identifier (`(busnum << 16) | devnum`, e.g. `0x0001_0001`).
    #[allow(dead_code)]
    pub fn dev_id(&self) -> u32 {
        self.dev_id
    }

    /// Serial string configured for this virtual DualSense slot.
    #[allow(dead_code)]
    pub fn serial_tag(&self) -> &str {
        &self.config.serial_tag
    }

    /// Returns `true` if a USB/IP client is currently attached to this slot.
    #[allow(dead_code)]
    pub fn is_imported(&self) -> bool {
        self.imported.load(Ordering::Acquire)
    }

    /// Updates the latest 64-byte USB Input Report `0x01` and immediately completes
    /// a pending Interrupt IN URB if the host already has one in flight.
    pub fn submit_report(&self, report: &[u8; DS_INPUT_REPORT_USB_SIZE]) {
        let maybe_urb = {
            let mut pipe = self.in_pipe.lock();
            pipe.report = *report;
            pipe.generation = pipe.generation.wrapping_add(1);
            if let Some(urb) = pipe.pending.pop_front() {
                pipe.last_sent_gen = pipe.generation;
                Some((urb, pipe.report))
            } else {
                None
            }
        };

        if let Some((urb, payload)) = maybe_urb {
            let send_len = urb.max_len.min(payload.len());
            let mut writer_guard = self.writer.lock();
            if let Some(stream) = writer_guard.as_mut() {
                if let Err(e) =
                    write_ret_submit(stream, urb.seqnum, 0, send_len as u32, &payload[..send_len])
                {
                    trace!(bus_id = %self.bus_id, err = %e, "failed to push interrupt IN report");
                    let _ = stream.shutdown(Shutdown::Both);
                    *writer_guard = None;
                }
            }
        }
    }

    /// Drains and returns the coalesced host feedback state (rumble, lightbar, player LEDs,
    /// adaptive triggers), or `None` if no new host output reports arrived since the last poll.
    pub fn poll_feedback(&self) -> Option<ParsedOutputReport> {
        self.feedback.lock().take()
    }

    /// Actively terminates any imported client socket for this slot and clears pending IN URBs.
    pub fn disconnect_client(&self) {
        self.imported.store(false, Ordering::Release);
        self.in_pipe.lock().pending.clear();
        if let Some(stream) = self.writer.lock().take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    fn encode_usbip_device_record(&self, out: &mut Vec<u8>) {
        // usbip_usb_device is exactly 312 bytes in big-endian wire format.
        let mut path = [0u8; 256];
        let path_str = format!("/sys/devices/dsmaster/usb{}/{}", self.busnum, self.bus_id);
        let path_bytes = path_str.as_bytes();
        let copy_path = path_bytes.len().min(path.len() - 1);
        path[..copy_path].copy_from_slice(&path_bytes[..copy_path]);
        out.extend_from_slice(&path);

        let mut busid = [0u8; 32];
        let bus_bytes = self.bus_id.as_bytes();
        let copy_bus = bus_bytes.len().min(busid.len() - 1);
        busid[..copy_bus].copy_from_slice(&bus_bytes[..copy_bus]);
        out.extend_from_slice(&busid);

        out.extend_from_slice(&self.busnum.to_be_bytes());
        out.extend_from_slice(&self.devnum.to_be_bytes());
        out.extend_from_slice(&USB_SPEED_FULL.to_be_bytes());
        out.extend_from_slice(&SONY_VID.to_be_bytes());
        out.extend_from_slice(&DUALSENSE_PID.to_be_bytes());
        out.extend_from_slice(&0x0100u16.to_be_bytes()); // bcdDevice 1.00
        out.push(0x00); // bDeviceClass (per-interface)
        out.push(0x00); // bDeviceSubClass
        out.push(0x00); // bDeviceProtocol
        out.push(1); // bConfigurationValue
        out.push(1); // bNumConfigurations
        out.push(1); // bNumInterfaces
    }

    fn handle_output_report_payload(&self, report_id_hint: u8, payload: &[u8]) {
        if payload.is_empty() {
            return;
        }
        let parsed = if payload[0] == DS_OUTPUT_REPORT_USB {
            parse_usb_output_report(payload)
        } else if report_id_hint == DS_OUTPUT_REPORT_USB
            && payload.len() >= DS_OUTPUT_REPORT_USB_MIN_SIZE - 1
        {
            let mut prefixed = Vec::with_capacity(payload.len() + 1);
            prefixed.push(DS_OUTPUT_REPORT_USB);
            prefixed.extend_from_slice(payload);
            parse_usb_output_report(&prefixed)
        } else {
            return;
        };

        match parsed {
            Ok(fb) => merge_parsed_output(&mut self.feedback.lock(), fb),
            Err(e) => {
                trace!(bus_id = %self.bus_id, err = %e, "ignored malformed output report 0x02")
            }
        }
    }

    fn handle_control_transfer(
        &self,
        setup: [u8; 8],
        transfer_buffer_length: u32,
        out_data: &[u8],
    ) -> (i32, u32, Vec<u8>) {
        let bm_request_type = setup[0];
        let b_request = setup[1];
        let w_value = u16::from_le_bytes([setup[2], setup[3]]);
        let _w_index = u16::from_le_bytes([setup[4], setup[5]]);
        let w_length = u16::from_le_bytes([setup[6], setup[7]]);

        let req_type = (bm_request_type >> 5) & 0x03;
        let max_len = (w_length as usize).min(transfer_buffer_length as usize);

        match req_type {
            // Standard USB 2.0 requests (Device / Interface / Endpoint)
            0x00 => match b_request {
                // GET_STATUS
                0x00 => {
                    let data = [0x00u8, 0x00u8];
                    let n = data.len().min(max_len);
                    (0, n as u32, data[..n].to_vec())
                }
                // CLEAR_FEATURE / SET_FEATURE
                0x01 | 0x03 => (0, 0, Vec::new()),
                // GET_DESCRIPTOR (includes HID 0x21 and HID Report 0x22 per HID 1.11)
                0x06 => {
                    let desc_type = (w_value >> 8) as u8;
                    let desc_index = (w_value & 0xFF) as u8;
                    let maybe_bytes: Option<Vec<u8>> = match desc_type {
                        0x01 => Some(DEVICE_DESCRIPTOR.to_vec()),
                        0x02 => Some(CONFIG_DESCRIPTOR.to_vec()),
                        0x03 => build_string_descriptor(desc_index, &self.config.serial_tag),
                        0x21 => Some(HID_DESCRIPTOR.to_vec()),
                        0x22 => Some(HID_REPORT_DESCRIPTOR.to_vec()),
                        _ => None,
                    };
                    match maybe_bytes {
                        Some(mut bytes) => {
                            bytes.truncate(max_len);
                            let len = bytes.len() as u32;
                            (0, len, bytes)
                        }
                        None => (ERR_EPIPE, 0, Vec::new()),
                    }
                }
                // GET_CONFIGURATION
                0x08 => {
                    let cfg = self.control.lock().configuration;
                    let n = 1usize.min(max_len);
                    (0, n as u32, if n == 1 { vec![cfg] } else { Vec::new() })
                }
                // SET_CONFIGURATION
                0x09 => {
                    self.control.lock().configuration = (w_value & 0xFF) as u8;
                    (0, 0, Vec::new())
                }
                // GET_INTERFACE
                0x0A => {
                    let alt = self.control.lock().alt_setting;
                    let n = 1usize.min(max_len);
                    (0, n as u32, if n == 1 { vec![alt] } else { Vec::new() })
                }
                // SET_INTERFACE
                0x0B => {
                    self.control.lock().alt_setting = (w_value & 0xFF) as u8;
                    (0, 0, Vec::new())
                }
                _ => (ERR_EPIPE, 0, Vec::new()),
            },
            // HID 1.11 Class requests
            0x01 => match b_request {
                // GET_REPORT
                0x01 => {
                    let report_type = (w_value >> 8) as u8;
                    let report_id = (w_value & 0xFF) as u8;
                    let mut bytes = match report_type {
                        // Input report
                        1 => self.in_pipe.lock().report.to_vec(),
                        // Feature report
                        3 => match report_id {
                            DS_FEATURE_REPORT_CALIBRATION => build_calibration_feature_05(
                                self.config.raw_calibration_05.as_ref(),
                            )
                            .to_vec(),
                            FEATURE_REPORT_PAIRING_ID => {
                                build_pairing_feature_09(self.config.virtual_mac).to_vec()
                            }
                            FEATURE_REPORT_FIRMWARE_ID => build_firmware_feature_20().to_vec(),
                            0x08 => {
                                let mut stub = vec![0u8; 48];
                                stub[0] = 0x08;
                                stub
                            }
                            0x22 => {
                                let mut stub = vec![0u8; 64];
                                stub[0] = 0x22;
                                stub
                            }
                            other => {
                                let mut stub = vec![0u8; max_len];
                                if !stub.is_empty() {
                                    stub[0] = other;
                                }
                                stub
                            }
                        },
                        _ => return (ERR_EPIPE, 0, Vec::new()),
                    };
                    // Clamp cleanly to `wLength.min(transfer_buffer_length)` with `status = 0`
                    // so `usbip-win2` never triggers a short-transfer error on `HidD_GetFeature`.
                    bytes.truncate(max_len);
                    let len = bytes.len() as u32;
                    (0, len, bytes)
                }
                // GET_IDLE
                0x02 => {
                    let idle = self.control.lock().idle_rate;
                    let n = 1usize.min(max_len);
                    (0, n as u32, if n == 1 { vec![idle] } else { Vec::new() })
                }
                // GET_PROTOCOL
                0x03 => {
                    let proto = self.control.lock().protocol;
                    let n = 1usize.min(max_len);
                    (0, n as u32, if n == 1 { vec![proto] } else { Vec::new() })
                }
                // SET_REPORT
                0x09 => {
                    let report_type = (w_value >> 8) as u8;
                    let report_id = (w_value & 0xFF) as u8;
                    if report_type == 2 || out_data.first() == Some(&DS_OUTPUT_REPORT_USB) {
                        self.handle_output_report_payload(report_id, out_data);
                    }
                    (0, transfer_buffer_length, Vec::new())
                }
                // SET_IDLE
                0x0A => {
                    self.control.lock().idle_rate = (w_value >> 8) as u8;
                    (0, 0, Vec::new())
                }
                // SET_PROTOCOL
                0x0B => {
                    self.control.lock().protocol = (w_value & 0xFF) as u8;
                    (0, 0, Vec::new())
                }
                _ => (ERR_EPIPE, 0, Vec::new()),
            },
            _ => (ERR_EPIPE, 0, Vec::new()),
        }
    }
}

struct ServerShared {
    slots: Mutex<HashMap<String, Arc<VirtualDualSenseSlot>>>,
    next_slot_index: AtomicU32,
    shutdown: AtomicBool,
}

/// Pure-Rust loopback USB/IP v1.1.1 server capable of hosting one or more virtual DualSense slots.
pub struct UsbipServer {
    port: u16,
    shared: Arc<ServerShared>,
    listener_thread: Mutex<Option<JoinHandle<()>>>,
}

impl UsbipServer {
    /// Binds a USB/IP v1.1.1 TCP server on `127.0.0.1:<port>` (`0` selects an ephemeral port).
    pub fn bind(port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let actual_port = listener.local_addr()?.port();

        let shared = Arc::new(ServerShared {
            slots: Mutex::new(HashMap::new()),
            next_slot_index: AtomicU32::new(0),
            shutdown: AtomicBool::new(false),
        });

        let thread_shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name(format!("usbip-srv-{actual_port}"))
            .spawn(move || accept_loop(listener, thread_shared))?;

        debug!(
            port = actual_port,
            "started clean-room USB/IP v1.1.1 loopback server"
        );

        Ok(Self {
            port: actual_port,
            shared,
            listener_thread: Mutex::new(Some(handle)),
        })
    }

    /// Binds the standard USB/IP port `3240`, falling back to an ephemeral port if `3240` is in use.
    pub fn bind_default_or_ephemeral() -> io::Result<Self> {
        match Self::bind(DEFAULT_USBIP_PORT) {
            Ok(srv) => Ok(srv),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                warn!(
                    port = DEFAULT_USBIP_PORT,
                    "USB/IP default port already in use; binding ephemeral loopback port"
                );
                Self::bind(0)
            }
            Err(e) => Err(e),
        }
    }

    /// Returns the local TCP port this USB/IP server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Exports a new virtual DualSense slot (`busid = "1-1"`, `"1-2"`, ...).
    pub fn add_slot(&self, config: VirtualDualSenseConfig) -> Arc<VirtualDualSenseSlot> {
        let idx = self.shared.next_slot_index.fetch_add(1, Ordering::Relaxed);
        let slot = Arc::new(VirtualDualSenseSlot::new(idx, config));
        self.shared
            .slots
            .lock()
            .insert(slot.bus_id.clone(), Arc::clone(&slot));
        debug!(bus_id = %slot.bus_id, serial = %slot.config.serial_tag, "exported virtual DualSense slot");
        slot
    }

    /// Removes an exported virtual DualSense slot and drops any active USB/IP connection to it.
    pub fn remove_slot(&self, bus_id: &str) {
        if let Some(slot) = self.shared.slots.lock().remove(bus_id) {
            slot.disconnect_client();
            debug!(bus_id, "removed virtual DualSense slot");
        }
    }

    /// Gracefully stops the listener and all active client worker threads. Idempotent.
    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::Release);

        // Close all active client sockets so client worker threads wake from blocking reads.
        let slots: Vec<Arc<VirtualDualSenseSlot>> = {
            let guard = self.shared.slots.lock();
            guard.values().cloned().collect()
        };
        for slot in slots {
            slot.disconnect_client();
        }

        // Wake the blocking `TcpListener::accept()` loop immediately.
        let _ = TcpStream::connect_timeout(
            &([127, 0, 0, 1], self.port).into(),
            Duration::from_millis(200),
        );

        if let Some(handle) = self.listener_thread.lock().take() {
            let _ = handle.join();
        }
    }
}

impl Drop for UsbipServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<ServerShared>) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();

    for incoming in listener.incoming() {
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        match incoming {
            Ok(stream) => {
                if shared.shutdown.load(Ordering::Acquire) {
                    let _ = stream.shutdown(Shutdown::Both);
                    break;
                }
                let _ = stream.set_nodelay(true);
                let worker_shared = Arc::clone(&shared);
                if let Ok(handle) = thread::Builder::new()
                    .name("usbip-client".to_string())
                    .spawn(move || handle_client(stream, worker_shared))
                {
                    workers.push(handle);
                }
                workers.retain(|h| !h.is_finished());
            }
            Err(e) => {
                if shared.shutdown.load(Ordering::Acquire) {
                    break;
                }
                warn!(err = %e, "USB/IP accept error");
            }
        }
    }

    for worker in workers {
        let _ = worker.join();
    }
}

fn handle_client(mut stream: TcpStream, shared: Arc<ServerShared>) {
    let mut op_hdr = [0u8; 8];
    if stream.read_exact(&mut op_hdr).is_err() {
        return;
    }

    let version = u16::from_be_bytes([op_hdr[0], op_hdr[1]]);
    let command = u16::from_be_bytes([op_hdr[2], op_hdr[3]]);
    if version != USBIP_VERSION {
        trace!(version, "rejecting unsupported USB/IP protocol version");
        return;
    }

    match command {
        OP_REQ_DEVLIST => {
            let slots: Vec<Arc<VirtualDualSenseSlot>> = {
                let guard = shared.slots.lock();
                let mut list: Vec<_> = guard.values().cloned().collect();
                list.sort_by_key(|s| s.devnum);
                list
            };

            let mut resp = Vec::with_capacity(12 + slots.len() * (312 + 4));
            resp.extend_from_slice(&USBIP_VERSION.to_be_bytes());
            resp.extend_from_slice(&OP_REP_DEVLIST.to_be_bytes());
            resp.extend_from_slice(&0u32.to_be_bytes()); // status = OK
            resp.extend_from_slice(&(slots.len() as u32).to_be_bytes());

            for slot in &slots {
                slot.encode_usbip_device_record(&mut resp);
                // usbip_usb_interface (4 bytes): HID class 0x03, subclass 0, protocol 0, pad 0.
                resp.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]);
            }

            let _ = stream.write_all(&resp);
            let _ = stream.flush();
        }
        OP_REQ_IMPORT => {
            let mut busid_buf = [0u8; 32];
            if stream.read_exact(&mut busid_buf).is_err() {
                return;
            }
            let nul_pos = busid_buf
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(busid_buf.len());
            let bus_id = String::from_utf8_lossy(&busid_buf[..nul_pos]).into_owned();

            let slot = shared.slots.lock().get(&bus_id).cloned();
            let Some(slot) = slot else {
                let mut err_resp = [0u8; 8];
                err_resp[0..2].copy_from_slice(&USBIP_VERSION.to_be_bytes());
                err_resp[2..4].copy_from_slice(&OP_REP_IMPORT.to_be_bytes());
                err_resp[4..8].copy_from_slice(&1u32.to_be_bytes()); // status = error
                let _ = stream.write_all(&err_resp);
                return;
            };

            // Replace any stale writer for this slot before installing the new session.
            slot.disconnect_client();

            let Ok(writer_clone) = stream.try_clone() else {
                return;
            };
            let _ = writer_clone.set_nodelay(true);

            let mut resp = Vec::with_capacity(8 + 312);
            resp.extend_from_slice(&USBIP_VERSION.to_be_bytes());
            resp.extend_from_slice(&OP_REP_IMPORT.to_be_bytes());
            resp.extend_from_slice(&0u32.to_be_bytes()); // status = OK
            slot.encode_usbip_device_record(&mut resp);

            if stream.write_all(&resp).is_err() || stream.flush().is_err() {
                return;
            }

            *slot.writer.lock() = Some(writer_clone);
            slot.imported.store(true, Ordering::Release);
            debug!(bus_id = %slot.bus_id, "USB/IP client imported virtual DualSense");

            run_urb_loop(&mut stream, &slot, &shared);

            slot.disconnect_client();
            debug!(bus_id = %slot.bus_id, "USB/IP client disconnected");
        }
        other => {
            trace!(
                command = format_args!("0x{other:04X}"),
                "unknown USB/IP op command"
            );
        }
    }
}

fn run_urb_loop(stream: &mut TcpStream, slot: &VirtualDualSenseSlot, shared: &ServerShared) {
    let mut hdr = [0u8; USBIP_HEADER_SIZE];
    let mut out_buf: Vec<u8> = Vec::with_capacity(256);

    while !shared.shutdown.load(Ordering::Acquire) {
        if stream.read_exact(&mut hdr).is_err() {
            break;
        }

        let command = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let seqnum = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
        let _devid = u32::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]);
        let direction = u32::from_be_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]);
        let ep = u32::from_be_bytes([hdr[16], hdr[17], hdr[18], hdr[19]]);

        match command {
            USBIP_CMD_SUBMIT => {
                let _transfer_flags = u32::from_be_bytes([hdr[20], hdr[21], hdr[22], hdr[23]]);
                let transfer_len = u32::from_be_bytes([hdr[24], hdr[25], hdr[26], hdr[27]]);
                let mut setup = [0u8; 8];
                setup.copy_from_slice(&hdr[40..48]);

                out_buf.clear();
                if direction == USBIP_DIR_OUT && transfer_len > 0 {
                    let len = transfer_len as usize;
                    if len > MAX_OUT_TRANSFER_BYTES {
                        warn!(len, "rejecting oversized USB/IP OUT transfer");
                        break;
                    }
                    out_buf.resize(len, 0);
                    if stream.read_exact(&mut out_buf).is_err() {
                        break;
                    }
                }

                match (ep, direction) {
                    // EP0 Control IN or OUT
                    (0, _) => {
                        let (status, actual_len, payload) =
                            slot.handle_control_transfer(setup, transfer_len, &out_buf);
                        let resp_payload = if direction == USBIP_DIR_IN {
                            payload.as_slice()
                        } else {
                            &[]
                        };
                        if send_ret_submit_locked(slot, seqnum, status, actual_len, resp_payload)
                            .is_err()
                        {
                            break;
                        }
                    }
                    // Interrupt IN EP 0x84 (ep == 4, direction == IN)
                    (EP_INTERRUPT_IN, USBIP_DIR_IN) => {
                        let max_len = (transfer_len as usize).min(DS_INPUT_REPORT_USB_SIZE);
                        let immediate_report = {
                            let mut pipe = slot.in_pipe.lock();
                            if pipe.generation > pipe.last_sent_gen && pipe.pending.is_empty() {
                                pipe.last_sent_gen = pipe.generation;
                                Some(pipe.report)
                            } else {
                                pipe.pending.push_back(PendingInUrb { seqnum, max_len });
                                None
                            }
                        };

                        if let Some(report) = immediate_report {
                            if send_ret_submit_locked(
                                slot,
                                seqnum,
                                0,
                                max_len as u32,
                                &report[..max_len],
                            )
                            .is_err()
                            {
                                break;
                            }
                        }
                    }
                    // Interrupt OUT EP 0x03 (ep == 3, direction == OUT)
                    (EP_INTERRUPT_OUT, USBIP_DIR_OUT) => {
                        slot.handle_output_report_payload(DS_OUTPUT_REPORT_USB, &out_buf);
                        if send_ret_submit_locked(slot, seqnum, 0, transfer_len, &[]).is_err() {
                            break;
                        }
                    }
                    _ => {
                        if send_ret_submit_locked(slot, seqnum, ERR_EPIPE, 0, &[]).is_err() {
                            break;
                        }
                    }
                }
            }
            USBIP_CMD_UNLINK => {
                let unlink_seqnum = u32::from_be_bytes([hdr[20], hdr[21], hdr[22], hdr[23]]);
                let removed = {
                    let mut pipe = slot.in_pipe.lock();
                    if let Some(pos) = pipe.pending.iter().position(|u| u.seqnum == unlink_seqnum) {
                        pipe.pending.remove(pos);
                        true
                    } else {
                        false
                    }
                };
                let status = if removed { ERR_ECONNRESET } else { 0 };
                if send_ret_unlink_locked(slot, seqnum, status).is_err() {
                    break;
                }
            }
            other => {
                warn!(
                    command = format_args!("0x{other:08X}"),
                    "unexpected USB/IP URB command"
                );
                break;
            }
        }
    }
}

fn send_ret_submit_locked(
    slot: &VirtualDualSenseSlot,
    seqnum: u32,
    status: i32,
    actual_length: u32,
    payload: &[u8],
) -> io::Result<()> {
    let mut guard = slot.writer.lock();
    let stream = guard
        .as_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "slot writer disconnected"))?;
    write_ret_submit(stream, seqnum, status, actual_length, payload)
}

fn send_ret_unlink_locked(slot: &VirtualDualSenseSlot, seqnum: u32, status: i32) -> io::Result<()> {
    let mut guard = slot.writer.lock();
    let stream = guard
        .as_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "slot writer disconnected"))?;
    write_ret_unlink(stream, seqnum, status)
}

fn write_ret_submit(
    stream: &mut TcpStream,
    seqnum: u32,
    status: i32,
    actual_length: u32,
    payload: &[u8],
) -> io::Result<()> {
    let mut packet = Vec::with_capacity(USBIP_HEADER_SIZE + payload.len());
    packet.extend_from_slice(&USBIP_RET_SUBMIT.to_be_bytes());
    packet.extend_from_slice(&seqnum.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes()); // devid
    packet.extend_from_slice(&0u32.to_be_bytes()); // direction
    packet.extend_from_slice(&0u32.to_be_bytes()); // ep
    packet.extend_from_slice(&status.to_be_bytes());
    packet.extend_from_slice(&actual_length.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes()); // start_frame
    packet.extend_from_slice(&0u32.to_be_bytes()); // number_of_packets
    packet.extend_from_slice(&0u32.to_be_bytes()); // error_count
    packet.extend_from_slice(&[0u8; 8]); // padding
    packet.extend_from_slice(payload);

    stream.write_all(&packet)?;
    stream.flush()
}

fn write_ret_unlink(stream: &mut TcpStream, seqnum: u32, status: i32) -> io::Result<()> {
    let mut hdr = [0u8; USBIP_HEADER_SIZE];
    hdr[0..4].copy_from_slice(&USBIP_RET_UNLINK.to_be_bytes());
    hdr[4..8].copy_from_slice(&seqnum.to_be_bytes());
    // devid (8..12), direction (12..16), ep (16..20) = 0
    hdr[20..24].copy_from_slice(&status.to_be_bytes());
    // padding (24..48) = 0
    stream.write_all(&hdr)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::constants::Transport;
    use crate::protocol::input::{DualSenseButtons, parse_input_report};
    use crate::protocol::output::{OutputState, encode_output_report};
    use crate::protocol::triggers::AdaptiveTriggerEffect;

    fn connect_client(port: u16) -> TcpStream {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to usbip server");
        stream.set_nodelay(true).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
    }

    fn import_bus_id(stream: &mut TcpStream, bus_id: &str) -> u32 {
        let mut req = Vec::with_capacity(40);
        req.extend_from_slice(&USBIP_VERSION.to_be_bytes());
        req.extend_from_slice(&OP_REQ_IMPORT.to_be_bytes());
        req.extend_from_slice(&0u32.to_be_bytes());
        let mut bus_bytes = [0u8; 32];
        bus_bytes[..bus_id.len()].copy_from_slice(bus_id.as_bytes());
        req.extend_from_slice(&bus_bytes);
        stream.write_all(&req).unwrap();

        let mut hdr = [0u8; 8];
        stream.read_exact(&mut hdr).unwrap();
        assert_eq!(u16::from_be_bytes([hdr[0], hdr[1]]), USBIP_VERSION);
        assert_eq!(u16::from_be_bytes([hdr[2], hdr[3]]), OP_REP_IMPORT);
        let status = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
        if status == 0 {
            let mut dev_rec = [0u8; 312];
            stream.read_exact(&mut dev_rec).unwrap();
            let vid = u16::from_be_bytes([dev_rec[300], dev_rec[301]]);
            let pid = u16::from_be_bytes([dev_rec[302], dev_rec[303]]);
            assert_eq!(vid, SONY_VID);
            assert_eq!(pid, DUALSENSE_PID);
        }
        status
    }

    fn send_cmd_submit(
        stream: &mut TcpStream,
        seqnum: u32,
        devid: u32,
        direction: u32,
        ep: u32,
        transfer_len: u32,
        setup: [u8; 8],
        out_payload: &[u8],
    ) {
        let mut pkt = Vec::with_capacity(USBIP_HEADER_SIZE + out_payload.len());
        pkt.extend_from_slice(&USBIP_CMD_SUBMIT.to_be_bytes());
        pkt.extend_from_slice(&seqnum.to_be_bytes());
        pkt.extend_from_slice(&devid.to_be_bytes());
        pkt.extend_from_slice(&direction.to_be_bytes());
        pkt.extend_from_slice(&ep.to_be_bytes());
        pkt.extend_from_slice(&0u32.to_be_bytes()); // transfer_flags
        pkt.extend_from_slice(&transfer_len.to_be_bytes());
        pkt.extend_from_slice(&0u32.to_be_bytes()); // start_frame
        pkt.extend_from_slice(&0u32.to_be_bytes()); // number_of_packets
        pkt.extend_from_slice(&0u32.to_be_bytes()); // interval
        pkt.extend_from_slice(&setup);
        pkt.extend_from_slice(out_payload);
        stream.write_all(&pkt).unwrap();
    }

    fn read_ret_submit(
        stream: &mut TcpStream,
        expect_in_payload: bool,
    ) -> (u32, i32, u32, Vec<u8>) {
        let mut hdr = [0u8; USBIP_HEADER_SIZE];
        stream.read_exact(&mut hdr).unwrap();
        assert_eq!(
            u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]),
            USBIP_RET_SUBMIT
        );
        let seqnum = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
        let status = i32::from_be_bytes([hdr[20], hdr[21], hdr[22], hdr[23]]);
        let actual_len = u32::from_be_bytes([hdr[24], hdr[25], hdr[26], hdr[27]]);
        let mut payload = Vec::new();
        if expect_in_payload && actual_len > 0 {
            payload.resize(actual_len as usize, 0);
            stream.read_exact(&mut payload).unwrap();
        }
        (seqnum, status, actual_len, payload)
    }

    #[test]
    fn usbip_devlist_and_import_handshake() {
        let server = UsbipServer::bind(0).unwrap();
        let slot1 = server.add_slot(VirtualDualSenseConfig::default());
        let slot2 = server.add_slot(VirtualDualSenseConfig {
            serial_tag: format!("{VIRTUAL_SERIAL_PREFIX}-02"),
            ..Default::default()
        });
        assert_eq!(slot1.bus_id(), "1-1");
        assert_eq!(slot1.dev_id(), 0x0001_0001);
        assert_eq!(slot2.bus_id(), "1-2");
        assert_eq!(slot2.dev_id(), 0x0001_0002);

        // OP_REQ_DEVLIST
        let mut client = connect_client(server.port());
        let mut req = [0u8; 8];
        req[0..2].copy_from_slice(&USBIP_VERSION.to_be_bytes());
        req[2..4].copy_from_slice(&OP_REQ_DEVLIST.to_be_bytes());
        client.write_all(&req).unwrap();

        let mut rep_hdr = [0u8; 12];
        client.read_exact(&mut rep_hdr).unwrap();
        assert_eq!(u16::from_be_bytes([rep_hdr[0], rep_hdr[1]]), USBIP_VERSION);
        assert_eq!(u16::from_be_bytes([rep_hdr[2], rep_hdr[3]]), OP_REP_DEVLIST);
        assert_eq!(
            u32::from_be_bytes([rep_hdr[4], rep_hdr[5], rep_hdr[6], rep_hdr[7]]),
            0
        );
        assert_eq!(
            u32::from_be_bytes([rep_hdr[8], rep_hdr[9], rep_hdr[10], rep_hdr[11]]),
            2
        );

        for expected_bus in ["1-1", "1-2"] {
            let mut dev = [0u8; 312];
            client.read_exact(&mut dev).unwrap();
            let bus_nul = dev[256..288].iter().position(|&b| b == 0).unwrap();
            assert_eq!(&dev[256..256 + bus_nul], expected_bus.as_bytes());
            assert_eq!(
                u32::from_be_bytes([dev[296], dev[297], dev[298], dev[299]]),
                USB_SPEED_FULL
            );
            assert_eq!(u16::from_be_bytes([dev[300], dev[301]]), SONY_VID);
            assert_eq!(u16::from_be_bytes([dev[302], dev[303]]), DUALSENSE_PID);
            assert_eq!(dev[311], 1); // bNumInterfaces

            let mut iface = [0u8; 4];
            client.read_exact(&mut iface).unwrap();
            assert_eq!(iface, [0x03, 0x00, 0x00, 0x00]);
        }

        // OP_REQ_IMPORT unknown busid -> error status 1
        let mut bad_client = connect_client(server.port());
        assert_eq!(import_bus_id(&mut bad_client, "9-9"), 1);

        // OP_REQ_IMPORT valid busid -> status 0
        let mut ok_client = connect_client(server.port());
        assert_eq!(import_bus_id(&mut ok_client, "1-1"), 0);
    }

    #[test]
    fn usbip_ep0_descriptors_and_feature_reports() {
        let server = UsbipServer::bind(0).unwrap();
        let cfg = VirtualDualSenseConfig {
            raw_calibration_05: None,
            virtual_mac: [0x02, 0x44, 0x53, 0x4D, 0x00, 0x07],
            serial_tag: format!("{VIRTUAL_SERIAL_PREFIX}-99"),
        };
        let slot = server.add_slot(cfg.clone());

        let mut client = connect_client(server.port());
        assert_eq!(import_bus_id(&mut client, slot.bus_id()), 0);

        // 1. GET_DESCRIPTOR Device (0x01)
        send_cmd_submit(
            &mut client,
            1,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            18,
            [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 18, 0],
            &[],
        );
        let (seq, st, len, data) = read_ret_submit(&mut client, true);
        assert_eq!((seq, st, len), (1, 0, 18));
        assert_eq!(data, DEVICE_DESCRIPTOR);

        // 2. GET_DESCRIPTOR Config (0x02)
        send_cmd_submit(
            &mut client,
            2,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            64,
            [0x80, 0x06, 0x00, 0x02, 0x00, 0x00, 64, 0],
            &[],
        );
        let (_, st, len, data) = read_ret_submit(&mut client, true);
        assert_eq!((st, len as usize), (0, CONFIG_DESCRIPTOR.len()));
        assert_eq!(data, CONFIG_DESCRIPTOR);

        // 3. GET_DESCRIPTOR HID Report (0x22)
        let rep_len = HID_REPORT_DESCRIPTOR.len() as u16;
        let [rl_lo, rl_hi] = (rep_len + 64).to_le_bytes();
        send_cmd_submit(
            &mut client,
            3,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            (rep_len + 64) as u32,
            [0x81, 0x06, 0x00, 0x22, 0x00, 0x00, rl_lo, rl_hi],
            &[],
        );
        let (_, st, len, data) = read_ret_submit(&mut client, true);
        assert_eq!((st, len as usize), (0, HID_REPORT_DESCRIPTOR.len()));
        assert_eq!(data, HID_REPORT_DESCRIPTOR);

        // 4. GET_DESCRIPTOR String index 3 (Serial) & invalid index 9 (STALL -32)
        send_cmd_submit(
            &mut client,
            4,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            255,
            [0x80, 0x06, 0x03, 0x03, 0x09, 0x04, 255, 0],
            &[],
        );
        let (_, st, _, data) = read_ret_submit(&mut client, true);
        assert_eq!(st, 0);
        assert_eq!(data, build_string_descriptor(3, &cfg.serial_tag).unwrap());

        send_cmd_submit(
            &mut client,
            5,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            64,
            [0x80, 0x06, 0x09, 0x03, 0x09, 0x04, 64, 0],
            &[],
        );
        let (_, st, len, _) = read_ret_submit(&mut client, true);
        assert_eq!((st, len), (ERR_EPIPE, 0));

        // 5. HID GET_REPORT Feature 0x05 (with oversized transfer_buffer_length=64 -> usbip-win2#203 test!)
        send_cmd_submit(
            &mut client,
            6,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            64,
            [0xA1, 0x01, 0x05, 0x03, 0x00, 0x00, 64, 0],
            &[],
        );
        let (_, st, len, data) = read_ret_submit(&mut client, true);
        assert_eq!((st, len), (0, 41));
        assert_eq!(data, build_calibration_feature_05(None));

        // 6. HID GET_REPORT Feature 0x09 (MAC) & clamped short wLength=8
        send_cmd_submit(
            &mut client,
            7,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            20,
            [0xA1, 0x01, 0x09, 0x03, 0x00, 0x00, 20, 0],
            &[],
        );
        let (_, st, len, data) = read_ret_submit(&mut client, true);
        assert_eq!((st, len), (0, 20));
        assert_eq!(data, build_pairing_feature_09(cfg.virtual_mac));

        send_cmd_submit(
            &mut client,
            8,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            8,
            [0xA1, 0x01, 0x09, 0x03, 0x00, 0x00, 8, 0],
            &[],
        );
        let (_, st, len, data) = read_ret_submit(&mut client, true);
        assert_eq!((st, len), (0, 8));
        assert_eq!(data, &build_pairing_feature_09(cfg.virtual_mac)[..8]);

        // 7. HID GET_REPORT Feature 0x20 (Firmware Info)
        send_cmd_submit(
            &mut client,
            9,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            64,
            [0xA1, 0x01, 0x20, 0x03, 0x00, 0x00, 64, 0],
            &[],
        );
        let (_, st, len, data) = read_ret_submit(&mut client, true);
        assert_eq!((st, len), (0, 64));
        assert_eq!(data, build_firmware_feature_20());

        // 8. SET_CONFIGURATION + GET_CONFIGURATION
        send_cmd_submit(
            &mut client,
            10,
            slot.dev_id(),
            USBIP_DIR_OUT,
            0,
            0,
            [0x00, 0x09, 0x01, 0x00, 0x00, 0x00, 0, 0],
            &[],
        );
        let (_, st, len, _) = read_ret_submit(&mut client, false);
        assert_eq!((st, len), (0, 0));

        send_cmd_submit(
            &mut client,
            11,
            slot.dev_id(),
            USBIP_DIR_IN,
            0,
            1,
            [0x80, 0x08, 0x00, 0x00, 0x00, 0x00, 1, 0],
            &[],
        );
        let (_, st, len, data) = read_ret_submit(&mut client, true);
        assert_eq!((st, len, data), (0, 1, vec![1]));
    }

    #[test]
    fn usbip_interrupt_in_out_coalescing_and_unlink() {
        let server = UsbipServer::bind(0).unwrap();
        let slot = server.add_slot(VirtualDualSenseConfig::default());

        let mut client = connect_client(server.port());
        assert_eq!(import_bus_id(&mut client, slot.bus_id()), 0);

        // 1. First Interrupt IN URB completes immediately with the initial centered neutral report.
        send_cmd_submit(
            &mut client,
            100,
            slot.dev_id(),
            USBIP_DIR_IN,
            EP_INTERRUPT_IN,
            64,
            [0; 8],
            &[],
        );
        let (seq, st, len, data) = read_ret_submit(&mut client, true);
        assert_eq!((seq, st, len), (100, 0, 64));
        let parsed0 = parse_input_report(&data, Transport::Usb, false).unwrap();
        assert_eq!(parsed0.left_x, 0x80);
        assert_eq!(parsed0.left_y, 0x80);
        assert_eq!(parsed0.dpad, DPad::Center);

        // 2. Second Interrupt IN URB stays pending until `slot.submit_report` pushes a fresh frame.
        send_cmd_submit(
            &mut client,
            101,
            slot.dev_id(),
            USBIP_DIR_IN,
            EP_INTERRUPT_IN,
            64,
            [0; 8],
            &[],
        );

        let live_state = RawInputState {
            left_x: 0xFF,
            left_y: 0x10,
            right_x: 0x40,
            right_y: 0xC0,
            l2_analog: 180,
            r2_analog: 255,
            seq_number: 7,
            dpad: DPad::NorthEast,
            buttons: DualSenseButtons::CROSS | DualSenseButtons::R1,
            ..Default::default()
        };
        let mut encoded = [0u8; 64];
        encode_usb_input_report(&live_state, &mut encoded);

        let slot_for_push = Arc::clone(&slot);
        let pusher = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            slot_for_push.submit_report(&encoded);
        });

        let (seq, st, len, data) = read_ret_submit(&mut client, true);
        pusher.join().unwrap();
        assert_eq!((seq, st, len), (101, 0, 64));
        let parsed1 = parse_input_report(&data, Transport::Usb, false).unwrap();
        assert_eq!(parsed1.left_x, 0xFF);
        assert_eq!(parsed1.r2_analog, 255);
        assert_eq!(parsed1.dpad, DPad::NorthEast);
        assert_eq!(
            parsed1.buttons,
            DualSenseButtons::CROSS | DualSenseButtons::R1
        );

        // 3. Interrupt OUT EP 0x03: trigger-only followed by rumble-only must coalesce cleanly!
        let trig_effect = AdaptiveTriggerEffect::Weapon {
            start_position: 2,
            end_position: 6,
            strength: 7,
        };
        let mut out_trig = [0u8; 48];
        encode_output_report(
            &OutputState {
                update_right_trigger: true,
                right_trigger: trig_effect,
                ..Default::default()
            },
            Transport::Usb,
            0,
            &mut out_trig,
        )
        .unwrap();
        send_cmd_submit(
            &mut client,
            200,
            slot.dev_id(),
            USBIP_DIR_OUT,
            EP_INTERRUPT_OUT,
            48,
            [0; 8],
            &out_trig,
        );
        let (seq, st, len, _) = read_ret_submit(&mut client, false);
        assert_eq!((seq, st, len), (200, 0, 48));

        let mut out_rumble = [0u8; 48];
        encode_output_report(
            &OutputState {
                update_rumble: true,
                motor_left: 220,
                motor_right: 90,
                ..Default::default()
            },
            Transport::Usb,
            0,
            &mut out_rumble,
        )
        .unwrap();
        send_cmd_submit(
            &mut client,
            201,
            slot.dev_id(),
            USBIP_DIR_OUT,
            EP_INTERRUPT_OUT,
            48,
            [0; 8],
            &out_rumble,
        );
        let (seq, st, len, _) = read_ret_submit(&mut client, false);
        assert_eq!((seq, st, len), (201, 0, 48));

        // 4. EP0 SET_REPORT Output 0x02 lightbar merges into the same coalesced mailbox!
        let mut out_rgb = [0u8; 48];
        encode_output_report(
            &OutputState {
                update_lightbar: true,
                lightbar_rgb: [15, 160, 250],
                ..Default::default()
            },
            Transport::Usb,
            0,
            &mut out_rgb,
        )
        .unwrap();
        send_cmd_submit(
            &mut client,
            202,
            slot.dev_id(),
            USBIP_DIR_OUT,
            0,
            48,
            [0x21, 0x09, 0x02, 0x02, 0x00, 0x00, 48, 0],
            &out_rgb,
        );
        let (seq, st, len, _) = read_ret_submit(&mut client, false);
        assert_eq!((seq, st, len), (202, 0, 48));

        let merged = slot.poll_feedback().expect("coalesced feedback available");
        assert_eq!(merged.right_trigger_raw, Some(trig_effect.encode()));
        assert_eq!(merged.rumble, Some((220, 90)));
        assert_eq!(merged.lightbar_rgb, Some([15, 160, 250]));
        assert_eq!(slot.poll_feedback(), None);

        // 5. USBIP_CMD_UNLINK cancels a pending Interrupt IN URB with -ECONNRESET (-104).
        send_cmd_submit(
            &mut client,
            300,
            slot.dev_id(),
            USBIP_DIR_IN,
            EP_INTERRUPT_IN,
            64,
            [0; 8],
            &[],
        );

        let mut unlink_pkt = [0u8; USBIP_HEADER_SIZE];
        unlink_pkt[0..4].copy_from_slice(&USBIP_CMD_UNLINK.to_be_bytes());
        unlink_pkt[4..8].copy_from_slice(&301u32.to_be_bytes());
        unlink_pkt[8..12].copy_from_slice(&slot.dev_id().to_be_bytes());
        unlink_pkt[20..24].copy_from_slice(&300u32.to_be_bytes()); // unlink_seqnum
        client.write_all(&unlink_pkt).unwrap();

        let mut ret_unlink = [0u8; USBIP_HEADER_SIZE];
        client.read_exact(&mut ret_unlink).unwrap();
        assert_eq!(
            u32::from_be_bytes([ret_unlink[0], ret_unlink[1], ret_unlink[2], ret_unlink[3]]),
            USBIP_RET_UNLINK
        );
        assert_eq!(
            u32::from_be_bytes([ret_unlink[4], ret_unlink[5], ret_unlink[6], ret_unlink[7]]),
            301
        );
        assert_eq!(
            i32::from_be_bytes([
                ret_unlink[20],
                ret_unlink[21],
                ret_unlink[22],
                ret_unlink[23]
            ]),
            ERR_ECONNRESET
        );

        // Unlinking an already-cancelled seqnum returns status 0.
        unlink_pkt[4..8].copy_from_slice(&302u32.to_be_bytes());
        client.write_all(&unlink_pkt).unwrap();
        client.read_exact(&mut ret_unlink).unwrap();
        assert_eq!(
            i32::from_be_bytes([
                ret_unlink[20],
                ret_unlink[21],
                ret_unlink[22],
                ret_unlink[23]
            ]),
            0
        );
    }
}
