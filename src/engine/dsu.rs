use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tracing::{info, warn};

use super::seqlock::SeqLock;
use super::session::ControllerSessionHandle;
use super::telemetry::TelemetrySnapshot;
use crate::protocol::{BatteryStatus, DPad, DualSenseButtons, Transport};

pub const DEFAULT_DSU_PORT: u16 = 26760;

const DSU_CLIENT_MAGIC: &[u8; 4] = b"DSUC";
const DSU_SERVER_MAGIC: &[u8; 4] = b"DSUS";
const DSU_PROTOCOL_VERSION: u16 = 1001;

const MSG_TYPE_VERSION: u32 = 0x0010_0000;
const MSG_TYPE_PORTS: u32 = 0x0010_0001;
const MSG_TYPE_DATA: u32 = 0x0010_0002;

const SUBSCRIBER_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SLOTS: usize = 4;

#[derive(Debug, Clone, Default)]
pub struct DsuStatus {
    pub running: bool,
    pub port: u16,
    pub subscriber_count: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct Subscriber {
    last_seen: Instant,
    slots_mask: u8,
}

pub struct DsuServer {
    stop_flag: Arc<AtomicBool>,
    status: Arc<Mutex<DsuStatus>>,
    thread: Option<JoinHandle<()>>,
}

impl DsuServer {
    pub fn start(
        port: u16,
        sessions: Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
    ) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(DsuStatus {
            running: false,
            port,
            subscriber_count: 0,
            last_error: None,
        }));

        let bind_addr = format!("127.0.0.1:{port}");
        let socket = match UdpSocket::bind(&bind_addr) {
            Ok(sock) => sock,
            Err(e) => {
                warn!(port, error = %e, "Failed to bind CemuHook DSU UDP server");
                status.lock().last_error = Some(format!("Port {port} unavailable: {e}"));
                return Self {
                    stop_flag,
                    status,
                    thread: None,
                };
            }
        };

        let _ = socket.set_nonblocking(true);
        {
            let mut st = status.lock();
            st.running = true;
            st.last_error = None;
        }
        info!(addr = %bind_addr, "CemuHook DSU UDP server started");

        let stop_c = Arc::clone(&stop_flag);
        let status_c = Arc::clone(&status);
        let thread = thread::Builder::new()
            .name("ds-dsu".to_owned())
            .spawn(move || {
                run_dsu_loop(socket, sessions, stop_c, status_c);
            })
            .ok();

        Self {
            stop_flag,
            status,
            thread,
        }
    }

    pub fn status(&self) -> DsuStatus {
        self.status.lock().clone()
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        let mut st = self.status.lock();
        st.running = false;
        st.subscriber_count = 0;
    }
}

impl Drop for DsuServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn collect_slot_snapshots(
    sessions: &Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
) -> Vec<Option<TelemetrySnapshot>> {
    let handles: Vec<Arc<SeqLock<TelemetrySnapshot>>> = {
        let guard = sessions.lock();
        let mut pairs: Vec<(&String, Arc<SeqLock<TelemetrySnapshot>>)> = guard
            .iter()
            .filter(|(_, h)| !h.is_finished())
            .map(|(k, h)| (k, Arc::clone(&h.telemetry)))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        pairs
            .into_iter()
            .take(MAX_SLOTS)
            .map(|(_, t)| t)
            .collect()
    };

    let mut slots = vec![None; MAX_SLOTS];
    for (idx, h) in handles.into_iter().enumerate() {
        let snap = h.read();
        if snap.active {
            slots[idx] = Some(snap);
        }
    }
    slots
}

fn run_dsu_loop(
    socket: UdpSocket,
    sessions: Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
    stop_flag: Arc<AtomicBool>,
    status: Arc<Mutex<DsuStatus>>,
) {
    let server_id: u32 = 0x4453_4D31; // "DSM1"
    let mut subscribers: HashMap<SocketAddr, Subscriber> = HashMap::new();
    let mut packet_counters = [0u32; MAX_SLOTS];
    let mut rx_buf = [0u8; 256];
    let mut last_send = Instant::now();

    while !stop_flag.load(Ordering::Acquire) {
        // Drain any incoming DSU client requests.
        loop {
            match socket.recv_from(&mut rx_buf) {
                Ok((len, addr)) => {
                    let slots = collect_slot_snapshots(&sessions);
                    handle_dsu_packet(
                        &socket,
                        server_id,
                        &rx_buf[..len],
                        addr,
                        &slots,
                        &mut subscribers,
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        let now = Instant::now();
        let before = subscribers.len();
        subscribers.retain(|_, sub| now.saturating_duration_since(sub.last_seen) < SUBSCRIBER_TIMEOUT);
        if subscribers.len() != before {
            status.lock().subscriber_count = subscribers.len();
        } else {
            let mut st = status.lock();
            if st.subscriber_count != subscribers.len() {
                st.subscriber_count = subscribers.len();
            }
        }

        if subscribers.is_empty() {
            thread::sleep(Duration::from_millis(16));
            continue;
        }

        if now.saturating_duration_since(last_send) >= Duration::from_millis(4) {
            last_send = now;
            let slots = collect_slot_snapshots(&sessions);
            let ts_us = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(0);

            for (slot_idx, maybe_snap) in slots.iter().enumerate() {
                let Some(snap) = maybe_snap else {
                    continue;
                };
                let bit = 1u8 << slot_idx;
                let any_sub = subscribers.values().any(|s| (s.slots_mask & bit) != 0);
                if !any_sub {
                    continue;
                }
                packet_counters[slot_idx] = packet_counters[slot_idx].wrapping_add(1);
                let pkt = encode_data_packet(
                    server_id,
                    slot_idx as u8,
                    snap,
                    packet_counters[slot_idx],
                    ts_us,
                );
                for (addr, sub) in &subscribers {
                    if (sub.slots_mask & bit) != 0 {
                        let _ = socket.send_to(&pkt, addr);
                    }
                }
            }
        }

        thread::sleep(Duration::from_millis(2));
    }

    status.lock().running = false;
    info!("CemuHook DSU UDP server stopped");
}

fn verify_dsu_request(buf: &[u8]) -> Option<(u32, &[u8])> {
    if buf.len() < 20 || &buf[0..4] != DSU_CLIENT_MAGIC {
        return None;
    }
    let payload_len = u16::from_le_bytes([buf[6], buf[7]]) as usize;
    let total_len = 16usize.checked_add(payload_len)?;
    if total_len > buf.len() || total_len < 20 {
        return None;
    }
    let expected_crc = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let mut check = buf[..total_len].to_vec();
    check[8..12].fill(0);
    if crc32fast::hash(&check) != expected_crc {
        return None;
    }
    let msg_type = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
    Some((msg_type, &buf[20..total_len]))
}

fn handle_dsu_packet(
    socket: &UdpSocket,
    server_id: u32,
    buf: &[u8],
    addr: SocketAddr,
    slots: &[Option<TelemetrySnapshot>],
    subscribers: &mut HashMap<SocketAddr, Subscriber>,
) {
    let Some((msg_type, body)) = verify_dsu_request(buf) else {
        return;
    };

    match msg_type {
        MSG_TYPE_VERSION => {
            let reply = encode_version_packet(server_id);
            let _ = socket.send_to(&reply, addr);
        }
        MSG_TYPE_PORTS => {
            if body.len() < 4 {
                return;
            }
            let count = (u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize)
                .min(MAX_SLOTS);
            for i in 0..count {
                let slot_idx = body.get(4 + i).copied().unwrap_or(i as u8);
                let snap = slots.get(slot_idx as usize).and_then(|s| s.as_ref());
                let reply = encode_port_info_packet(server_id, slot_idx, snap);
                let _ = socket.send_to(&reply, addr);
            }
        }
        MSG_TYPE_DATA => {
            if body.len() < 8 {
                return;
            }
            let reg_flags = body[0];
            let slot_idx = body[1];
            let mac = &body[2..8];
            let mask: u8 = match reg_flags {
                0 => 0x0F,
                1 => {
                    if (slot_idx as usize) < MAX_SLOTS {
                        1u8 << slot_idx
                    } else {
                        0
                    }
                }
                2 => {
                    let mut m = 0u8;
                    for (idx, s) in slots.iter().enumerate() {
                        if let Some(snap) = s {
                            if snap.mac_address == mac {
                                m |= 1u8 << idx;
                            }
                        }
                    }
                    m
                }
                _ => 0x0F,
            };
            let entry = subscribers.entry(addr).or_insert(Subscriber {
                last_seen: Instant::now(),
                slots_mask: 0,
            });
            entry.last_seen = Instant::now();
            entry.slots_mask |= mask;
        }
        _ => {}
    }
}

fn finalize_dsu_header(pkt: &mut [u8], server_id: u32, msg_type: u32) {
    let payload_len = (pkt.len() - 16) as u16;
    pkt[0..4].copy_from_slice(DSU_SERVER_MAGIC);
    pkt[4..6].copy_from_slice(&DSU_PROTOCOL_VERSION.to_le_bytes());
    pkt[6..8].copy_from_slice(&payload_len.to_le_bytes());
    pkt[8..12].fill(0);
    pkt[12..16].copy_from_slice(&server_id.to_le_bytes());
    pkt[16..20].copy_from_slice(&msg_type.to_le_bytes());
    let crc = crc32fast::hash(pkt);
    pkt[8..12].copy_from_slice(&crc.to_le_bytes());
}

fn dsu_battery_byte(pct: u8, status: BatteryStatus) -> u8 {
    match status {
        BatteryStatus::Charging => 0xEE,
        BatteryStatus::Full => 0xEF,
        BatteryStatus::Discharging => {
            if pct >= 90 {
                0x05
            } else if pct >= 65 {
                0x04
            } else if pct >= 40 {
                0x03
            } else if pct >= 15 {
                0x02
            } else {
                0x01
            }
        }
        _ => 0x00,
    }
}

fn write_slot_header(pkt: &mut [u8], slot: u8, snap: Option<&TelemetrySnapshot>) {
    pkt[20] = slot;
    if let Some(s) = snap {
        pkt[21] = 2; // Connected
        pkt[22] = 2; // Full Gyro (DualSense)
        pkt[23] = match s.transport {
            Transport::Usb => 1,
            Transport::Bluetooth => 2,
        };
        pkt[24..30].copy_from_slice(&s.mac_address);
        pkt[30] = dsu_battery_byte(s.battery_percent, s.battery_status);
    } else {
        pkt[21] = 0; // Disconnected
        pkt[22] = 0;
        pkt[23] = 0;
        pkt[24..30].fill(0);
        pkt[30] = 0;
    }
}

pub fn encode_version_packet(server_id: u32) -> [u8; 22] {
    let mut pkt = [0u8; 22];
    pkt[20..22].copy_from_slice(&DSU_PROTOCOL_VERSION.to_le_bytes());
    finalize_dsu_header(&mut pkt, server_id, MSG_TYPE_VERSION);
    pkt
}

pub fn encode_port_info_packet(
    server_id: u32,
    slot: u8,
    snap: Option<&TelemetrySnapshot>,
) -> [u8; 32] {
    let mut pkt = [0u8; 32];
    write_slot_header(&mut pkt, slot, snap);
    pkt[31] = 0;
    finalize_dsu_header(&mut pkt, server_id, MSG_TYPE_PORTS);
    pkt
}

pub fn encode_data_packet(
    server_id: u32,
    slot: u8,
    snap: &TelemetrySnapshot,
    packet_num: u32,
    timestamp_us: u64,
) -> [u8; 100] {
    let mut pkt = [0u8; 100];
    write_slot_header(&mut pkt, slot, Some(snap));
    pkt[31] = 1; // is_connected
    pkt[32..36].copy_from_slice(&packet_num.to_le_bytes());

    let b = snap.buttons;
    let d_up = matches!(snap.dpad, DPad::North | DPad::NorthEast | DPad::NorthWest);
    let d_right = matches!(snap.dpad, DPad::East | DPad::NorthEast | DPad::SouthEast);
    let d_down = matches!(snap.dpad, DPad::South | DPad::SouthEast | DPad::SouthWest);
    let d_left = matches!(snap.dpad, DPad::West | DPad::NorthWest | DPad::SouthWest);

    let mut mask1 = 0u8;
    if b.contains(DualSenseButtons::CREATE) {
        mask1 |= 1 << 0;
    }
    if b.contains(DualSenseButtons::L3) {
        mask1 |= 1 << 1;
    }
    if b.contains(DualSenseButtons::R3) {
        mask1 |= 1 << 2;
    }
    if b.contains(DualSenseButtons::OPTIONS) {
        mask1 |= 1 << 3;
    }
    if d_up {
        mask1 |= 1 << 4;
    }
    if d_right {
        mask1 |= 1 << 5;
    }
    if d_down {
        mask1 |= 1 << 6;
    }
    if d_left {
        mask1 |= 1 << 7;
    }
    pkt[36] = mask1;

    let mut mask2 = 0u8;
    if b.contains(DualSenseButtons::L2_DIGITAL) || snap.l2_analog > 16 {
        mask2 |= 1 << 0;
    }
    if b.contains(DualSenseButtons::R2_DIGITAL) || snap.r2_analog > 16 {
        mask2 |= 1 << 1;
    }
    if b.contains(DualSenseButtons::L1) {
        mask2 |= 1 << 2;
    }
    if b.contains(DualSenseButtons::R1) {
        mask2 |= 1 << 3;
    }
    if b.contains(DualSenseButtons::TRIANGLE) {
        mask2 |= 1 << 4;
    }
    if b.contains(DualSenseButtons::CIRCLE) {
        mask2 |= 1 << 5;
    }
    if b.contains(DualSenseButtons::CROSS) {
        mask2 |= 1 << 6;
    }
    if b.contains(DualSenseButtons::SQUARE) {
        mask2 |= 1 << 7;
    }
    pkt[37] = mask2;
    pkt[38] = u8::from(b.contains(DualSenseButtons::PS_HOME));
    pkt[39] = u8::from(b.contains(DualSenseButtons::TOUCHPAD));

    // DSU stick Y is inverted relative to HID (255 = up, 0 = down).
    pkt[40] = snap.left_stick[0];
    pkt[41] = 255u8.saturating_sub(snap.left_stick[1]);
    pkt[42] = snap.right_stick[0];
    pkt[43] = 255u8.saturating_sub(snap.right_stick[1]);

    // Analog D-Pad & buttons
    pkt[44] = if d_left { 255 } else { 0 };
    pkt[45] = if d_down { 255 } else { 0 };
    pkt[46] = if d_right { 255 } else { 0 };
    pkt[47] = if d_up { 255 } else { 0 };
    pkt[48] = if b.contains(DualSenseButtons::SQUARE) { 255 } else { 0 };
    pkt[49] = if b.contains(DualSenseButtons::CROSS) { 255 } else { 0 };
    pkt[50] = if b.contains(DualSenseButtons::CIRCLE) { 255 } else { 0 };
    pkt[51] = if b.contains(DualSenseButtons::TRIANGLE) { 255 } else { 0 };
    pkt[52] = if b.contains(DualSenseButtons::R1) { 255 } else { 0 };
    pkt[53] = if b.contains(DualSenseButtons::L1) { 255 } else { 0 };
    pkt[54] = snap.r2_analog;
    pkt[55] = snap.l2_analog;

    // 2 touch points (6 bytes each)
    for i in 0..2 {
        let base = 56 + i * 6;
        let tp = snap.touch[i];
        pkt[base] = u8::from(tp.active);
        pkt[base + 1] = tp.id;
        pkt[base + 2..base + 4].copy_from_slice(&tp.x.to_le_bytes());
        pkt[base + 4..base + 6].copy_from_slice(&tp.y.to_le_bytes());
    }

    pkt[68..76].copy_from_slice(&timestamp_us.to_le_bytes());

    // Accel (g): DSU convention is X = left(+), Y = down(+), Z = away(+)
    pkt[76..80].copy_from_slice(&(-snap.accel_g[0]).to_le_bytes());
    pkt[80..84].copy_from_slice(&(-snap.accel_g[1]).to_le_bytes());
    pkt[84..88].copy_from_slice(&(-snap.accel_g[2]).to_le_bytes());

    // Gyro (deg/s): pitch, yaw, roll
    pkt[88..92].copy_from_slice(&snap.gyro_dps[0].to_le_bytes());
    pkt[92..96].copy_from_slice(&snap.gyro_dps[1].to_le_bytes());
    pkt[96..100].copy_from_slice(&snap.gyro_dps[2].to_le_bytes());

    finalize_dsu_header(&mut pkt, server_id, MSG_TYPE_DATA);
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_client_req(msg_type: u32, body: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; 20 + body.len()];
        buf[0..4].copy_from_slice(DSU_CLIENT_MAGIC);
        buf[4..6].copy_from_slice(&DSU_PROTOCOL_VERSION.to_le_bytes());
        buf[6..8].copy_from_slice(&((4 + body.len()) as u16).to_le_bytes());
        buf[12..16].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        buf[16..20].copy_from_slice(&msg_type.to_le_bytes());
        buf[20..].copy_from_slice(body);
        let crc = crc32fast::hash(&buf);
        buf[8..12].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    #[test]
    fn dsu_request_crc_validation() {
        let mut req = build_client_req(MSG_TYPE_VERSION, &[]);
        let (ty, body) = verify_dsu_request(&req).expect("valid request");
        assert_eq!(ty, MSG_TYPE_VERSION);
        assert!(body.is_empty());

        req[16] ^= 0xFF;
        assert!(verify_dsu_request(&req).is_none());
    }

    #[test]
    fn dsu_packets_have_valid_headers_and_crc() {
        let snap = TelemetrySnapshot {
            active: true,
            transport: Transport::Bluetooth,
            mac_address: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            battery_percent: 80,
            battery_status: BatteryStatus::Discharging,
            buttons: DualSenseButtons::CROSS | DualSenseButtons::OPTIONS,
            dpad: DPad::North,
            left_stick: [128, 0],
            l2_analog: 200,
            r2_analog: 100,
            gyro_dps: [10.5, -20.25, 3.0],
            accel_g: [0.1, -0.98, 0.02],
            ..Default::default()
        };

        let ver = encode_version_packet(0xABCD);
        assert_eq!(&ver[0..4], b"DSUS");
        assert_eq!(u16::from_le_bytes([ver[6], ver[7]]), 6);

        let port = encode_port_info_packet(0xABCD, 0, Some(&snap));
        assert_eq!(port[21], 2);
        assert_eq!(port[23], 2);
        assert_eq!(&port[24..30], &snap.mac_address);
        assert_eq!(port[30], 0x04);

        let data = encode_data_packet(0xABCD, 0, &snap, 42, 999_999);
        assert_eq!(data.len(), 100);
        assert_eq!(u16::from_le_bytes([data[6], data[7]]), 84);
        assert_eq!(data[36] & (1 << 3), 1 << 3); // Options
        assert_eq!(data[36] & (1 << 4), 1 << 4); // D-Up
        assert_eq!(data[37] & (1 << 6), 1 << 6); // Cross
        assert_eq!(data[41], 255); // Y inverted (0 -> 255)
        assert_eq!(data[54], 100); // R2
        assert_eq!(data[55], 200); // L2

        // Verify CRC of data packet
        let expected_crc = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        let mut check = data;
        check[8..12].fill(0);
        assert_eq!(crc32fast::hash(&check), expected_crc);
    }
}
