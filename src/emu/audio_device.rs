//! Clean-room UAC 1.0 Virtual Windows Audio Device (Speakers + Microphone) over USB/IP v1.1.1.
//!
//! Exposes a full-duplex USB Audio Class 1.0 device (`0x1209:0xD5A0`, `"DualSense Wireless Audio"`)
//! via user-installed `vadimgrn/usbip-win2` so Windows creates native `Speakers` (48 kHz 16-bit
//! stereo) and `Microphone` (48 kHz 16-bit mono) endpoints backed by `SharedAudioBridge`.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use parking_lot::Mutex;
use tracing::{debug, trace, warn};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, SetWaitableTimerEx,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};


use super::dualsense::driver::{attach_local, detach_port, is_usbip_win2_installed};
use crate::protocol::constants::DS_BT_MIC_OPUS_BYTES;

const USBIP_VERSION: u16 = 0x0111;
const OP_REQ_DEVLIST: u16 = 0x8005;
const OP_REP_DEVLIST: u16 = 0x0005;
const OP_REQ_IMPORT: u16 = 0x8003;
const OP_REP_IMPORT: u16 = 0x0003;

const USBIP_CMD_SUBMIT: u32 = 0x0000_0001;
const USBIP_CMD_UNLINK: u32 = 0x0000_0002;
const USBIP_RET_SUBMIT: u32 = 0x0000_0003;
const USBIP_RET_UNLINK: u32 = 0x0000_0004;

const USBIP_DIR_OUT: u32 = 0;
const USBIP_DIR_IN: u32 = 1;
const USB_SPEED_FULL: u32 = 2;

const ERR_EPIPE: i32 = -32;
const ERR_ECONNRESET: i32 = -104;

const USBIP_HEADER_SIZE: usize = 48;
const MAX_OUT_TRANSFER_BYTES: usize = 65_536;
const MAX_ISO_PACKETS: usize = 128;

const EP_ISO_OUT: u32 = 1;
const EP_ISO_IN: u32 = 2;

const AUDIO_VID: u16 = 0x1209;
const AUDIO_PID: u16 = 0xD5A0;
const AUDIO_MANUFACTURER: &str = "DSMaster";
const AUDIO_PRODUCT: &str = "DualSense Wireless Audio";
const AUDIO_SERIAL: &str = "DSMASTER-VAUDIO-01";

const RING_CAP_FRAMES: usize = 9600;
const RING_LAG_LIMIT: usize = 4800;
const SPEAKER_TRIM_TARGET: usize = 1920;
const SPEAKER_PREBUFFER_FRAMES: usize = 960;
const MIC_TRIM_TARGET: usize = 960;

/// Thread-safe audio bridge shared between the virtual USB/IP audio device and the Bluetooth worker.
pub struct SharedAudioBridge {
    pub speaker_ring: Mutex<VecDeque<(f32, f32)>>,
    pub mic_ring: Mutex<VecDeque<f32>>,
    pub mic_opus_tx: Sender<[u8; DS_BT_MIC_OPUS_BYTES]>,
    pub mic_opus_rx: Receiver<[u8; DS_BT_MIC_OPUS_BYTES]>,
    pub speaker_endpoint_open: AtomicBool,
    pub mic_endpoint_open: AtomicBool,
    pub virtual_audio_attached: AtomicBool,
    speaker_primed: AtomicBool,
}

impl SharedAudioBridge {
    pub fn new() -> Self {
        let (mic_opus_tx, mic_opus_rx) = bounded(32);
        Self {
            speaker_ring: Mutex::new(VecDeque::with_capacity(RING_CAP_FRAMES)),
            mic_ring: Mutex::new(VecDeque::with_capacity(RING_CAP_FRAMES)),
            mic_opus_tx,
            mic_opus_rx,
            speaker_endpoint_open: AtomicBool::new(false),
            mic_endpoint_open: AtomicBool::new(false),
            virtual_audio_attached: AtomicBool::new(false),
            speaker_primed: AtomicBool::new(false),
        }
    }

    pub fn clear_speaker(&self) {
        self.speaker_ring.lock().clear();
        self.speaker_primed.store(false, Ordering::Release);
    }

    pub fn push_speaker_i16_stereo(&self, bytes: &[u8], gain: f32, muted: bool) {
        let mut ring = self.speaker_ring.lock();
        let scale = if muted { 0.0 } else { gain } / 32768.0;
        for frame in bytes.chunks_exact(4) {
            let l = i16::from_le_bytes([frame[0], frame[1]]) as f32 * scale;
            let r = i16::from_le_bytes([frame[2], frame[3]]) as f32 * scale;
            if ring.len() >= RING_CAP_FRAMES {
                ring.pop_front();
            }
            ring.push_back((l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0)));
        }
        if ring.len() > RING_LAG_LIMIT {
            let drop_n = ring.len() - SPEAKER_TRIM_TARGET;
            ring.drain(..drop_n);
        }
    }

    pub fn pull_speaker_512(&self, out: &mut [f32; 512 * 2]) -> bool {
        let mut ring = self.speaker_ring.lock();
        if !self.speaker_primed.load(Ordering::Acquire) {
            if ring.len() < SPEAKER_PREBUFFER_FRAMES {
                out.fill(0.0);
                return false;
            }
            self.speaker_primed.store(true, Ordering::Release);
        }
        if ring.is_empty() {
            self.speaker_primed.store(false, Ordering::Release);
            out.fill(0.0);
            return false;
        }
        let avail = ring.len().min(512);
        for i in 0..avail {
            let (l, r) = ring.pop_front().unwrap_or((0.0, 0.0));
            out[i * 2] = l;
            out[i * 2 + 1] = r;
        }
        if avail < 512 {
            out[avail * 2..].fill(0.0);
            self.speaker_primed.store(false, Ordering::Release);
        }
        true
    }

    pub fn push_mic_mono_f32(&self, samples: &[f32]) {
        let mut ring = self.mic_ring.lock();
        for &s in samples {
            if ring.len() >= RING_CAP_FRAMES {
                ring.pop_front();
            }
            ring.push_back(s.clamp(-1.0, 1.0));
        }
        if ring.len() > RING_LAG_LIMIT {
            let drop_n = ring.len() - MIC_TRIM_TARGET;
            ring.drain(..drop_n);
        }
    }

    pub fn pull_mic_i16_mono(
        &self,
        sample_count: usize,
        gain: f32,
        muted: bool,
        out_bytes: &mut Vec<u8>,
    ) {
        out_bytes.clear();
        out_bytes.reserve(sample_count * 2);
        let mut ring = self.mic_ring.lock();
        let scale = if muted { 0.0 } else { gain } * 32767.0;
        for _ in 0..sample_count {
            let s = ring.pop_front().unwrap_or(0.0);
            let pcm = (s * scale).round().clamp(-32768.0, 32767.0) as i16;
            out_bytes.extend_from_slice(&pcm.to_le_bytes());
        }
    }
}

impl Default for SharedAudioBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Clean-room USB 1.1 Device Descriptor (18 bytes) for UAC 1.0 Virtual DualSense Audio.
pub const AUDIO_DEVICE_DESCRIPTOR: [u8; 18] = [
    18,                         // bLength
    0x01,                       // bDescriptorType (Device)
    0x10,                       // bcdUSB lo (1.10)
    0x01,                       // bcdUSB hi
    0x00,                       // bDeviceClass (defined at interface level)
    0x00,                       // bDeviceSubClass
    0x00,                       // bDeviceProtocol
    64,                         // bMaxPacketSize0
    (AUDIO_VID & 0xFF) as u8,   // idVendor lo
    (AUDIO_VID >> 8) as u8,     // idVendor hi
    (AUDIO_PID & 0xFF) as u8,   // idProduct lo
    (AUDIO_PID >> 8) as u8,     // idProduct hi
    0x00,                       // bcdDevice lo (1.00)
    0x01,                       // bcdDevice hi
    1,                          // iManufacturer
    2,                          // iProduct
    3,                          // iSerialNumber
    1,                          // bNumConfigurations
];

/// Clean-room UAC 1.0 Configuration Descriptor (193 bytes total):
/// - Interface 0: AudioControl (Header + Speaker IT1->FU2->OT3 + Mic IT4->FU5->OT6, CS length = 71B)
/// - Interface 1: AudioStreaming OUT (48 kHz 16-bit Stereo, EP 0x01 ISO OUT 192B/ms)
/// - Interface 2: AudioStreaming IN (48 kHz 16-bit Mono, EP 0x82 ISO IN 96B/ms)
pub const AUDIO_CONFIG_DESCRIPTOR: [u8; 193] = [
    // Configuration Descriptor (9B)
    9, 0x02, 193, 0, 3, 1, 0, 0x80, 0xFA,
    // Interface 0: AudioControl (9B)
    9, 0x04, 0, 0, 0, 0x01, 0x01, 0x00, 0,
    // Class-Specific AC Interface Header (10B, wTotalLength = 71)
    10, 0x24, 0x01, 0x00, 0x01, 71, 0, 2, 1, 2,
    // Input Terminal 1: USB Streaming -> Speaker (12B)
    12, 0x24, 0x02, 1, 0x01, 0x01, 0, 2, 0x03, 0x00, 0, 0,
    // Feature Unit 2: Speaker Mute + Volume (10B)
    10, 0x24, 0x06, 2, 1, 1, 0x03, 0x00, 0x00, 0,
    // Output Terminal 3: Speaker (9B)
    9, 0x24, 0x03, 3, 0x01, 0x03, 0, 2, 0,
    // Input Terminal 4: Microphone (12B)
    12, 0x24, 0x02, 4, 0x01, 0x02, 0, 1, 0x00, 0x00, 0, 0,
    // Feature Unit 5: Microphone Mute + Volume (9B)
    9, 0x24, 0x06, 5, 4, 1, 0x03, 0x00, 0,
    // Output Terminal 6: Microphone -> USB Streaming (9B)
    9, 0x24, 0x03, 6, 0x01, 0x01, 0, 5, 0,

    // Interface 1 Alt 0: Speaker Zero-Bandwidth (9B)
    9, 0x04, 1, 0, 0, 0x01, 0x02, 0x00, 0,
    // Interface 1 Alt 1: Speaker Active (9B)
    9, 0x04, 1, 1, 1, 0x01, 0x02, 0x00, 0,
    // Class-Specific AS General (7B, bTerminalLink = 1, PCM = 0x0001)
    7, 0x24, 0x01, 1, 1, 0x01, 0x00,
    // Class-Specific AS Format Type I (11B: 2ch, 2B subframe, 16-bit, 48000 Hz)
    11, 0x24, 0x02, 0x01, 2, 2, 16, 1, 0x80, 0xBB, 0x00,
    // Standard ISO OUT Endpoint 0x01 (9B: Iso + Adaptive = 0x09, wMaxPacketSize = 192, bInterval = 1)
    9, 0x05, 0x01, 0x09, 192, 0, 1, 0, 0,
    // Class-Specific AS Isochronous Audio Data Endpoint (7B)
    7, 0x25, 0x01, 0x01, 0, 0, 0,

    // Interface 2 Alt 0: Microphone Zero-Bandwidth (9B)
    9, 0x04, 2, 0, 0, 0x01, 0x02, 0x00, 0,
    // Interface 2 Alt 1: Microphone Active (9B)
    9, 0x04, 2, 1, 1, 0x01, 0x02, 0x00, 0,
    // Class-Specific AS General (7B, bTerminalLink = 6, PCM = 0x0001)
    7, 0x24, 0x01, 6, 1, 0x01, 0x00,
    // Class-Specific AS Format Type I (11B: 1ch, 2B subframe, 16-bit, 48000 Hz)
    11, 0x24, 0x02, 0x01, 1, 2, 16, 1, 0x80, 0xBB, 0x00,
    // Standard ISO IN Endpoint 0x82 (9B: Iso + Async = 0x05, wMaxPacketSize = 96, bInterval = 1)
    9, 0x05, 0x82, 0x05, 96, 0, 1, 0, 0,
    // Class-Specific AS Isochronous Audio Data Endpoint (7B)
    7, 0x25, 0x01, 0x01, 0, 0, 0,
];

#[derive(Debug, Clone, Copy)]
struct IsoPacketDesc {
    offset: u32,
    length: u32,
    actual_length: u32,
    status: u32,
}

impl IsoPacketDesc {
    fn from_be_bytes(b: &[u8]) -> Self {
        Self {
            offset: u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            length: u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
            actual_length: u32::from_be_bytes([b[8], b[9], b[10], b[11]]),
            status: u32::from_be_bytes([b[12], b[13], b[14], b[15]]),
        }
    }

    fn write_be_bytes(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.offset.to_be_bytes());
        out.extend_from_slice(&self.length.to_be_bytes());
        out.extend_from_slice(&self.actual_length.to_be_bytes());
        out.extend_from_slice(&self.status.to_be_bytes());
    }
}

struct PendingIsoUrb {
    seqnum: u32,
    start_frame: u32,
    due: Instant,
    packets: Vec<IsoPacketDesc>,
}

struct UacControlState {
    configuration: u8,
    alt_settings: [u8; 3],
    spk_mute: bool,
    spk_vol_q8: i16, // 0x0000 = 0 dB, min -64 dB = -0x4000
    mic_mute: bool,
    mic_vol_q8: i16,
    sample_rate: u32,
}

impl Default for UacControlState {
    fn default() -> Self {
        Self {
            configuration: 1,
            alt_settings: [0, 0, 0],
            spk_mute: false,
            spk_vol_q8: 0,
            mic_mute: false,
            mic_vol_q8: 0,
            sample_rate: 48_000,
        }
    }
}

impl UacControlState {
    #[inline]
    fn spk_gain(&self) -> f32 {
        db_q8_to_linear(self.spk_vol_q8)
    }

    #[inline]
    fn mic_gain(&self) -> f32 {
        db_q8_to_linear(self.mic_vol_q8)
    }
}

#[inline]
fn encode_utf16_string(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + s.len() * 2);
    out.push(0);
    out.push(0x03);
    for ch in s.encode_utf16() {
        out.extend_from_slice(&ch.to_le_bytes());
    }
    out[0] = out.len().min(254) as u8;
    out
}

#[inline]
fn db_q8_to_linear(vol_q8: i16) -> f32 {
    if vol_q8 <= -0x4000 {
        return 0.0;
    }
    let db = (vol_q8 as f32) / 256.0;
    10.0_f32.powf(db / 20.0).clamp(0.0, 2.0)
}

struct AudioServerShared {
    bridge: Arc<SharedAudioBridge>,
    control: Mutex<UacControlState>,
    iso_out_pending: Mutex<VecDeque<PendingIsoUrb>>,
    iso_in_pending: Mutex<VecDeque<PendingIsoUrb>>,
    client_writer: Mutex<Option<Arc<Mutex<TcpStream>>>>,
    shutdown: AtomicBool,
}

impl AudioServerShared {
    fn disconnect_client(&self) {
        if let Some(w) = self.client_writer.lock().take() {
            let stream = w.lock();
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.iso_out_pending.lock().clear();
        self.iso_in_pending.lock().clear();
        self.bridge
            .speaker_endpoint_open
            .store(false, Ordering::Release);
        self.bridge.clear_speaker();
        self.bridge
            .mic_endpoint_open
            .store(false, Ordering::Release);
    }

    fn handle_control_transfer(
        &self,
        setup: [u8; 8],
        transfer_len: u32,
        out_data: &[u8],
    ) -> (i32, u32, Vec<u8>) {
        let bm_request_type = setup[0];
        let b_request = setup[1];
        let w_value = u16::from_le_bytes([setup[2], setup[3]]);
        let w_index = u16::from_le_bytes([setup[4], setup[5]]);
        let w_length = u16::from_le_bytes([setup[6], setup[7]]) as usize;
        let max_len = w_length.min(transfer_len as usize);
        let req_type = (bm_request_type >> 5) & 0x03;

        match req_type {
            // Standard USB requests
            0x00 => match b_request {
                // GET_STATUS
                0x00 => {
                    let n = 2usize.min(max_len);
                    (0, n as u32, vec![0u8; n])
                }
                // CLEAR_FEATURE / SET_FEATURE
                0x01 | 0x03 => (0, 0, Vec::new()),
                // GET_DESCRIPTOR
                0x06 => {
                    let desc_type = (w_value >> 8) as u8;
                    let desc_idx = (w_value & 0xFF) as u8;
                    let mut bytes = match desc_type {
                        0x01 => AUDIO_DEVICE_DESCRIPTOR.to_vec(),
                        0x02 => AUDIO_CONFIG_DESCRIPTOR.to_vec(),
                        0x03 => match desc_idx {
                            0 => vec![4, 0x03, 0x09, 0x04],
                            1 => encode_utf16_string(AUDIO_MANUFACTURER),
                            2 => encode_utf16_string(AUDIO_PRODUCT),
                            3 => encode_utf16_string(AUDIO_SERIAL),
                            _ => return (ERR_EPIPE, 0, Vec::new()),
                        },
                        _ => return (ERR_EPIPE, 0, Vec::new()),
                    };
                    bytes.truncate(max_len);
                    let len = bytes.len() as u32;
                    (0, len, bytes)
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
                    let iface = (w_index & 0xFF) as usize;
                    let alt = self
                        .control
                        .lock()
                        .alt_settings
                        .get(iface)
                        .copied()
                        .unwrap_or(0);
                    let n = 1usize.min(max_len);
                    (0, n as u32, if n == 1 { vec![alt] } else { Vec::new() })
                }
                // SET_INTERFACE
                0x0B => {
                    let iface = (w_index & 0xFF) as usize;
                    let alt = (w_value & 0xFF) as u8;
                    if iface < 3 {
                        self.control.lock().alt_settings[iface] = alt;
                        if iface == 1 {
                            self.bridge
                                .speaker_endpoint_open
                                .store(alt == 1, Ordering::Release);
                            if alt == 0 {
                                self.bridge.clear_speaker();
                            }
                        } else if iface == 2 {
                            self.bridge
                                .mic_endpoint_open
                                .store(alt == 1, Ordering::Release);
                        }
                    }
                    (0, 0, Vec::new())
                }
                _ => (ERR_EPIPE, 0, Vec::new()),
            },
            // UAC 1.0 Class requests
            0x01 => {
                let unit_id = (w_index >> 8) as u8;
                let cs = (w_value >> 8) as u8;
                let recipient = bm_request_type & 0x1F;
                let dir_in = (bm_request_type & 0x80) != 0;

                // Endpoint Sampling Frequency Control (cs == 0x01, recipient == 2 Endpoint)
                if recipient == 2 && cs == 0x01 {
                    if dir_in {
                        let rate = self.control.lock().sample_rate;
                        let mut b = vec![
                            (rate & 0xFF) as u8,
                            ((rate >> 8) & 0xFF) as u8,
                            ((rate >> 16) & 0xFF) as u8,
                        ];
                        b.truncate(max_len);
                        let len = b.len() as u32;
                        return (0, len, b);
                    } else {
                        if out_data.len() >= 3 {
                            let rate = (out_data[0] as u32)
                                | ((out_data[1] as u32) << 8)
                                | ((out_data[2] as u32) << 16);
                            if rate > 0 {
                                self.control.lock().sample_rate = rate;
                            }
                        }
                        return (0, transfer_len, Vec::new());
                    }
                }

                // Feature Unit 2 (Speaker) and Feature Unit 5 (Microphone)
                if matches!(unit_id, 2 | 5) {
                    let is_spk = unit_id == 2;
                    match (b_request, cs) {
                        // SET_CUR Mute (0x01)
                        (0x01, 0x01) if !dir_in => {
                            if let Some(&m) = out_data.first() {
                                let mut ctl = self.control.lock();
                                if is_spk {
                                    ctl.spk_mute = m != 0;
                                } else {
                                    ctl.mic_mute = m != 0;
                                }
                            }
                            (0, transfer_len, Vec::new())
                        }
                        // GET_CUR Mute
                        (0x81, 0x01) if dir_in => {
                            let ctl = self.control.lock();
                            let m = if is_spk { ctl.spk_mute } else { ctl.mic_mute };
                            let n = 1usize.min(max_len);
                            (0, n as u32, vec![u8::from(m); n])
                        }
                        // SET_CUR Volume (0x02)
                        (0x01, 0x02) if !dir_in => {
                            if out_data.len() >= 2 {
                                let v = i16::from_le_bytes([out_data[0], out_data[1]]);
                                let mut ctl = self.control.lock();
                                if is_spk {
                                    ctl.spk_vol_q8 = v;
                                } else {
                                    ctl.mic_vol_q8 = v;
                                }
                            }
                            (0, transfer_len, Vec::new())
                        }
                        // GET_CUR Volume
                        (0x81, 0x02) if dir_in => {
                            let ctl = self.control.lock();
                            let v = if is_spk { ctl.spk_vol_q8 } else { ctl.mic_vol_q8 };
                            let mut b = v.to_le_bytes().to_vec();
                            b.truncate(max_len);
                            let len = b.len() as u32;
                            (0, len, b)
                        }
                        // GET_MIN Volume (-64 dB = -0x4000)
                        (0x82, 0x02) if dir_in => {
                            let mut b = (-0x4000_i16).to_le_bytes().to_vec();
                            b.truncate(max_len);
                            let len = b.len() as u32;
                            (0, len, b)
                        }
                        // GET_MAX Volume (0 dB = 0x0000)
                        (0x83, 0x02) if dir_in => {
                            let mut b = 0_i16.to_le_bytes().to_vec();
                            b.truncate(max_len);
                            let len = b.len() as u32;
                            (0, len, b)
                        }
                        // GET_RES Volume (1 dB = 0x0100)
                        (0x84, 0x02) if dir_in => {
                            let mut b = 0x0100_i16.to_le_bytes().to_vec();
                            b.truncate(max_len);
                            let len = b.len() as u32;
                            (0, len, b)
                        }
                        _ => (ERR_EPIPE, 0, Vec::new()),
                    }
                } else {
                    (ERR_EPIPE, 0, Vec::new())
                }
            }
            _ => (ERR_EPIPE, 0, Vec::new()),
        }
    }
}

/// Embedded USB/IP v1.1.1 UAC 1.0 server (used directly in tests or wrapped by [`VirtualAudioDevice`]).
pub(crate) struct VirtualAudioServer {
    port: u16,
    shared: Arc<AudioServerShared>,
    listener_thread: Mutex<Option<JoinHandle<()>>>,
}

impl VirtualAudioServer {
    pub fn bind(port: u16, bridge: Arc<SharedAudioBridge>) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let actual_port = listener.local_addr()?.port();
        let shared = Arc::new(AudioServerShared {
            bridge,
            control: Mutex::new(UacControlState::default()),
            iso_out_pending: Mutex::new(VecDeque::with_capacity(32)),
            iso_in_pending: Mutex::new(VecDeque::with_capacity(32)),
            client_writer: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        });
        let shared_c = Arc::clone(&shared);
        let listener_thread = thread::Builder::new()
            .name(format!("usbip-audio-{actual_port}"))
            .spawn(move || accept_loop(listener, shared_c))?;
        Ok(Self {
            port: actual_port,
            shared,
            listener_thread: Mutex::new(Some(listener_thread)),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.disconnect_client();
        let _ = TcpStream::connect_timeout(
            &([127, 0, 0, 1], self.port).into(),
            Duration::from_millis(200),
        );
        if let Some(h) = self.listener_thread.lock().take() {
            let _ = h.join();
        }
    }
}

impl Drop for VirtualAudioServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Active Virtual DualSense Windows Audio Device (Speakers + Microphone) attached via `usbip-win2`.
pub struct VirtualAudioDevice {
    server: VirtualAudioServer,
    bridge: Arc<SharedAudioBridge>,
    vhci_port: Option<u32>,
}

impl VirtualAudioDevice {
    pub fn spawn(bridge: Arc<SharedAudioBridge>) -> Result<Self, String> {
        if !is_usbip_win2_installed() {
            return Err("usbip-win2 is not installed".to_owned());
        }
        let server = VirtualAudioServer::bind(0, Arc::clone(&bridge))
            .map_err(|e| format!("failed to bind virtual audio USB/IP server: {e}"))?;
        let vhci_port = attach_local(server.port(), "1-1")?;
        bridge.virtual_audio_attached.store(true, Ordering::Release);
        debug!(
            tcp_port = server.port(),
            vhci_port, "Attached Virtual DualSense Audio device"
        );
        Ok(Self {
            server,
            bridge,
            vhci_port: Some(vhci_port),
        })
    }
}

impl Drop for VirtualAudioDevice {
    fn drop(&mut self) {
        if let Some(port) = self.vhci_port.take() {
            if let Err(e) = detach_port(port) {
                warn!(vhci_port = port, err = %e, "Failed to detach virtual audio usbip-win2 port");
            }
        }
        self.bridge
            .virtual_audio_attached
            .store(false, Ordering::Release);
        self.server.shutdown();
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<AudioServerShared>) {
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
                let shared_c = Arc::clone(&shared);
                if let Ok(h) = thread::Builder::new()
                    .name("usbip-audio-client".to_owned())
                    .spawn(move || handle_client(stream, shared_c))
                {
                    workers.push(h);
                }
                workers.retain(|h| !h.is_finished());
            }
            Err(e) => {
                if shared.shutdown.load(Ordering::Acquire) {
                    break;
                }
                warn!(err = %e, "Virtual audio USB/IP accept error");
            }
        }
    }
    for w in workers {
        let _ = w.join();
    }
}

fn encode_usbip_audio_device_record(out: &mut Vec<u8>) {
    let mut path = [0u8; 256];
    let p = b"/sys/devices/pci0000:00/0000:00:14.0/usb1/1-1";
    path[..p.len()].copy_from_slice(p);
    out.extend_from_slice(&path);

    let mut busid = [0u8; 32];
    busid[..3].copy_from_slice(b"1-1");
    out.extend_from_slice(&busid);

    out.extend_from_slice(&1u32.to_be_bytes()); // busnum
    out.extend_from_slice(&1u32.to_be_bytes()); // devnum
    out.extend_from_slice(&USB_SPEED_FULL.to_be_bytes());
    out.extend_from_slice(&AUDIO_VID.to_be_bytes());
    out.extend_from_slice(&AUDIO_PID.to_be_bytes());
    out.extend_from_slice(&0x0100u16.to_be_bytes()); // bcdDevice
    out.push(0x00); // bDeviceClass
    out.push(0x00); // bDeviceSubClass
    out.push(0x00); // bDeviceProtocol
    out.push(1); // bConfigurationValue
    out.push(1); // bNumConfigurations
    out.push(3); // bNumInterfaces
}

fn handle_client(mut stream: TcpStream, shared: Arc<AudioServerShared>) {
    let mut op_hdr = [0u8; 8];
    if stream.read_exact(&mut op_hdr).is_err() {
        return;
    }
    let version = u16::from_be_bytes([op_hdr[0], op_hdr[1]]);
    let command = u16::from_be_bytes([op_hdr[2], op_hdr[3]]);
    if version != USBIP_VERSION {
        return;
    }

    match command {
        OP_REQ_DEVLIST => {
            let mut resp = Vec::with_capacity(12 + 312 + 12);
            resp.extend_from_slice(&USBIP_VERSION.to_be_bytes());
            resp.extend_from_slice(&OP_REP_DEVLIST.to_be_bytes());
            resp.extend_from_slice(&0u32.to_be_bytes());
            resp.extend_from_slice(&1u32.to_be_bytes());
            encode_usbip_audio_device_record(&mut resp);
            // 3 interfaces: AudioControl (0x01,0x01,0x00), AS OUT (0x01,0x02,0x00), AS IN (0x01,0x02,0x00)
            resp.extend_from_slice(&[0x01, 0x01, 0x00, 0x00]);
            resp.extend_from_slice(&[0x01, 0x02, 0x00, 0x00]);
            resp.extend_from_slice(&[0x01, 0x02, 0x00, 0x00]);
            let _ = stream.write_all(&resp);
            let _ = stream.flush();
        }
        OP_REQ_IMPORT => {
            let mut busid_buf = [0u8; 32];
            if stream.read_exact(&mut busid_buf).is_err() {
                return;
            }
            let nul = busid_buf.iter().position(|&b| b == 0).unwrap_or(32);
            let bus_id = String::from_utf8_lossy(&busid_buf[..nul]);
            if bus_id != "1-1" {
                let mut err_resp = [0u8; 8];
                err_resp[0..2].copy_from_slice(&USBIP_VERSION.to_be_bytes());
                err_resp[2..4].copy_from_slice(&OP_REP_IMPORT.to_be_bytes());
                err_resp[4..8].copy_from_slice(&1u32.to_be_bytes());
                let _ = stream.write_all(&err_resp);
                return;
            }

            shared.disconnect_client();
            let Ok(writer_clone) = stream.try_clone() else {
                return;
            };
            let _ = writer_clone.set_nodelay(true);

            let mut resp = Vec::with_capacity(8 + 312);
            resp.extend_from_slice(&USBIP_VERSION.to_be_bytes());
            resp.extend_from_slice(&OP_REP_IMPORT.to_be_bytes());
            resp.extend_from_slice(&0u32.to_be_bytes());
            encode_usbip_audio_device_record(&mut resp);
            if stream.write_all(&resp).is_err() || stream.flush().is_err() {
                return;
            }

            let writer_arc = Arc::new(Mutex::new(writer_clone));
            *shared.client_writer.lock() = Some(Arc::clone(&writer_arc));

            let pacer_stop = Arc::new(AtomicBool::new(false));
            let pacer_stop_c = Arc::clone(&pacer_stop);
            let shared_c = Arc::clone(&shared);
            let pacer = thread::Builder::new()
                .name("usbip-audio-iso".to_owned())
                .spawn(move || iso_pacer_loop(shared_c, pacer_stop_c))
                .ok();

            run_urb_loop(&mut stream, &shared);

            pacer_stop.store(true, Ordering::Release);
            if let Some(h) = pacer {
                let _ = h.join();
            }
            shared.disconnect_client();
        }
        other => {
            trace!(command = format_args!("0x{other:04X}"), "unknown audio USB/IP command");
        }
    }
}

fn run_urb_loop(stream: &mut TcpStream, shared: &AudioServerShared) {
    let mut hdr = [0u8; USBIP_HEADER_SIZE];
    let mut out_buf: Vec<u8> = Vec::with_capacity(4096);
    let mut iso_buf: Vec<u8> = Vec::with_capacity(1024);
    let mut out_clock = Instant::now();
    let mut in_clock = Instant::now();

    while !shared.shutdown.load(Ordering::Acquire) {
        if stream.read_exact(&mut hdr).is_err() {
            break;
        }
        let command = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let seqnum = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
        let direction = u32::from_be_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]);
        let ep = u32::from_be_bytes([hdr[16], hdr[17], hdr[18], hdr[19]]);

        match command {
            USBIP_CMD_SUBMIT => {
                let transfer_len = u32::from_be_bytes([hdr[24], hdr[25], hdr[26], hdr[27]]);
                let start_frame = u32::from_be_bytes([hdr[28], hdr[29], hdr[30], hdr[31]]);
                let num_packets_raw = i32::from_be_bytes([hdr[32], hdr[33], hdr[34], hdr[35]]);
                let num_packets = if num_packets_raw > 0 {
                    num_packets_raw as usize
                } else {
                    0
                };
                let mut setup = [0u8; 8];
                setup.copy_from_slice(&hdr[40..48]);

                out_buf.clear();
                if direction == USBIP_DIR_OUT && transfer_len > 0 {
                    let len = transfer_len as usize;
                    if len > MAX_OUT_TRANSFER_BYTES {
                        break;
                    }
                    out_buf.resize(len, 0);
                    if stream.read_exact(&mut out_buf).is_err() {
                        break;
                    }
                }

                let mut packets = Vec::new();
                if num_packets > 0 {
                    if num_packets > MAX_ISO_PACKETS {
                        break;
                    }
                    iso_buf.resize(num_packets * 16, 0);
                    if stream.read_exact(&mut iso_buf).is_err() {
                        break;
                    }
                    packets.reserve(num_packets);
                    for chunk in iso_buf.chunks_exact(16) {
                        packets.push(IsoPacketDesc::from_be_bytes(chunk));
                    }
                }

                match (ep, direction) {
                    (0, _) => {
                        let (status, actual_len, payload) =
                            shared.handle_control_transfer(setup, transfer_len, &out_buf);
                        let resp_payload = if direction == USBIP_DIR_IN {
                            payload.as_slice()
                        } else {
                            &[]
                        };
                        if send_ret_submit(shared, seqnum, status, actual_len, 0, 0, resp_payload, &[]).is_err()
                        {
                            break;
                        }
                    }
                    (EP_ISO_OUT, USBIP_DIR_OUT) if !packets.is_empty() => {
                        let (gain, muted) = {
                            let ctl = shared.control.lock();
                            (ctl.spk_gain(), ctl.spk_mute)
                        };
                        for pkt in &mut packets {
                            let start = pkt.offset as usize;
                            let end = start.saturating_add(pkt.length as usize);
                            if end <= out_buf.len() {
                                shared.bridge.push_speaker_i16_stereo(
                                    &out_buf[start..end],
                                    gain,
                                    muted,
                                );
                            }
                            pkt.actual_length = pkt.length;
                            pkt.status = 0;
                        }
                        let now = Instant::now();
                        if out_clock < now {
                            out_clock = now;
                        }
                        out_clock += Duration::from_millis(packets.len() as u64);
                        shared.iso_out_pending.lock().push_back(PendingIsoUrb {
                            seqnum,
                            start_frame,
                            due: out_clock,
                            packets,
                        });
                    }
                    (EP_ISO_IN, USBIP_DIR_IN) if !packets.is_empty() => {
                        let now = Instant::now();
                        if in_clock < now {
                            in_clock = now;
                        }
                        in_clock += Duration::from_millis(packets.len() as u64);
                        shared.iso_in_pending.lock().push_back(PendingIsoUrb {
                            seqnum,
                            start_frame,
                            due: in_clock,
                            packets,
                        });
                    }
                    _ => {
                        if send_ret_submit(shared, seqnum, ERR_EPIPE, 0, 0, 0, &[], &[]).is_err() {
                            break;
                        }
                    }
                }
            }
            USBIP_CMD_UNLINK => {
                let unlink_seq = u32::from_be_bytes([hdr[20], hdr[21], hdr[22], hdr[23]]);
                let mut removed = false;
                {
                    let mut q = shared.iso_out_pending.lock();
                    if let Some(pos) = q.iter().position(|u| u.seqnum == unlink_seq) {
                        q.remove(pos);
                        removed = true;
                    }
                }
                if !removed {
                    let mut q = shared.iso_in_pending.lock();
                    if let Some(pos) = q.iter().position(|u| u.seqnum == unlink_seq) {
                        q.remove(pos);
                        removed = true;
                    }
                }
                let status = if removed { ERR_ECONNRESET } else { 0 };
                if send_ret_unlink(shared, seqnum, status).is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
}

struct HighResTimer(HANDLE);

impl HighResTimer {
    fn new() -> Option<Self> {
        let h = unsafe {
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .ok()?;
        Some(Self(h))
    }

    fn wait_us(&self, us: u64) {
        let due_100ns = -((us as i64) * 10);
        unsafe {
            if SetWaitableTimerEx(self.0, &due_100ns, 0, None, None, None, 0).is_ok() {
                let _ = WaitForSingleObject(self.0, 100);
            } else {
                thread::sleep(Duration::from_micros(us));
            }
        }
    }
}

impl Drop for HighResTimer {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn iso_pacer_loop(shared: Arc<AudioServerShared>, stop: Arc<AtomicBool>) {
    let timer = HighResTimer::new();
    let mut pcm_buf: Vec<u8> = Vec::with_capacity(4096);
    while !stop.load(Ordering::Acquire) && !shared.shutdown.load(Ordering::Acquire) {
        let now = Instant::now();

        loop {
            let due_out = {
                let mut q = shared.iso_out_pending.lock();
                if q.front().is_some_and(|u| u.due <= now) {
                    q.pop_front()
                } else {
                    None
                }
            };
            let Some(urb) = due_out else {
                break;
            };
            let total_len: u32 = urb.packets.iter().map(|p| p.actual_length).sum();
            if send_ret_submit(
                &shared,
                urb.seqnum,
                0,
                total_len,
                urb.start_frame,
                urb.packets.len() as u32,
                &[],
                &urb.packets,
            )
            .is_err()
            {
                return;
            }
        }

        loop {
            let due_in = {
                let mut q = shared.iso_in_pending.lock();
                if q.front().is_some_and(|u| u.due <= now) {
                    q.pop_front()
                } else {
                    None
                }
            };
            let Some(mut urb) = due_in else {
                break;
            };
            let (gain, muted) = {
                let ctl = shared.control.lock();
                (ctl.mic_gain(), ctl.mic_mute)
            };
            let mut total_samples = 0usize;
            for pkt in &mut urb.packets {
                let pkt_bytes = pkt.length.min(96);
                let aligned = (pkt_bytes / 2) * 2;
                pkt.actual_length = aligned;
                pkt.status = 0;
                total_samples += (aligned / 2) as usize;
            }
            shared
                .bridge
                .pull_mic_i16_mono(total_samples, gain, muted, &mut pcm_buf);
            if send_ret_submit(
                &shared,
                urb.seqnum,
                0,
                pcm_buf.len() as u32,
                urb.start_frame,
                urb.packets.len() as u32,
                &pcm_buf,
                &urb.packets,
            )
            .is_err()
            {
                return;
            }
        }

        let earliest = {
            let out_due = shared.iso_out_pending.lock().front().map(|u| u.due);
            let in_due = shared.iso_in_pending.lock().front().map(|u| u.due);
            match (out_due, in_due) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        };
        let now_after = Instant::now();
        if earliest.is_some_and(|d| d <= now_after) {
            continue;
        }
        let wait_us = match earliest {
            Some(d) => d
                .saturating_duration_since(now_after)
                .as_micros()
                .clamp(250, 1_000) as u64,
            None => 1_000,
        };
        if let Some(t) = timer.as_ref() {
            t.wait_us(wait_us);
        } else {
            thread::sleep(Duration::from_micros(wait_us));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn send_ret_submit(
    shared: &AudioServerShared,
    seqnum: u32,
    status: i32,
    actual_length: u32,
    start_frame: u32,
    number_of_packets: u32,
    payload: &[u8],
    iso_packets: &[IsoPacketDesc],
) -> io::Result<()> {
    let guard = shared.client_writer.lock();
    let Some(writer_arc) = guard.as_ref().cloned() else {
        return Err(io::Error::new(io::ErrorKind::NotConnected, "no client"));
    };
    drop(guard);

    let mut frame = Vec::with_capacity(USBIP_HEADER_SIZE + payload.len() + iso_packets.len() * 16);
    frame.extend_from_slice(&USBIP_RET_SUBMIT.to_be_bytes());
    frame.extend_from_slice(&seqnum.to_be_bytes());
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame.extend_from_slice(&status.to_be_bytes());
    frame.extend_from_slice(&actual_length.to_be_bytes());
    frame.extend_from_slice(&start_frame.to_be_bytes());
    frame.extend_from_slice(&number_of_packets.to_be_bytes());
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame.extend_from_slice(&[0u8; 8]);
    frame.extend_from_slice(payload);
    for pkt in iso_packets {
        pkt.write_be_bytes(&mut frame);
    }

    let mut stream = writer_arc.lock();
    stream.write_all(&frame)?;
    stream.flush()
}

fn send_ret_unlink(shared: &AudioServerShared, seqnum: u32, status: i32) -> io::Result<()> {
    let guard = shared.client_writer.lock();
    let Some(writer_arc) = guard.as_ref().cloned() else {
        return Err(io::Error::new(io::ErrorKind::NotConnected, "no client"));
    };
    drop(guard);

    let mut frame = [0u8; USBIP_HEADER_SIZE];
    frame[0..4].copy_from_slice(&USBIP_RET_UNLINK.to_be_bytes());
    frame[4..8].copy_from_slice(&seqnum.to_be_bytes());
    frame[20..24].copy_from_slice(&status.to_be_bytes());

    let mut stream = writer_arc.lock();
    stream.write_all(&frame)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uac_descriptors_have_valid_structure_and_lengths() {
        assert_eq!(AUDIO_DEVICE_DESCRIPTOR.len(), 18);
        assert_eq!(AUDIO_CONFIG_DESCRIPTOR.len(), 193);
        let total = u16::from_le_bytes([AUDIO_CONFIG_DESCRIPTOR[2], AUDIO_CONFIG_DESCRIPTOR[3]]);
        assert_eq!(total, 193);
        assert_eq!(AUDIO_CONFIG_DESCRIPTOR[4], 3); // 3 interfaces

        // Walk descriptors sequentially to verify lengths sum cleanly to 193.
        let mut off = 0usize;
        let mut count = 0usize;
        while off < AUDIO_CONFIG_DESCRIPTOR.len() {
            let len = AUDIO_CONFIG_DESCRIPTOR[off] as usize;
            assert!(len >= 2);
            off += len;
            count += 1;
        }
        assert_eq!(off, AUDIO_CONFIG_DESCRIPTOR.len());
        assert_eq!(count, 21);
    }

    #[test]
    fn usbip_audio_loopback_control_and_iso_round_trip() {
        let bridge = Arc::new(SharedAudioBridge::new());
        let srv = VirtualAudioServer::bind(0, Arc::clone(&bridge)).unwrap();
        let addr = format!("127.0.0.1:{}", srv.port());

        // 1. OP_REQ_DEVLIST
        {
            let mut s = TcpStream::connect(&addr).unwrap();
            let mut req = [0u8; 8];
            req[0..2].copy_from_slice(&USBIP_VERSION.to_be_bytes());
            req[2..4].copy_from_slice(&OP_REQ_DEVLIST.to_be_bytes());
            s.write_all(&req).unwrap();
            let mut rep = vec![0u8; 12 + 312 + 12];
            s.read_exact(&mut rep).unwrap();
            assert_eq!(u16::from_be_bytes([rep[2], rep[3]]), OP_REP_DEVLIST);
            assert_eq!(u32::from_be_bytes([rep[8], rep[9], rep[10], rep[11]]), 1);
        }

        // 2. OP_REQ_IMPORT + Control + ISO OUT + ISO IN
        let mut s = TcpStream::connect(&addr).unwrap();
        s.set_nodelay(true).unwrap();
        let mut imp = vec![0u8; 8 + 32];
        imp[0..2].copy_from_slice(&USBIP_VERSION.to_be_bytes());
        imp[2..4].copy_from_slice(&OP_REQ_IMPORT.to_be_bytes());
        imp[8..11].copy_from_slice(b"1-1");
        s.write_all(&imp).unwrap();
        let mut imp_rep = vec![0u8; 8 + 312];
        s.read_exact(&mut imp_rep).unwrap();
        assert_eq!(u32::from_be_bytes([imp_rep[4], imp_rep[5], imp_rep[6], imp_rep[7]]), 0);

        // SET_INTERFACE(1, 1) & SET_INTERFACE(2, 1) with usbip-win2's -1 non-isoch sentinel
        for iface in [1u16, 2u16] {
            let mut urb = [0u8; 48];
            urb[0..4].copy_from_slice(&USBIP_CMD_SUBMIT.to_be_bytes());
            urb[4..8].copy_from_slice(&(iface as u32).to_be_bytes());
            urb[12..16].copy_from_slice(&USBIP_DIR_OUT.to_be_bytes());
            urb[32..36].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
            urb[40..48].copy_from_slice(&[0x01, 0x0B, 1, 0, iface as u8, 0, 0, 0]);
            s.write_all(&urb).unwrap();
            let mut ret = [0u8; 48];
            s.read_exact(&mut ret).unwrap();
            assert_eq!(i32::from_be_bytes([ret[20], ret[21], ret[22], ret[23]]), 0);
        }
        assert!(bridge.speaker_endpoint_open.load(Ordering::Acquire));
        assert!(bridge.mic_endpoint_open.load(Ordering::Acquire));

        // ISO OUT: 1 packet of 192 bytes (48 stereo frames)
        let mut iso_out = Vec::with_capacity(48 + 192 + 16);
        let mut hdr = [0u8; 48];
        hdr[0..4].copy_from_slice(&USBIP_CMD_SUBMIT.to_be_bytes());
        hdr[4..8].copy_from_slice(&10u32.to_be_bytes());
        hdr[12..16].copy_from_slice(&USBIP_DIR_OUT.to_be_bytes());
        hdr[16..20].copy_from_slice(&EP_ISO_OUT.to_be_bytes());
        hdr[24..28].copy_from_slice(&192u32.to_be_bytes());
        hdr[32..36].copy_from_slice(&1u32.to_be_bytes());
        iso_out.extend_from_slice(&hdr);
        for _ in 0..48 {
            iso_out.extend_from_slice(&16384_i16.to_le_bytes());
            iso_out.extend_from_slice(&(-16384_i16).to_le_bytes());
        }
        IsoPacketDesc {
            offset: 0,
            length: 192,
            actual_length: 0,
            status: 0,
        }
        .write_be_bytes(&mut iso_out);
        s.write_all(&iso_out).unwrap();
        let mut ret_out = [0u8; 48 + 16];
        s.read_exact(&mut ret_out).unwrap();
        assert_eq!(u32::from_be_bytes([ret_out[24], ret_out[25], ret_out[26], ret_out[27]]), 192);
        assert_eq!(bridge.speaker_ring.lock().len(), 48);

        // ISO IN: push 48 mono samples -> request 1 packet of 96 bytes
        bridge.push_mic_mono_f32(&vec![0.5_f32; 48]);
        let mut iso_in = Vec::with_capacity(48 + 16);
        let mut in_hdr = [0u8; 48];
        in_hdr[0..4].copy_from_slice(&USBIP_CMD_SUBMIT.to_be_bytes());
        in_hdr[4..8].copy_from_slice(&11u32.to_be_bytes());
        in_hdr[12..16].copy_from_slice(&USBIP_DIR_IN.to_be_bytes());
        in_hdr[16..20].copy_from_slice(&EP_ISO_IN.to_be_bytes());
        in_hdr[24..28].copy_from_slice(&96u32.to_be_bytes());
        in_hdr[32..36].copy_from_slice(&1u32.to_be_bytes());
        iso_in.extend_from_slice(&in_hdr);
        IsoPacketDesc {
            offset: 0,
            length: 96,
            actual_length: 0,
            status: 0,
        }
        .write_be_bytes(&mut iso_in);
        s.write_all(&iso_in).unwrap();

        let mut ret_in = vec![0u8; 48 + 96 + 16];
        s.read_exact(&mut ret_in).unwrap();
        assert_eq!(u32::from_be_bytes([ret_in[24], ret_in[25], ret_in[26], ret_in[27]]), 96);
        let sample0 = i16::from_le_bytes([ret_in[48], ret_in[49]]);
        assert!(sample0 > 16000, "Expected ~16384 mono mic sample, got {sample0}");
        let desc = IsoPacketDesc::from_be_bytes(&ret_in[48 + 96..48 + 96 + 16]);
        assert_eq!(desc.actual_length, 96);
        assert_eq!(desc.status, 0);
    }

    #[test]
    fn speaker_ring_prebuffers_and_drains_partial_tail() {
        let bridge = SharedAudioBridge::new();
        let mut out = [0.0_f32; 512 * 2];

        let mut chunk_480 = Vec::with_capacity(480 * 4);
        for _ in 0..480 {
            chunk_480.extend_from_slice(&16384_i16.to_le_bytes());
            chunk_480.extend_from_slice(&(-16384_i16).to_le_bytes());
        }

        // First 480 frames (< 960 prebuffer) holds silence and preserves queued frames.
        bridge.push_speaker_i16_stereo(&chunk_480, 1.0, false);
        assert!(!bridge.pull_speaker_512(&mut out));
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(bridge.speaker_ring.lock().len(), 480);

        // Second 480 frames reaches 960 -> primes and pulls 512 frames.
        bridge.push_speaker_i16_stereo(&chunk_480, 1.0, false);
        assert!(bridge.pull_speaker_512(&mut out));
        assert!((out[0] - 0.5).abs() < 1e-4);
        assert!((out[1] + 0.5).abs() < 1e-4);
        assert_eq!(bridge.speaker_ring.lock().len(), 448);

        // Remaining 448 frames drain cleanly with zero-padded tail and reset primed state.
        assert!(bridge.pull_speaker_512(&mut out));
        assert!((out[447 * 2] - 0.5).abs() < 1e-4);
        assert!(out[448 * 2..].iter().all(|&s| s == 0.0));
        assert_eq!(bridge.speaker_ring.lock().len(), 0);
        assert!(!bridge.speaker_primed.load(Ordering::Acquire));
    }
}
