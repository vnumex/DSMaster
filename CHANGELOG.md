# Changelog

All notable changes to DSMaster are documented in this file.

## [v0.3.1] - 2026-11-27

### Added
- **Windows desktop notifications:** Native system-tray alerts for DualSense / DualSense Edge connect, disconnect, and low-battery warnings (with 5% discharge hysteresis), individually configurable in **Settings → Notifications & Bluetooth power**.
- **Optional Bluetooth auto-disconnect & manual disconnect:** Idle sleep timer (`Off`, `5m`, `10m`, `15m`, `20m`, `30m`) and one-click **Disconnect BT** action using Win32 `IOCTL_BTH_DISCONNECT_DEVICE` so the controller powers off cleanly to conserve battery.
- **CemuHook / DSU UDP motion server (`127.0.0.1:26760`):** Streams calibrated gyro, accelerometer, sticks, buttons, triggers, 2-finger touchpad, battery, and MAC address over DSU v1001 to emulators (Cemu, Dolphin, Ryujinx, RPCS3, PCSX2), with live client status in **Settings → Integrations**.
- **DSX UDP mod server (`127.0.0.1:6969`):** Accepts DSX-compatible JSON UDP packets from PC game mods to drive adaptive triggers (modes `0–18`), lightbar RGB, player LEDs, and mic mute LED in real time, with live packet/override status and a **Reset mod override** button.

### Fixed & Polished
- **LED brightness dimming & lightbar passthrough restore:** Improved perceptual LED brightness dimming and restored the controller's native PlayStation blue glow when returning to passthrough over Bluetooth.
- **UI polish:** Live 250 ms refresh for Settings integration status pills, lighting/settings layout refinements, and crisper window control icons.
