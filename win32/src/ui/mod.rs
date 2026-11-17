//! Native UI. Everything here exists only while the window is open: the window
//! is created on open and destroyed on close, with all of its resources.

pub mod chrome;
pub mod frame;
pub mod prefs;
pub mod render;
pub mod selftest;
pub mod strings;
pub mod text;
pub mod theme;
pub mod tree;
pub mod window;

use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// Per-monitor DPI awareness (v2). Call once, before creating any window.
pub fn init() {
    // SAFETY: plain process-wide setting; failing (already set) is harmless.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
