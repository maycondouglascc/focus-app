//! Gate 0 measurement: what does reading a browser's address bar through UI
//! Automation cost the browser?
//!
//! Usage: cargo run --release --example uia_probe -- <image.exe> [baseline_s] [probe_s] [after_s] [root_pid]
//!
//! Three phases. Browser memory and CPU (all of its processes) are sampled
//! every 5 s in each; during the probe phase the address bar is read once a
//! second, far more often than the monitor will (it only reads when the title
//! changes). Prints numbers only: never titles or URLs, only the URL scheme.

use std::time::{Duration, Instant};

use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, InternalGetWindowText, IsWindowVisible,
};

fn filetime_100ns(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// PIDs of the processes whose image name matches (case-insensitive). With
/// `root`, only that process and its descendants: an isolated browser instance
/// that does not mix with whatever else is running.
fn pids_named(image: &str, root: Option<u32>) -> Vec<u32> {
    let mut all: Vec<(u32, u32, bool)> = Vec::new(); // (pid, parent, name matches)
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return Vec::new();
        };
        let mut e = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(e.szExeFile.len());
                let name_ok =
                    String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(image);
                all.push((e.th32ProcessID, e.th32ParentProcessID, name_ok));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    let Some(root) = root else {
        return all.iter().filter(|p| p.2).map(|p| p.0).collect();
    };
    let mut keep = vec![root];
    loop {
        let before = keep.len();
        for &(pid, ppid, _) in &all {
            if keep.contains(&ppid) && !keep.contains(&pid) {
                keep.push(pid);
            }
        }
        if keep.len() == before {
            break;
        }
    }
    keep
}

/// (private bytes, cpu 100ns units) for one process.
fn usage(pid: u32) -> Option<(u64, u64)> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut mem = PROCESS_MEMORY_COUNTERS_EX::default();
        let ok = GetProcessMemoryInfo(
            h,
            &mut mem as *mut _ as *mut _,
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        );
        let (mut c, mut x, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let t = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u);
        let _ = CloseHandle(h);
        if ok.is_err() || t.is_err() {
            return None;
        }
        Some((
            mem.PrivateUsage as u64,
            filetime_100ns(k) + filetime_100ns(u),
        ))
    }
}

#[derive(Clone, Copy, Default)]
struct Sample {
    procs: usize,
    private: u64,
    cpu: u64,
}

fn sample(image: &str, root: Option<u32>) -> Sample {
    let mut s = Sample::default();
    for pid in pids_named(image, root) {
        if let Some((m, c)) = usage(pid) {
            s.procs += 1;
            s.private += m;
            s.cpu += c;
        }
    }
    s
}

struct Find {
    pids: Vec<u32>,
    hwnd: Option<HWND>,
}

unsafe extern "system" fn pick_window(h: HWND, lparam: LPARAM) -> BOOL {
    let f = &mut *(lparam.0 as *mut Find);
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, Some(&mut pid));
    if f.pids.contains(&pid) && IsWindowVisible(h).as_bool() {
        let mut buf = [0u16; 64];
        if InternalGetWindowText(h, &mut buf) > 0 {
            f.hwnd = Some(h);
            return BOOL(0);
        }
    }
    BOOL(1)
}

/// Depth-limited search for the first Edit control with a value. Never
/// descends into a Document (the page): that is what would make the browser
/// build its accessibility tree for web content.
fn find_edit(
    walker: &IUIAutomationTreeWalker,
    el: &IUIAutomationElement,
    depth: u32,
    budget: &mut u32,
) -> Option<IUIAutomationElement> {
    if depth > 9 || *budget == 0 {
        return None;
    }
    let mut child = unsafe { walker.GetFirstChildElement(el).ok() };
    while let Some(c) = child {
        *budget = budget.saturating_sub(1);
        let kind = unsafe { c.CurrentControlType().ok() };
        if kind == Some(UIA_EditControlTypeId) {
            return Some(c);
        }
        if kind != Some(UIA_DocumentControlTypeId) {
            if let Some(found) = find_edit(walker, &c, depth + 1, budget) {
                return Some(found);
            }
        }
        if *budget == 0 {
            return None;
        }
        child = unsafe { walker.GetNextSiblingElement(&c).ok() };
    }
    None
}

/// Reads the address bar: (latency, url chars, scheme) or None if not found.
fn read_url(auto: &IUIAutomation, hwnd: HWND) -> (Duration, Option<(usize, String)>) {
    let t = Instant::now();
    let result = (|| unsafe {
        let root = auto.ElementFromHandle(hwnd).ok()?;
        let walker = auto.ControlViewWalker().ok()?;
        let mut budget = 600u32;
        let edit = find_edit(&walker, &root, 0, &mut budget)?;
        let pat: IUIAutomationValuePattern = edit
            .GetCurrentPattern(UIA_ValuePatternId)
            .ok()?
            .cast()
            .ok()?;
        let v = pat.CurrentValue().ok()?.to_string();
        let scheme = v
            .split_once("://")
            .map(|(s, _)| s.to_string())
            .unwrap_or_else(|| "none".into());
        Some((v.chars().count(), scheme))
    })();
    (t.elapsed(), result)
}

fn mb(b: u64) -> f64 {
    b as f64 / 1_048_576.0
}

fn phase(
    name: &str,
    image: &str,
    root: Option<u32>,
    secs: u64,
    probe: Option<(&IUIAutomation, HWND)>,
) {
    let start = sample(image, root);
    let t0 = Instant::now();
    let (mut min, mut max, mut sum, mut n) = (u64::MAX, 0u64, 0u64, 0u64);
    let mut next_sample = Instant::now();
    let (mut lat_sum, mut lat_max, mut queries, mut found) =
        (Duration::ZERO, Duration::ZERO, 0u32, 0u32);
    let mut scheme = String::from("-");
    while t0.elapsed() < Duration::from_secs(secs) {
        if let Some((auto, hwnd)) = probe {
            let (d, r) = read_url(auto, hwnd);
            queries += 1;
            lat_sum += d;
            lat_max = lat_max.max(d);
            if let Some((_, s)) = r {
                found += 1;
                scheme = s;
            }
        }
        if Instant::now() >= next_sample {
            let s = sample(image, root);
            min = min.min(s.private);
            max = max.max(s.private);
            sum += s.private;
            n += 1;
            next_sample += Duration::from_secs(5);
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    let end = sample(image, root);
    let cpu_pct =
        (end.cpu.saturating_sub(start.cpu)) as f64 / 1e7 / t0.elapsed().as_secs_f64() * 100.0;
    println!(
        "{name:<9} procs {:>2}->{:>2} | private MB min {:>7.1} avg {:>7.1} max {:>7.1} | end-start {:+7.1} MB | browser cpu {:>5.1}% of one core",
        start.procs,
        end.procs,
        mb(min),
        mb(sum / n.max(1)),
        mb(max),
        mb(end.private) - mb(start.private),
        cpu_pct
    );
    if probe.is_some() {
        println!(
            "          queries {queries}, address bar found {found}/{queries}, url scheme {scheme}, latency avg {:.1} ms max {:.1} ms",
            lat_sum.as_secs_f64() * 1000.0 / queries.max(1) as f64,
            lat_max.as_secs_f64() * 1000.0
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let image = args.get(1).cloned().unwrap_or_else(|| "msedge.exe".into());
    let secs = |i: usize, d: u64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (baseline, probe, after) = (secs(2, 30), secs(3, 60), secs(4, 30));
    let root: Option<u32> = args.get(5).and_then(|s| s.parse().ok());

    let pids = pids_named(&image, root);
    let mut find = Find { pids, hwnd: None };
    unsafe {
        let _ = EnumWindows(Some(pick_window), LPARAM(&mut find as *mut _ as isize));
    }
    let Some(hwnd) = find.hwnd else {
        println!("no visible {image} window found");
        return;
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let auto: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.expect("UIA");

    println!(
        "browser: {image}, {baseline}s baseline, {probe}s probing (1 query/s), {after}s after"
    );
    phase("baseline", &image, root, baseline, None);
    let own_before = sample("uia_probe.exe", None);
    phase("probing", &image, root, probe, Some((&auto, hwnd)));
    let own_after = sample("uia_probe.exe", None);
    println!(
        "          probe process private: {:.1} MB -> {:.1} MB",
        mb(own_before.private),
        mb(own_after.private)
    );
    drop(auto);
    phase("after", &image, root, after, None);
}
