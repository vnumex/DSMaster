# DSMaster

Low-latency Windows desktop utility for Sony PlayStation 5 DualSense and DualSense Edge controllers, written in Rust with a GPU-accelerated [GPUI](https://gpui.rs) interface.

## Current Features

- **USB and Bluetooth DualSense / DualSense Edge support:** Native Win32 overlapped HID I/O with event-driven hot-plug detection (`CM_Register_Notification`) and CRC-32 verification on Bluetooth `0x31` reports.
- **Xbox 360 and DualShock 4 emulation:** Pure-Rust overlapped `ViGEmBus` IOCTL client with motor rumble and DualShock 4 lightbar feedback routed back to the physical DualSense.
- **Virtual DualSense emulation (`usbip-win2`):** Clean-room Rust USB/IP v1.1.1 loopback server paired with user-installed `vadimgrn/usbip-win2`, exposing a virtual wired USB DualSense (`0x054C:0x0CE6`) with native adaptive triggers, lightbar RGB, player LEDs, and rumble forwarded back to your physical controller over USB or Bluetooth.
- **HidHide physical controller cloaking:** Direct `HidHide` IOCTL integration with 3-layer virtual-device self-exclusion to hide only the physical DualSense and prevent double input.
- **Stick, trigger, and button tuning:** Radial deadzone, anti-deadzone, response curves (`Linear`, `Quadratic`, `Cubic`, `Aggressive`), hair-trigger scaling, and button presets (`Standard`, `SwapFace`, `Tactical`, `BumperJumper`).
- **Adaptive triggers, lightbar, and player LEDs:** Independent L2 and R2 presets (`Off`, `Rigid`, `Weapon`, `Pulse`), 1-click RGB presets, player indicator LEDs, and rumble toggle.
- **Touchpad and gyro mouse:** Touchpad-to-mouse and calibrated gyro-to-mouse (`Off`, `Hold L2`, `Hold L1`, `Always On`).
- **Per-application profiles:** Automatic foreground-window profile switching via `SetWinEventHook` and TOML persistence at `%APPDATA%\DSMaster\profiles.toml`.
- **Background operation and settings:** Native Windows system tray with quick mode/cloak menu, close-to-tray, start minimized, launch at Windows startup, and single-instance wake-up.
- **VSync Live Input Monitor:** Wait-free `SeqLock` telemetry snapshots rendered at display refresh rate for sticks, triggers, D-Pad, buttons, and rumble feedback.

## Coming Soon

- Per-button custom remapping and macros

## Requirements

- Windows 10 or Windows 11 (x64)
- [Nefarius ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) driver for Xbox 360 and DualShock 4 emulation
- [Nefarius HidHide](https://github.com/nefarius/HidHide/releases) driver (optional) for physical controller cloaking
- [vadimgrn/usbip-win2](https://github.com/vadimgrn/usbip-win2/releases) (BSD-2-Clause, optional) for Virtual DualSense emulation. *Anti-cheat note:* Virtual USB/IP emulation is intended for single-player and native-DualSense PC titles; switch DSMaster to **Passthrough** mode (and avoid running `usbip-win2`) when playing competitive titles with strict kernel anti-cheat (e.g., FACEIT, Vanguard, Ricochet).

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

- `src/protocol/` — Zero-allocation DualSense USB/Bluetooth report parser & encoder, output parser/encoder, IMU calibration, adaptive trigger payloads, synthetic feature reports (`0x05`, `0x09`, `0x20`), and clean-room USB/HID descriptors.
- `src/hid/` — Overlapped Win32 HID transport, 3-layer virtual-device self-exclusion, and PnP device arrival/removal monitor.
- `src/emu/` — DualSense-to-XUSB/DS4 mapping, pure-Rust `ViGEmBus` IOCTL client, and clean-room USB/IP v1.1.1 Virtual DualSense backend (`usbip-win2`).
- `src/cloak/` — Direct `HidHide` IOCTL client, virtual-device safety guards, and NT path translation.
- `src/engine/` — MMCSS `"Games"` per-controller I/O loop, input processing, mouse injection, `ArcSwap` profiles, foreground app watcher, and wait-free `SeqLock` telemetry.
- `src/tray.rs` & `src/startup.rs` — Native Win32 system tray, single-instance guard, and Windows startup registration.
- `src/ui/` — GPUI interface (`theme`, `widgets`, `DsMasterView`).

## Disclaimer

DSMaster is an independent, clean-room open-source project and is not affiliated with, endorsed by, or sponsored by Sony Interactive Entertainment LLC, Microsoft Corporation, or Valve Corporation. PlayStation, DualSense, DualShock, and Xbox are trademarks of their respective owners, used strictly for interoperability identification.

## License

Licensed under the [MIT License](LICENSE).
