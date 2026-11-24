<h1 align="center">
  <img src="assets/icon-blue.svg" alt="DSMaster" width="96" align="absmiddle" />
  &nbsp;DSMaster
</h1>

<p align="center">
  Low-latency Windows desktop utility for Sony PlayStation 5 DualSense and DualSense Edge controllers, written in Rust with a GPU-accelerated <a href="https://gpui.rs">GPUI</a> interface.
</p>

![DSMaster Overview](assets/screenshot.jpg)

## Current Features

- **USB and Bluetooth DualSense / DualSense Edge support:** Native Win32 overlapped HID I/O with event-driven hot-plug detection (`CM_Register_Notification`) and CRC-32 verification on Bluetooth `0x31` reports.
- **Xbox 360 and DualShock 4 emulation:** Pure-Rust overlapped `ViGEmBus` IOCTL client with motor rumble and DualShock 4 lightbar feedback routed back to the physical DualSense.
- **Virtual DualSense emulation (`usbip-win2`):** Clean-room Rust USB/IP v1.1.1 loopback server paired with user-installed `vadimgrn/usbip-win2`, exposing a virtual wired USB DualSense (`0x054C:0x0CE6`) with native adaptive triggers, lightbar RGB, player LEDs, and rumble forwarded back to your physical controller over USB or Bluetooth.
- **HidHide physical controller cloaking:** Direct `HidHide` IOCTL integration with 3-layer virtual-device self-exclusion to hide only the physical DualSense and prevent double input.
- **Stick, trigger, and button tuning:** Radial/Axial/Passthrough deadzones, outer deadzone (saturation), anti-deadzone, axis inversion, L/R stick swap, response curves (`Linear`, `Quadratic`, `Cubic`, `Aggressive`), independent L2/R2 deadzones & hair-trigger scaling, full customizable button-to-button remapping (with swap & disable), and Turbo rapid-fire.
- **Adaptive triggers, lighting, and rumble:** Independent L2 and R2 presets (`Off`, `Soft`, `Rigid`, `Weapon`, `Bow`, `Slope`, `Pulse`, `Machine`, `Galloping`, `Custom`), custom trigger parameters, rumble intensity scaling with live Left/Right/Both motor testing, lightbar RGB picker & animations (`Static`, `Breathing`, `Rainbow`, `Battery`, `Trigger`), lightbar & player LED brightness, mic mute LED override, low-battery flash alert, and idle lightbar dim timeout.
- **Bluetooth speaker, microphone & HD voice-coil haptics:** Dedicated **Audio & mic** section with an optional Virtual Windows Audio Device (`Speakers` + `Microphone (DualSense Wireless Audio)` via `usbip-win2` UAC 1.0), WASAPI loopback PC audio mirroring to the DualSense speaker/3.5 mm jack over Bluetooth (`0x35` 48 kHz 10 ms CBR Opus), full-duplex Bluetooth microphone capture (`0x31` mono Opus), low-passed 3 kHz stereo HD haptics (`0x32` / combined `0x35`), route selection (`Built-in speaker`, `Headphone jack`), independent volume/gain, and built-in test chime & HD haptics generators.
- **Touchpad, gyro & diagnostics:** Touchpad-to-mouse with 2-finger vertical scroll, tap-to-click, and split L/R click zones; calibrated gyro-to-mouse or gyro-to-right-stick (`Off`, `Hold L2`, `Hold L1`, `Hold R2`, `Hold R1`, `Touchpad`, `Always On`) with deadzone, yaw/roll axes, inversion, and live IMU telemetry; and **Device info** hardware/firmware diagnostics (`0x09`/`0x20` reports).
- **Per-application profiles:** Automatic foreground-window profile switching via `SetWinEventHook`, profile duplicate/reset, clipboard TOML export/import, and TOML persistence at `%APPDATA%\DSMaster\profiles.toml`.
- **Background operation and settings:** Native Windows system tray with quick mode/cloak menu, close-to-tray, start minimized, launch at Windows startup, and single-instance wake-up.
- **VSync Live Input Monitor:** Wait-free `SeqLock` telemetry snapshots rendered at display refresh rate for sticks, triggers, D-Pad, buttons, Edge paddles, and rumble feedback.

## Coming Soon (Skipped Big Stuff)

- Full keyboard/mouse & multi-step macro timeline editor
- DSX UDP Port (`6969`) game mod listener API
- DualSense Edge onboard flash profile editor & PSVR2 Sense controller support
- Steam Workshop / community profile browser & installed games launcher
- Multi-language UI localization, lifetime usage stats & achievements, custom gamepad skins, and in-app log viewer

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
