# DSMaster

Low-latency Windows desktop utility for Sony PlayStation 5 DualSense and DualSense Edge controllers, written in Rust with a GPU-accelerated [GPUI](https://gpui.rs) interface.

## Current Features

- **USB and Bluetooth DualSense / DualSense Edge support:** Native Win32 overlapped HID I/O with event-driven hot-plug detection (`CM_Register_Notification`) and CRC-32 verification on Bluetooth `0x31` reports.
- **Xbox 360 (XInput) emulation:** Pure-Rust overlapped `ViGEmBus` IOCTL client with motor rumble routed back to the physical DualSense.
- **Adaptive Triggers:** Independent L2 and R2 presets (`Off`, `Rigid`, `Weapon`, `Pulse`) with automatic actuator withdrawal on exit.
- **Lightbar and Player LEDs:** 1-click RGB presets, player indicator LED masks, and rumble toggle.
- **VSync Live Input Monitor:** Wait-free `SeqLock` telemetry snapshots rendered at display refresh rate for sticks, triggers, D-Pad, buttons, and rumble feedback.

## Coming Soon

- DualShock 4 emulation
- Virtual DualSense emulation
- HidHide device cloaking integration
- Stick deadzones, response curves, remapping, and per-application profiles
- System tray background operation

## Requirements

- Windows 10 or Windows 11 (x64)
- [Nefarius ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) driver for Xbox 360 controller emulation

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
- `src/emu/` — DualSense-to-XUSB mapping and pure-Rust `ViGEmBus` IOCTL client.
- `src/engine/` — MMCSS `"Games"` per-controller I/O loop, `ArcSwap` profile updates, and wait-free `SeqLock` telemetry.
- `src/ui/` — GPUI interface (`theme`, `widgets`, `DsMasterView`).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
