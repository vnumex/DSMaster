# DSMaster

Low-latency Windows desktop utility for Sony PlayStation 5 DualSense and DualSense Edge controllers, written in Rust with a GPU-accelerated [GPUI](https://gpui.rs) interface.

## Current Features

- **USB and Bluetooth DualSense / DualSense Edge support:** Native Win32 overlapped HID I/O with event-driven hot-plug detection (`CM_Register_Notification`) and CRC-32 verification on Bluetooth `0x31` reports.
- **Xbox 360 and DualShock 4 emulation:** Pure-Rust overlapped `ViGEmBus` IOCTL client with motor rumble and DualShock 4 lightbar feedback routed back to the physical DualSense.
- **HidHide physical controller cloaking:** Direct `HidHide` IOCTL integration to hide the physical DualSense from games and prevent double input.
- **Stick, trigger, and button tuning:** Radial deadzone, anti-deadzone, response curves (`Linear`, `Quadratic`, `Cubic`, `Aggressive`), hair-trigger scaling, and button presets (`Standard`, `SwapFace`, `Tactical`, `BumperJumper`).
- **Adaptive triggers, lightbar, and player LEDs:** Independent L2 and R2 presets (`Off`, `Rigid`, `Weapon`, `Pulse`), 1-click RGB presets, player indicator LEDs, and rumble toggle.
- **Touchpad and gyro mouse:** Touchpad-to-mouse and calibrated gyro-to-mouse (`Off`, `Hold L2`, `Hold L1`, `Always On`).
- **Per-application profiles:** Automatic foreground-window profile switching via `SetWinEventHook` and TOML persistence at `%APPDATA%\DSMaster\profiles.toml`.
- **Background operation and settings:** Native Windows system tray with quick mode/cloak menu, close-to-tray, start minimized, launch at Windows startup, and single-instance wake-up.
- **VSync Live Input Monitor:** Wait-free `SeqLock` telemetry snapshots rendered at display refresh rate for sticks, triggers, D-Pad, buttons, and rumble feedback.

## Coming Soon

- Virtual DualSense emulation (`usbip-win2`)
- Per-button custom remapping and macros

## Requirements

- Windows 10 or Windows 11 (x64)
- [Nefarius ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) driver for Xbox 360 and DualShock 4 emulation
- [Nefarius HidHide](https://github.com/nefarius/HidHide/releases) driver (optional) for physical controller cloaking

## Building

### MSVC (with Windows 11 SDK)

```powershell
cargo build --release
```

### MinGW / GNU (`x86_64-pc-windows-gnu`)

```powershell
cargo build --profile gui
```

Run unit tests:

```powershell
cargo test
```

## Architecture

- `src/protocol/` — Zero-allocation DualSense USB/Bluetooth report parser, output encoder, IMU calibration, and adaptive trigger payloads.
- `src/hid/` — Overlapped Win32 HID transport and PnP device arrival/removal monitor.
- `src/emu/` — DualSense-to-XUSB/DS4 mapping and pure-Rust `ViGEmBus` IOCTL client.
- `src/cloak/` — Direct `HidHide` IOCTL client and NT path translation.
- `src/engine/` — MMCSS `"Games"` per-controller I/O loop, input processing, mouse injection, `ArcSwap` profiles, foreground app watcher, and wait-free `SeqLock` telemetry.
- `src/tray.rs` & `src/startup.rs` — Native Win32 system tray, single-instance guard, and Windows startup registration.
- `src/ui/` — GPUI interface (`theme`, `widgets`, `DsMasterView`).

## License

Licensed under the [MIT License](LICENSE).
