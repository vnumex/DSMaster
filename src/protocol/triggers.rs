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
}

impl AdaptiveTriggerEffect {
    /// Encodes the effect into the 11-byte DualSense trigger payload.
    #[inline]
    pub fn encode(&self) -> [u8; 11] {
        let mut out = [0u8; 11];
        match *self {
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
    }
}
