//! Claude Code session logs (`<root>\<project>\<session>.jsonl`) -> activity.
//!
//! Same rules as the Electron app: messages less than GAP apart belong to one
//! run, a run's end gets TAIL after its last message, 90 days of history on the
//! first scan, sessions started by programs (Agent SDK) are never recorded.
//! Files are read incrementally by byte offset and never locked: they are
//! opened with every share mode so Claude Code can keep writing, renaming and
//! deleting them.
//!
//! Trigger: the Windows change-notification handle on the projects folder,
//! throttled to one scan every 15 s, plus a slow 5 min safety net. It replaces
//! the old 60 s polling.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, WaitForSingleObject, THREAD_MODE_BACKGROUND_BEGIN,
};

use crate::db::{Activity, ClaudeSession, Db};

const GAP_MS: i64 = 10 * 60_000;
/// A run's end gets this much after its last message: reading time after a reply.
const TAIL_MS: i64 = 60_000;
const BACKFILL_DAYS: i64 = 90;
/// After a notification, wait this long so a burst of writes is one read.
const THROTTLE: Duration = Duration::from_secs(15);
/// Safety net in case a notification is missed (cached writes can be late).
const SAFETY_NET_MS: u32 = 5 * 60_000;
/// How often to look again when there is no log folder or the feature is off.
const IDLE_POLL: Duration = Duration::from_secs(5 * 60);
const CHUNK: usize = 1 << 20;
const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4; // read | write | delete

#[derive(Debug)]
pub enum ScanError {
    Io(io::Error),
    Db(rusqlite::Error),
}

impl From<io::Error> for ScanError {
    fn from(e: io::Error) -> Self {
        ScanError::Io(e)
    }
}
impl From<rusqlite::Error> for ScanError {
    fn from(e: rusqlite::Error) -> Self {
        ScanError::Db(e)
    }
}
impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::Io(e) => write!(f, "io: {e}"),
            ScanError::Db(e) => write!(f, "db: {e}"),
        }
    }
}

// ── Line parsing (pure) ──────────────────────────────────────────────────

/// Raw (still JSON-escaped) value of `"name":"..."`, or empty. Scans the text
/// instead of parsing it: some lines are megabytes (tool output, images).
fn field(line: &str, name: &str) -> String {
    let needle = format!("\"{name}\":\"");
    let Some(i) = line.find(&needle) else {
        return String::new();
    };
    let rest = &line[i + needle.len()..];
    let b = rest.as_bytes();
    let mut j = 0;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return rest[..j].to_string(),
            _ => j += 1,
        }
    }
    String::new()
}

fn unescape(raw: &str) -> String {
    serde_json::from_str::<String>(&format!("\"{raw}\"")).unwrap_or_else(|_| raw.to_string())
}

/// Removes `<tag>...</tag>` blocks, collapses whitespace, caps at 90 chars.
fn clean_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let block_end = after.find('>').filter(|&g| g > 1).and_then(|g| {
            let tail = &after[g + 1..];
            let k = tail.find("</")?;
            let l = tail[k + 2..].find('>').filter(|&l| l > 0)?;
            Some(g + 1 + k + 2 + l + 1)
        });
        match block_end {
            Some(end) => {
                out.push(' ');
                rest = &after[end..];
            }
            None => {
                out.push('<');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(90)
        .collect()
}

fn first_text(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .find(|c| c.get("type").and_then(|t| t.as_str()) == Some("text"))
            .and_then(|c| c.get("text").and_then(|t| t.as_str()))
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "2026-11-16T15:22:19.123Z" (or with a +hh:mm offset) -> epoch ms.
fn parse_iso8601_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut i = 19;
    let mut ms = 0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let digits = &s[start..i];
        let three = format!("{:0<3}", &digits[..digits.len().min(3)]);
        ms = three.parse::<i64>().ok()?;
    }
    let offset_min = match b.get(i) {
        Some(b'Z') | None => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let oh = num(i + 1, i + 3)?;
            let om = num(i + 4, i + 6).unwrap_or(0);
            let v = oh * 60 + om;
            if sign == b'-' {
                -v
            } else {
                v
            }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    Some(((days * 24 + h) * 60 + mi - offset_min) * 60_000 + sec * 1000 + ms)
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn basename(cwd: &str) -> Option<String> {
    cwd.rsplit(['\\', '/'])
        .find(|p| !p.is_empty())
        .map(str::to_string)
}

/// Applies one log line. Returns true if the activity stream changed.
pub fn apply_line(
    line: &str,
    sessions: &mut HashMap<String, ClaudeSession>,
    db: &Db,
) -> rusqlite::Result<bool> {
    if line.len() < 20 {
        return Ok(false);
    }
    // Titles are written by Claude Code as their own lines.
    if line.contains(r#""type":"custom-title""#)
        || line.contains(r#""type":"ai-title""#)
        || line.contains(r#""type":"summary""#)
    {
        let sid = field(line, "sessionId");
        let raw = ["customTitle", "aiTitle", "title", "summary"]
            .iter()
            .map(|f| field(line, f))
            .find(|v| !v.is_empty())
            .unwrap_or_default();
        if !sid.is_empty() && !raw.is_empty() {
            if !sessions.contains_key(&sid) {
                if let Some(s) = db.claude_session(&sid)? {
                    sessions.insert(sid.clone(), s);
                }
            }
            if let Some(s) = sessions.get_mut(&sid) {
                s.title = clean_title(&unescape(&raw));
                if let Some(id) = s.activity_id {
                    db.set_activity_detail(id, &s.title)?;
                }
            }
        }
        return Ok(false);
    }

    let head = &line[..floor_boundary(line, 400)];
    let is_user = head.contains(r#""type":"user""#);
    let is_msg = is_user
        || head.contains(r#""type":"assistant""#)
        || line.contains(r#""type":"user","message""#)
        || line.contains(r#""type":"assistant","message""#);
    if !is_msg || line.contains(r#""isSidechain":true"#) {
        return Ok(false);
    }
    let ts_raw = field(line, "timestamp");
    let sid = field(line, "sessionId");
    if ts_raw.is_empty() || sid.is_empty() {
        return Ok(false);
    }
    let Some(ts) = parse_iso8601_ms(&ts_raw) else {
        return Ok(false);
    };
    let cwd = unescape(&field(line, "cwd"));
    let origin = field(line, "turnOrigin");

    let mut s = match sessions.remove(&sid) {
        Some(s) => s,
        None => match db.claude_session(&sid)? {
            Some(s) => s,
            // Decided by the first prompt: SDK sessions mark it turnOrigin "sdk";
            // yours say "human". Older CLIs write no marker, and those were typed by hand.
            None => ClaudeSession {
                automated: !origin.is_empty() && origin != "human",
                project: basename(&cwd).unwrap_or_else(|| "Claude Code".into()),
                ..Default::default()
            },
        },
    };
    if s.automated && origin == "human" {
        s.automated = false; // you took over a session a program started
    }
    let changed = if s.automated {
        false
    } else {
        record(&mut s, line, head, is_user, ts, db)?
    };
    sessions.insert(sid, s);
    Ok(changed)
}

fn record(
    s: &mut ClaudeSession,
    line: &str,
    head: &str,
    is_user: bool,
    ts: i64,
    db: &Db,
) -> rusqlite::Result<bool> {
    if s.title.is_empty() && line.len() < 60_000 && is_user && head.contains(r#""type":"user""#) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let t = clean_title(&first_text(v.pointer("/message/content")));
            if !t.is_empty() && !t.starts_with("[Request interrupted") {
                s.title = t;
            }
        }
    }
    if ts <= s.last_ts {
        return Ok(false);
    }
    match s.activity_id {
        Some(id) if ts - s.last_ts <= GAP_MS => db.set_activity_end(id, ts + TAIL_MS)?,
        _ => {
            s.activity_id = Some(db.insert_activity(&Activity {
                id: 0,
                kind: "claude".into(),
                app: "Claude Code".into(),
                bundle_id: unescape(&field(line, "entrypoint")),
                detail: s.title.clone(),
                context: s.project.clone(),
                url: String::new(),
                start: ts,
                end: ts + TAIL_MS,
            })?);
        }
    }
    if let (false, Some(id)) = (s.title.is_empty(), s.activity_id) {
        db.set_activity_detail(id, &s.title)?;
    }
    s.last_ts = ts;
    Ok(true)
}

// ── File scanning ────────────────────────────────────────────────────────

/// Calls `f` for every complete line in [from, size) and returns the byte
/// offset just after the last one. A final line without `\n` is still being
/// written and is left for the next scan.
fn read_lines<E: From<io::Error>>(
    path: &Path,
    from: u64,
    size: u64,
    mut f: impl FnMut(&str) -> Result<(), E>,
) -> Result<u64, E> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_ALL)
        .open(path)?;
    file.seek(SeekFrom::Start(from))?;
    let mut remaining = size - from;
    let mut chunk = vec![0u8; CHUNK];
    let mut carry: Vec<u8> = Vec::new();
    let mut consumed = from;
    while remaining > 0 {
        let want = (remaining as usize).min(CHUNK);
        let n = file.read(&mut chunk[..want])?;
        if n == 0 {
            break;
        }
        remaining -= n as u64;
        carry.extend_from_slice(&chunk[..n]);
        let mut start = 0;
        while let Some(pos) = carry[start..].iter().position(|&b| b == b'\n') {
            let mut line = &carry[start..start + pos];
            consumed += pos as u64 + 1;
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            f(&String::from_utf8_lossy(line))?;
            start += pos + 1;
        }
        carry.drain(..start);
    }
    Ok(consumed)
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn scan_file(db: &Db, path: &Path, size: u64, mtime: i64, cutoff: i64) -> Result<bool, ScanError> {
    let key = path.to_string_lossy().to_string();
    let prev = db.claude_file(&key)?;
    if let Some((off, mt)) = prev {
        if mt == mtime && off >= size as i64 {
            return Ok(false);
        }
    }
    if prev.is_none() && mtime < cutoff {
        db.save_claude_file(&key, size as i64, mtime)?; // too old: remember, never read
        return Ok(false);
    }
    // A file that shrank was rewritten: read it again from the top.
    let from = match prev {
        Some((off, _)) if off <= size as i64 => off as u64,
        _ => 0,
    };
    if from >= size {
        db.save_claude_file(&key, from as i64, mtime)?;
        return Ok(false);
    }
    let mut sessions = HashMap::new();
    let mut changed = false;
    db.in_transaction(|| -> Result<(), ScanError> {
        let offset = read_lines(path, from, size, |line| -> Result<(), ScanError> {
            changed |= apply_line(line, &mut sessions, db)?;
            Ok(())
        })?;
        for (id, s) in &sessions {
            db.save_claude_session(id, s)?;
        }
        db.save_claude_file(&key, offset as i64, mtime)?;
        Ok(())
    })?;
    Ok(changed)
}

/// Reads whatever was appended to every `*.jsonl` under `root`.
pub fn scan(db: &Db, root: &Path, now_ms: i64) -> Result<bool, ScanError> {
    let cutoff = now_ms - BACKFILL_DAYS * 86_400_000;
    let mut changed = false;
    for dir in std::fs::read_dir(root)?.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = f.metadata() else { continue };
            // One unreadable file must not stop the rest of the scan.
            match scan_file(db, &path, meta.len(), mtime_ms(&meta), cutoff) {
                Ok(c) => changed |= c,
                Err(e) => eprintln!("focus: claude log skipped: {e}"),
            }
        }
    }
    Ok(changed)
}

// ── Background thread ────────────────────────────────────────────────────

fn claude_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir).join("projects");
    }
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".claude").join("projects")
}

fn enabled(db: &Db) -> bool {
    !matches!(db.get_setting("claudeSessions"), Ok(Some(v)) if v.trim() == "false")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Blocks until something under `root` changes (or the safety net expires).
/// The handle stays signalled across changes that happen while we scan, so
/// nothing is lost between a scan and the next wait.
struct Watch(windows::Win32::Foundation::HANDLE);

impl Watch {
    fn new(root: &Path) -> Option<Watch> {
        let wide: Vec<u16> = root
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let h = unsafe {
            FindFirstChangeNotificationW(
                PCWSTR(wide.as_ptr()),
                true,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE,
            )
        }
        .ok()?;
        Some(Watch(h))
    }

    /// True if signalled, false on the safety-net timeout.
    fn wait(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, SAFETY_NET_MS) == WAIT_OBJECT_0 }
    }

    /// Re-arm before scanning, so writes during the scan signal again.
    fn rearm(&self) {
        unsafe {
            let _ = FindNextChangeNotification(self.0);
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            let _ = FindCloseChangeNotification(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

fn run(db_path: PathBuf) {
    // Low CPU, disk and memory priority for this thread's whole life.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
    let db = match Db::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("focus: claude reader cannot open the database: {e}");
            return;
        }
    };
    loop {
        let root = claude_root();
        if !enabled(&db) || !root.is_dir() {
            std::thread::sleep(IDLE_POLL);
            continue;
        }
        let Some(watch) = Watch::new(&root) else {
            std::thread::sleep(IDLE_POLL);
            continue;
        };
        loop {
            watch.rearm();
            if let Err(e) = scan(&db, &root, now_ms()) {
                eprintln!("focus: claude scan failed: {e}");
            }
            if !root.is_dir() || !enabled(&db) {
                break;
            }
            if watch.wait() {
                std::thread::sleep(THROTTLE); // let a burst of writes settle
            }
        }
    }
}

/// Starts the reader on its own thread with its own connection (WAL lets it
/// write next to the monitor).
pub fn spawn(db_path: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("claude-logs".into())
        .spawn(move || run(db_path));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn user(sid: &str, ts: &str, origin: &str, text: &str) -> String {
        format!(
            r#"{{"type":"user","sessionId":"{sid}","timestamp":"{ts}","cwd":"C:\\Users\\me\\trackerApp","turnOrigin":"{origin}","entrypoint":"cli","message":{{"role":"user","content":"{text}"}}}}"#
        )
    }
    fn assistant(sid: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"{sid}","timestamp":"{ts}","cwd":"C:\\Users\\me\\trackerApp","message":{{"role":"assistant","content":[]}}}}"#
        )
    }
    fn runs(db: &Db) -> Vec<Activity> {
        db.list_activity(0, i64::MAX / 4).unwrap()
    }

    #[test]
    fn parses_iso_timestamps() {
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso8601_ms("2026-11-16T15:22:19.123Z"),
            Some(1_794_842_539_123)
        );
        assert_eq!(
            parse_iso8601_ms("2000-02-29T23:59:59Z"),
            Some(951_868_799_000)
        );
        assert_eq!(
            parse_iso8601_ms("2026-11-16T12:22:19.123-03:00"),
            Some(1_794_842_539_123)
        );
        assert_eq!(
            parse_iso8601_ms("2026-11-16T15:22:19.5Z"),
            Some(1_794_842_539_500)
        );
        assert_eq!(parse_iso8601_ms("garbage"), None);
    }

    #[test]
    fn extracts_escaped_fields() {
        let l = r#"{"a":"x","cwd":"C:\\Users\\me \"q\"","b":"y"}"#;
        assert_eq!(unescape(&field(l, "cwd")), r#"C:\Users\me "q""#);
        assert_eq!(field(l, "missing"), "");
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(
            clean_title("<ide>junk</ide>  Fix   the\n bug"),
            "Fix the bug"
        );
        assert_eq!(clean_title("a < b and c > d"), "a < b and c > d");
        assert_eq!(clean_title(&"x".repeat(200)).chars().count(), 90);
    }

    #[test]
    fn messages_within_the_gap_form_one_run_with_a_tail() {
        let db = Db::open_in_memory().unwrap();
        let mut s = HashMap::new();
        for l in [
            user("s1", "2026-11-16T15:00:00.000Z", "human", "Fix the bug"),
            assistant("s1", "2026-11-16T15:03:00.000Z"),
            assistant("s1", "2026-11-16T15:08:00.000Z"),
        ] {
            assert!(apply_line(&l, &mut s, &db).unwrap());
        }
        let r = runs(&db);
        assert_eq!(r.len(), 1);
        let start = parse_iso8601_ms("2026-11-16T15:00:00Z").unwrap();
        assert_eq!(
            (r[0].start, r[0].end),
            (start, start + 8 * 60_000 + TAIL_MS)
        );
        assert_eq!(
            (
                r[0].app.as_str(),
                r[0].context.as_str(),
                r[0].detail.as_str()
            ),
            ("Claude Code", "trackerApp", "Fix the bug")
        );
        assert_eq!(r[0].bundle_id, "cli");
    }

    #[test]
    fn a_long_pause_starts_a_new_run() {
        let db = Db::open_in_memory().unwrap();
        let mut s = HashMap::new();
        apply_line(
            &user("s1", "2026-11-16T15:00:00Z", "human", "Hi"),
            &mut s,
            &db,
        )
        .unwrap();
        apply_line(&assistant("s1", "2026-11-16T15:11:00Z"), &mut s, &db).unwrap();
        assert_eq!(runs(&db).len(), 2);
    }

    #[test]
    fn automated_sessions_are_ignored_until_a_human_takes_over() {
        let db = Db::open_in_memory().unwrap();
        let mut s = HashMap::new();
        assert!(!apply_line(
            &user("a", "2026-11-16T15:00:00Z", "sdk", "robot"),
            &mut s,
            &db
        )
        .unwrap());
        assert!(!apply_line(&assistant("a", "2026-11-16T15:01:00Z"), &mut s, &db).unwrap());
        assert!(runs(&db).is_empty());
        assert!(apply_line(
            &user("a", "2026-11-16T15:02:00Z", "human", "me now"),
            &mut s,
            &db
        )
        .unwrap());
        assert_eq!(runs(&db).len(), 1);
    }

    #[test]
    fn sidechains_duplicates_and_old_timestamps_are_skipped() {
        let db = Db::open_in_memory().unwrap();
        let mut s = HashMap::new();
        apply_line(
            &user("s1", "2026-11-16T15:00:00Z", "human", "Hi"),
            &mut s,
            &db,
        )
        .unwrap();
        let side = assistant("s1", "2026-11-16T15:01:00Z")
            .replace("\"message\"", "\"isSidechain\":true,\"message\"");
        assert!(!apply_line(&side, &mut s, &db).unwrap());
        assert!(!apply_line(
            &user("s1", "2026-11-16T15:00:00Z", "human", "Hi"),
            &mut s,
            &db
        )
        .unwrap());
        assert!(!apply_line(&assistant("s1", "2026-11-16T14:59:00Z"), &mut s, &db).unwrap());
        assert_eq!(runs(&db).len(), 1);
    }

    #[test]
    fn a_title_line_renames_the_run() {
        let db = Db::open_in_memory().unwrap();
        let mut s = HashMap::new();
        apply_line(
            &user("s1", "2026-11-16T15:00:00Z", "human", "first prompt"),
            &mut s,
            &db,
        )
        .unwrap();
        let t = r#"{"type":"ai-title","sessionId":"s1","aiTitle":"Better name here"}"#;
        assert!(!apply_line(t, &mut s, &db).unwrap());
        assert_eq!(runs(&db)[0].detail, "Better name here");
    }

    #[test]
    fn scans_incrementally_and_waits_for_a_complete_line() {
        let dir = std::env::temp_dir().join(format!("focus-claude-test-{}", std::process::id()));
        let proj = dir.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let file = proj.join("s1.jsonl");
        // Fresh timestamps inside the backfill window relative to `now`.
        let l1 = user("s1", "2026-11-16T15:00:00Z", "human", "Hi");
        std::fs::write(&file, format!("{l1}\n")).unwrap();
        let db = Db::open_in_memory().unwrap();
        // File mtime is "now" in real time, so use a real-time `now` for the cutoff.
        let real_now = super::now_ms();
        assert!(scan(&db, &dir, real_now).unwrap());
        assert_eq!(runs(&db).len(), 1);
        // Nothing new: no change.
        assert!(!scan(&db, &dir, real_now).unwrap());
        // Append one full line and one unterminated line.
        let l2 = assistant("s1", "2026-11-16T15:02:00Z");
        let mut f = OpenOptions::new().append(true).open(&file).unwrap();
        write!(f, "{l2}\n{}", assistant("s1", "2026-11-16T15:04:00Z")).unwrap();
        drop(f);
        assert!(scan(&db, &dir, real_now).unwrap());
        let start = parse_iso8601_ms("2026-11-16T15:00:00Z").unwrap();
        assert_eq!(runs(&db)[0].end, start + 2 * 60_000 + TAIL_MS);
        // Finish the partial line: it is read now, exactly once.
        let mut f = OpenOptions::new().append(true).open(&file).unwrap();
        writeln!(f).unwrap();
        drop(f);
        assert!(scan(&db, &dir, real_now).unwrap());
        assert_eq!(runs(&db)[0].end, start + 4 * 60_000 + TAIL_MS);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
