//! `focus.exe --ui-cycle N`: opens and closes the window N times and reports
//! private memory, for the F2 gate ("memory returns after closing the window").
//! Prints numbers only. Exit code 1 if the budget is exceeded.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::Gdi::UpdateWindow;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, SetTimer, TranslateMessage, MSG, WM_TIMER,
};

use super::window;

/// Decision 0004: the graphics stack keeps about 21 MB after the first open.
const PRIVATE_BUDGET_MB: f64 = 30.0;
const WARMUP_CYCLES: u32 = 100;
/// After warm-up, N more open/close cycles must not grow memory by more than this.
const GROWTH_BUDGET_MB: f64 = 1.0;

fn private_mb() -> f64 {
    let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
    // SAFETY: `c` is a valid out buffer and `cb` states its real size.
    unsafe {
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb);
    }
    c.PrivateUsage as f64 / (1024.0 * 1024.0)
}

fn pump_for(d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        window::pump();
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn cycle(n: u32) -> i32 {
    super::init();
    let baseline = private_mb();
    // One warm-up cycle so one-time allocations (window class, DLLs) do not
    // count as growth.
    window::open();
    pump_for(Duration::from_millis(100));
    window::close();
    pump_for(Duration::from_millis(100));
    // Memory settles over the first ~100 opens (driver and allocator warm-up,
    // then flat: 100 and 300 cycles both grew 6.4 MB). Only growth after that
    // counts as a leak.
    for _ in 0..WARMUP_CYCLES {
        window::open();
        window::pump();
        window::close();
        window::pump();
    }
    pump_for(Duration::from_millis(500));
    let start = private_mb();
    println!("baseline before any window {baseline:.2} MB, after warm-up cycle {start:.2} MB");
    let t = Instant::now();
    let mut first_open_ms = 0.0;
    for i in 0..n {
        let t0 = Instant::now();
        if !window::open() {
            println!("open failed at cycle {i}");
            return 1;
        }
        window::pump();
        if i == 0 {
            first_open_ms = t0.elapsed().as_secs_f64() * 1000.0;
        }
        pump_for(Duration::from_millis(20));
        window::close();
        window::pump();
    }
    pump_for(Duration::from_secs(2));
    let end = private_mb();
    let leaked = window::is_open();
    println!(
        "cycles {n} in {:.1}s | first open {first_open_ms:.1} ms | private MB start {start:.2} end {end:.2} growth {:+.2} | window left open: {leaked}",
        t.elapsed().as_secs_f64(),
        end - start
    );
    super::render::reset_shared();
    if leaked || end > PRIVATE_BUDGET_MB || end - start > GROWTH_BUDGET_MB {
        println!("FAIL");
        return 1;
    }
    println!("OK");
    0
}

/// `focus.exe --ui-show SECONDS [--gallery]`: shows the window and keeps it
/// alive, logging the cold open time to the first painted frame.
pub fn show(seconds: u64, gallery: bool) -> i32 {
    let t0 = Instant::now();
    super::init();
    if !window::open() {
        return 1;
    }
    if gallery {
        let _ = window::set_screen(Box::new(super::screen::WidgetGalleryScreen::new()));
    }
    if let Some(hwnd) = window::hwnd() {
        // SAFETY: `hwnd` is our newly created window on this thread.
        unsafe {
            let _ = UpdateWindow(hwnd);
        }
    }
    let mut first_frame_ms =
        (window::paint_count() >= 1).then(|| t0.elapsed().as_secs_f64() * 1000.0);
    let ms = (seconds.saturating_mul(1000).min(u32::MAX as u64)).max(100) as u32;
    // SAFETY: thread-scoped timer and `GetMessageW` loop run on the UI thread that owns the window.
    unsafe {
        let timer = SetTimer(None, 0, ms, None);
        let mut msg = MSG::default();
        while window::is_open() && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER && msg.hwnd.0.is_null() && msg.wParam.0 == timer {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if first_frame_ms.is_none() && window::paint_count() >= 1 {
                first_frame_ms = Some(t0.elapsed().as_secs_f64() * 1000.0);
            }
        }
        if timer != 0 {
            let _ = KillTimer(None, timer);
        }
    }
    println!(
        "first_frame_ms={:.1} paints={}",
        first_frame_ms.unwrap_or(-1.0),
        window::paint_count()
    );
    window::close();
    super::render::reset_shared();
    0
}
