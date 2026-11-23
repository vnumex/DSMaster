//! Clean-room helper for detecting and invoking user-installed `vadimgrn/usbip-win2`.
//!
//! DSMaster never bundles or silently installs kernel drivers. This module only detects an
//! existing `usbip-win2` installation (`usbip2_ude` / `usbip2_filter` service + `usbip.exe`)
//! and invokes `usbip.exe` with `CREATE_NO_WINDOW` to attach/detach local loopback devices.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use tracing::{debug, warn};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
};
use windows::core::{PCWSTR, w};

/// Standard USB/IP TCP port (`3240`).
pub const DEFAULT_USBIP_PORT: u16 = 3240;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Locates `usbip.exe` in standard `usbip-win2` installation directories or on `PATH`.
pub fn find_usbip_exe() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::with_capacity(12);

    for env_key in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(env_key) {
            let base_path = PathBuf::from(base);
            candidates.push(base_path.join("USBip").join("usbip.exe"));
            candidates.push(base_path.join("usbip-win2").join("usbip.exe"));
        }
    }

    candidates.push(PathBuf::from(r"C:\Program Files\USBip\usbip.exe"));
    candidates.push(PathBuf::from(r"C:\Program Files\usbip-win2\usbip.exe"));

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if !dir.as_os_str().is_empty() {
                candidates.push(dir.join("usbip.exe"));
            }
        }
    }

    for candidate in candidates {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

fn registry_service_exists(subkey: PCWSTR) -> bool {
    let mut hkey = HKEY::default();
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, subkey, Some(0), KEY_READ, &mut hkey) };
    if status.is_ok() {
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        true
    } else {
        false
    }
}

/// Returns `true` if `usbip-win2`'s kernel service (`usbip2_ude` or `usbip2_filter`) is registered.
pub fn is_usbip_win2_service_registered() -> bool {
    registry_service_exists(w!(r"SYSTEM\CurrentControlSet\Services\usbip2_ude"))
        || registry_service_exists(w!(r"SYSTEM\CurrentControlSet\Services\usbip2_filter"))
}

/// Returns `true` if a user-installed `usbip-win2` driver and `usbip.exe` CLI are available.
pub fn is_usbip_win2_installed() -> bool {
    is_usbip_win2_service_registered() && find_usbip_exe().is_some()
}

/// Attaches a locally exported USB/IP device (`127.0.0.1:<port>`, `bus_id`) via `usbip.exe`
/// and returns the assigned `usbip-win2` VHCI port number.
pub fn attach_local(port: u16, bus_id: &str) -> Result<u32, String> {
    let exe = find_usbip_exe().ok_or_else(|| {
        "usbip.exe not found; install vadimgrn/usbip-win2 to enable Virtual DualSense".to_string()
    })?;

    let mut cmd = Command::new(&exe);
    cmd.creation_flags(CREATE_NO_WINDOW);
    if port != DEFAULT_USBIP_PORT {
        cmd.arg("--tcp-port").arg(port.to_string());
    }
    cmd.arg("attach")
        .arg("-r")
        .arg("127.0.0.1")
        .arg("-b")
        .arg(bus_id);

    debug!(
        exe = %exe.display(),
        tcp_port = port,
        bus_id,
        "invoking usbip.exe attach"
    );

    let output = cmd
        .output()
        .map_err(|e| format!("failed to execute {}: {e}", exe.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        return Err(format!(
            "usbip attach failed (exit {:?}): {}",
            output.status.code(),
            combined.trim()
        ));
    }

    if let Some(vhci_port) = parse_attach_port(&combined) {
        debug!(vhci_port, bus_id, "usbip-win2 attached virtual DualSense");
        return Ok(vhci_port);
    }

    // Fallback: query `usbip.exe port` and match the attached bus_id.
    let port_out = Command::new(&exe)
        .creation_flags(CREATE_NO_WINDOW)
        .arg("port")
        .output()
        .map_err(|e| format!("failed to execute {} port: {e}", exe.display()))?;

    let port_stdout = String::from_utf8_lossy(&port_out.stdout);
    if let Some(vhci_port) = parse_port_list_for_bus(&port_stdout, bus_id) {
        debug!(
            vhci_port,
            bus_id, "resolved usbip-win2 VHCI port from `usbip port`"
        );
        return Ok(vhci_port);
    }

    Err(format!(
        "usbip attach succeeded but VHCI port number could not be parsed (attach: {}, port: {})",
        combined.trim(),
        port_stdout.trim()
    ))
}

/// Detaches a previously attached `usbip-win2` VHCI port via `usbip.exe detach -p <vhci_port>`.
pub fn detach_port(vhci_port: u32) -> Result<(), String> {
    let exe = find_usbip_exe().ok_or_else(|| "usbip.exe not found during detach".to_string())?;

    let output = Command::new(&exe)
        .creation_flags(CREATE_NO_WINDOW)
        .arg("detach")
        .arg("-p")
        .arg(vhci_port.to_string())
        .output()
        .map_err(|e| format!("failed to execute {} detach: {e}", exe.display()))?;

    if output.status.success() {
        debug!(vhci_port, "usbip-win2 detached virtual DualSense port");
        Ok(())
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = format!(
            "usbip detach -p {vhci_port} failed (exit {:?}): {} {}",
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        );
        warn!("{msg}");
        Err(msg)
    }
}

/// Parses a decimal integer from the start of `s`, skipping leading spaces/colons.
fn parse_leading_u32(s: &str) -> Option<u32> {
    let trimmed = s.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ':' || c == '=');
    let digits_len = trimmed.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }
    trimmed[..digits_len].parse::<u32>().ok()
}

/// Parses the assigned VHCI port number from `usbip.exe attach` stdout/stderr.
///
/// Recognizes `vadimgrn/usbip-win2`'s `"successfully attached to port <N>"` as well as
/// generic `"port <N>"` / `"port: <N>"` outputs while ignoring `"tcp port <N>"`.
pub(crate) fn parse_attach_port(output: &str) -> Option<u32> {
    let lower = output.to_ascii_lowercase();

    // Highest-confidence patterns first so TCP port numbers in verbose logs are never mistaken.
    for needle in ["attached to port", "vhci port", "rhport"] {
        if let Some(pos) = lower.find(needle) {
            let after = &output[pos + needle.len()..];
            if let Some(port) = parse_leading_u32(after) {
                return Some(port);
            }
        }
    }

    // Fallback scan for standalone "port <N>" lines not prefixed by "tcp ".
    for line in output.lines() {
        let line_lower = line.to_ascii_lowercase();
        let mut search_start = 0usize;
        while let Some(rel) = line_lower[search_start..].find("port") {
            let idx = search_start + rel;
            let preceded_by_tcp = line_lower[..idx].trim_end().ends_with("tcp");
            let before_ok = idx == 0 || !line_lower.as_bytes()[idx - 1].is_ascii_alphanumeric();
            if before_ok && !preceded_by_tcp {
                if let Some(port) = parse_leading_u32(&line[idx + 4..]) {
                    return Some(port);
                }
            }
            search_start = idx + 4;
        }
    }

    None
}

/// Returns `true` if `line` contains `bus_id` delimited by non-busid characters
/// (so `"1-1"` does not falsely match `"1-10"`).
fn line_matches_bus_id(line: &str, bus_id: &str) -> bool {
    if bus_id.is_empty() {
        return false;
    }
    let bytes = line.as_bytes();
    let mut start = 0usize;
    while let Some(rel) = line[start..].find(bus_id) {
        let idx = start + rel;
        let end = idx + bus_id.len();
        let left_ok = idx == 0
            || !(bytes[idx - 1].is_ascii_alphanumeric()
                || bytes[idx - 1] == b'-'
                || bytes[idx - 1] == b'.');
        let right_ok = end == bytes.len()
            || !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-' || bytes[end] == b'.');
        if left_ok && right_ok {
            return true;
        }
        start = idx + 1;
    }
    false
}

/// Parses `usbip.exe port` output to find the VHCI port assigned to `bus_id`.
pub(crate) fn parse_port_list_for_bus(output: &str, bus_id: &str) -> Option<u32> {
    let mut current_port: Option<u32> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("port ") {
            if let Some(port) = parse_leading_u32(rest) {
                current_port = Some(port);
                continue;
            }
        }
        if let Some(port) = current_port {
            if line_matches_bus_id(trimmed, bus_id) {
                return Some(port);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_usbip_win2_attach_output() {
        assert_eq!(
            parse_attach_port("successfully attached to port 1\r\n"),
            Some(1)
        );
        assert_eq!(
            parse_attach_port("successfully attached to port 0"),
            Some(0)
        );
        assert_eq!(
            parse_attach_port("usbip: info: successfully attached to port 12\n"),
            Some(12)
        );
    }

    #[test]
    fn ignores_tcp_port_and_prefers_vhci_port() {
        let verbose = "connecting to 127.0.0.1 tcp port 3240\nsuccessfully attached to port 3\n";
        assert_eq!(parse_attach_port(verbose), Some(3));

        let tcp_only = "error: failed to connect to 127.0.0.1 tcp port 3240\n";
        assert_eq!(parse_attach_port(tcp_only), None);
    }

    #[test]
    fn parses_usbip_port_listing_with_exact_bus_id_boundaries() {
        let listing = "\
Imported USB devices
====================
Port 00: <Port in Use> at Full Speed(12Mbps)
       Sony Corp. : DualSense Wireless Controller (054c:0ce6)
       1-10 -> usbip://127.0.0.1:3240/1-10
           -> remote bus/dev 001/010
Port 02: <Port in Use> at Full Speed(12Mbps)
       DSMaster : Virtual DualSense Controller (054c:0ce6)
       1-1 -> usbip://127.0.0.1:3240/1-1
           -> remote bus/dev 001/001
Port 05: <Port in Use> at Full Speed(12Mbps)
       DSMaster : Virtual DualSense Controller (054c:0ce6)
       1-2 -> usbip://127.0.0.1:3240/1-2
           -> remote bus/dev 001/002
";

        assert_eq!(parse_port_list_for_bus(listing, "1-10"), Some(0));
        assert_eq!(parse_port_list_for_bus(listing, "1-1"), Some(2));
        assert_eq!(parse_port_list_for_bus(listing, "1-2"), Some(5));
        assert_eq!(parse_port_list_for_bus(listing, "1-3"), None);
    }

    #[test]
    fn parse_empty_or_error_outputs_return_none() {
        assert_eq!(parse_attach_port(""), None);
        assert_eq!(parse_attach_port("usbip: error: no exported devices"), None);
        assert_eq!(
            parse_port_list_for_bus("Imported USB devices\n====", "1-1"),
            None
        );
    }
}
