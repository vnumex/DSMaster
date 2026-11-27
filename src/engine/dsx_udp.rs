use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, warn};

use super::session::ControllerSessionHandle;
use crate::protocol::{AdaptiveTriggerEffect, MicMuteLed};

pub const DEFAULT_DSX_UDP_PORT: u16 = 6969;

/// Live override layer written by external DSX-compatible game mods over UDP.
///
/// Any `Some(...)` field overrides the active profile while `active == true`.
/// Sending DSX instruction type `7` (`ResetToUserSettings`) or disabling the server
/// clears the override so the user's profile resumes cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DsxOverrideState {
    pub active: bool,
    pub generation: u64,
    pub left_trigger: Option<AdaptiveTriggerEffect>,
    pub right_trigger: Option<AdaptiveTriggerEffect>,
    pub lightbar_rgb: Option<[u8; 3]>,
    pub player_leds: Option<u8>,
    pub mic_mute_led: Option<MicMuteLed>,
}

#[derive(Debug, Clone, Default)]
pub struct DsxUdpStatus {
    pub running: bool,
    pub port: u16,
    pub packets_received: u64,
    pub override_active: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DsxPacket {
    #[serde(alias = "Instructions", default)]
    instructions: Vec<DsxInstruction>,
}

#[derive(Debug, Deserialize)]
struct DsxInstruction {
    #[serde(rename = "type", alias = "Type", default)]
    kind: u8,
    #[serde(alias = "Parameters", default)]
    parameters: Vec<Value>,
}

pub struct DsxUdpServer {
    stop_flag: Arc<AtomicBool>,
    override_state: Arc<ArcSwap<DsxOverrideState>>,
    status: Arc<Mutex<DsxUdpStatus>>,
    thread: Option<JoinHandle<()>>,
}

impl DsxUdpServer {
    pub fn start(
        port: u16,
        override_state: Arc<ArcSwap<DsxOverrideState>>,
        sessions: Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
    ) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(DsxUdpStatus {
            running: false,
            port,
            packets_received: 0,
            override_active: false,
            last_error: None,
        }));

        let bind_addr = format!("127.0.0.1:{port}");
        let socket = match UdpSocket::bind(&bind_addr) {
            Ok(sock) => sock,
            Err(e) => {
                warn!(port, error = %e, "Failed to bind DSX UDP mod server");
                status.lock().last_error = Some(format!("Port {port} unavailable: {e}"));
                return Self {
                    stop_flag,
                    override_state,
                    status,
                    thread: None,
                };
            }
        };

        let _ = socket.set_read_timeout(Some(Duration::from_millis(50)));
        {
            let mut st = status.lock();
            st.running = true;
            st.last_error = None;
        }
        info!(addr = %bind_addr, "DSX UDP mod server started");

        let stop_c = Arc::clone(&stop_flag);
        let state_c = Arc::clone(&override_state);
        let status_c = Arc::clone(&status);
        let thread = thread::Builder::new()
            .name("ds-dsx-udp".to_owned())
            .spawn(move || {
                run_dsx_loop(socket, state_c, sessions, stop_c, status_c);
            })
            .ok();

        Self {
            stop_flag,
            override_state,
            status,
            thread,
        }
    }

    pub fn status(&self) -> DsxUdpStatus {
        self.status.lock().clone()
    }

    pub fn clear_override(&self) {
        let prev = **self.override_state.load();
        self.override_state.store(Arc::new(DsxOverrideState {
            active: false,
            generation: prev.generation.wrapping_add(1),
            ..Default::default()
        }));
        self.status.lock().override_active = false;
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        self.clear_override();
        self.status.lock().running = false;
    }
}

impl Drop for DsxUdpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_dsx_loop(
    socket: UdpSocket,
    override_state: Arc<ArcSwap<DsxOverrideState>>,
    sessions: Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
    stop_flag: Arc<AtomicBool>,
    status: Arc<Mutex<DsxUdpStatus>>,
) {
    let mut rx_buf = [0u8; 4096];

    while !stop_flag.load(Ordering::Acquire) {
        let (len, peer) = match socket.recv_from(&mut rx_buf) {
            Ok(pair) => pair,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => continue,
        };

        let Ok(text) = std::str::from_utf8(&rx_buf[..len]) else {
            continue;
        };
        let Ok(packet) = serde_json::from_str::<DsxPacket>(text.trim_end_matches('\0')) else {
            continue;
        };

        let mut next = **override_state.load();
        if apply_dsx_packet(&mut next, &packet) {
            next.generation = next.generation.wrapping_add(1);
            override_state.store(Arc::new(next));
        }

        {
            let mut st = status.lock();
            st.packets_received = st.packets_received.wrapping_add(1);
            st.override_active = next.active;
        }

        // Send DSX-compatible status reply so mods awaiting an ACK stay happy.
        let (connected, battery) = primary_controller_status(&sessions);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let reply = format!(
            "{{\"Status\":\"OK\",\"TimeReceived\":\"{ts}\",\"isControllerConnected\":{connected},\"BatteryLevel\":{battery}}}"
        );
        let _ = socket.send_to(reply.as_bytes(), peer);
    }

    info!("DSX UDP mod server stopped");
}

fn primary_controller_status(
    sessions: &Arc<Mutex<HashMap<String, ControllerSessionHandle>>>,
) -> (bool, u8) {
    let guard = sessions.lock();
    for h in guard.values() {
        if !h.is_finished() {
            let snap = h.telemetry.read();
            if snap.active {
                return (true, snap.battery_percent);
            }
        }
    }
    (false, 0)
}

fn param_u8(params: &[Value], idx: usize) -> Option<u8> {
    let v = params.get(idx)?;
    if let Some(n) = v.as_u64() {
        return Some(n.min(255) as u8);
    }
    if let Some(n) = v.as_i64() {
        return Some(n.clamp(0, 255) as u8);
    }
    if let Some(f) = v.as_f64() {
        return Some(f.round().clamp(0.0, 255.0) as u8);
    }
    if let Some(b) = v.as_bool() {
        return Some(u8::from(b));
    }
    if let Some(s) = v.as_str() {
        return s.parse::<u8>().ok();
    }
    None
}

fn param_bool(params: &[Value], idx: usize) -> Option<bool> {
    let v = params.get(idx)?;
    if let Some(b) = v.as_bool() {
        return Some(b);
    }
    if let Some(n) = v.as_u64() {
        return Some(n != 0);
    }
    if let Some(n) = v.as_i64() {
        return Some(n != 0);
    }
    None
}

fn map_dsx_trigger_mode(params: &[Value]) -> Option<AdaptiveTriggerEffect> {
    let mode = param_u8(params, 2)?;
    let p3 = param_u8(params, 3).unwrap_or(0);
    let p4 = param_u8(params, 4).unwrap_or(0);
    let p5 = param_u8(params, 5).unwrap_or(0);
    let p6 = param_u8(params, 6).unwrap_or(0);
    let p7 = param_u8(params, 7).unwrap_or(0);
    let p8 = param_u8(params, 8).unwrap_or(0);

    let eff = match mode {
        0 => AdaptiveTriggerEffect::Off,
        1 => AdaptiveTriggerEffect::Weapon {
            start_position: 4,
            end_position: 7,
            strength: 8,
        }, // GameCube
        2 => AdaptiveTriggerEffect::Feedback {
            start_position: 1,
            strength: 2,
        }, // VerySoft
        3 => AdaptiveTriggerEffect::Feedback {
            start_position: 1,
            strength: 4,
        }, // Soft
        4 => AdaptiveTriggerEffect::Feedback {
            start_position: 1,
            strength: 6,
        }, // Hard
        5 => AdaptiveTriggerEffect::Feedback {
            start_position: 1,
            strength: 8,
        }, // VeryHard
        6 | 7 => AdaptiveTriggerEffect::Feedback {
            start_position: 0,
            strength: 8,
        }, // Hardest / Rigid
        8 => AdaptiveTriggerEffect::Vibration {
            position: p3.min(9),
            amplitude: p4.clamp(1, 8),
            frequency: p5.max(1),
        }, // VibrateTrigger
        9 => AdaptiveTriggerEffect::Machine {
            start_position: 1,
            end_position: 8,
            strength_a: 2,
            strength_b: 6,
            frequency: 20,
            period: 2,
        }, // Choppy
        10 => AdaptiveTriggerEffect::Feedback {
            start_position: 2,
            strength: 5,
        }, // Medium
        11 => AdaptiveTriggerEffect::Vibration {
            position: 1,
            amplitude: 6,
            frequency: 40,
        }, // VibrateTriggerPulse
        12 => {
            // CustomTriggerValue: p3 is CustomTriggerValueMode, p4.. are raw DualSense bytes
            match p3 {
                0 => AdaptiveTriggerEffect::Off,
                1..=4 => AdaptiveTriggerEffect::Feedback {
                    start_position: (p4 / 26).min(9),
                    strength: ((p5 / 32) + 1).clamp(1, 8),
                },
                5..=8 => AdaptiveTriggerEffect::Weapon {
                    start_position: (p4 / 32).clamp(2, 7),
                    end_position: (p5 / 32).clamp(3, 8),
                    strength: ((p6 / 32) + 1).clamp(1, 8),
                },
                9..=11 => AdaptiveTriggerEffect::Vibration {
                    position: (p5 / 26).min(9),
                    amplitude: ((p6 / 32) + 1).clamp(1, 8),
                    frequency: p4.max(1),
                },
                _ => AdaptiveTriggerEffect::Off,
            }
        }
        13 => AdaptiveTriggerEffect::Feedback {
            start_position: p3.min(9),
            strength: p4.clamp(1, 8),
        }, // Resistance
        14 => AdaptiveTriggerEffect::Bow {
            start_position: p3.clamp(0, 7),
            end_position: p4.clamp(p3.clamp(0, 7) + 1, 8),
            strength: p5.clamp(1, 8),
            snap_force: p6.clamp(1, 8),
        }, // Bow
        15 => AdaptiveTriggerEffect::Galloping {
            start_position: p3.min(8),
            end_position: p4.clamp(p3.min(8) + 1, 9),
            first_foot: p5.min(6),
            second_foot: p6.clamp(p5.min(6) + 1, 7),
            frequency: p7.max(1),
        }, // Galloping
        16 => AdaptiveTriggerEffect::Weapon {
            start_position: p3.clamp(2, 7),
            end_position: p4.clamp(p3.clamp(2, 7) + 1, 8),
            strength: p5.clamp(1, 8),
        }, // SemiAutomaticGun
        17 => AdaptiveTriggerEffect::Vibration {
            position: p3.min(9),
            amplitude: p4.clamp(1, 8),
            frequency: p5.max(1),
        }, // AutomaticGun
        18 => AdaptiveTriggerEffect::Machine {
            start_position: p3.clamp(1, 8),
            end_position: p4.clamp(p3.clamp(1, 8) + 1, 9),
            strength_a: p5.min(7),
            strength_b: p6.min(7),
            frequency: p7.max(1),
            period: p8.max(1),
        }, // Machine
        _ => return None,
    };
    Some(eff)
}

fn apply_dsx_packet(state: &mut DsxOverrideState, packet: &DsxPacket) -> bool {
    let mut changed = false;

    for instr in &packet.instructions {
        let p = &instr.parameters;
        match instr.kind {
            // 1: TriggerUpdate -> [controllerIdx, trigger(1=L, 2=R), mode, ...]
            1 => {
                let Some(trigger) = param_u8(p, 1) else {
                    continue;
                };
                let Some(eff) = map_dsx_trigger_mode(p) else {
                    continue;
                };
                match trigger {
                    1 => {
                        state.left_trigger = Some(eff);
                        state.active = true;
                        changed = true;
                    }
                    2 => {
                        state.right_trigger = Some(eff);
                        state.active = true;
                        changed = true;
                    }
                    _ => {}
                }
            }
            // 2: RGBUpdate -> [controllerIdx, r, g, b]
            2 => {
                if let (Some(r), Some(g), Some(b)) =
                    (param_u8(p, 1), param_u8(p, 2), param_u8(p, 3))
                {
                    state.lightbar_rgb = Some([r, g, b]);
                    state.active = true;
                    changed = true;
                }
            }
            // 3: PlayerLED -> [controllerIdx, l1, l2, l3, l4, l5]
            3 => {
                let mut mask = 0u8;
                for bit in 0..5 {
                    if param_bool(p, 1 + bit).unwrap_or(false) {
                        mask |= 1u8 << bit;
                    }
                }
                state.player_leds = Some(mask);
                state.active = true;
                changed = true;
            }
            // 5: MicLED -> [controllerIdx, mode(0=On, 1=Pulse, 2=Off)]
            5 => {
                if let Some(mode) = param_u8(p, 1) {
                    let led = match mode {
                        0 => MicMuteLed::Solid,
                        1 => MicMuteLed::Pulse,
                        _ => MicMuteLed::Off,
                    };
                    state.mic_mute_led = Some(led);
                    state.active = true;
                    changed = true;
                }
            }
            // 6: PlayerLEDNewRevision -> [controllerIdx, preset(0=One..4=Five, 5=AllOff)]
            6 => {
                if let Some(preset) = param_u8(p, 1) {
                    let mask = match preset {
                        0 => 0x04, // One (center)
                        1 => 0x0A, // Two
                        2 => 0x15, // Three
                        3 => 0x1B, // Four
                        4 => 0x1F, // Five
                        _ => 0x00, // AllOff
                    };
                    state.player_leds = Some(mask);
                    state.active = true;
                    changed = true;
                }
            }
            // 7: ResetToUserSettings -> [controllerIdx]
            7 => {
                state.active = false;
                state.left_trigger = None;
                state.right_trigger = None;
                state.lightbar_rgb = None;
                state.player_leds = None;
                state.mic_mute_led = None;
                changed = true;
            }
            _ => {}
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dsx_trigger_rgb_and_reset_packets() {
        let json = r#"{
            "instructions": [
                { "type": 1, "parameters": [0, 2, 16, 2, 6, 8] },
                { "type": 1, "parameters": [0, 1, 13, 1, 5] },
                { "type": 2, "parameters": [0, 255, 64, 0] },
                { "type": 3, "parameters": [0, true, false, true, false, true] },
                { "type": 5, "parameters": [0, 1] }
            ]
        }"#;
        let pkt: DsxPacket = serde_json::from_str(json).expect("valid dsx json");
        let mut state = DsxOverrideState::default();
        assert!(apply_dsx_packet(&mut state, &pkt));
        assert!(state.active);
        assert_eq!(
            state.right_trigger,
            Some(AdaptiveTriggerEffect::Weapon {
                start_position: 2,
                end_position: 6,
                strength: 8,
            })
        );
        assert_eq!(
            state.left_trigger,
            Some(AdaptiveTriggerEffect::Feedback {
                start_position: 1,
                strength: 5,
            })
        );
        assert_eq!(state.lightbar_rgb, Some([255, 64, 0]));
        assert_eq!(state.player_leds, Some(0b10101));
        assert_eq!(state.mic_mute_led, Some(MicMuteLed::Pulse));

        let reset: DsxPacket =
            serde_json::from_str(r#"{"Instructions":[{"Type":7,"Parameters":[0]}]}"#)
                .expect("pascal case reset");
        assert!(apply_dsx_packet(&mut state, &reset));
        assert!(!state.active);
        assert!(state.left_trigger.is_none());
        assert!(state.right_trigger.is_none());
        assert!(state.lightbar_rgb.is_none());
    }
}
