//! System accessibility and visual preferences:
//! - Windows text size (`UISettings.TextScaleFactor` via WinRT)
//! - High contrast (`SPI_GETHIGHCONTRAST`)
//! - Reduced motion (`SPI_GETCLIENTAREAANIMATION`)
//! - Windows 11 Mica backdrop (`DWMWA_SYSTEMBACKDROP_TYPE`) with solid fallback

use std::mem::size_of;
use std::sync::Once;

use windows::core::BOOL;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMSBT_NONE,
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW, HIGHCONTRASTW_FLAGS};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};
use windows::UI::ViewManagement::UISettings;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemPrefs {
    pub text_scale: f32,
    pub high_contrast: bool,
    pub reduced_motion: bool,
}

impl Default for SystemPrefs {
    fn default() -> Self {
        Self {
            text_scale: 1.0,
            high_contrast: false,
            reduced_motion: false,
        }
    }
}

impl SystemPrefs {
    pub fn query() -> Self {
        Self {
            text_scale: query_text_scale(),
            high_contrast: query_high_contrast(),
            reduced_motion: query_reduced_motion(),
        }
    }
}

pub fn normalize_text_scale(raw: f64) -> f32 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    (raw as f32).clamp(0.5, 4.0)
}

static MTA_KEEPALIVE: Once = Once::new();

fn query_text_scale() -> f32 {
    // Keep the process MTA alive so WinRT's `FactoryCache` for `UISettings` is
    // never invalidated across worker threads while keeping callers free to use STA.
    // SAFETY: `CoIncrementMTAUsage` initializes the process MTA idempotently.
    MTA_KEEPALIVE.call_once(|| unsafe {
        let _ = CoIncrementMTAUsage();
    });
    UISettings::new()
        .and_then(|s| s.TextScaleFactor())
        .map(normalize_text_scale)
        .unwrap_or(1.0)
}

fn query_high_contrast() -> bool {
    let mut hc = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: `hc` is properly sized with `cbSize` matching `HIGHCONTRASTW`.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some((&mut hc as *mut HIGHCONTRASTW).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && (hc.dwFlags & HCF_HIGHCONTRASTON) != HIGHCONTRASTW_FLAGS(0)
}

fn query_reduced_motion() -> bool {
    let mut enabled: BOOL = BOOL(1);
    // SAFETY: `enabled` is a valid out `BOOL` for `SPI_GETCLIENTAREAANIMATION`.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && !enabled.as_bool()
}

/// Applies Windows 11 Mica backdrop to `hwnd` unless high contrast is active.
/// Returns `true` when Mica is active; `false` means the caller should paint
/// an opaque solid fallback surface.
pub fn apply_mica(hwnd: HWND, high_contrast: bool) -> bool {
    if high_contrast {
        let margins = MARGINS {
            cyTopHeight: 1,
            ..Default::default()
        };
        let backdrop: DWM_SYSTEMBACKDROP_TYPE = DWMSBT_NONE;
        // SAFETY: `hwnd` is our live window and `backdrop` has the expected size.
        unsafe {
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                std::ptr::from_ref(&backdrop).cast(),
                size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
            );
        }
        return false;
    }

    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    let backdrop: DWM_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW;
    // SAFETY: `hwnd` is our live window and `margins`/`backdrop` match DWM ABIs.
    unsafe {
        if DwmExtendFrameIntoClientArea(hwnd, &margins).is_err() {
            return false;
        }
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            std::ptr::from_ref(&backdrop).cast(),
            size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        )
        .is_ok()
    }
}

/// Requests Windows 11 DWM rounded corners on a floating popup window.
#[allow(dead_code)] // used by popup windows in the popup commit
pub fn apply_rounded_corners(hwnd: HWND) {
    let pref: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
    // SAFETY: `hwnd` is our live popup window and `pref` is a 4-byte DWM enum.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&pref).cast(),
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_text_scale_handles_edge_values() {
        assert_eq!(normalize_text_scale(1.0), 1.0);
        assert_eq!(normalize_text_scale(1.5), 1.5);
        assert_eq!(normalize_text_scale(0.0), 1.0);
        assert_eq!(normalize_text_scale(-2.0), 1.0);
        assert_eq!(normalize_text_scale(f64::NAN), 1.0);
        assert_eq!(normalize_text_scale(10.0), 4.0);
    }

    #[test]
    fn system_prefs_query_returns_valid_scale() {
        let p = SystemPrefs::query();
        assert!(p.text_scale >= 0.5 && p.text_scale <= 4.0);
    }
}
