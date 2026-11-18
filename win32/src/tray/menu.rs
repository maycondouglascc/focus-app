//! Native Win32 tray context menu (`TrackPopupMenuEx`).

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow,
    TrackPopupMenuEx, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

use crate::db::{Db, TimerStartInput};
use crate::db_thread::DbHandle;
use crate::locale::SystemLocale;
use crate::monitor;
use crate::ui::strings;

const CMD_TOGGLE_TIMER: u32 = 1001;
const CMD_TOGGLE_PAUSE: u32 = 1002;
const CMD_OPEN_APP: u32 = 1003;
const CMD_TOGGLE_MINI: u32 = 1004;
const CMD_QUIT: u32 = 1005;
const CMD_QUICK_NO_PROJECT: u32 = 1999;
const CMD_QUICK_PROJECT_BASE: u32 = 2000;
const MAX_RECENT_PROJECTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSnapshot {
    pub running_label: Option<String>,
    pub recent_projects: Vec<(String, String)>,
    pub monitor_paused: bool,
    pub mini_timer_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    None,
    OpenFlyout,
    StopTimer,
    QuickStartProject(Option<String>),
    TogglePause { next_enabled: bool },
    OpenApp,
    ToggleMiniTimer { next_enabled: bool },
    Quit,
}

impl MenuSnapshot {
    pub fn from_db(db_path: &Path) -> Self {
        let Ok(db) = Db::open_readonly(db_path) else {
            return Self {
                running_label: None,
                recent_projects: Vec::new(),
                monitor_paused: false,
                mini_timer_enabled: false,
            };
        };
        Self::from_open_db(&db)
    }

    pub fn from_open_db(db: &Db) -> Self {
        let locale = SystemLocale::query();
        let settings = db.load_settings(&locale).ok();
        let monitor_paused = settings.as_ref().is_some_and(|s| !s.monitor_enabled);
        let mini_timer_enabled = settings.as_ref().is_some_and(|s| s.mini_timer_enabled);

        let running_label = db.running_entry().ok().flatten().map(|e| {
            let desc = e.description.trim();
            if !desc.is_empty() {
                desc.to_string()
            } else if let Some(pid) = e.project_id.as_deref() {
                db.get_project(pid)
                    .map(|p| p.name)
                    .unwrap_or_else(|_| strings::TRAY_MENU_STOP.to_string())
            } else {
                strings::TRAY_MENU_NO_PROJECTS.to_string()
            }
        });

        let recent_projects = db
            .recent_projects(MAX_RECENT_PROJECTS)
            .map(|ps| ps.into_iter().map(|p| (p.id, p.name)).collect())
            .unwrap_or_default();

        Self {
            running_label,
            recent_projects,
            monitor_paused,
            mini_timer_enabled,
        }
    }

    pub fn timer_menu_label(&self) -> String {
        match &self.running_label {
            Some(label) => format!("{} — {}", strings::TRAY_MENU_STOP, label),
            None => strings::TRAY_MENU_START.to_string(),
        }
    }

    pub fn resolve_command(&self, cmd: u32) -> MenuAction {
        match cmd {
            0 => MenuAction::None,
            CMD_TOGGLE_TIMER => {
                if self.running_label.is_some() {
                    MenuAction::StopTimer
                } else {
                    MenuAction::OpenFlyout
                }
            }
            CMD_TOGGLE_PAUSE => MenuAction::TogglePause {
                next_enabled: self.monitor_paused,
            },
            CMD_OPEN_APP => MenuAction::OpenApp,
            CMD_TOGGLE_MINI => MenuAction::ToggleMiniTimer {
                next_enabled: !self.mini_timer_enabled,
            },
            CMD_QUIT => MenuAction::Quit,
            CMD_QUICK_NO_PROJECT => MenuAction::QuickStartProject(None),
            n if n >= CMD_QUICK_PROJECT_BASE => {
                let idx = (n - CMD_QUICK_PROJECT_BASE) as usize;
                self.recent_projects
                    .get(idx)
                    .map(|(id, _)| MenuAction::QuickStartProject(Some(id.clone())))
                    .unwrap_or(MenuAction::None)
            }
            _ => MenuAction::None,
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    w.push(0);
    w
}

/// Shows the native tray context menu at `anchor_px` (or current cursor) and returns the selected action.
pub fn show_context_menu(
    tray_hwnd: HWND,
    anchor_px: Option<(i32, i32)>,
    db_path: &Path,
) -> MenuAction {
    let snap = MenuSnapshot::from_db(db_path);
    // SAFETY: creates, populates, tracks, and destroys temporary `HMENU`s on the UI thread.
    unsafe {
        let Ok(hmenu) = CreatePopupMenu() else {
            return MenuAction::None;
        };

        let timer_label = to_wide(&snap.timer_menu_label());
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            CMD_TOGGLE_TIMER as usize,
            PCWSTR(timer_label.as_ptr()),
        );

        if let Ok(sub) = CreatePopupMenu() {
            for (i, (_, name)) in snap.recent_projects.iter().enumerate() {
                let wname = to_wide(name);
                let _ = AppendMenuW(
                    sub,
                    MF_STRING,
                    (CMD_QUICK_PROJECT_BASE + i as u32) as usize,
                    PCWSTR(wname.as_ptr()),
                );
            }
            if !snap.recent_projects.is_empty() {
                let _ = AppendMenuW(sub, MF_SEPARATOR, 0, PCWSTR::null());
            }
            let none_w = to_wide(strings::TRAY_MENU_NO_PROJECTS);
            let _ = AppendMenuW(
                sub,
                MF_STRING,
                CMD_QUICK_NO_PROJECT as usize,
                PCWSTR(none_w.as_ptr()),
            );
            let quick_w = to_wide(strings::TRAY_MENU_QUICK_START);
            let _ = AppendMenuW(hmenu, MF_POPUP, sub.0 as usize, PCWSTR(quick_w.as_ptr()));
        }

        let pause_label = if snap.monitor_paused {
            strings::TRAY_MENU_RESUME
        } else {
            strings::TRAY_MENU_PAUSE
        };
        let pause_w = to_wide(pause_label);
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            CMD_TOGGLE_PAUSE as usize,
            PCWSTR(pause_w.as_ptr()),
        );

        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

        let open_w = to_wide(strings::TRAY_MENU_OPEN);
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            CMD_OPEN_APP as usize,
            PCWSTR(open_w.as_ptr()),
        );

        let mini_flags = if snap.mini_timer_enabled {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING | MF_UNCHECKED
        };
        let mini_w = to_wide(strings::TRAY_MENU_MINI_TIMER);
        let _ = AppendMenuW(
            hmenu,
            mini_flags,
            CMD_TOGGLE_MINI as usize,
            PCWSTR(mini_w.as_ptr()),
        );

        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

        let quit_w = to_wide(strings::TRAY_MENU_QUIT);
        let _ = AppendMenuW(hmenu, MF_STRING, CMD_QUIT as usize, PCWSTR(quit_w.as_ptr()));

        let pt = match anchor_px {
            Some((x, y)) => POINT { x, y },
            None => {
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                cur
            }
        };

        // KB135788: foreground tray window + trailing WM_NULL so the menu dismisses cleanly on outside click.
        let _ = SetForegroundWindow(tray_hwnd);
        let cmd = TrackPopupMenuEx(
            hmenu,
            (TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON).0,
            pt.x,
            pt.y,
            tray_hwnd,
            None,
        )
        .0 as u32;
        let _ = PostMessageW(Some(tray_hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(hmenu);

        snap.resolve_command(cmd)
    }
}

/// Applies menu DB mutations through `DbHandle` (UI/tray updates happen via caller/notify).
pub fn execute_db_action(action: &MenuAction, db_handle: &DbHandle, now_ms: i64) -> bool {
    let locale = SystemLocale::query();
    match action {
        MenuAction::StopTimer => db_handle.stop_timers(now_ms).is_ok(),
        MenuAction::QuickStartProject(project_id) => db_handle
            .start_timer(
                TimerStartInput {
                    project_id: project_id.clone(),
                    description: String::new(),
                    billable: true,
                },
                now_ms,
            )
            .is_ok(),
        MenuAction::TogglePause { next_enabled } => {
            let patch = crate::db::SettingsPatch {
                monitor_enabled: Some(*next_enabled),
                ..Default::default()
            };
            let ok = db_handle.save_settings(patch, locale).is_ok();
            if ok {
                monitor::request_reload();
            }
            ok
        }
        MenuAction::ToggleMiniTimer { next_enabled } => {
            let patch = crate::db::SettingsPatch {
                mini_timer_enabled: Some(*next_enabled),
                ..Default::default()
            };
            db_handle.save_settings(patch, locale).is_ok()
        }
        _ => false,
    }
}

/// Headless exercise for `--tray-cycle`: builds & immediately destroys native HMENU handles cleanly.
pub fn exercise_menu_handles(snap: &MenuSnapshot) {
    // SAFETY: creates and destroys a temporary `HMENU` without entering modal tracking.
    unsafe {
        if let Ok(hmenu) = CreatePopupMenu() {
            let timer_w = to_wide(&snap.timer_menu_label());
            let _ = AppendMenuW(
                hmenu,
                MF_STRING,
                CMD_TOGGLE_TIMER as usize,
                PCWSTR(timer_w.as_ptr()),
            );
            if let Ok(sub) = CreatePopupMenu() {
                for (i, (_, name)) in snap.recent_projects.iter().enumerate() {
                    let wname = to_wide(name);
                    let _ = AppendMenuW(
                        sub,
                        MF_STRING,
                        (CMD_QUICK_PROJECT_BASE + i as u32) as usize,
                        PCWSTR(wname.as_ptr()),
                    );
                }
                let quick_w = to_wide(strings::TRAY_MENU_QUICK_START);
                let _ = AppendMenuW(hmenu, MF_POPUP, sub.0 as usize, PCWSTR(quick_w.as_ptr()));
            }
            let _ = DestroyMenu(hmenu);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{EntryInput, ProjectInput};

    #[test]
    fn menu_snapshot_caps_recent_projects_at_five_and_maps_commands() {
        let db = Db::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for i in 1..=7 {
            let p = db
                .create_project(
                    &ProjectInput {
                        name: format!("Project {i}"),
                        client: String::new(),
                        color: "#3b7ddd".into(),
                        rate: 0.0,
                        currency: "USD".into(),
                        billable_default: true,
                        archived: false,
                    },
                    1000 + i as i64,
                )
                .unwrap();
            ids.push(p.id);
        }
        // Make Project 2 the most recent entry.
        db.create_entry(&EntryInput {
            project_id: Some(ids[1].clone()),
            description: "Task".into(),
            start: 9_000_000,
            end: Some(9_060_000),
            billable: true,
            source: "manual".into(),
        })
        .unwrap();
        db.start_timer(
            &TimerStartInput {
                project_id: Some(ids[1].clone()),
                description: "Writing docs".into(),
                billable: true,
            },
            9_100_000,
        )
        .unwrap();

        let snap = MenuSnapshot::from_open_db(&db);
        assert_eq!(snap.running_label.as_deref(), Some("Writing docs"));
        assert_eq!(snap.recent_projects.len(), 5);
        assert_eq!(snap.recent_projects[0].0, ids[1]);
        assert_eq!(
            snap.resolve_command(CMD_TOGGLE_TIMER),
            MenuAction::StopTimer
        );
        assert_eq!(
            snap.resolve_command(CMD_QUICK_PROJECT_BASE),
            MenuAction::QuickStartProject(Some(ids[1].clone()))
        );
        assert_eq!(
            snap.resolve_command(CMD_QUICK_NO_PROJECT),
            MenuAction::QuickStartProject(None)
        );
    }
}
