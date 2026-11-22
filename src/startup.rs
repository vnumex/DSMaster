use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "DSMaster";

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn format_startup_command(exe_path: &Path) -> String {
    format!("\"{}\"", exe_path.display())
}

pub fn is_run_at_startup_enabled() -> bool {
    let subkey = to_wide_null(RUN_SUBKEY);
    let val_name = to_wide_null(VALUE_NAME);
    let mut hkey = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_QUERY_VALUE,
            &mut hkey,
        )
    };
    if status != ERROR_SUCCESS {
        return false;
    }
    let mut size: u32 = 0;
    let query_status = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(val_name.as_ptr()),
            None,
            None,
            None,
            Some(&mut size),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    query_status == ERROR_SUCCESS && size > 0
}

pub fn set_run_at_startup(enabled: bool) -> Result<(), String> {
    let subkey = to_wide_null(RUN_SUBKEY);
    let val_name = to_wide_null(VALUE_NAME);
    let mut hkey = HKEY::default();
    let open_status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut hkey,
        )
    };
    if open_status != ERROR_SUCCESS {
        return Err(format!("RegOpenKeyExW failed with error {}", open_status.0));
    }

    let result = if enabled {
        match std::env::current_exe() {
            Ok(exe_path) => {
                let cmd = format_startup_command(&exe_path);
                let wide = to_wide_null(&cmd);
                let byte_len = wide.len() * std::mem::size_of::<u16>();
                let bytes =
                    unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), byte_len) };
                let err = unsafe {
                    RegSetValueExW(hkey, PCWSTR(val_name.as_ptr()), None, REG_SZ, Some(bytes))
                };
                if err == ERROR_SUCCESS {
                    Ok(())
                } else {
                    Err(format!("RegSetValueExW failed with error {}", err.0))
                }
            }
            Err(e) => Err(e.to_string()),
        }
    } else {
        let err = unsafe { RegDeleteValueW(hkey, PCWSTR(val_name.as_ptr())) };
        if err == ERROR_SUCCESS || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("RegDeleteValueW failed with error {}", err.0))
        }
    };

    unsafe {
        let _ = RegCloseKey(hkey);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn formats_quoted_startup_path() {
        let path = PathBuf::from(r"C:\Program Files\DSMaster\dsmaster.exe");
        assert_eq!(
            format_startup_command(&path),
            r#""C:\Program Files\DSMaster\dsmaster.exe""#
        );
    }
}
