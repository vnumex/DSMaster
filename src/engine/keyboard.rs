use std::collections::VecDeque;
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
    VIRTUAL_KEY,
};

use super::profile::{
    ButtonAction, ControllerProfile, MAX_MACRO_STEPS, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN,
    RemappableButton,
};
use crate::protocol::{DPad, DualSenseButtons, RawInputState};

const VK_CONTROL: u16 = 0x11;
const VK_SHIFT: u16 = 0x10;
const VK_MENU: u16 = 0x12; // Alt
const VK_LWIN: u16 = 0x5B;
const VK_ESCAPE: u16 = 0x1B;

#[derive(Debug, Clone, Copy)]
struct QueuedMacroStep {
    vk: u16,
    modifiers: u8,
    due_at: Instant,
}

#[derive(Default)]
pub struct KeyboardInjector {
    prev_pressed: [bool; 18],
    held_keys: [Option<(u16, u8)>; 18],
    macro_queue: VecDeque<QueuedMacroStep>,
}

impl KeyboardInjector {
    #[inline]
    pub fn has_pending_macro(&self) -> bool {
        !self.macro_queue.is_empty()
    }

    pub fn update(&mut self, raw: &RawInputState, profile: &ControllerProfile, now: Instant) {
        let states = button_states(raw);

        for (idx, &src) in RemappableButton::SOURCES.iter().enumerate() {
            let pressed = states[idx];
            let was_pressed = self.prev_pressed[idx];
            self.prev_pressed[idx] = pressed;

            let action = profile.action_map.get(src);

            // If a previously held key is no longer bound to the same hold action, release it.
            if let Some((held_vk, held_mods)) = self.held_keys[idx] {
                let still_matching = pressed
                    && matches!(
                        action,
                        ButtonAction::Key {
                            vk,
                            modifiers,
                            hold: true
                        } if vk == held_vk && modifiers == held_mods
                    );
                if !still_matching {
                    send_combo_up(held_vk, held_mods);
                    self.held_keys[idx] = None;
                }
            }

            if pressed && !was_pressed {
                match action {
                    ButtonAction::None => {}
                    ButtonAction::Key {
                        vk,
                        modifiers,
                        hold,
                    } if vk != 0 => {
                        if hold {
                            send_combo_down(vk, modifiers);
                            self.held_keys[idx] = Some((vk, modifiers));
                        } else {
                            send_combo_tap(vk, modifiers);
                        }
                    }
                    ButtonAction::Key { .. } => {}
                    ButtonAction::Media(media) => {
                        send_combo_tap(media.vk(), 0);
                    }
                    ButtonAction::Macro { steps, len } => {
                        let count = (len as usize).min(MAX_MACRO_STEPS);
                        let mut cursor = now;
                        for step in steps.iter().take(count) {
                            if step.vk != 0 {
                                self.macro_queue.push_back(QueuedMacroStep {
                                    vk: step.vk,
                                    modifiers: step.modifiers,
                                    due_at: cursor,
                                });
                            }
                            cursor += Duration::from_millis(step.delay_ms.max(15) as u64);
                        }
                    }
                }
            }
        }

        self.tick(now);
    }

    pub fn tick(&mut self, now: Instant) {
        while let Some(front) = self.macro_queue.front().copied() {
            if front.due_at > now {
                break;
            }
            self.macro_queue.pop_front();
            send_combo_tap(front.vk, front.modifiers);
        }
    }

    pub fn release_all(&mut self) {
        for slot in &mut self.held_keys {
            if let Some((vk, mods)) = slot.take() {
                send_combo_up(vk, mods);
            }
        }
        self.macro_queue.clear();
        self.prev_pressed = [false; 18];
    }
}

impl Drop for KeyboardInjector {
    fn drop(&mut self) {
        self.release_all();
    }
}

fn button_states(raw: &RawInputState) -> [bool; 18] {
    let b = raw.buttons;
    let d_up = matches!(raw.dpad, DPad::North | DPad::NorthEast | DPad::NorthWest);
    let d_down = matches!(raw.dpad, DPad::South | DPad::SouthEast | DPad::SouthWest);
    let d_left = matches!(raw.dpad, DPad::West | DPad::NorthWest | DPad::SouthWest);
    let d_right = matches!(raw.dpad, DPad::East | DPad::NorthEast | DPad::SouthEast);
    [
        b.contains(DualSenseButtons::CROSS),
        b.contains(DualSenseButtons::CIRCLE),
        b.contains(DualSenseButtons::SQUARE),
        b.contains(DualSenseButtons::TRIANGLE),
        b.contains(DualSenseButtons::L1),
        b.contains(DualSenseButtons::R1),
        raw.l2_analog >= 32,
        raw.r2_analog >= 32,
        b.contains(DualSenseButtons::L3),
        b.contains(DualSenseButtons::R3),
        b.contains(DualSenseButtons::OPTIONS),
        b.contains(DualSenseButtons::CREATE),
        b.contains(DualSenseButtons::TOUCHPAD),
        b.contains(DualSenseButtons::MIC_MUTE),
        d_up,
        d_down,
        d_left,
        d_right,
    ]
}

fn is_extended_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28 // PgUp..Down
            | 0x2C..=0x2E // PrintScr, Ins, Del
            | 0x5B..=0x5D // LWin, RWin, Apps
            | 0x6F // Numpad /
            | 0x90 // NumLock
            | 0xA3 | 0xA5 // RCtrl, RAlt
            | 0xAD..=0xB3 // Volume & Media keys
    )
}

fn make_key_input(vk: u16, key_up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if is_extended_vk(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn push_modifiers(out: &mut Vec<INPUT>, modifiers: u8, key_up: bool) {
    let list = [
        (MOD_CTRL, VK_CONTROL),
        (MOD_SHIFT, VK_SHIFT),
        (MOD_ALT, VK_MENU),
        (MOD_WIN, VK_LWIN),
    ];
    if key_up {
        for &(bit, vk) in list.iter().rev() {
            if (modifiers & bit) != 0 {
                out.push(make_key_input(vk, true));
            }
        }
    } else {
        for &(bit, vk) in &list {
            if (modifiers & bit) != 0 {
                out.push(make_key_input(vk, false));
            }
        }
    }
}

fn send_combo_down(vk: u16, modifiers: u8) {
    let mut inputs = Vec::with_capacity(5);
    push_modifiers(&mut inputs, modifiers, false);
    inputs.push(make_key_input(vk, false));
    unsafe {
        let _ = SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

fn send_combo_up(vk: u16, modifiers: u8) {
    let mut inputs = Vec::with_capacity(5);
    inputs.push(make_key_input(vk, true));
    push_modifiers(&mut inputs, modifiers, true);
    unsafe {
        let _ = SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

pub fn send_combo_tap(vk: u16, modifiers: u8) {
    let mut inputs = Vec::with_capacity(10);
    push_modifiers(&mut inputs, modifiers, false);
    inputs.push(make_key_input(vk, false));
    inputs.push(make_key_input(vk, true));
    push_modifiers(&mut inputs, modifiers, true);
    unsafe {
        let _ = SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

/// Polls Windows keyboard state for live hotkey recording in the UI.
///
/// Returns:
/// - `Some(Err(()))` if `Escape` was pressed (cancel recording),
/// - `Some(Ok((vk, modifiers)))` when any non-modifier key is currently down,
/// - `None` when no recordable key is pressed.
pub fn poll_recorded_hotkey() -> Option<Result<(u16, u8), ()>> {
    let is_down = |vk: i32| unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 };

    if is_down(VK_ESCAPE as i32) {
        return Some(Err(()));
    }

    let mut mods = 0u8;
    if is_down(0x11) || is_down(0xA2) || is_down(0xA3) {
        mods |= MOD_CTRL;
    }
    if is_down(0x10) || is_down(0xA0) || is_down(0xA1) {
        mods |= MOD_SHIFT;
    }
    if is_down(0x12) || is_down(0xA4) || is_down(0xA5) {
        mods |= MOD_ALT;
    }
    if is_down(0x5B) || is_down(0x5C) {
        mods |= MOD_WIN;
    }

    // Scan standard keyboard, function (F1-F24), numpad, media, and OEM virtual keys.
    for vk in 0x08u16..=0xDFu16 {
        let is_modifier_or_mouse = matches!(
            vk,
            0x10..=0x12 // Shift, Ctrl, Alt
                | 0x1B // Esc (handled above)
                | 0x5B..=0x5D // Win, Apps
                | 0xA0..=0xA5 // L/R Shift, Ctrl, Alt
        );
        if is_modifier_or_mouse {
            continue;
        }
        if is_down(vk as i32) {
            return Some(Ok((vk, mods)));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::profile::{ActionMap, MacroStep, MediaKey, format_key_combo};

    #[test]
    fn key_combo_and_media_formatting() {
        assert_eq!(format_key_combo(0x4D, MOD_CTRL | MOD_SHIFT), "Ctrl+Shift+M");
        assert_eq!(format_key_combo(0x7C, 0), "F13");
        assert_eq!(
            ButtonAction::Media(MediaKey::PlayPause).format_label(),
            "Play/Pause"
        );
        assert_eq!(
            ButtonAction::Key {
                vk: 0x4D,
                modifiers: MOD_CTRL | MOD_SHIFT,
                hold: true
            }
            .format_label(),
            "Ctrl+Shift+M (Hold)"
        );
    }

    #[test]
    fn action_map_toml_skips_unbound_buttons() {
        let mut map = ActionMap::default();
        map.set(
            RemappableButton::MicMute,
            ButtonAction::Key {
                vk: 0x4D,
                modifiers: MOD_CTRL | MOD_SHIFT,
                hold: false,
            },
        );
        let mut steps = [MacroStep::default(); MAX_MACRO_STEPS];
        steps[0] = MacroStep {
            vk: 0x45,
            modifiers: 0,
            delay_ms: 50,
        };
        steps[1] = MacroStep {
            vk: 0x20,
            modifiers: 0,
            delay_ms: 50,
        };
        map.set(
            RemappableButton::Create,
            ButtonAction::Macro { steps, len: 2 },
        );
        assert_eq!(map.custom_count(), 2);

        let raw = toml::to_string_pretty(&map).unwrap();
        assert!(raw.contains("mic_mute"));
        assert!(raw.contains("create"));
        assert!(!raw.contains("cross"));

        let parsed: ActionMap = toml::from_str(&raw).unwrap();
        assert_eq!(parsed, map);
    }
}
