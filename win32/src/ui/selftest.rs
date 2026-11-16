//! `focus.exe --ui-cycle N`: opens and closes the window N times and reports
//! private memory, for the F2 gate ("memory returns after closing the window").
//! Prints numbers only. Exit code 1 if the budget is exceeded.

use std::time::{Duration, Instant};

use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;

use super::window;

/// Gate 2: after closing, private memory is back under 5 MB.
const PRIVATE_BUDGET_MB: f64 = 5.0;
/// Opening and closing 50 times must not grow memory by more than this.
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
    // One warm-up cycle so one-time allocations (window class, DLLs) do not
    // count as growth.
    window::open();
    pump_for(Duration::from_millis(100));
    window::close();
    pump_for(Duration::from_millis(100));
    let start = private_mb();
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
    if leaked || end > PRIVATE_BUDGET_MB || end - start > GROWTH_BUDGET_MB {
        println!("FAIL");
        return 1;
    }
    println!("OK");
    0
}
