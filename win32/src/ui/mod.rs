//! Native UI. Everything here exists only while the window is open: the window
//! is created on open and destroyed on close, with all of its resources.

pub mod a11y;
pub mod chrome;
pub mod focus;
pub mod frame;
pub mod input;
pub mod popup;
pub mod prefs;
pub mod render;
pub mod screen;
pub mod selftest;
pub mod strings;
pub mod text;
pub mod theme;
pub mod tree;
pub mod widgets;
pub mod window;

use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::Pointer::EnableMouseInPointer;

/// Per-monitor DPI awareness (v2) and unified `WM_POINTER*` input.
/// Call once, before creating any window.
pub fn init() {
    // SAFETY: plain process-wide settings; failing (already set) is harmless.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = EnableMouseInPointer(true);
    }
}
