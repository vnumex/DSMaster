//! GitHub Releases auto-updater for DSMaster (`vnumex/DSMaster`).
//!
//! Uses Windows' built-in `curl.exe` and `tar.exe` plus an in-Rust FIPS 180-4
//! SHA-256 verifier against `SHA256SUMS.txt` so no extra HTTP/JSON/ZIP crates
//! are required. Updates are never installed silently — the user must trigger
//! download and restart explicitly from the in-app UI.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::Shell::{SHELLEXECUTEINFOW, ShellExecuteExW, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};
use windows::core::PCWSTR;

const API_LATEST_URL: &str = "https://api.github.com/repos/vnumex/DSMaster/releases/latest";
const ALLOWED_DOWNLOAD_PREFIX: &str = "https://github.com/vnumex/DSMaster/releases/download/";
const ALLOWED_RELEASE_PREFIX: &str = "https://github.com/vnumex/DSMaster/releases/";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MIN_EXE_BYTES: u64 = 100 * 1024;
const WAIT_PID_ARG: &str = "--wait-pid";
const WAIT_PARENT_TIMEOUT_MS: u32 = 15_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: String,
    pub tag: String,
    pub release_url: String,
    pub zip_name: String,
    pub zip_url: String,
    pub sums_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheckOutcome {
    UpToDate,
    Available(ReleaseInfo),
    Failed(String),
}

pub struct UpdateCheckJob {
    result: Arc<Mutex<Option<UpdateCheckOutcome>>>,
}

impl UpdateCheckJob {
    pub fn spawn() -> Self {
        let result = Arc::new(Mutex::new(None));
        let result_bg = Arc::clone(&result);

        let _ = thread::Builder::new()
            .name("ds-update-check".to_owned())
            .spawn(move || {
                let outcome = check_latest_release(CURRENT_VERSION);
                *result_bg.lock() = Some(outcome);
            });

        Self { result }
    }

    pub fn take_result(&self) -> Option<UpdateCheckOutcome> {
        self.result.lock().take()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedUpdate {
    pub version: String,
    pub staged_exe: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateInstallPhase {
    Downloading,
    Verifying,
    Extracting,
    ReadyToRestart(PreparedUpdate),
    Failed(String),
}

pub struct UpdateInstallJob {
    pub release: ReleaseInfo,
    phase: Arc<Mutex<UpdateInstallPhase>>,
}

impl UpdateInstallJob {
    pub fn spawn(release: ReleaseInfo) -> Self {
        let phase = Arc::new(Mutex::new(UpdateInstallPhase::Downloading));
        let phase_bg = Arc::clone(&phase);
        let release_bg = release.clone();

        let _ = thread::Builder::new()
            .name("ds-update-install".to_owned())
            .spawn(move || match prepare_update(&release_bg, &phase_bg) {
                Ok(prepared) => {
                    *phase_bg.lock() = UpdateInstallPhase::ReadyToRestart(prepared);
                }
                Err(err) => {
                    *phase_bg.lock() = UpdateInstallPhase::Failed(err);
                }
            });

        Self { release, phase }
    }

    pub fn phase(&self) -> UpdateInstallPhase {
        self.phase.lock().clone()
    }
}

/// Parses a stable `MAJOR.MINOR.PATCH` version (with optional leading `v`/`V`).
/// Rejects prereleases (`-rc.1`, `-beta`, etc.).
pub fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    if stripped.contains('-') {
        return None;
    }
    let core = stripped.split('+').next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    matches!(
        (parse_semver(current), parse_semver(candidate)),
        (Some(cur), Some(cand)) if cand > cur
    )
}

fn check_latest_release(current_version: &str) -> UpdateCheckOutcome {
    let json = match fetch_latest_release_json() {
        Ok(body) => body,
        Err(err) => return UpdateCheckOutcome::Failed(err),
    };

    let release = match parse_release_json(&json) {
        Ok(rel) => rel,
        Err(err) => return UpdateCheckOutcome::Failed(err),
    };

    if is_newer_version(current_version, &release.version) {
        UpdateCheckOutcome::Available(release)
    } else {
        UpdateCheckOutcome::UpToDate
    }
}

fn fetch_latest_release_json() -> Result<String, String> {
    let out = Command::new("curl.exe")
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
            API_LATEST_URL,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to invoke curl.exe: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("Update check failed (exit {:?})", out.status.code())
        } else {
            format!("Update check failed: {stderr}")
        });
    }

    String::from_utf8(out.stdout).map_err(|e| format!("Invalid UTF-8 from GitHub API: {e}"))
}

/// Depth-aware scanner for the GitHub `/releases/latest` payload so fields inside
/// release notes (`body`) cannot shadow top-level keys or asset download URLs.
pub fn parse_release_json(json: &str) -> Result<ReleaseInfo, String> {
    let bytes = json.as_bytes();
    let mut i = 0usize;
    let mut obj_depth = 0usize;
    let mut in_assets_array = false;
    let mut assets_array_depth = 0usize;
    let mut arr_depth = 0usize;

    let mut tag_name: Option<String> = None;
    let mut html_url: Option<String> = None;
    let mut draft = false;
    let mut prerelease = false;
    let mut zip_asset: Option<(String, String)> = None;
    let mut sums_url: Option<String> = None;

    let skip_ws = |idx: &mut usize| {
        while *idx < bytes.len() && bytes[*idx].is_ascii_whitespace() {
            *idx += 1;
        }
    };

    while i < bytes.len() {
        skip_ws(&mut i);
        if i >= bytes.len() {
            break;
        }
        match bytes[i] {
            b'{' => {
                obj_depth += 1;
                i += 1;
            }
            b'}' => {
                obj_depth = obj_depth.saturating_sub(1);
                i += 1;
            }
            b'[' => {
                arr_depth += 1;
                i += 1;
            }
            b']' => {
                if in_assets_array && arr_depth == assets_array_depth {
                    in_assets_array = false;
                }
                arr_depth = arr_depth.saturating_sub(1);
                i += 1;
            }
            b'"' => {
                let s = parse_json_string(bytes, &mut i)?;
                skip_ws(&mut i);
                if i < bytes.len() && bytes[i] == b':' {
                    i += 1;
                    skip_ws(&mut i);
                    if obj_depth == 1 && arr_depth == 0 {
                        match s.as_str() {
                            "tag_name" if bytes.get(i) == Some(&b'"') => {
                                tag_name = Some(parse_json_string(bytes, &mut i)?);
                            }
                            "html_url" if bytes.get(i) == Some(&b'"') => {
                                html_url = Some(parse_json_string(bytes, &mut i)?);
                            }
                            "draft" => {
                                if let Some(v) = parse_json_bool(bytes, &mut i) {
                                    draft = v;
                                }
                            }
                            "prerelease" => {
                                if let Some(v) = parse_json_bool(bytes, &mut i) {
                                    prerelease = v;
                                }
                            }
                            "assets" if bytes.get(i) == Some(&b'[') => {
                                arr_depth += 1;
                                in_assets_array = true;
                                assets_array_depth = arr_depth;
                                i += 1;
                            }
                            _ => {}
                        }
                    } else if in_assets_array
                        && obj_depth == 2
                        && arr_depth == assets_array_depth
                        && s == "browser_download_url"
                        && bytes.get(i) == Some(&b'"')
                    {
                        let url = parse_json_string(bytes, &mut i)?;
                        if url.starts_with(ALLOWED_DOWNLOAD_PREFIX)
                            && let Some(filename) = url.rsplit('/').next()
                        {
                            let lower = filename.to_ascii_lowercase();
                            if lower == "sha256sums.txt" {
                                sums_url = Some(url);
                            } else if lower.starts_with("dsmaster-")
                                && lower.ends_with("-windows-x64.zip")
                            {
                                zip_asset = Some((sanitize_filename(filename), url));
                            }
                        }
                    }
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    if draft || prerelease {
        return Err("Latest release is marked as draft or prerelease".to_owned());
    }

    let tag = tag_name.ok_or_else(|| "Release response missing tag_name".to_owned())?;
    let (maj, min, pat) =
        parse_semver(&tag).ok_or_else(|| format!("Unsupported release version tag: {tag}"))?;
    let version = format!("{maj}.{min}.{pat}");

    let release_url = html_url
        .filter(|u| u.starts_with(ALLOWED_RELEASE_PREFIX))
        .unwrap_or_else(|| format!("{ALLOWED_RELEASE_PREFIX}tag/{tag}"));

    let (zip_name, zip_url) =
        zip_asset.ok_or_else(|| "Release is missing the Windows x64 zip asset".to_owned())?;
    let sums_url = sums_url.ok_or_else(|| "Release is missing SHA256SUMS.txt".to_owned())?;

    Ok(ReleaseInfo {
        version,
        tag,
        release_url,
        zip_name,
        zip_url,
        sums_url,
    })
}

fn parse_json_string(bytes: &[u8], i: &mut usize) -> Result<String, String> {
    if bytes.get(*i) != Some(&b'"') {
        return Err("Expected JSON string".to_owned());
    }
    *i += 1;
    let mut out = String::new();
    while *i < bytes.len() {
        let b = bytes[*i];
        *i += 1;
        match b {
            b'"' => return Ok(out),
            b'\\' => {
                if *i >= bytes.len() {
                    return Err("Unterminated JSON escape".to_owned());
                }
                let esc = bytes[*i];
                *i += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'u' => {
                        if *i + 4 <= bytes.len()
                            && let Ok(hex) = std::str::from_utf8(&bytes[*i..*i + 4])
                            && let Ok(cp) = u16::from_str_radix(hex, 16)
                        {
                            *i += 4;
                            if let Some(ch) = char::from_u32(cp as u32) {
                                out.push(ch);
                            }
                        }
                    }
                    other => out.push(other as char),
                }
            }
            _ => {
                // Fast-path UTF-8 slice for non-ASCII characters.
                let start = *i - 1;
                while *i < bytes.len() && bytes[*i] != b'"' && bytes[*i] != b'\\' {
                    *i += 1;
                }
                let chunk = std::str::from_utf8(&bytes[start..*i])
                    .map_err(|e| format!("Invalid UTF-8 in JSON string: {e}"))?;
                out.push_str(chunk);
            }
        }
    }
    Err("Unterminated JSON string".to_owned())
}

fn parse_json_bool(bytes: &[u8], i: &mut usize) -> Option<bool> {
    if bytes[*i..].starts_with(b"true") {
        *i += 4;
        Some(true)
    } else if bytes[*i..].starts_with(b"false") {
        *i += 5;
        Some(false)
    } else {
        None
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect()
}

/// Looks up a lowercase SHA-256 hex digest in `SHA256SUMS.txt`, tolerating UTF-8 BOM,
/// CRLF line endings, optional leading `*` or `./`, and uppercase hex.
pub fn find_expected_sha256(sums_text: &str, filename: &str) -> Option<String> {
    let cleaned = sums_text.strip_prefix('\u{feff}').unwrap_or(sums_text);
    for line in cleaned.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let hash = parts.next()?.trim();
        let raw_name = parts.next()?.trim();
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let normalized_name = raw_name
            .strip_prefix('*')
            .unwrap_or(raw_name)
            .strip_prefix("./")
            .unwrap_or_else(|| raw_name.strip_prefix('*').unwrap_or(raw_name))
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(raw_name);
        if normalized_name.eq_ignore_ascii_case(filename) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

fn prepare_update(
    release: &ReleaseInfo,
    phase: &Arc<Mutex<UpdateInstallPhase>>,
) -> Result<PreparedUpdate, String> {
    if !release.zip_url.starts_with(ALLOWED_DOWNLOAD_PREFIX)
        || !release.sums_url.starts_with(ALLOWED_DOWNLOAD_PREFIX)
    {
        return Err("Refusing to download update from untrusted URL".to_owned());
    }

    let tag_dir = sanitize_filename(&release.tag);
    if tag_dir.is_empty() {
        return Err("Invalid release tag".to_owned());
    }

    let stage_root = std::env::temp_dir()
        .join("DSMaster")
        .join("updates")
        .join(&tag_dir);
    let _ = fs::remove_dir_all(&stage_root);
    fs::create_dir_all(&stage_root)
        .map_err(|e| format!("Failed to create temp update folder: {e}"))?;

    let zip_path = stage_root.join(&release.zip_name);
    let sums_path = stage_root.join("SHA256SUMS.txt");

    download_file(&release.sums_url, &sums_path)?;
    download_file(&release.zip_url, &zip_path)?;

    *phase.lock() = UpdateInstallPhase::Verifying;
    let sums_text = fs::read_to_string(&sums_path)
        .map_err(|e| format!("Failed to read SHA256SUMS.txt: {e}"))?;

    let expected_zip_hash = find_expected_sha256(&sums_text, &release.zip_name)
        .ok_or_else(|| format!("SHA256SUMS.txt has no entry for {}", release.zip_name))?;
    let expected_exe_hash = find_expected_sha256(&sums_text, "dsmaster.exe")
        .ok_or_else(|| "SHA256SUMS.txt has no entry for dsmaster.exe".to_owned())?;

    let actual_zip_hash = sha256_file(&zip_path)?;
    if actual_zip_hash != expected_zip_hash {
        let _ = fs::remove_dir_all(&stage_root);
        return Err("Downloaded archive failed SHA-256 verification".to_owned());
    }

    *phase.lock() = UpdateInstallPhase::Extracting;
    let extract_dir = stage_root.join("extracted");
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Failed to create extract folder: {e}"))?;
    extract_zip(&zip_path, &extract_dir)?;

    let staged_exe = locate_extracted_exe(&extract_dir)
        .ok_or_else(|| "Release archive did not contain dsmaster.exe".to_owned())?;

    validate_exe_file(&staged_exe)?;
    let actual_exe_hash = sha256_file(&staged_exe)?;
    if actual_exe_hash != expected_exe_hash {
        let _ = fs::remove_dir_all(&stage_root);
        return Err("Extracted dsmaster.exe failed SHA-256 verification".to_owned());
    }

    let _ = fs::remove_file(&zip_path);
    Ok(PreparedUpdate {
        version: release.version.clone(),
        staged_exe,
    })
}

fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    let dest_str = dest
        .to_str()
        .ok_or_else(|| "Invalid temporary download path".to_owned())?;
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

fn extract_zip(zip_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let out = Command::new("tar.exe")
        .arg("-xf")
        .arg(zip_path)
        .arg("-C")
        .arg(dest_dir)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to invoke tar.exe: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("Extraction failed (exit {:?})", out.status.code())
        } else {
            format!("Extraction failed: {stderr}")
        });
    }
    Ok(())
}

fn locate_extracted_exe(extract_dir: &Path) -> Option<PathBuf> {
    let direct = extract_dir.join("dsmaster.exe");
    if direct.is_file() {
        return Some(direct);
    }
    let nested = extract_dir.join("DSMaster").join("dsmaster.exe");
    if nested.is_file() {
        return Some(nested);
    }
    None
}

fn validate_exe_file(path: &Path) -> Result<(), String> {
    let meta = fs::metadata(path).map_err(|e| format!("Staged binary unreadable: {e}"))?;
    if meta.len() < MIN_EXE_BYTES {
        return Err("Extracted dsmaster.exe is unexpectedly small".to_owned());
    }
    let mut f = File::open(path).map_err(|e| format!("Failed to open staged binary: {e}"))?;
    let mut magic = [0u8; 2];
    f.read_exact(&mut magic)
        .map_err(|e| format!("Failed to read staged binary header: {e}"))?;
    if &magic != b"MZ" {
        return Err("Extracted dsmaster.exe is not a valid Windows executable".to_owned());
    }
    Ok(())
}

#[derive(Debug)]
enum SwapError {
    PermissionDenied,
    Other(String),
}

fn old_binary_path(exe_path: &Path) -> PathBuf {
    let mut s = OsString::from(exe_path.as_os_str());
    s.push(".old");
    PathBuf::from(s)
}

fn swap_binary_in_place(current_exe: &Path, staged_exe: &Path) -> Result<(), SwapError> {
    let backup = old_binary_path(current_exe);
    if backup.exists()
        && let Err(e) = fs::remove_file(&backup)
    {
        return if e.kind() == io::ErrorKind::PermissionDenied {
            Err(SwapError::PermissionDenied)
        } else {
            Err(SwapError::Other(format!(
                "Failed to remove previous backup binary: {e}"
            )))
        };
    }

    if let Err(e) = fs::rename(current_exe, &backup) {
        return if e.kind() == io::ErrorKind::PermissionDenied {
            Err(SwapError::PermissionDenied)
        } else {
            Err(SwapError::Other(format!(
                "Failed to move running executable aside: {e}"
            )))
        };
    }

    if let Err(e) = fs::copy(staged_exe, current_exe) {
        let _ = fs::rename(&backup, current_exe);
        return if e.kind() == io::ErrorKind::PermissionDenied {
            Err(SwapError::PermissionDenied)
        } else {
            Err(SwapError::Other(format!(
                "Failed to copy updated executable into place: {e}"
            )))
        };
    }

    Ok(())
}

/// Applies a verified update and relaunches DSMaster.
///
/// `before_relaunch` is invoked only after the replacement has succeeded (or the
/// elevated helper has been accepted through UAC), so a failure or canceled UAC
/// prompt leaves the running app completely untouched.
pub fn apply_and_relaunch<F: FnOnce()>(
    prepared: &PreparedUpdate,
    before_relaunch: F,
) -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Unable to resolve current exe path: {e}"))?;
    let current_pid = std::process::id();

    match swap_binary_in_place(&current_exe, &prepared.staged_exe) {
        Ok(()) => {
            before_relaunch();
            Command::new(&current_exe)
                .arg(WAIT_PID_ARG)
                .arg(current_pid.to_string())
                .spawn()
                .map_err(|e| format!("Updated binary installed, but failed to relaunch: {e}"))?;
            Ok(())
        }
        Err(SwapError::PermissionDenied) => {
            launch_elevated_swap_helper(&current_exe, &prepared.staged_exe, current_pid)?;
            before_relaunch();
            Ok(())
        }
        Err(SwapError::Other(msg)) => Err(msg),
    }
}

fn ps_single_quote(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn encode_utf16le_base64(text: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut raw = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        raw.extend_from_slice(&unit.to_le_bytes());
    }
    let mut out = String::with_capacity(raw.len().div_ceil(3) * 4);
    for chunk in raw.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn launch_elevated_swap_helper(
    current_exe: &Path,
    staged_exe: &Path,
    parent_pid: u32,
) -> Result<(), String> {
    let exe_q = ps_single_quote(current_exe);
    let staged_q = ps_single_quote(staged_exe);
    let backup_q = ps_single_quote(&old_binary_path(current_exe));

    let script = format!(
        "$p = Get-Process -Id {parent_pid} -ErrorAction SilentlyContinue; \
         if ($p) {{ $p.WaitForExit(15000) | Out-Null }}; \
         if (Test-Path -LiteralPath '{backup_q}') {{ Remove-Item -LiteralPath '{backup_q}' -Force -ErrorAction SilentlyContinue }}; \
         Move-Item -LiteralPath '{exe_q}' -Destination '{backup_q}' -Force; \
         try {{ Copy-Item -LiteralPath '{staged_q}' -Destination '{exe_q}' -Force }} \
         catch {{ Move-Item -LiteralPath '{backup_q}' -Destination '{exe_q}' -Force; exit 1 }}; \
         Start-Process -FilePath '{exe_q}'"
    );

    let encoded = encode_utf16le_base64(&script);
    let verb = wide_null(OsStr::new("runas"));
    let file = wide_null(OsStr::new("powershell.exe"));
    let params_str = format!("-NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand {encoded}");
    let params = wide_null(OsStr::new(&params_str));

    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };

    // SAFETY: `sei` points to valid NUL-terminated UTF-16 buffers for the duration of the call.
    unsafe {
        ShellExecuteExW(&mut sei)
            .map_err(|e| format!("Administrator permission was canceled or failed ({e})"))?;
    }
    Ok(())
}

pub fn open_release_url(url: &str) -> Result<(), String> {
    if !url.starts_with(ALLOWED_RELEASE_PREFIX) {
        return Err("Refusing to open untrusted release URL".to_owned());
    }
    let verb = wide_null(OsStr::new("open"));
    let target = wide_null(OsStr::new(url));
    // SAFETY: `verb` and `target` are valid NUL-terminated UTF-16 strings.
    let res = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (res.0 as isize) <= 32 {
        return Err("Failed to open release notes in browser".to_owned());
    }
    Ok(())
}

fn parse_wait_pid_from_args<I, S>(args: I) -> Option<u32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg.as_ref() == WAIT_PID_ARG {
            return iter.next()?.as_ref().parse::<u32>().ok();
        }
    }
    None
}

/// When relaunched by an update, waits for the exiting previous process to finish
/// so the single-instance mutex and `.old` executable lock are released first.
pub fn wait_for_parent_exit_if_requested() {
    let Some(pid) = parse_wait_pid_from_args(std::env::args().skip(1)) else {
        return;
    };
    if pid == 0 || pid == std::process::id() {
        return;
    }
    // SAFETY: Standard Win32 synchronization wait on the parent process ID.
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(handle, WAIT_PARENT_TIMEOUT_MS);
            let _ = CloseHandle(handle);
        }
    }
}

/// Removes any leftover `dsmaster.exe.old` left behind by a previous update.
pub fn cleanup_old_binary() {
    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };
    let backup = old_binary_path(&current_exe);
    if !backup.exists() {
        return;
    }
    for _ in 0..3 {
        if fs::remove_file(&backup).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wide_null(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------
// Minimal FIPS 180-4 SHA-256 implementation (zero external dependencies)
// ---------------------------------------------------------------------------

struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Sha256 {
    const INIT: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    const K: [u32; 64] = [
        0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5, 0x3956_c25b, 0x59f1_11f1, 0x923f_82a4,
        0xab1c_5ed5, 0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3, 0x72be_5d74, 0x80de_b1fe,
        0x9bdc_06a7, 0xc19b_f174, 0xe49b_69c1, 0xefbe_4786, 0x0fc1_9dc6, 0x240c_a1cc, 0x2de9_2c6f,
        0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da, 0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7,
        0xc6e0_0bf3, 0xd5a7_9147, 0x06ca_6351, 0x1429_2967, 0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc,
        0x5338_0d13, 0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85, 0xa2bf_e8a1, 0xa81a_664b,
        0xc24b_8b70, 0xc76c_51a3, 0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070, 0x19a4_c116,
        0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5, 0x391c_0cb3, 0x4ed8_aa4a, 0x5b9c_ca4f, 0x682e_6ff3,
        0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208, 0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7,
        0xc671_78f2,
    ];

    fn new() -> Self {
        Self {
            state: Self::INIT,
            buf: [0u8; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(Self::K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }

    fn finalize_hex(mut self) -> String {
        let bit_len = self.total_len.wrapping_mul(8);
        self.update(&[0x80]);
        if self.buf_len > 56 {
            let pad = [0u8; 64];
            self.update(&pad[..64 - self.buf_len]);
        }
        let pad = [0u8; 56];
        self.update(&pad[..56 - self.buf_len]);
        self.update(&bit_len.to_be_bytes());

        let mut out = String::with_capacity(64);
        for word in self.state {
            use std::fmt::Write;
            let _ = write!(&mut out, "{word:08x}");
        }
        out
    }
}

#[cfg(test)]
fn sha256_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize_hex()
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("Failed to hash file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Failed while hashing file: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_nist_vectors() {
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_bytes(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn semver_numeric_ordering_and_prerelease_rejection() {
        assert_eq!(parse_semver("v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_semver("0.10.0"), Some((0, 10, 0)));
        assert_eq!(parse_semver("v0.3.0-rc.1"), None);
        assert!(is_newer_version("0.2.1", "0.10.0"));
        assert!(is_newer_version("0.2.1", "v0.2.2"));
        assert!(!is_newer_version("0.2.1", "0.2.1"));
        assert!(!is_newer_version("0.2.1", "0.2.0"));
        assert!(!is_newer_version("0.2.1", "0.3.0-beta.1"));
    }

    #[test]
    fn parses_valid_github_release_json() {
        let json = r#"{
            "html_url": "https://github.com/vnumex/DSMaster/releases/tag/v0.3.0",
            "tag_name": "v0.3.0",
            "draft": false,
            "prerelease": false,
            "body": "Notes mentioning \"tag_name\": \"v9.9.9\" and \"draft\": true",
            "assets": [
                {
                    "name": "DSMaster-v0.3.0-windows-x64.zip",
                    "browser_download_url": "https://github.com/vnumex/DSMaster/releases/download/v0.3.0/DSMaster-v0.3.0-windows-x64.zip"
                },
                {
                    "name": "SHA256SUMS.txt",
                    "browser_download_url": "https://github.com/vnumex/DSMaster/releases/download/v0.3.0/SHA256SUMS.txt"
                }
            ]
        }"#;
        let rel = parse_release_json(json).unwrap();
        assert_eq!(rel.version, "0.3.0");
        assert_eq!(rel.tag, "v0.3.0");
        assert_eq!(rel.zip_name, "DSMaster-v0.3.0-windows-x64.zip");
        assert!(rel.zip_url.ends_with("DSMaster-v0.3.0-windows-x64.zip"));
        assert!(rel.sums_url.ends_with("SHA256SUMS.txt"));
    }

    #[test]
    fn rejects_prerelease_foreign_urls_and_missing_checksums() {
        let pre = r#"{
            "tag_name": "v0.3.0",
            "draft": false,
            "prerelease": true,
            "assets": [
                {"browser_download_url": "https://github.com/vnumex/DSMaster/releases/download/v0.3.0/DSMaster-v0.3.0-windows-x64.zip"},
                {"browser_download_url": "https://github.com/vnumex/DSMaster/releases/download/v0.3.0/SHA256SUMS.txt"}
            ]
        }"#;
        assert!(parse_release_json(pre).is_err());

        let evil = r#"{
            "tag_name": "v0.3.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                {"browser_download_url": "https://evil.example.com/DSMaster-v0.3.0-windows-x64.zip"},
                {"browser_download_url": "https://github.com/vnumex/DSMaster/releases/download/v0.3.0/SHA256SUMS.txt"}
            ]
        }"#;
        assert!(parse_release_json(evil).is_err());

        let no_sums = r#"{
            "tag_name": "v0.3.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                {"browser_download_url": "https://github.com/vnumex/DSMaster/releases/download/v0.3.0/DSMaster-v0.3.0-windows-x64.zip"}
            ]
        }"#;
        assert!(parse_release_json(no_sums).is_err());
    }

    #[test]
    fn parses_sha256sums_with_bom_crlf_and_uppercase() {
        let zip_hash = "A".repeat(64);
        let exe_hash = "b".repeat(64);
        let sums = format!(
            "\u{feff}{zip_hash}  DSMaster-v0.3.0-windows-x64.zip\r\n{exe_hash} *dsmaster.exe\r\n"
        );
        assert_eq!(
            find_expected_sha256(&sums, "DSMaster-v0.3.0-windows-x64.zip"),
            Some("a".repeat(64))
        );
        assert_eq!(find_expected_sha256(&sums, "dsmaster.exe"), Some(exe_hash));
        assert_eq!(find_expected_sha256(&sums, "missing.exe"), None);
    }

    #[test]
    fn swap_binary_in_place_replaces_and_rolls_back_on_error() {
        let base = std::env::temp_dir().join(format!("dsmaster-swap-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let current = base.join("dsmaster.exe");
        let staged = base.join("new.exe");
        let backup = old_binary_path(&current);

        fs::write(&current, b"old-binary").unwrap();
        fs::write(&staged, b"new-binary").unwrap();

        swap_binary_in_place(&current, &staged).unwrap();
        assert_eq!(fs::read(&current).unwrap(), b"new-binary");
        assert_eq!(fs::read(&backup).unwrap(), b"old-binary");

        // Rollback check when staged file is missing
        let missing = base.join("missing.exe");
        assert!(swap_binary_in_place(&current, &missing).is_err());
        assert_eq!(fs::read(&current).unwrap(), b"new-binary");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn parses_wait_pid_argument() {
        assert_eq!(
            parse_wait_pid_from_args(["--foo", "--wait-pid", "4242"]),
            Some(4242)
        );
        assert_eq!(parse_wait_pid_from_args(["--wait-pid", "nope"]), None);
        assert_eq!(parse_wait_pid_from_args(["--other"]), None);
    }
}
