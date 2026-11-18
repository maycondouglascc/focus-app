//! Windows logon autostart (`HKCU\...\Run\Focus`, `StartupApproved\Run\Focus`)
//! and Windows Restart Manager (`RegisterApplicationRestart`).

use std::ffi::c_void;
use std::path::Path;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Recovery::{
    RegisterApplicationRestart, REGISTER_APPLICATION_RESTART_FLAGS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_BINARY,
    RRF_RT_REG_SZ,
};

use crate::log::{self, Category};

const RUN_SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const APPROVED_SUBKEY: PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");
const VALUE_NAME: PCWSTR = w!("Focus");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalState {
    Enabled,
    DisabledByUser,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStatus {
    Enabled,
    Off,
    DisabledInTaskManager,
}

/// Formats the quoted `HKCU\...\Run` command line with `--background`.
pub fn format_run_command(exe_path: &Path) -> String {
    format!("\"{}\" --background", exe_path.display())
}

/// Decodes a `StartupApproved\Run` binary value (Windows 10/11 uses byte 0:
/// `0x02`/`0x06` enabled, odd values such as `0x03`/`0x01` disabled in Task Manager).
pub fn parse_startup_approved(bytes: &[u8]) -> ApprovalState {
    if bytes.is_empty() {
        ApprovalState::Missing
    } else if (bytes[0] & 1) == 1 {
        ApprovalState::DisabledByUser
    } else {
        ApprovalState::Enabled
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    w.push(0);
    w
}

fn has_run_entry(run_subkey: PCWSTR, value_name: PCWSTR) -> bool {
    let mut size: u32 = 0;
    // SAFETY: queries value presence/length only (`pvData = NULL`).
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            run_subkey,
            value_name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    rc == ERROR_SUCCESS && size > 2
}

fn read_approved_state(approved_subkey: PCWSTR, value_name: PCWSTR) -> ApprovalState {
    let mut buf = [0u8; 32];
    let mut size: u32 = buf.len() as u32;
    // SAFETY: reads `REG_BINARY` into stack buffer `buf`.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            approved_subkey,
            value_name,
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    if rc == ERROR_SUCCESS && size > 0 {
        parse_startup_approved(&buf[..size as usize])
    } else {
        ApprovalState::Missing
    }
}

fn set_enabled_in_subkey(
    run_subkey: PCWSTR,
    value_name: PCWSTR,
    exe_path: &Path,
    enable: bool,
) -> Result<(), String> {
    if enable {
        let cmd = format_run_command(exe_path);
        let wide = to_wide(&cmd);
        let mut hkey = HKEY::default();
        // SAFETY: opens/creates `HKCU\run_subkey` and writes a NUL-terminated `REG_SZ`.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                run_subkey,
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(format!("RegCreateKeyExW failed: {}", rc.0));
        }
        // SAFETY: `wide` is NUL-terminated UTF-16 bytes inside `slice::from_raw_parts`.
        let write_rc = unsafe {
            let byte_len = wide.len() * std::mem::size_of::<u16>();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, byte_len);
            let r = RegSetValueExW(hkey, value_name, None, REG_SZ, Some(bytes));
            let _ = RegCloseKey(hkey);
            r
        };
        if write_rc != ERROR_SUCCESS {
            return Err(format!("RegSetValueExW failed: {}", write_rc.0));
        }
        Ok(())
    } else {
        let mut hkey = HKEY::default();
        // SAFETY: opens existing HKCU key to delete `value_name`.
        let rc = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                run_subkey,
                None,
                KEY_SET_VALUE,
                &mut hkey,
            )
        };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if rc != ERROR_SUCCESS {
            return Err(format!("RegOpenKeyExW failed: {}", rc.0));
        }
        // SAFETY: deletes value under `hkey` and closes handle.
        let del_rc = unsafe {
            let r = RegDeleteValueW(hkey, value_name);
            let _ = RegCloseKey(hkey);
            r
        };
        if del_rc != ERROR_SUCCESS && del_rc != ERROR_FILE_NOT_FOUND {
            return Err(format!("RegDeleteValueW failed: {}", del_rc.0));
        }
        Ok(())
    }
}

pub fn status() -> StartupStatus {
    if !has_run_entry(RUN_SUBKEY, VALUE_NAME) {
        return StartupStatus::Off;
    }
    match read_approved_state(APPROVED_SUBKEY, VALUE_NAME) {
        ApprovalState::DisabledByUser => StartupStatus::DisabledInTaskManager,
        ApprovalState::Enabled | ApprovalState::Missing => StartupStatus::Enabled,
    }
}

pub fn set_enabled(enable: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?;
    let res = set_enabled_in_subkey(RUN_SUBKEY, VALUE_NAME, &exe, enable);
    match &res {
        Ok(()) => log::event(
            Category::Startup,
            if enable {
                "run_enabled"
            } else {
                "run_disabled"
            },
        ),
        Err(_) => log::event(Category::Startup, "run_write_failed"),
    }
    res
}

/// Registers Focus with Windows Restart Manager so Windows updates restart it in background mode.
pub fn register_restart() {
    // SAFETY: static wide command-line string with flags = 0 (restarts after crash/hang/OS update).
    let ok = unsafe {
        RegisterApplicationRestart(w!("--background"), REGISTER_APPLICATION_RESTART_FLAGS(0))
            .is_ok()
    };
    log::flag(Category::Startup, "register_restart", "ok", ok);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use windows::Win32::System::Registry::RegDeleteTreeW;

    #[test]
    fn formats_quoted_run_command_and_parses_startup_approved() {
        let p = PathBuf::from(r"C:\Users\Test User\AppData\Local\Programs\Focus\Focus.exe");
        assert_eq!(
            format_run_command(&p),
            "\"C:\\Users\\Test User\\AppData\\Local\\Programs\\Focus\\Focus.exe\" --background"
        );

        assert_eq!(parse_startup_approved(&[]), ApprovalState::Missing);
        assert_eq!(
            parse_startup_approved(&[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            ApprovalState::Enabled
        );
        assert_eq!(
            parse_startup_approved(&[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            ApprovalState::DisabledByUser
        );
        assert_eq!(
            parse_startup_approved(&[0x01, 0, 0, 0]),
            ApprovalState::DisabledByUser
        );
    }

    #[test]
    fn roundtrip_registry_write_and_delete_in_temp_hkcu_subkey() {
        let sub_str = format!("Software\\FocusTestStartup-{}", std::process::id());
        let wide_sub = to_wide(&sub_str);
        let sub_pcwstr = PCWSTR(wide_sub.as_ptr());
        let exe = PathBuf::from(r"C:\Programs\Focus\Focus.exe");

        assert!(!has_run_entry(sub_pcwstr, VALUE_NAME));
        set_enabled_in_subkey(sub_pcwstr, VALUE_NAME, &exe, true).unwrap();
        assert!(has_run_entry(sub_pcwstr, VALUE_NAME));
        set_enabled_in_subkey(sub_pcwstr, VALUE_NAME, &exe, false).unwrap();
        assert!(!has_run_entry(sub_pcwstr, VALUE_NAME));

        // SAFETY: cleans up temporary HKCU test subkey.
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, sub_pcwstr);
        }
    }
}
