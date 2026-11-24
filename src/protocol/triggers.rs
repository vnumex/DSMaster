#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AdaptiveTriggerEffect {
    /// Fully disengages and withdraws the trigger actuator (`0x05`).
    #[default]
    Off,
    /// Continuous resistance starting at `start_position` (0..=9) with `strength` (1..=8).
    Feedback { start_position: u8, strength: u8 },
    /// Sectional resistance from `start_position` (2..=7) to `end_position` (start+1..=8) with `strength` (1..=8).
    Weapon {
        start_position: u8,
        end_position: u8,
        strength: u8,
    },
    /// Vibrating trigger at `position` (0..=9), `amplitude` (1..=8), `frequency` (Hz).
    Vibration {
        position: u8,
        amplitude: u8,
        frequency: u8,
    },
    /// Linearly interpolated resistance from `start_position` (0..=8) to `end_position` (start+1..=9).
    SlopeFeedback {
        start_position: u8,
        end_position: u8,
        start_strength: u8,
        end_strength: u8,
    },
    /// Bowstring snap-back effect from `start_position` (0..=7) to `end_position` (start+1..=8).
    Bow {
        start_position: u8,
        end_position: u8,
        strength: u8,
        snap_force: u8,
    },
    /// Rhythmic dual-foot galloping pulse from `start_position` (0..=8) to `end_position` (start+1..=9).
    Galloping {
        start_position: u8,
        end_position: u8,
        first_foot: u8,
        second_foot: u8,
        frequency: u8,
    },
    /// Automatic-weapon cycling vibration between `start_position` (1..=8) and `end_position` (start+1..=9).
    Machine {
        start_position: u8,
        end_position: u8,
        strength_a: u8,
        strength_b: u8,
        frequency: u8,
        period: u8,
    },
    /// Lossless raw 11-byte DualSense adaptive trigger payload (e.g. forwarded from a game).
    #[allow(dead_code)]
    Raw([u8; 11]),
}

impl AdaptiveTriggerEffect {
    /// Encodes the effect into the 11-byte DualSense trigger payload.
    #[inline]
    pub fn encode(&self) -> [u8; 11] {
        let mut out = [0u8; 11];
        match *self {
            Self::Raw(raw) => return raw,
            Self::Off => {
                out[0] = 0x05;
            }
            Self::Feedback {
                start_position,
                strength,
            } => {
                let start = start_position.min(9);
                let force = strength.clamp(1, 8).saturating_sub(1);
                let mut zone_mask: u16 = 0;
                let mut force_zones: u32 = 0;
                for zone in start..10 {
                    zone_mask |= 1u16 << zone;
                    force_zones |= (force as u32) << (3 * zone);
                }
                out[0] = 0x21;
                out[1] = (zone_mask & 0xFF) as u8;
                out[2] = ((zone_mask >> 8) & 0x03) as u8;
                out[3] = (force_zones & 0xFF) as u8;
                out[4] = ((force_zones >> 8) & 0xFF) as u8;
                out[5] = ((force_zones >> 16) & 0xFF) as u8;
                out[6] = ((force_zones >> 24) & 0xFF) as u8;
            }
            Self::Weapon {
                start_position,
                end_position,
                strength,
            } => {
                let start = start_position.clamp(2, 7);
                let end = end_position.clamp(start + 1, 8);
                let force = strength.clamp(1, 8).saturating_sub(1);
                let mask: u16 = (1u16 << start) | (1u16 << end);
                out[0] = 0x25;
                out[1] = (mask & 0xFF) as u8;
                out[2] = ((mask >> 8) & 0xFF) as u8;
                out[3] = force;
            }
            Self::Vibration {
                position,
                amplitude,
                frequency,
            } => {
                let start = position.min(9);
                let amp = amplitude.clamp(1, 8).saturating_sub(1);
                let mut zone_mask: u16 = 0;
                let mut amp_zones: u32 = 0;
                for zone in start..10 {
                    zone_mask |= 1u16 << zone;
                    amp_zones |= (amp as u32) << (3 * zone);
                }
                out[0] = 0x26;
                out[1] = (zone_mask & 0xFF) as u8;
                out[2] = ((zone_mask >> 8) & 0x03) as u8;
                out[3] = (amp_zones & 0xFF) as u8;
                out[4] = ((amp_zones >> 8) & 0xFF) as u8;
                out[5] = ((amp_zones >> 16) & 0xFF) as u8;
                out[6] = ((amp_zones >> 24) & 0xFF) as u8;
                out[9] = frequency.max(1);
            }
            Self::SlopeFeedback {
                start_position,
                end_position,
                start_strength,
                end_strength,
            } => {
                let start = start_position.min(8);
                let end = end_position.clamp(start + 1, 9);
                let s = start_strength.clamp(1, 8).saturating_sub(1);
                let e = end_strength.clamp(1, 8).saturating_sub(1);
                let span = (end - start) as f32;
                let mut zone_mask: u16 = 0;
                let mut force_zones: u32 = 0;
                for zone in start..=end {
                    let t = (zone - start) as f32 / span;
                    let f = ((s as f32) + ((e as f32) - (s as f32)) * t)
                        .round()
                        .clamp(0.0, 7.0) as u32;
                    zone_mask |= 1u16 << zone;
                    force_zones |= f << (3 * zone);
                }
                out[0] = 0x21;
                out[1] = (zone_mask & 0xFF) as u8;
                out[2] = ((zone_mask >> 8) & 0x03) as u8;
                out[3] = (force_zones & 0xFF) as u8;
                out[4] = ((force_zones >> 8) & 0xFF) as u8;
                out[5] = ((force_zones >> 16) & 0xFF) as u8;
                out[6] = ((force_zones >> 24) & 0xFF) as u8;
            }
            Self::Bow {
                start_position,
                end_position,
                strength,
                snap_force,
            } => {
                let start = start_position.clamp(0, 7);
                let end = end_position.clamp(start + 1, 8);
                let s = strength.clamp(1, 8).saturating_sub(1) & 0x07;
                let snap = snap_force.clamp(1, 8).saturating_sub(1) & 0x07;
                let mask: u16 = (1u16 << start) | (1u16 << end);
                out[0] = 0x22;
                out[1..3].copy_from_slice(&mask.to_le_bytes());
                out[3] = s | (snap << 3);
            }
            Self::Galloping {
                start_position,
                end_position,
                first_foot,
                second_foot,
                frequency,
            } => {
                let start = start_position.clamp(0, 8);
                let end = end_position.clamp(start + 1, 9);
                let first = first_foot.min(6);
                let second = second_foot.clamp(first + 1, 7);
                let mask: u16 = (1u16 << start) | (1u16 << end);
                out[0] = 0x23;
                out[1..3].copy_from_slice(&mask.to_le_bytes());
                out[3] = ((second & 0x07) << 3) | (first & 0x07);
                out[4] = frequency.max(1);
            }
            Self::Machine {
                start_position,
                end_position,
                strength_a,
                strength_b,
                frequency,
                period,
            } => {
                let start = start_position.clamp(1, 8);
                let end = end_position.clamp(start + 1, 9);
                let a = strength_a.clamp(0, 7) & 0x07;
                let b = strength_b.clamp(0, 7) & 0x07;
                let mask: u16 = (1u16 << start) | (1u16 << end);
                out[0] = 0x27;
                out[1..3].copy_from_slice(&mask.to_le_bytes());
                out[3] = a | (b << 3);
                out[4] = frequency.max(1);
                out[5] = period;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_opcodes_match_spec() {
        assert_eq!(AdaptiveTriggerEffect::Off.encode()[0], 0x05);
        assert_eq!(
            AdaptiveTriggerEffect::Feedback {
                start_position: 2,
                strength: 6
            }
            .encode()[0],
            0x21
        );
        assert_eq!(
            AdaptiveTriggerEffect::Weapon {
                start_position: 3,
                end_position: 6,
                strength: 8
            }
            .encode()[0],
            0x25
        );
        assert_eq!(
            AdaptiveTriggerEffect::Vibration {
                position: 1,
                amplitude: 5,
                frequency: 40
            }
            .encode()[0],
            0x26
        );
        let raw = [0x22, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A];
        assert_eq!(AdaptiveTriggerEffect::Raw(raw).encode(), raw);

        let slope = AdaptiveTriggerEffect::SlopeFeedback {
            start_position: 1,
            end_position: 5,
            start_strength: 2,
            end_strength: 8,
        }
        .encode();
        assert_eq!(slope[0], 0x21);
        let slope_mask = u16::from_le_bytes([slope[1], slope[2]]);
        assert_eq!(slope_mask, 0b00_0011_1110);

        let bow = AdaptiveTriggerEffect::Bow {
            start_position: 2,
            end_position: 6,
            strength: 5,
            snap_force: 7,
        }
        .encode();
        assert_eq!(bow[0], 0x22);
        assert_eq!(u16::from_le_bytes([bow[1], bow[2]]), (1 << 2) | (1 << 6));
        assert_eq!(bow[3], 4 | (6 << 3));

        let gallop = AdaptiveTriggerEffect::Galloping {
            start_position: 1,
            end_position: 8,
            first_foot: 2,
            second_foot: 5,
            frequency: 12,
        }
        .encode();
        assert_eq!(gallop[0], 0x23);
        assert_eq!(u16::from_le_bytes([gallop[1], gallop[2]]), (1 << 1) | (1 << 8));
        assert_eq!(gallop[3], (5 << 3) | 2);
        assert_eq!(gallop[4], 12);

        let machine = AdaptiveTriggerEffect::Machine {
            start_position: 9, // clamped to 8
            end_position: 2,   // clamped to 9
            strength_a: 3,
            strength_b: 9,     // clamped to 7
            frequency: 0,      // clamped to 1
            period: 4,
        }
        .encode();
        assert_eq!(machine[0], 0x27);
        assert_eq!(u16::from_le_bytes([machine[1], machine[2]]), (1 << 8) | (1 << 9));
        assert_eq!(machine[3], 3 | (7 << 3));
        assert_eq!(machine[4], 1);
        assert_eq!(machine[5], 4);
    }
}
