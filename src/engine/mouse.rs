use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput,
};

use super::processing::ProcessedInput;
use super::profile::{ControllerProfile, GyroAimMode};
use crate::protocol::{DS_GYRO_RES_PER_DEG_S, DualSenseButtons};

pub struct MouseInjector {
    last_active: bool,
    last_id: u8,
    last_x: u16,
    last_y: u16,
    prev_click: bool,
    clicked_right: bool,
    rem_x: f32,
    rem_y: f32,
}

impl Default for MouseInjector {
    fn default() -> Self {
        Self {
            last_active: false,
            last_id: 0,
            last_x: 0,
            last_y: 0,
            prev_click: false,
            clicked_right: false,
            rem_x: 0.0,
            rem_y: 0.0,
        }
    }
}

impl MouseInjector {
    pub fn update(&mut self, input: &ProcessedInput, profile: &ControllerProfile) {
        let mut dx_f = 0.0f32;
        let mut dy_f = 0.0f32;
        let mut flags = MOUSE_EVENT_FLAGS(0);

        if profile.touchpad_mouse {
            let tp0 = input.touch[0];
            if tp0.active {
                if self.last_active && self.last_id == tp0.id {
                    let raw_dx = tp0.x as i32 - self.last_x as i32;
                    let raw_dy = tp0.y as i32 - self.last_y as i32;
                    let sens = profile.touchpad_sensitivity.clamp(0.1, 5.0) * 0.65;
                    dx_f += raw_dx as f32 * sens;
                    dy_f += raw_dy as f32 * sens;
                }
                self.last_active = true;
                self.last_id = tp0.id;
                self.last_x = tp0.x;
                self.last_y = tp0.y;
            } else {
                self.last_active = false;
            }

            let click = input.buttons.contains(DualSenseButtons::TOUCHPAD);
            if click && !self.prev_click {
                self.clicked_right = input.touch[1].active;
                flags |= if self.clicked_right {
                    MOUSEEVENTF_RIGHTDOWN
                } else {
                    MOUSEEVENTF_LEFTDOWN
                };
            } else if !click && self.prev_click {
                flags |= if self.clicked_right {
                    MOUSEEVENTF_RIGHTUP
                } else {
                    MOUSEEVENTF_LEFTUP
                };
            }
            self.prev_click = click;
        } else {
            self.last_active = false;
            if self.prev_click {
                flags |= if self.clicked_right {
                    MOUSEEVENTF_RIGHTUP
                } else {
                    MOUSEEVENTF_LEFTUP
                };
                self.prev_click = false;
            }
        }

        let gyro_active = match profile.gyro_mode {
            GyroAimMode::Off => false,
            GyroAimMode::HoldL2 => input.l2_analog > 32,
            GyroAimMode::HoldL1 => input.buttons.contains(DualSenseButtons::L1),
            GyroAimMode::AlwaysOn => true,
        };
        if gyro_active {
            // DualSense calibrated_gyro is scaled by DS_GYRO_RES_PER_DEG_S (1024 per deg/s).
            // Yaw = -gyro[1], Pitch = -gyro[0].
            let deg_s_yaw = -(input.calibrated_gyro[1] as f32) / (DS_GYRO_RES_PER_DEG_S as f32);
            let deg_s_pitch = -(input.calibrated_gyro[0] as f32) / (DS_GYRO_RES_PER_DEG_S as f32);
            let sens = profile.gyro_sensitivity.clamp(0.1, 5.0) * 0.035;
            dx_f += deg_s_yaw * sens;
            dy_f += deg_s_pitch * sens;
        }

        self.rem_x += dx_f;
        self.rem_y += dy_f;
        let move_x = self.rem_x.trunc() as i32;
        let move_y = self.rem_y.trunc() as i32;
        self.rem_x -= move_x as f32;
        self.rem_y -= move_y as f32;

        if move_x != 0 || move_y != 0 {
            flags |= MOUSEEVENTF_MOVE;
        }

        if flags.0 != 0 {
            let ev = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: move_x,
                        dy: move_y,
                        mouseData: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe {
                let _ = SendInput(&[ev], size_of::<INPUT>() as i32);
            }
        }
    }
}
