//! Official upstream driver discovery, download, and interactive installer launch.
//!
//! DSMaster never bundles or silently installs third-party kernel drivers.
//! When the user explicitly clicks **Install** in Settings, this module downloads
//! the official signed x64 installer from the upstream GitHub release over HTTPS
//! into `%TEMP%\DSMaster\installers` and launches it interactively through the
//! Windows Shell (`ShellExecuteExW`) so Windows displays the vendor signature and
//! standard UAC prompt.

use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;

use parking_lot::Mutex;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MIN_INSTALLER_BYTES: u64 = 100 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    VigemBus,
    HidHide,
    UsbipWin2,
}

impl DriverKind {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::VigemBus => "ViGEmBus",
            Self::HidHide => "HidHide",
            Self::UsbipWin2 => "usbip-win2",
        }
    }

    pub fn api_latest_url(self) -> &'static str {
        match self {
            Self::VigemBus => "https://api.github.com/repos/nefarius/ViGEmBus/releases/latest",
            Self::HidHide => "https://api.github.com/repos/nefarius/HidHide/releases/latest",
            Self::UsbipWin2 => "https://api.github.com/repos/vadimgrn/usbip-win2/releases/latest",
        }
    }

    pub fn fallback_installer(self) -> (&'static str, &'static str) {
        match self {
            Self::VigemBus => (
                "ViGEmBus_1.22.0_x64_x86_arm64.exe",
                "https://github.com/nefarius/ViGEmBus/releases/download/v1.22.0/ViGEmBus_1.22.0_x64_x86_arm64.exe",
            ),
            Self::HidHide => (
                "HidHide_1.5.230_x64.exe",
                "https://github.com/nefarius/HidHide/releases/download/v1.5.230.0/HidHide_1.5.230_x64.exe",
            ),
            Self::UsbipWin2 => (
                "USBip-0.9.8.1-x64.exe",
                "https://github.com/vadimgrn/usbip-win2/releases/download/v.0.9.8.1/USBip-0.9.8.1-x64.exe",
            ),
        }
    }

    fn matches_x64_asset(self, filename: &str) -> bool {
        let lower = filename.to_ascii_lowercase();
        if !(lower.ends_with(".exe") || lower.ends_with(".msi")) {
            return false;
        }
        match self {
            Self::VigemBus => lower.contains("vigembus") && lower.contains("x64"),
            Self::HidHide => lower.contains("hidhide") && lower.contains("x64"),
            Self::UsbipWin2 => {
                lower.contains("usbip") && lower.contains("x64") && !lower.contains("arm64")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverInstallPhase {
    Downloading,
    RunningInstaller,
    Finished(Result<(), String>),
}

pub struct DriverInstallJob {
    pub kind: DriverKind,
    phase: Arc<Mutex<DriverInstallPhase>>,
}

impl DriverInstallJob {
    pub fn spawn(kind: DriverKind) -> Self {
        let phase = Arc::new(Mutex::new(DriverInstallPhase::Downloading));
        let phase_bg = Arc::clone(&phase);

        let _ = thread::Builder::new()
            .name(format!("ds-drv-{}", kind.display_name()))
            .spawn(move || {
                let result = run_driver_install(kind, &phase_bg);
                *phase_bg.lock() = DriverInstallPhase::Finished(result);
            });

        Self { kind, phase }
    }

    pub fn phase(&self) -> DriverInstallPhase {
        self.phase.lock().clone()
    }
}

fn run_driver_install(
    kind: DriverKind,
    phase: &Arc<Mutex<DriverInstallPhase>>,
) -> Result<(), String> {
    let (filename, url) = resolve_installer_url(kind);
    let dest = installer_download_path(&filename)?;
    download_file(&url, &dest)?;
    validate_installer_file(&dest)?;

    *phase.lock() = DriverInstallPhase::RunningInstaller;
    launch_and_wait_installer(&dest)?;
    Ok(())
}

/// Queries GitHub's latest release metadata via Windows' built-in `curl.exe`,
/// falling back cleanly to the pinned verified release URL if rate-limited or offline.
fn resolve_installer_url(kind: DriverKind) -> (String, String) {
    if let Ok(out) = Command::new("curl.exe")
        .args([
            "-fsSL",
            "--connect-timeout",
            "10",
            "--max-time",
            "20",
            "-H",
            "User-Agent: DSMaster",
            "-H",
            "Accept: application/vnd.github+json",
            kind.api_latest_url(),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        && out.status.success()
    {
        let body = String::from_utf8_lossy(&out.stdout);
        if let Some(found) = extract_release_asset_url(&body, kind) {
            return found;
        }
    }

    let (name, url) = kind.fallback_installer();
    (name.to_owned(), url.to_owned())
}

/// Lightweight scanner for `"browser_download_url": "https://..."` inside a GitHub release JSON
/// without pulling in an HTTP or JSON crate dependency.
pub fn extract_release_asset_url(json: &str, kind: DriverKind) -> Option<(String, String)> {
    const NEEDLE: &str = "\"browser_download_url\"";
    let mut rest = json;
    while let Some(pos) = rest.find(NEEDLE) {
        rest = &rest[pos + NEEDLE.len()..];
        let colon = rest.find(':')?;
        rest = rest[colon + 1..].trim_start();
        if !rest.starts_with('"') {
            continue;
        }
        rest = &rest[1..];
        let end = rest.find('"')?;
        let url = &rest[..end];
        rest = &rest[end + 1..];

        if !url.starts_with("https://github.com/") {
            continue;
        }
        let filename = url.rsplit('/').next()?;
        if kind.matches_x64_asset(filename) {
            return Some((sanitize_filename(filename), url.to_owned()));
        }
    }
    None
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect()
}

fn installer_download_path(filename: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("DSMaster").join("installers");
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create temp installer dir: {e}"))?;
    let clean = sanitize_filename(filename);
    if clean.is_empty() {
        return Err("Invalid installer filename".to_owned());
    }
    Ok(dir.join(clean))
}

fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    let dest_str = dest
        .to_str()
        .ok_or_else(|| "Invalid temporary installer path".to_owned())?;
    let out = Command::new("curl.exe")
        .args([
            "-fsSL",
            "--retry",
            "2",
            "--connect-timeout",
            "15",
            "--max-time",
            "180",
            "-H",
            "User-Agent: DSMaster",
            "-o",
            dest_str,
            url,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to invoke curl.exe: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("Download failed (exit {:?})", out.status.code())
        } else {
            format!("Download failed: {stderr}")
        });
    }
    Ok(())
}

fn validate_installer_file(path: &Path) -> Result<(), String> {
    let meta = fs::metadata(path).map_err(|e| format!("Downloaded file unreadable: {e}"))?;
    if meta.len() < MIN_INSTALLER_BYTES {
        let _ = fs::remove_file(path);
        return Err("Downloaded installer is unexpectedly small or incomplete".to_owned());
    }
    let header = fs::read(path).map_err(|e| format!("Failed to read installer header: {e}"))?;
    let is_exe = header.starts_with(b"MZ");
    let is_msi = header.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    if !is_exe && !is_msi {
        let _ = fs::remove_file(path);
        return Err("Downloaded file is not a valid Windows installer".to_owned());
    }
    Ok(())
}

fn wide_null(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Launches the official signed installer interactively via Windows Shell (triggering native
/// UAC elevation) and waits until the installer exits so DSMaster can refresh driver status.
fn launch_and_wait_installer(path: &Path) -> Result<(), String> {
    let verb = wide_null(OsStr::new("open"));
    let file = wide_null(path.as_os_str());
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };

    // SAFETY: `sei` points to valid NUL-terminated UTF-16 buffers for the duration of the call.
    unsafe {
        ShellExecuteExW(&mut sei)
            .map_err(|e| format!("Installer was canceled or could not start ({e})"))?;
        if !sei.hProcess.is_invalid() && sei.hProcess != HANDLE::default() {
            let _ = WaitForSingleObject(sei.hProcess, INFINITE);
            let _ = CloseHandle(sei.hProcess);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_x64_asset_and_skips_arm64_for_usbip() {
        let json = r#"{
            "tag_name": "v.0.9.8.1",
            "assets": [
                {"name": "USBip-0.9.8.1-arm64.exe", "browser_download_url": "https://github.com/vadimgrn/usbip-win2/releases/download/v.0.9.8.1/USBip-0.9.8.1-arm64.exe"},
                {"name": "USBip-0.9.8.1-x64.exe", "browser_download_url": "https://github.com/vadimgrn/usbip-win2/releases/download/v.0.9.8.1/USBip-0.9.8.1-x64.exe"}
            ]
        }"#;
        let (name, url) = extract_release_asset_url(json, DriverKind::UsbipWin2).unwrap();
        assert_eq!(name, "USBip-0.9.8.1-x64.exe");
        assert!(url.ends_with("USBip-0.9.8.1-x64.exe"));
    }

    #[test]
    fn extracts_vigem_and_hidhide_assets() {
        let vigem = r#"{"assets":[{"browser_download_url":"https://github.com/nefarius/ViGEmBus/releases/download/v1.22.0/ViGEmBus_1.22.0_x64_x86_arm64.exe"}]}"#;
        let (vname, _) = extract_release_asset_url(vigem, DriverKind::VigemBus).unwrap();
        assert_eq!(vname, "ViGEmBus_1.22.0_x64_x86_arm64.exe");

        let hidhide = r#"{"assets":[{"browser_download_url":"https://github.com/nefarius/HidHide/releases/download/v1.5.230.0/HidHide_1.5.230_x64.exe"}]}"#;
        let (hname, _) = extract_release_asset_url(hidhide, DriverKind::HidHide).unwrap();
        assert_eq!(hname, "HidHide_1.5.230_x64.exe");
    }

    #[test]
    fn rejects_non_github_urls() {
        let bad = r#"{"assets":[{"browser_download_url":"https://evil.example.com/USBip-0.9.8.1-x64.exe"}]}"#;
        assert!(extract_release_asset_url(bad, DriverKind::UsbipWin2).is_none());
    }
}
