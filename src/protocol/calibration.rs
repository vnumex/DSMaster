use super::constants::{
    DS_ACC_RANGE, DS_ACC_RES_PER_G, DS_FEATURE_REPORT_CALIBRATION, DS_GYRO_RANGE,
    DS_GYRO_RES_PER_DEG_S, ProtocolError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisCalibration {
    pub bias: i32,
    pub sens_numer: i32,
    pub sens_denom: i32,
}

impl AxisCalibration {
    #[inline]
    pub const fn apply(&self, raw: i16) -> i32 {
        let centered = (raw as i64) - (self.bias as i64);
        ((centered * (self.sens_numer as i64)) / (self.sens_denom as i64)) as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImuCalibration {
    pub gyro: [AxisCalibration; 3],
    pub accel: [AxisCalibration; 3],
}

impl Default for ImuCalibration {
    fn default() -> Self {
        Self {
            gyro: [AxisCalibration {
                bias: 0,
                sens_numer: DS_GYRO_RANGE,
                sens_denom: i16::MAX as i32,
            }; 3],
            accel: [AxisCalibration {
                bias: 0,
                sens_numer: DS_ACC_RANGE,
                sens_denom: i16::MAX as i32,
            }; 3],
        }
    }
}

#[inline]
fn le_i16(buf: &[u8], offset: usize) -> i32 {
    i16::from_le_bytes([buf[offset], buf[offset + 1]]) as i32
}

/// Parses DualSense Feature Report `0x05` (41 bytes) into IMU calibration parameters.
pub fn parse_calibration_report(buf: &[u8]) -> Result<ImuCalibration, ProtocolError> {
    if buf.len() < 35 {
        return Err(ProtocolError::ReportTooShort {
            expected: 35,
            actual: buf.len(),
        });
    }
    if buf[0] != DS_FEATURE_REPORT_CALIBRATION {
        return Err(ProtocolError::UnexpectedReportId(buf[0]));
    }

    let gyro_pitch_bias = le_i16(buf, 1);
    let gyro_yaw_bias = le_i16(buf, 3);
    let gyro_roll_bias = le_i16(buf, 5);
    let gyro_pitch_plus = le_i16(buf, 7);
    let gyro_pitch_minus = le_i16(buf, 9);
    let gyro_yaw_plus = le_i16(buf, 11);
    let gyro_yaw_minus = le_i16(buf, 13);
    let gyro_roll_plus = le_i16(buf, 15);
    let gyro_roll_minus = le_i16(buf, 17);
    let gyro_speed_plus = le_i16(buf, 19);
    let gyro_speed_minus = le_i16(buf, 21);
    let acc_x_plus = le_i16(buf, 23);
    let acc_x_minus = le_i16(buf, 25);
    let acc_y_plus = le_i16(buf, 27);
    let acc_y_minus = le_i16(buf, 29);
    let acc_z_plus = le_i16(buf, 31);
    let acc_z_minus = le_i16(buf, 33);

    let speed_2x = gyro_speed_plus + gyro_speed_minus;
    let make_gyro = |plus: i32, minus: i32, bias: i32| -> AxisCalibration {
        let denom = (plus - bias).abs() + (minus - bias).abs();
        if denom == 0 || speed_2x == 0 {
            AxisCalibration {
                bias: 0,
                sens_numer: DS_GYRO_RANGE,
                sens_denom: i16::MAX as i32,
            }
        } else {
            AxisCalibration {
                bias: 0,
                sens_numer: speed_2x * DS_GYRO_RES_PER_DEG_S,
                sens_denom: denom,
            }
        }
    };

    let make_accel = |plus: i32, minus: i32| -> AxisCalibration {
        let range_2g = plus - minus;
        if range_2g == 0 {
            AxisCalibration {
                bias: 0,
                sens_numer: DS_ACC_RANGE,
                sens_denom: i16::MAX as i32,
            }
        } else {
            AxisCalibration {
                bias: plus - range_2g / 2,
                sens_numer: 2 * DS_ACC_RES_PER_G,
                sens_denom: range_2g,
            }
        }
    };

    Ok(ImuCalibration {
        gyro: [
            make_gyro(gyro_pitch_plus, gyro_pitch_minus, gyro_pitch_bias),
            make_gyro(gyro_yaw_plus, gyro_yaw_minus, gyro_yaw_bias),
            make_gyro(gyro_roll_plus, gyro_roll_minus, gyro_roll_bias),
        ],
        accel: [
            make_accel(acc_x_plus, acc_x_minus),
            make_accel(acc_y_plus, acc_y_minus),
            make_accel(acc_z_plus, acc_z_minus),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_calibration_falls_back_safely() {
        let mut buf = [0u8; 41];
        buf[0] = DS_FEATURE_REPORT_CALIBRATION;
        let cal = parse_calibration_report(&buf).unwrap();
        for axis in cal.gyro.iter().chain(cal.accel.iter()) {
            assert_ne!(axis.sens_denom, 0);
            let _ = axis.apply(1000);
        }
    }

    #[test]
    fn valid_calibration_scales_correctly() {
        let mut buf = [0u8; 41];
        buf[0] = DS_FEATURE_REPORT_CALIBRATION;
        buf[19..21].copy_from_slice(&540i16.to_le_bytes()); // speed_plus
        buf[21..23].copy_from_slice(&540i16.to_le_bytes()); // speed_minus
        buf[7..9].copy_from_slice(&10800i16.to_le_bytes()); // pitch_plus
        buf[9..11].copy_from_slice(&(-10800i16).to_le_bytes()); // pitch_minus
        buf[23..25].copy_from_slice(&8192i16.to_le_bytes()); // acc_x_plus
        buf[25..27].copy_from_slice(&(-8192i16).to_le_bytes()); // acc_x_minus

        let cal = parse_calibration_report(&buf).unwrap();
        assert_eq!(cal.accel[0].apply(8192), DS_ACC_RES_PER_G);
        assert_ne!(cal.gyro[0].apply(100), 0);
    }
}
