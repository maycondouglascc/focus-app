//! Windows taskbar notification icon (`Shell_NotifyIconW`, `NOTIFYICON_VERSION_4`).
//!
//! Owns the active `HICON` handle cleanly (destroyed after `NIM_MODIFY` or on
//! shutdown), adapts to `SystemUsesLightTheme` and system DPI changes, and
//! re-adds itself on `TaskbarCreated` when Explorer restarts.

pub mod flyout;
pub mod icon;
pub mod menu;
pub mod mini_timer;

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::{w, GUID};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, RECT};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP,
    NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICONDATAW_0, NOTIFYICONIDENTIFIER, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    RegisterWindowMessageW, SystemParametersInfoW, HICON, SM_CXSMICON, SPI_GETWORKAREA,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_SETTINGCHANGE,
    WM_THEMECHANGED,
};

use crate::log::{self, Category};
use crate::ui::popup::ScreenRect;
use icon::{create_tray_hicon, destroy_hicon, IconState, TaskbarTheme};

const TRAY_ICON_ID: u32 = 1;

/// Copies `text` into a fixed UTF-16 buffer, NUL-terminating and never splitting a surrogate pair.
pub fn copy_utf16_trunc(dst: &mut [u16], text: &str) {
    if dst.is_empty() {
        return;
    }
    let max_units = dst.len() - 1;
    let mut len = 0usize;
    let mut last_char_end = 0usize;
    for ch in text.chars() {
        let mut pair = [0u16; 2];
        let encoded = ch.encode_utf16(&mut pair);
        if len + encoded.len() > max_units {
            break;
        }
        dst[len..len + encoded.len()].copy_from_slice(encoded);
        len += encoded.len();
        last_char_end = len;
    }
    dst[last_char_end] = 0;
}

/// Reads `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Themes\Personalize\SystemUsesLightTheme`.
pub fn query_taskbar_theme() -> TaskbarTheme {
    let mut val: u32 = 0;
    let mut size: u32 = size_of::<u32>() as u32;
    // SAFETY: queries a single DWORD registry value into stack-local `val`.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut val as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    if rc == ERROR_SUCCESS && val != 0 {
        TaskbarTheme::Light
    } else {
        TaskbarTheme::Dark
    }
}

pub fn query_icon_px(hwnd: HWND) -> u32 {
    // SAFETY: `GetDpiForWindow` / `GetDpiForSystem` / `GetSystemMetricsForDpi` query metric integers only.
    let px = unsafe {
        let mut dpi = if !hwnd.is_invalid() {
            GetDpiForWindow(hwnd)
        } else {
            0
        };
        if dpi == 0 {
            dpi = GetDpiForSystem();
        }
        if dpi == 0 {
            dpi = 96;
        }
        GetSystemMetricsForDpi(SM_CXSMICON, dpi)
    };
    px.max(16) as u32
}

pub struct Tray {
    hwnd: HWND,
    callback_msg: u32,
    taskbar_created_msg: u32,
    state: IconState,
    theme: TaskbarTheme,
    icon_px: u32,
    tip: String,
    hicon: Option<HICON>,
    added: bool,
}

impl Tray {
    pub fn add(hwnd: HWND, callback_msg: u32, initial_tip: &str) -> Self {
        // Register TaskbarCreated before NIM_ADD so Explorer restarts never race past us.
        // SAFETY: static wide literal passed to `RegisterWindowMessageW`.
        let taskbar_created_msg = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        let mut tray = Self {
            hwnd,
            callback_msg,
            taskbar_created_msg,
            state: IconState::Stopped,
            theme: query_taskbar_theme(),
            icon_px: query_icon_px(hwnd),
            tip: initial_tip.to_string(),
            hicon: None,
            added: false,
        };
        tray.add_to_shell();
        tray
    }

    #[allow(dead_code)]
    pub fn icon_id(&self) -> u32 {
        TRAY_ICON_ID
    }

    #[allow(dead_code)]
    pub fn state(&self) -> IconState {
        self.state
    }

    pub fn anchor_rect(&self) -> ScreenRect {
        let ident = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ICON_ID,
            guidItem: GUID::zeroed(),
        };
        // SAFETY: `ident` is initialized on the stack with `cbSize`; `Shell_NotifyIconGetRect` writes a `RECT`.
        if let Ok(rc) = unsafe { Shell_NotifyIconGetRect(&ident) } {
            if rc.right > rc.left && rc.bottom > rc.top {
                return ScreenRect {
                    left: rc.left,
                    top: rc.top,
                    right: rc.right,
                    bottom: rc.bottom,
                };
            }
        }
        let mut work = RECT {
            left: 0,
            top: 0,
            right: 1280,
            bottom: 800,
        };
        // SAFETY: fallback to primary work-area bottom-right when notification icon rect is unavailable.
        unsafe {
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut work as *mut RECT as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
        }
        ScreenRect {
            left: (work.right - 24).max(work.left),
            top: work.bottom,
            right: (work.right - 12).max(work.left),
            bottom: work.bottom + 4,
        }
    }

    fn base_nid(&self, icon: HICON) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ICON_ID,
            uCallbackMessage: self.callback_msg,
            hIcon: icon,
            Anonymous: NOTIFYICONDATAW_0 {
                uVersion: NOTIFYICON_VERSION_4,
            },
            ..Default::default()
        };
        copy_utf16_trunc(&mut nid.szTip, &self.tip);
        nid
    }

    fn add_to_shell(&mut self) -> bool {
        let Some(new_icon) = create_tray_hicon(self.icon_px, self.state, self.theme) else {
            log::event(Category::Tray, "hicon_create_failed");
            return false;
        };
        let mut nid = self.base_nid(new_icon);
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;

        // SAFETY: `nid` is a stack-allocated, fully initialized `NOTIFYICONDATAW` bound to `self.hwnd`.
        let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid).as_bool() };
        if !ok {
            destroy_hicon(new_icon);
            log::event(Category::Tray, "nim_add_failed");
            self.added = false;
            return false;
        }
        // SAFETY: sets `NOTIFYICON_VERSION_4` immediately after `NIM_ADD`.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
        if let Some(old) = self.hicon.replace(new_icon) {
            destroy_hicon(old);
        }
        self.added = true;
        true
    }

    pub fn readd(&mut self) -> bool {
        self.theme = query_taskbar_theme();
        self.icon_px = query_icon_px(self.hwnd);
        self.added = false;
        self.add_to_shell()
    }

    pub fn set_state(&mut self, state: IconState, tip: &str) {
        if !self.added {
            self.state = state;
            self.tip = tip.to_string();
            self.add_to_shell();
            return;
        }
        let icon_changed = self.state != state;
        let tip_changed = self.tip != tip;
        if !icon_changed && !tip_changed {
            return;
        }
        self.state = state;
        self.tip = tip.to_string();

        if icon_changed {
            let Some(new_icon) = create_tray_hicon(self.icon_px, self.state, self.theme) else {
                return;
            };
            let mut nid = self.base_nid(new_icon);
            nid.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
            // SAFETY: `nid` carries a valid `HICON` and UTF-16 tip for existing `TRAY_ICON_ID`.
            let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() };
            if ok {
                if let Some(old) = self.hicon.replace(new_icon) {
                    destroy_hicon(old);
                }
            } else {
                destroy_hicon(new_icon);
            }
        } else if let Some(cur_icon) = self.hicon {
            let mut nid = self.base_nid(cur_icon);
            nid.uFlags = NIF_TIP | NIF_SHOWTIP;
            // SAFETY: updates tooltip text only for existing `TRAY_ICON_ID`.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
        }
    }

    pub fn refresh_environment(&mut self) {
        let next_theme = query_taskbar_theme();
        let next_px = query_icon_px(self.hwnd);
        if next_theme == self.theme && next_px == self.icon_px && self.added {
            return;
        }
        self.theme = next_theme;
        self.icon_px = next_px;
        if !self.added {
            self.add_to_shell();
            return;
        }
        let Some(new_icon) = create_tray_hicon(self.icon_px, self.state, self.theme) else {
            return;
        };
        let mut nid = self.base_nid(new_icon);
        nid.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        // SAFETY: updates icon bitmap after taskbar theme or DPI change.
        let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() };
        if ok {
            if let Some(old) = self.hicon.replace(new_icon) {
                destroy_hicon(old);
            }
        } else {
            destroy_hicon(new_icon);
        }
    }

    pub fn show_balloon(&self, title: &str, body: &str, warn: bool) {
        if !self.added {
            return;
        }
        let Some(cur_icon) = self.hicon else {
            return;
        };
        let mut nid = self.base_nid(cur_icon);
        nid.uFlags = NIF_INFO;
        copy_utf16_trunc(&mut nid.szInfoTitle, title);
        copy_utf16_trunc(&mut nid.szInfo, body);
        nid.dwInfoFlags = if warn { NIIF_WARNING } else { NIIF_INFO };
        // SAFETY: displays a shell notification balloon anchored to `TRAY_ICON_ID`.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Processes shell/environment broadcast messages (`TaskbarCreated`, `WM_SETTINGCHANGE`,
    /// `WM_DISPLAYCHANGE`, `WM_DPICHANGED`, `WM_THEMECHANGED`). Returns `true` if handled.
    pub fn handle_env_message(&mut self, msg: u32, lparam: LPARAM) -> bool {
        if self.taskbar_created_msg != 0 && msg == self.taskbar_created_msg {
            log::event(Category::Tray, "taskbar_created");
            self.readd();
            return true;
        }
        match msg {
            WM_SETTINGCHANGE => {
                if is_immersive_color_set(lparam) {
                    self.refresh_environment();
                }
                true
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED | WM_THEMECHANGED => {
                self.refresh_environment();
                true
            }
            _ => false,
        }
    }

    pub fn remove(&mut self) {
        if self.added {
            let mut nid = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: TRAY_ICON_ID,
                ..Default::default()
            };
            nid.uFlags = NIF_MESSAGE;
            // SAFETY: deletes the registered notification icon for `(self.hwnd, TRAY_ICON_ID)`.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            self.added = false;
        }
        if let Some(icon) = self.hicon.take() {
            destroy_hicon(icon);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
    }
}

fn is_immersive_color_set(lparam: LPARAM) -> bool {
    if lparam.0 == 0 {
        return true;
    }
    let p = lparam.0 as *const u16;
    if p.is_null() {
        return true;
    }
    // Check prefix length up to 32 UTF-16 code units against "ImmersiveColorSet".
    const TARGET: &[u16] = &[
        b'I' as u16,
        b'm' as u16,
        b'm' as u16,
        b'e' as u16,
        b'r' as u16,
        b's' as u16,
        b'i' as u16,
        b'v' as u16,
        b'e' as u16,
        b'C' as u16,
        b'o' as u16,
        b'l' as u16,
        b'o' as u16,
        b'r' as u16,
        b'S' as u16,
        b'e' as u16,
        b't' as u16,
        0,
    ];
    // SAFETY: `WM_SETTINGCHANGE` `lparam` is either null (checked) or a NUL-terminated UTF-16 string pointer.
    unsafe {
        for (i, &expected) in TARGET.iter().enumerate() {
            if *p.add(i) != expected {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_utf16_trunc_handles_nul_limit_and_surrogates() {
        let mut buf = [0xFFFFu16; 6];
        copy_utf16_trunc(&mut buf, "abc");
        assert_eq!(&buf[..4], &[b'a' as u16, b'b' as u16, b'c' as u16, 0]);

        // Surrogate pair '😀' takes 2 u16 units; with capacity 4 (max 3 units), 'ab' + '😀' = 4 > 3 so stops at 'ab'.
        let mut small = [0xFFFFu16; 4];
        copy_utf16_trunc(&mut small, "ab😀c");
        assert_eq!(&small[..3], &[b'a' as u16, b'b' as u16, 0]);
    }
}
