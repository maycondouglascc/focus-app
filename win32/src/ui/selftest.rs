//! UI selftest, live host, and diagnostics CLI entry points:
//! - `--ui` / `--demo [--fresh] [--screen <name>]`: interactive desktop window
//! - `--ui-cycle N [--all-screens]`: open/close memory leak test (Gate 2 / Gate 3)
//! - `--ui-show SECONDS [--gallery] [--demo] [--fresh] [--screen <name>]`: cold-open + memory check
//! - `--diag-drag`: 4K 200% DPI continuous drag frame counter + Esc/Alt/Autoscroll check

use std::path::Path;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Gdi::UpdateWindow;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, KillTimer, PeekMessageW, SetTimer, TranslateMessage, WaitMessage, MSG,
    PM_REMOVE, WM_QUIT, WM_TIMER,
};

use crate::db::{shared_store, Db, DbStore, Store};
use crate::demo::DemoStore;
use crate::locale::SystemLocale;

use super::app::{AppShell, Section};
use super::screen::Screen;
use super::strings;
use super::window;

/// Decision 0004: the graphics stack keeps about 21 MB after the first open.
const PRIVATE_BUDGET_MB: f64 = 30.0;
const WARMUP_CYCLES: u32 = 100;
/// After warm-up, N more open/close cycles must not grow memory by more than this.
const GROWTH_BUDGET_MB: f64 = 1.0;

fn memory_mb() -> (f64, f64) {
    let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
    // SAFETY: `c` is a valid out buffer and `cb` states its real size.
    unsafe {
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb);
    }
    let priv_mb = c.PrivateUsage as f64 / (1024.0 * 1024.0);
    let ws_mb = c.WorkingSetSize as f64 / (1024.0 * 1024.0);
    (priv_mb, ws_mb)
}

fn private_mb() -> f64 {
    memory_mb().0
}

fn pump_for(d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        window::pump();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn apply_screen_arg(shell: &mut AppShell, screen_name: Option<&str>) -> Result<(), i32> {
    let Some(name) = screen_name else {
        return Ok(());
    };
    if name.eq_ignore_ascii_case("welcome") {
        shell.force_welcome();
        return Ok(());
    }
    if let Some(sec) = Section::from_cli_name(name) {
        shell.navigate(sec);
        return Ok(());
    }
    eprintln!(
        "focus: unknown --screen '{name}' (expected welcome|calendar|activity|reports|projects|project|settings)"
    );
    Err(2)
}

fn make_demo_shell(fresh: bool, screen_name: Option<&str>) -> Result<Box<AppShell>, i32> {
    let locale = SystemLocale::query();
    let store = if fresh {
        DemoStore::fresh(locale)
    } else {
        DemoStore::new(locale)
    };
    let mut shell = Box::new(AppShell::new(shared_store(Box::new(store))));
    apply_screen_arg(&mut shell, screen_name)?;
    Ok(shell)
}

fn make_db_shell(path: &Path, screen_name: Option<&str>) -> Result<Box<AppShell>, i32> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let db = match Db::open(path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("focus: cannot open database at {}: {e}", path.display());
            return Err(1);
        }
    };
    let store = DbStore::new(db, SystemLocale::query());
    let mut shell = Box::new(AppShell::new(shared_store(Box::new(store))));
    apply_screen_arg(&mut shell, screen_name)?;
    Ok(shell)
}

fn run_loop_until_closed() {
    // SAFETY: owns the main window and thread-local message queue on this UI thread.
    unsafe {
        let mut msg = MSG::default();
        while window::is_open() {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    window::close();
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !window::is_open() {
                break;
            }
            let _ = WaitMessage();
        }
    }
}

pub fn run_demo(fresh: bool, screen_name: Option<&str>) -> i32 {
    let shell = match make_demo_shell(fresh, screen_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    run_screen(shell)
}

pub fn run_db(path: &Path, screen_name: Option<&str>) -> i32 {
    let shell = match make_db_shell(path, screen_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    run_screen(shell)
}

fn run_screen(screen: Box<dyn Screen>) -> i32 {
    super::init();
    if !window::open() {
        return 1;
    }
    if !window::set_screen(screen) {
        window::close();
        super::render::reset_shared();
        return 1;
    }
    if let Some(hwnd) = window::hwnd() {
        // SAFETY: `hwnd` is our thread-owned window.
        unsafe {
            let _ = UpdateWindow(hwnd);
        }
    }
    run_loop_until_closed();
    window::close();
    super::render::reset_shared();
    0
}

fn tour_all_screens(proj_name: Option<&str>) -> bool {
    let targets: Vec<&str> = [
        Some(strings::NAV_ACTIVITY),
        Some(strings::NAV_REPORTS),
        Some(strings::NAV_PROJECTS),
        proj_name,
        Some(strings::NAV_SETTINGS),
        Some(strings::NAV_CALENDAR),
    ]
    .into_iter()
    .flatten()
    .collect();

    for name in targets {
        if !window::click_by_name(name) {
            eprintln!("focus: missing navigation target '{name}' during --all-screens cycle");
            return false;
        }
        if let Some(hwnd) = window::hwnd() {
            // SAFETY: `hwnd` belongs to our thread.
            unsafe {
                let _ = UpdateWindow(hwnd);
            }
        }
        window::pump();
    }
    true
}

pub fn cycle(n: u32, all_screens: bool) -> i32 {
    super::init();
    let baseline = private_mb();
    let proj_name = if all_screens {
        DemoStore::new(SystemLocale::en_us())
            .list_projects()
            .ok()
            .and_then(|ps| ps.into_iter().next().map(|p| p.name))
    } else {
        None
    };

    // One warm-up cycle so one-time allocations (window class, DLLs) do not
    // count as growth.
    window::open();
    if all_screens {
        if let Ok(shell) = make_demo_shell(false, None) {
            let _ = window::set_screen(shell);
            let _ = tour_all_screens(proj_name.as_deref());
        }
    }
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
        if all_screens {
            let Ok(shell) = make_demo_shell(false, None) else {
                println!("demo shell init failed at cycle {i}");
                return 1;
            };
            if !window::set_screen(shell) || !tour_all_screens(proj_name.as_deref()) {
                println!("all-screens navigation failed at cycle {i}");
                return 1;
            }
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
        "cycles {n}{} in {:.1}s | first open {first_open_ms:.1} ms | private MB start {start:.2} end {end:.2} growth {:+.2} | window left open: {leaked}",
        if all_screens { " (all-screens)" } else { "" },
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

/// `focus.exe --ui-show SECONDS [--gallery] [--demo [--fresh] | --db <path>] [--screen <name>]`:
/// opens the window cleanly, logs cold-open timing and live memory usage, then closes cleanly.
pub fn show(
    seconds: u64,
    gallery: bool,
    demo: bool,
    fresh: bool,
    db_path: Option<&Path>,
    screen_name: Option<&str>,
) -> i32 {
    let t0 = Instant::now();
    super::init();
    if !window::open() {
        return 1;
    }
    if gallery {
        let _ = window::set_screen(Box::new(super::screen::WidgetGalleryScreen::new()));
    } else if let Some(p) = db_path {
        match make_db_shell(p, screen_name) {
            Ok(shell) => {
                let _ = window::set_screen(shell);
            }
            Err(code) => {
                window::close();
                super::render::reset_shared();
                return code;
            }
        }
    } else if demo || screen_name.is_some() {
        match make_demo_shell(fresh, screen_name) {
            Ok(shell) => {
                let _ = window::set_screen(shell);
            }
            Err(code) => {
                window::close();
                super::render::reset_shared();
                return code;
            }
        }
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
    // SAFETY: thread-scoped timer and message loop run on the UI thread that owns the window.
    unsafe {
        let timer = SetTimer(None, 0, ms, None);
        let mut msg = MSG::default();
        while window::is_open() {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_TIMER && msg.hwnd.0.is_null() && msg.wParam.0 == timer {
                    if timer != 0 {
                        let _ = KillTimer(None, timer);
                    }
                    let (priv_mb, ws_mb) = memory_mb();
                    println!(
                        "first_frame_ms={:.1} paints={} private_mb={:.2} working_set_mb={:.2}",
                        first_frame_ms.unwrap_or(-1.0),
                        window::paint_count(),
                        priv_mb,
                        ws_mb
                    );
                    window::close();
                    super::render::reset_shared();
                    return 0;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if first_frame_ms.is_none() && window::paint_count() >= 1 {
                    first_frame_ms = Some(t0.elapsed().as_secs_f64() * 1000.0);
                }
            }
            if !window::is_open() {
                break;
            }
            let _ = WaitMessage();
        }
        if timer != 0 {
            let _ = KillTimer(None, timer);
        }
    }
    let (priv_mb, ws_mb) = memory_mb();
    println!(
        "first_frame_ms={:.1} paints={} private_mb={:.2} working_set_mb={:.2}",
        first_frame_ms.unwrap_or(-1.0),
        window::paint_count(),
        priv_mb,
        ws_mb
    );
    window::close();
    super::render::reset_shared();
    0
}

/// `focus.exe --diag-drag`: verifies Esc cancel, Alt no-snap, edge autoscroll, and
/// benchmarks 180 continuous drag frames rendered at 3840x2160 (`200%` DPI).
pub fn diag_drag() -> i32 {
    super::init();
    if !window::open() {
        eprintln!("focus: diag_drag could not open window");
        return 1;
    }
    let Ok(shell) = make_demo_shell(false, Some("calendar")) else {
        window::close();
        super::render::reset_shared();
        return 1;
    };
    if !window::set_screen(shell) {
        window::close();
        super::render::reset_shared();
        return 1;
    }
    let metrics = window::bench_drag_4k(180);
    window::close();
    super::render::reset_shared();

    match metrics {
        Ok(m) => {
            println!(
                "diag_drag checks: esc_cancel={} alt_nosnap={} autoscroll={}",
                m.esc_cancel_ok, m.alt_nosnap_ok, m.autoscroll_ok
            );
            println!(
                "diag_drag res={}x{} dpi={}% frames={} elapsed_ms={:.2} avg_ms={:.3} p95_ms={:.3} max_ms={:.3} fps={:.2}",
                m.res_w_px,
                m.res_h_px,
                (m.dpi * 100) / 96,
                m.frames,
                m.elapsed_ms,
                m.avg_ms,
                m.p95_ms,
                m.max_ms,
                m.fps
            );
            if m.esc_cancel_ok && m.alt_nosnap_ok && m.autoscroll_ok && m.fps >= 60.0 {
                println!("OK");
                0
            } else {
                println!("FAIL");
                1
            }
        }
        Err(e) => {
            eprintln!("diag_drag error: {e}");
            println!("FAIL");
            1
        }
    }
}
