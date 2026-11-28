#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    SendInput,
};

use super::processing::{ProcessedInput, compute_gyro_delta};
use super::profile::{ControllerProfile, GyroOutputMode};
use crate::protocol::DualSenseButtons;

pub struct MouseInjector {
    last_active: bool,
    last_id: u8,
    last_x: u16,
    last_y: u16,
    two_finger_active: bool,
    last_scroll_y: i32,
    scroll_rem: f32,
    tap_start: Option<(Instant, u16, u16, u32)>,
    prev_click: bool,
    clicked_right: bool,
    rem_x: f32,
    rem_y: f32,
    flick_active: bool,
    flick_prev_angle: f32,
    flick_pending_rad: f32,
    flick_elapsed_s: f32,
    flick_duration_s: f32,
    flick_last_time: Option<Instant>,
}

impl Default for MouseInjector {
    fn default() -> Self {
        Self {
            last_active: false,
            last_id: 0,
            last_x: 0,
            last_y: 0,
            two_finger_active: false,
            last_scroll_y: 0,
            scroll_rem: 0.0,
            tap_start: None,
            prev_click: false,
            clicked_right: false,
            rem_x: 0.0,
            rem_y: 0.0,
            flick_active: false,
            flick_prev_angle: 0.0,
            flick_pending_rad: 0.0,
            flick_elapsed_s: 0.0,
            flick_duration_s: 0.1,
            flick_last_time: None,
        }
    }
}

impl MouseInjector {
    pub fn update(&mut self, input: &ProcessedInput, profile: &ControllerProfile) {
        let mut dx_f = 0.0f32;
        let mut dy_f = 0.0f32;
        let mut flags = MOUSE_EVENT_FLAGS(0);
        let mut wheel_delta: i32 = 0;
        let mut send_tap_click = false;

        if profile.touchpad_mouse {
            let tp0 = input.touch[0];
            let tp1 = input.touch[1];
            let click = input.buttons.contains(DualSenseButtons::TOUCHPAD);

            if profile.touchpad_scroll && tp0.active && tp1.active {
                let mid_y = (tp0.y as i32 + tp1.y as i32) / 2;
                if self.two_finger_active {
                    let raw_dy = mid_y - self.last_scroll_y;
                    let sens = profile.touchpad_sensitivity.clamp(0.1, 5.0) * 1.4;
                    self.scroll_rem += -(raw_dy as f32) * sens;
                    let steps = self.scroll_rem.trunc() as i32;
                    if steps != 0 {
                        wheel_delta = steps;
                        self.scroll_rem -= steps as f32;
                        flags |= MOUSEEVENTF_WHEEL;
                    }
                }
                self.two_finger_active = true;
                self.last_scroll_y = mid_y;
                self.last_active = false;
                self.tap_start = None;
            } else {
                self.two_finger_active = false;
                if tp0.active {
                    if self.last_active && self.last_id == tp0.id {
                        let raw_dx = tp0.x as i32 - self.last_x as i32;
                        let raw_dy = tp0.y as i32 - self.last_y as i32;
                        let sens = profile.touchpad_sensitivity.clamp(0.1, 5.0) * 0.65;
                        dx_f += raw_dx as f32 * sens;
                        dy_f += raw_dy as f32 * sens;
                    }
                    if profile.touchpad_tap_to_click && !click {
                        match &mut self.tap_start {
                            Some((_, sx, sy, max_d)) => {
                                let dist = tp0.x.abs_diff(*sx) as u32 + tp0.y.abs_diff(*sy) as u32;
                                *max_d = (*max_d).max(dist);
                            }
                            None if !self.last_active => {
                                self.tap_start = Some((Instant::now(), tp0.x, tp0.y, 0));
                            }
                            _ => {}
                        }
                    } else if click {
                        self.tap_start = None;
                    }
                    self.last_active = true;
                    self.last_id = tp0.id;
                    self.last_x = tp0.x;
                    self.last_y = tp0.y;
                } else {
                    if profile.touchpad_tap_to_click {
                        if let Some((t0, _, _, max_d)) = self.tap_start.take() {
                            if t0.elapsed().as_millis() <= 220 && max_d < 32 && !self.prev_click {
                                send_tap_click = true;
                            }
                        }
                    }
                    self.last_active = false;
                }
            }

            if click && !self.prev_click {
                self.clicked_right = tp1.active
                    || (profile.touchpad_split_click && tp0.active && tp0.x >= 960);
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
            self.two_finger_active = false;
            self.tap_start = None;
            if self.prev_click {
                flags |= if self.clicked_right {
                    MOUSEEVENTF_RIGHTUP
                } else {
                    MOUSEEVENTF_LEFTUP
                };
                self.prev_click = false;
            }
        }

        if profile.gyro_output == GyroOutputMode::Mouse {
            if let Some((gx, gy)) = compute_gyro_delta(
                input.calibrated_gyro,
                input.l2_analog,
                input.r2_analog,
                input.buttons,
                input.touch[0].active,
                profile,
            ) {
                let sens = profile.gyro_sensitivity.clamp(0.1, 5.0) * 0.035;
                dx_f += gx * sens;
                dy_f += gy * sens;
            }
        }

        if profile.flick_stick_enabled {
            let now = Instant::now();
            dx_f += self.update_flick_stick(
                input.raw_right_x,
                input.raw_right_y,
                profile.flick_stick_pixels_per_turn,
                profile.flick_stick_threshold,
                profile.flick_stick_flick_ms,
                now,
            );
        } else {
            self.flick_active = false;
            self.flick_pending_rad = 0.0;
            self.flick_last_time = None;
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
                        mouseData: wheel_delta as u32,
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

        if send_tap_click {
            let down = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_LEFTDOWN,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            let up = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_LEFTUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe {
                let _ = SendInput(&[down, up], size_of::<INPUT>() as i32);
            }
        }
    }

    fn update_flick_stick(
        &mut self,
        raw_x: u8,
        raw_y: u8,
        pixels_per_turn: f32,
        threshold: f32,
        flick_ms: u16,
        now: Instant,
    ) -> f32 {
        let dt = self
            .flick_last_time
            .map(|prev| now.saturating_duration_since(prev).as_secs_f32().min(0.1))
            .unwrap_or(0.004);
        self.flick_last_time = Some(now);

        let nx = (raw_x as f32 - 128.0) / 127.5;
        let ny = -(raw_y as f32 - 128.0) / 127.5; // up = +1
        let mag = nx.hypot(ny);
        let engage = threshold.clamp(0.45, 0.95);
        let release = engage * 0.75;
        let ppt = pixels_per_turn.clamp(200.0, 40_000.0);
        let px_per_rad = ppt / std::f32::consts::TAU;

        let mut delta_rad = 0.0f32;

        if !self.flick_active {
            if mag >= engage {
                let angle = nx.atan2(ny); // 0 = up, +pi/2 = right, -pi/2 = left, +/-pi = down
                self.flick_active = true;
                self.flick_prev_angle = angle;
                self.flick_pending_rad = angle;
                self.flick_elapsed_s = 0.0;
                self.flick_duration_s = (flick_ms.clamp(20, 250) as f32) / 1000.0;
            }
        } else if mag >= release {
            let angle = nx.atan2(ny);
            let mut sweep = angle - self.flick_prev_angle;
            if sweep > std::f32::consts::PI {
                sweep -= std::f32::consts::TAU;
            } else if sweep < -std::f32::consts::PI {
                sweep += std::f32::consts::TAU;
            }
            self.flick_prev_angle = angle;
            delta_rad += sweep;
        } else {
            self.flick_active = false;
        }

        if self.flick_pending_rad.abs() > 1e-5 {
            let dur = self.flick_duration_s.max(0.02);
            let t0 = (self.flick_elapsed_s / dur).clamp(0.0, 1.0);
            self.flick_elapsed_s += dt;
            let t1 = (self.flick_elapsed_s / dur).clamp(0.0, 1.0);
            let ease = |t: f32| 1.0 - (1.0 - t).powi(3);
            let frac = ease(t1) - ease(t0);
            delta_rad += self.flick_pending_rad * frac;
            if t1 >= 1.0 {
                self.flick_pending_rad = 0.0;
            }
        }

        delta_rad * px_per_rad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flick_stick_snaps_right_and_sweeps_smoothly() {
        let mut mouse = MouseInjector::default();
        let t0 = Instant::now();
        // Prime clock at neutral
        assert_eq!(mouse.update_flick_stick(128, 128, 2000.0, 0.8, 100, t0), 0.0);

        // Deflect full right (90 deg = +500 px over 100 ms)
        let mut total_px = 0.0f32;
        for step in 1..=25 {
            let t = t0 + Duration::from_millis(step * 5);
            total_px += mouse.update_flick_stick(255, 128, 2000.0, 0.8, 100, t);
        }
        assert!(
            (total_px - 500.0).abs() < 5.0,
            "expected ~500 px for 90 deg right flick, got {total_px}"
        );

        // Sweep from right (90 deg) to down (180 deg) = +500 px
        let sweep_px = mouse.update_flick_stick(
            128,
            255,
            2000.0,
            0.8,
            100,
            t0 + Duration::from_millis(130),
        );
        assert!(
            (sweep_px - 500.0).abs() < 5.0,
            "expected ~500 px sweep, got {sweep_px}"
        );
    }
}
