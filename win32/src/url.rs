//! Asynchronous browser address-bar URL reader via Windows UI Automation.
//!
//! Runs on a dedicated COM MTA thread (`focus-url`), queries only when the
//! foreground app is a supported browser and its window title changed, traverses
//! browser chrome strictly with chained `TreeScope_Children` (never entering
//! `Document` nodes), caches the address-bar element per HWND, and respects a
//! 500 ms budget, keyboard-focus suppression, private windows, and elevated
//! browser processes.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationValuePattern, TreeScope_Children, UIA_ComboBoxControlTypeId,
    UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_TextControlTypeId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, PostMessageW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_NULL,
};

const QUERY_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_DEPTH: u32 = 9;
const MAX_NODES: u32 = 600;
const MAX_URL_CHARS: usize = 1000;
const MAX_HOST_CHARS: usize = 120;

#[derive(Debug, Clone)]
pub struct UrlLookupReq {
    pub run_id: i64,
    pub seq: u64,
    pub hwnd_raw: isize,
    pub pid: u32,
}

#[derive(Debug, Clone)]
pub struct UrlResult {
    pub run_id: i64,
    pub url: String,
    pub context: String,
}

/// Normalizes a browser address bar string according to Focus privacy rules:
/// - Prefixes `https://` when scheme is omitted (Chrome/Firefox omit it).
/// - Drops `?query` and `#fragment` (no OAuth tokens/secrets in SQLite).
/// - Returns `None` for empty strings, search queries, or internal schemes (`chrome://`, `edge://`, `about:`).
pub fn normalize_url(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }

    let with_scheme = if let Some((scheme, rest)) = trimmed.split_once("://") {
        let s = scheme.to_ascii_lowercase();
        if s != "http" && s != "https" {
            return None;
        }
        format!("{s}://{rest}")
    } else {
        let authority = trimmed.split('/').next().unwrap_or(trimmed);
        if let Some((host_part, port_part)) = authority.split_once(':') {
            // Reject non-http pseudo-schemes like `about:blank` or `data:text/html`.
            if port_part.is_empty() || !port_part.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            if host_part.is_empty()
                || (!host_part.contains('.') && !host_part.eq_ignore_ascii_case("localhost"))
            {
                return None;
            }
        } else if !authority.contains('.') && !authority.eq_ignore_ascii_case("localhost") {
            return None;
        }
        format!("https://{trimmed}")
    };

    let without_frag = with_scheme.split('#').next().unwrap_or(&with_scheme);
    let clean = without_frag.split('?').next().unwrap_or(without_frag);
    let after_scheme = clean.split_once("://")?.1;
    let authority = after_scheme.split('/').next().unwrap_or("");
    let raw_host = authority.split(':').next().unwrap_or("").trim();
    if raw_host.is_empty() {
        return None;
    }
    let lower_host = raw_host.to_ascii_lowercase();
    let host = lower_host.strip_prefix("www.").unwrap_or(&lower_host);
    if host.is_empty() {
        return None;
    }

    let url_out: String = clean.chars().take(MAX_URL_CHARS).collect();
    let ctx_out: String = host.chars().take(MAX_HOST_CHARS).collect();
    Some((url_out, ctx_out))
}

fn is_elevated_pid(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: opens process and token with query-only rights; handles are always closed before returning.
    unsafe {
        let Ok(proc_h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return true;
        };
        let mut token = HANDLE::default();
        if OpenProcessToken(proc_h, TOKEN_QUERY, &mut token).is_err() {
            let _ = CloseHandle(proc_h);
            return true;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elev as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        );
        let _ = CloseHandle(token);
        let _ = CloseHandle(proc_h);
        ok.is_err() || elev.TokenIsElevated != 0
    }
}

fn is_hung(hwnd: HWND) -> bool {
    // SAFETY: `WM_NULL` ping with `SMTO_ABORTIFHUNG` detects unresponsive UI threads without modifying state.
    unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            200,
            None,
        )
        .0 == 0
    }
}

fn has_value_pattern(el: &IUIAutomationElement) -> bool {
    // SAFETY: `GetCurrentPattern` checks whether the UIA element exposes `IUIAutomationValuePattern`.
    unsafe { el.GetCurrentPattern(UIA_ValuePatternId).is_ok() }
}

/// Traverses browser chrome strictly via chained `TreeScope_Children`, never
/// entering `Document` nodes, stopping on timeout or node budget exhaustion.
fn find_edit_children(
    cond: &IUIAutomationCondition,
    el: &IUIAutomationElement,
    depth: u32,
    budget: &mut u32,
    started: Instant,
) -> Option<IUIAutomationElement> {
    if depth > MAX_DEPTH || *budget == 0 || started.elapsed() >= QUERY_TIMEOUT {
        return None;
    }
    // SAFETY: `FindAll(TreeScope_Children, ...)` queries direct children of `el` only.
    let children = unsafe { el.FindAll(TreeScope_Children, cond).ok()? };
    let len = unsafe { children.Length().ok()? };
    for i in 0..len {
        if *budget == 0 || started.elapsed() >= QUERY_TIMEOUT {
            return None;
        }
        *budget = budget.saturating_sub(1);
        // SAFETY: `i` is bounded by `0..len`.
        let Ok(c) = (unsafe { children.GetElement(i) }) else {
            continue;
        };
        let kind = unsafe { c.CurrentControlType().ok() };
        if kind == Some(UIA_EditControlTypeId) && has_value_pattern(&c) {
            return Some(c);
        }
        if kind == Some(UIA_ComboBoxControlTypeId) {
            if has_value_pattern(&c) {
                return Some(c);
            }
            // SAFETY: inspects direct children of Firefox's address-bar ComboBox only.
            if let Ok(sub) = unsafe { c.FindAll(TreeScope_Children, cond) } {
                let slen = unsafe { sub.Length().unwrap_or(0) };
                for j in 0..slen {
                    let Ok(x) = (unsafe { sub.GetElement(j) }) else {
                        continue;
                    };
                    let xk = unsafe { x.CurrentControlType().ok() };
                    if xk == Some(UIA_EditControlTypeId) && has_value_pattern(&x) {
                        return Some(x);
                    }
                    if xk == Some(UIA_TextControlTypeId) {
                        return Some(x);
                    }
                }
            }
        }
        if kind != Some(UIA_DocumentControlTypeId) {
            if let Some(found) = find_edit_children(cond, &c, depth + 1, budget, started) {
                return Some(found);
            }
        }
    }
    None
}

enum ReadBar {
    Ok(String),
    Focused,
    Stale,
}

fn read_cached_bar(el: &IUIAutomationElement) -> ReadBar {
    // SAFETY: queries `HasKeyboardFocus`, `ValuePattern`, or `Name` on a cached `IUIAutomationElement`.
    unsafe {
        match el.CurrentHasKeyboardFocus() {
            Ok(b) if b.as_bool() => return ReadBar::Focused,
            Err(_) => return ReadBar::Stale,
            Ok(_) => {}
        }
        if let Ok(pat_unk) = el.GetCurrentPattern(UIA_ValuePatternId) {
            if let Ok(pat) = pat_unk.cast::<IUIAutomationValuePattern>() {
                return match pat.CurrentValue() {
                    Ok(v) => ReadBar::Ok(v.to_string()),
                    Err(_) => ReadBar::Stale,
                };
            }
        }
        match el.CurrentName() {
            Ok(n) => ReadBar::Ok(n.to_string()),
            Err(_) => ReadBar::Stale,
        }
    }
}

fn query_address_bar(
    auto: &IUIAutomation,
    cond: &IUIAutomationCondition,
    cache: &mut HashMap<isize, IUIAutomationElement>,
    req: &UrlLookupReq,
) -> Option<(String, String)> {
    if is_elevated_pid(req.pid) {
        return None;
    }
    let hwnd = HWND(req.hwnd_raw as *mut _);
    if is_hung(hwnd) {
        return None;
    }
    let started = Instant::now();
    if let Some(cached) = cache.get(&req.hwnd_raw) {
        match read_cached_bar(cached) {
            ReadBar::Ok(raw) => {
                if started.elapsed() > QUERY_TIMEOUT {
                    return None;
                }
                return normalize_url(&raw);
            }
            ReadBar::Focused => return None,
            ReadBar::Stale => {
                cache.remove(&req.hwnd_raw);
            }
        }
    }
    if started.elapsed() >= QUERY_TIMEOUT {
        return None;
    }
    // SAFETY: `ElementFromHandle` wraps `hwnd` for child-scoped traversal on this MTA thread.
    let root = unsafe { auto.ElementFromHandle(hwnd).ok()? };
    let mut budget = MAX_NODES;
    let bar = find_edit_children(cond, &root, 0, &mut budget, started)?;
    cache.insert(req.hwnd_raw, bar.clone());
    if started.elapsed() >= QUERY_TIMEOUT {
        return None;
    }
    match read_cached_bar(&bar) {
        ReadBar::Ok(raw) => normalize_url(&raw),
        ReadBar::Focused | ReadBar::Stale => None,
    }
}

struct SharedQueue {
    pending: Option<UrlLookupReq>,
    stop: bool,
    results: VecDeque<UrlResult>,
}

pub struct UrlWorker {
    latest_seq: Arc<AtomicU64>,
    shared: Arc<(Mutex<SharedQueue>, Condvar)>,
    join: Option<JoinHandle<()>>,
}

impl UrlWorker {
    pub fn spawn(monitor_hwnd_raw: isize, ready_msg: u32) -> Self {
        let latest_seq = Arc::new(AtomicU64::new(0));
        let shared = Arc::new((
            Mutex::new(SharedQueue {
                pending: None,
                stop: false,
                results: VecDeque::new(),
            }),
            Condvar::new(),
        ));
        let seq_clone = Arc::clone(&latest_seq);
        let shared_clone = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("focus-url".into())
            .spawn(move || run_worker(monitor_hwnd_raw, ready_msg, seq_clone, shared_clone))
            .ok();
        Self {
            latest_seq,
            shared,
            join,
        }
    }

    pub fn submit(&self, run_id: i64, hwnd_raw: isize, pid: u32) {
        let seq = self.latest_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let (lock, cvar) = &*self.shared;
        if let Ok(mut g) = lock.lock() {
            g.pending = Some(UrlLookupReq {
                run_id,
                seq,
                hwnd_raw,
                pid,
            });
            cvar.notify_one();
        }
    }

    pub fn invalidate(&self) {
        self.latest_seq.fetch_add(1, Ordering::SeqCst);
        let (lock, _) = &*self.shared;
        if let Ok(mut g) = lock.lock() {
            g.pending = None;
        }
    }

    pub fn drain_results(&self) -> Vec<UrlResult> {
        let (lock, _) = &*self.shared;
        let Ok(mut g) = lock.lock() else {
            return Vec::new();
        };
        g.results.drain(..).collect()
    }

    pub fn stop(&mut self) {
        let (lock, cvar) = &*self.shared;
        if let Ok(mut g) = lock.lock() {
            g.stop = true;
            g.pending = None;
            cvar.notify_all();
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for UrlWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn init_uia() -> Option<(IUIAutomation, IUIAutomationCondition)> {
    // SAFETY: initializes COM MTA and creates `CUIAutomation` on this dedicated thread.
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return None;
        }
        let auto: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        if let Ok(a2) = auto.cast::<IUIAutomation2>() {
            let _ = a2.SetConnectionTimeout(500);
            let _ = a2.SetTransactionTimeout(500);
        }
        let cond = auto.CreateTrueCondition().ok()?;
        Some((auto, cond))
    }
}

fn run_worker(
    monitor_hwnd_raw: isize,
    ready_msg: u32,
    latest_seq: Arc<AtomicU64>,
    shared: Arc<(Mutex<SharedQueue>, Condvar)>,
) {
    crate::qos::set_background_qos(true);
    let mut uia: Option<(IUIAutomation, IUIAutomationCondition)> = None;
    let mut uia_attempted = false;

    let mut cache: HashMap<isize, IUIAutomationElement> = HashMap::new();
    let (lock, cvar) = &*shared;
    loop {
        let req = {
            let Ok(mut g) = lock.lock() else { break };
            while g.pending.is_none() && !g.stop {
                let Ok(next) = cvar.wait(g) else { return };
                g = next;
            }
            if g.stop {
                break;
            }
            g.pending.take()
        };
        let Some(req) = req else { continue };
        if !uia_attempted {
            uia_attempted = true;
            uia = init_uia();
        }
        if let Some((ref auto, ref cond)) = uia {
            if cache.len() > 16 {
                cache.retain(|&raw, _| {
                    // SAFETY: `IsWindow` safely validates any HWND value.
                    unsafe { IsWindow(Some(HWND(raw as *mut _))).as_bool() }
                });
            }
            if let Some((url, context)) = query_address_bar(auto, cond, &mut cache, &req) {
                if latest_seq.load(Ordering::SeqCst) == req.seq {
                    if let Ok(mut g) = lock.lock() {
                        g.results.push_back(UrlResult {
                            run_id: req.run_id,
                            url,
                            context,
                        });
                    }
                    if monitor_hwnd_raw != 0 {
                        // SAFETY: `monitor_hwnd_raw` is the monitor thread's `FocusHiddenWindow`.
                        unsafe {
                            let _ = PostMessageW(
                                Some(HWND(monitor_hwnd_raw as *mut _)),
                                ready_msg,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        }
                    }
                }
            }
        }
    }
    cache.clear();
    if uia.is_some() {
        drop(uia);
        // SAFETY: pairs with `CoInitializeEx` inside `init_uia`.
        unsafe {
            CoUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_url_prefixes_https_strips_query_and_fragment_and_skips_internal() {
        assert_eq!(
            normalize_url("github.com/rust-lang/rust?tab=issues#top"),
            Some((
                "https://github.com/rust-lang/rust".into(),
                "github.com".into()
            ))
        );
        assert_eq!(
            normalize_url("https://www.Docs.RS/windows/latest/?q=abc#frag"),
            Some((
                "https://www.Docs.RS/windows/latest/".into(),
                "docs.rs".into()
            ))
        );
        assert_eq!(
            normalize_url("http://localhost:3000/login?token=secret"),
            Some(("http://localhost:3000/login".into(), "localhost".into()))
        );
        assert_eq!(
            normalize_url("localhost:5173/app#view"),
            Some(("https://localhost:5173/app".into(), "localhost".into()))
        );

        for skip in [
            "",
            "   ",
            "how to center a div",
            "chrome://settings/privacy",
            "edge://flags",
            "about:blank",
            "brave://rewards",
            "file:///C:/Users/me/note.html",
            "javascript:void(0)",
            "data:text/plain,hello",
        ] {
            assert_eq!(normalize_url(skip), None, "expected None for {skip:?}");
        }
    }
}
