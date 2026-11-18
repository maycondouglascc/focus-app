//! Thread EcoQoS (`ThreadPowerThrottling`) and memory priority helpers for Windows 11.
//!
//! Background threads (`monitor`, `db`, `claude`, `url`) run with EcoQoS and
//! `MEMORY_PRIORITY_LOW` permanently; the UI thread enables EcoQoS whenever no
//! window/flyout/mini-timer is open and disables it while visible.

use std::mem::size_of;

use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, ThreadMemoryPriority, ThreadPowerThrottling,
    MEMORY_PRIORITY_INFORMATION, MEMORY_PRIORITY_LOW, MEMORY_PRIORITY_NORMAL,
    THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    THREAD_POWER_THROTTLING_STATE,
};

/// Enables or disables EcoQoS execution-speed throttling and low memory priority
/// on the calling thread.
pub fn set_background_qos(background: bool) -> bool {
    let power_state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if background {
            THREAD_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };
    let mem_prio = MEMORY_PRIORITY_INFORMATION {
        MemoryPriority: if background {
            MEMORY_PRIORITY_LOW
        } else {
            MEMORY_PRIORITY_NORMAL
        },
    };
    // SAFETY: `GetCurrentThread()` is a pseudo-handle valid for this thread; both info structs
    // match the `THREAD_INFORMATION_CLASS` discriminants and sizes required by `SetThreadInformation`.
    unsafe {
        let h = GetCurrentThread();
        let p_ok = SetThreadInformation(
            h,
            ThreadPowerThrottling,
            std::ptr::from_ref(&power_state).cast(),
            size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        )
        .is_ok();
        let m_ok = SetThreadInformation(
            h,
            ThreadMemoryPriority,
            std::ptr::from_ref(&mem_prio).cast(),
            size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
        )
        .is_ok();
        p_ok && m_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_background_qos_succeeds_on_windows_11() {
        assert!(set_background_qos(true));
        assert!(set_background_qos(false));
    }
}
