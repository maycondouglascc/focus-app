//! Who is in the foreground: a stable identity (key) and a readable name.
//!
//! - Name: the exe's FileDescription, then ProductName, then the exe name,
//!   skipping useless values such as "Electron" or "Java(TM) Platform".
//! - Key: the exe path, without per-version folders (Squirrel `app-1.2.3\`,
//!   Store `Name_1.2.3.0_x64__publisher`), so updates do not split history.
//! - Store apps (Calculator, Photos): their window belongs to
//!   ApplicationFrameHost.exe; the real app is the CoreWindow child.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::{BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, MAX_PATH};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetWindowThreadProcessId, InternalGetWindowText,
};

use crate::recorder::Info;

thread_local! {
    /// exe path -> display name. A few hundred bytes per app ever seen.
    static NAMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

// ── Pure rules (tested) ──────────────────────────────────────────────────

/// "C:\Program Files\X\code.exe" -> "code".
pub fn exe_stem(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match file.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("exe") => stem.to_string(),
        _ => file.to_string(),
    }
}

fn is_version_dir(seg: &str) -> bool {
    let lower = seg.to_ascii_lowercase();
    lower
        .strip_prefix("app-")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_ascii_digit())
}

/// Store package folder "Name_1.2.3.0_x64__publisher" -> "name_publisher".
fn package_identity(lower_path: &str) -> Option<String> {
    let rest = lower_path.split("\\windowsapps\\").nth(1)?;
    let folder = rest.split('\\').next()?;
    let (left, publisher) = folder.split_once("__")?;
    let name = left.split('_').next()?;
    Some(format!("pkg:{name}_{publisher}"))
}

/// Stable identity for an exe path.
pub fn normalize_key(path: &str) -> String {
    let lower = path.to_lowercase();
    let stem = exe_stem(&lower);
    if let Some(pkg) = package_identity(&lower) {
        return format!("{pkg}/{stem}");
    }
    lower
        .split('\\')
        .filter(|seg| !is_version_dir(seg))
        .collect::<Vec<_>>()
        .join("\\")
}

fn is_generic(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    n.is_empty()
        || n == "electron"
        || n.starts_with("java(tm)")
        || n.starts_with("java platform")
        || n.contains("operating system")
        || n.contains("host process")
}

/// FileDescription, else ProductName, else the exe name.
pub fn choose_name(description: Option<&str>, product: Option<&str>, stem: &str) -> String {
    [description, product]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|n| !is_generic(n))
        .map(str::to_string)
        .unwrap_or_else(|| stem.to_string())
}

pub fn is_frame_host(stem: &str) -> bool {
    stem.eq_ignore_ascii_case("applicationframehost")
}

// ── Windows plumbing ─────────────────────────────────────────────────────

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn path_for_pid(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; MAX_PATH as usize];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Reads one string from the exe's version resource (e.g. "FileDescription").
fn version_string(path: &str, field: &str) -> Option<String> {
    unsafe {
        let wide = to_wide(path);
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(
            PCWSTR(wide.as_ptr()),
            None,
            size,
            buf.as_mut_ptr() as *mut c_void,
        )
        .ok()?;

        let mut p: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let tr = to_wide("\\VarFileInfo\\Translation");
        if !VerQueryValueW(
            buf.as_ptr() as *const c_void,
            PCWSTR(tr.as_ptr()),
            &mut p,
            &mut len,
        )
        .as_bool()
            || len < 4
        {
            return None;
        }
        // Pairs of (language, code page); try each until the field exists.
        let pairs = std::slice::from_raw_parts(p as *const u16, (len / 2) as usize);
        for pair in pairs.as_chunks::<2>().0 {
            let sub = to_wide(&format!(
                "\\StringFileInfo\\{:04x}{:04x}\\{field}",
                pair[0], pair[1]
            ));
            let mut sp: *mut c_void = std::ptr::null_mut();
            let mut slen = 0u32;
            if VerQueryValueW(
                buf.as_ptr() as *const c_void,
                PCWSTR(sub.as_ptr()),
                &mut sp,
                &mut slen,
            )
            .as_bool()
                && slen > 1
            {
                let s = std::slice::from_raw_parts(sp as *const u16, (slen - 1) as usize);
                return Some(String::from_utf16_lossy(s));
            }
        }
        None
    }
}

fn display_name(path: &str) -> String {
    NAMES.with(|cache| {
        cache
            .borrow_mut()
            .entry(path.to_string())
            .or_insert_with(|| {
                choose_name(
                    version_string(path, "FileDescription").as_deref(),
                    version_string(path, "ProductName").as_deref(),
                    &exe_stem(path),
                )
            })
            .clone()
    })
}

struct ChildSearch {
    host_pid: u32,
    found_pid: u32,
}

unsafe extern "system" fn find_core_window(child: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut ChildSearch);
    let mut class = [0u16; 64];
    let n = GetClassNameW(child, &mut class);
    if n > 0 && String::from_utf16_lossy(&class[..n as usize]) == "Windows.UI.Core.CoreWindow" {
        let mut pid = 0u32;
        GetWindowThreadProcessId(child, Some(&mut pid));
        if pid != 0 && pid != search.host_pid {
            search.found_pid = pid;
            return BOOL(0); // stop
        }
    }
    BOOL(1)
}

/// The process that really owns what the user sees. For Store apps this is
/// the CoreWindow's process, not ApplicationFrameHost.
fn real_pid(hwnd: HWND, host_pid: u32) -> u32 {
    let mut search = ChildSearch {
        host_pid,
        found_pid: 0,
    };
    unsafe {
        let _ = EnumChildWindows(
            Some(hwnd),
            Some(find_core_window),
            LPARAM(&mut search as *mut _ as isize),
        );
    }
    if search.found_pid != 0 {
        search.found_pid
    } else {
        host_pid
    }
}

/// InternalGetWindowText never sends WM_GETTEXT, so a hung app cannot block us.
fn title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { InternalGetWindowText(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Identity, name and title of the window, or None for our own process or
/// one whose image cannot be queried (protected processes).
pub fn info_for(hwnd: HWND, own_pid: u32) -> Option<Info> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 || pid == own_pid {
        return None;
    }
    let host_path = path_for_pid(pid)?;
    if is_frame_host(&exe_stem(&host_path)) {
        let real = real_pid(hwnd, pid);
        if real == pid {
            // Suspended or minimized Store app: no CoreWindow to follow.
            return Some(Info {
                key: "pkg:applicationframehost".into(),
                app: "Windows app".into(),
                detail: title(hwnd),
            });
        }
        let path = path_for_pid(real)?;
        return Some(Info {
            key: normalize_key(&path),
            app: display_name(&path),
            detail: title(hwnd),
        });
    }
    Some(Info {
        key: normalize_key(&host_path),
        app: display_name(&host_path),
        detail: title(hwnd),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_stem_strips_directory_and_extension() {
        assert_eq!(
            exe_stem(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
            "firefox"
        );
        assert_eq!(exe_stem("code.EXE"), "code");
        assert_eq!(exe_stem("noext"), "noext");
        assert_eq!(exe_stem("a.b.exe"), "a.b");
    }

    #[test]
    fn squirrel_version_folder_is_removed() {
        let a = normalize_key(r"C:\Users\Me\AppData\Local\slack\app-4.38.121\slack.exe");
        let b = normalize_key(r"C:\Users\Me\AppData\Local\Slack\app-4.39.0\slack.exe");
        assert_eq!(a, b);
        assert_eq!(a, r"c:\users\me\appdata\local\slack\slack.exe");
        // A folder merely starting with "app-" but not a version is kept.
        assert!(normalize_key(r"C:\tools\app-server\run.exe").contains("app-server"));
    }

    #[test]
    fn store_package_version_is_removed() {
        let a = normalize_key(
            r"C:\Program Files\WindowsApps\Claude_5.11492.5.0_x64__pzs8sxrjxfjjc\app\claude.exe",
        );
        let b = normalize_key(
            r"C:\Program Files\WindowsApps\Claude_5.12000.1.0_x64__pzs8sxrjxfjjc\app\claude.exe",
        );
        assert_eq!(a, b);
        assert_eq!(a, "pkg:claude_pzs8sxrjxfjjc/claude");
    }

    #[test]
    fn different_exes_in_one_package_stay_distinct() {
        let a = normalize_key(r"C:\Program Files\WindowsApps\Foo_1.0.0.0_x64__abc\a.exe");
        let b = normalize_key(r"C:\Program Files\WindowsApps\Foo_1.0.0.0_x64__abc\b.exe");
        assert_ne!(a, b);
    }

    #[test]
    fn generic_descriptions_fall_back() {
        assert_eq!(
            choose_name(Some("Electron"), Some("Slack"), "slack"),
            "Slack"
        );
        assert_eq!(
            choose_name(Some("Java(TM) Platform SE binary"), None, "idea64"),
            "idea64"
        );
        assert_eq!(
            choose_name(Some("  Windows Explorer "), None, "explorer"),
            "Windows Explorer"
        );
        assert_eq!(
            choose_name(
                None,
                Some("Microsoft\u{ae} Windows\u{ae} Operating System"),
                "foo"
            ),
            "foo"
        );
        assert_eq!(choose_name(Some(""), Some(""), "tool"), "tool");
    }

    #[test]
    fn frame_host_is_recognized() {
        assert!(is_frame_host("ApplicationFrameHost"));
        assert!(!is_frame_host("calculator"));
    }
}
