//! Dedicated SQLite writer thread (`focus-db`) owning the single write connection.
//!
//! Background monitor runs are queued fire-and-forget via monotonic local tokens so
//! slow disk writes never stall WinEvent debouncing. UI and Claude batch writes route
//! through the same writer so there is never multi-writer contention in WAL mode.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::ErrorCode;

use crate::backup::{self, HousekeepingReport};
use crate::db::{
    Activity, ClaudeSession, Db, EntryInput, EntryPatch, Project, ProjectInput, ProjectPatch,
    Settings, SettingsPatch, TimeEntry, TimerStartInput,
};
use crate::locale::SystemLocale;
use crate::log::{self, Category};
use crate::qos;
use crate::recorder::ActivitySink;

const TOKEN_CACHE_CAP: usize = 256;

#[derive(Debug, Clone)]
pub enum ClaudeOp {
    InsertActivity { temp_id: i64, activity: Activity },
    SetActivityEnd { id: i64, end: i64 },
    SetActivityDetail { id: i64, detail: String },
}

#[derive(Debug, Clone)]
pub struct ClaudeBatch {
    pub ops: Vec<ClaudeOp>,
    pub sessions: Vec<(String, ClaudeSession)>,
    pub file_key: String,
    pub offset: i64,
    pub mtime: i64,
}

type Reply<T> = mpsc::SyncSender<Result<T, String>>;
type ErrorCallbackHolder = std::sync::Mutex<Option<Box<dyn Fn(bool) + Send + Sync>>>;

pub enum DbCmd {
    // Monitor (fire-and-forget via token)
    OpenRun {
        token: i64,
        activity: Activity,
    },
    SetRunEnd {
        token: i64,
        end: i64,
    },
    SetRunUrl {
        token: i64,
        url: String,
        context: String,
    },
    // Claude Code reader batch
    ApplyClaudeBatch {
        batch: ClaudeBatch,
        reply: Reply<()>,
    },
    // UI & Tray mutations
    CreateProject {
        input: ProjectInput,
        now_ms: i64,
        reply: Reply<Project>,
    },
    UpdateProject {
        id: String,
        patch: ProjectPatch,
        reply: Reply<Project>,
    },
    DeleteProject {
        id: String,
        reply: Reply<()>,
    },
    CreateEntry {
        input: EntryInput,
        reply: Reply<TimeEntry>,
    },
    UpdateEntry {
        id: String,
        patch: EntryPatch,
        reply: Reply<TimeEntry>,
    },
    DeleteEntry {
        id: String,
        reply: Reply<()>,
    },
    StartTimer {
        input: TimerStartInput,
        now_ms: i64,
        reply: Reply<TimeEntry>,
    },
    StopTimers {
        now_ms: i64,
        reply: Reply<Option<TimeEntry>>,
    },
    ClearActivity {
        reply: Reply<()>,
    },
    SaveSettings {
        patch: SettingsPatch,
        locale: SystemLocale,
        reply: Reply<Settings>,
    },
    // Maintenance & lifecycle
    Checkpoint {
        reply: Reply<()>,
    },
    ProbeWrite {
        reply: Reply<bool>,
    },
    RunHousekeeping {
        backup_dir: PathBuf,
        now_ms: i64,
        reply: Reply<HousekeepingReport>,
    },
    #[cfg(test)]
    SleepForTest {
        duration: Duration,
    },
    Shutdown {
        reply: mpsc::SyncSender<()>,
    },
}

#[derive(Clone)]
pub struct DbHandle {
    tx: mpsc::Sender<DbCmd>,
    next_token: Arc<AtomicI64>,
    in_error: Arc<AtomicBool>,
    on_error_change: Arc<ErrorCallbackHolder>,
}

impl DbHandle {
    pub fn next_run_token(&self) -> i64 {
        self.next_token.fetch_add(1, Ordering::SeqCst)
    }

    pub fn open_run(&self, token: i64, activity: Activity) {
        let _ = self.tx.send(DbCmd::OpenRun { token, activity });
    }

    pub fn set_run_end(&self, token: i64, end: i64) {
        let _ = self.tx.send(DbCmd::SetRunEnd { token, end });
    }

    pub fn set_run_url(&self, token: i64, url: String, context: String) {
        let _ = self.tx.send(DbCmd::SetRunUrl {
            token,
            url,
            context,
        });
    }

    pub fn apply_claude_batch(&self, batch: ClaudeBatch) -> Result<(), String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.tx
            .send(DbCmd::ApplyClaudeBatch { batch, reply })
            .map_err(|_| "db thread stopped".to_string())?;
        rx.recv()
            .map_err(|_| "db thread dropped reply".to_string())?
    }

    pub fn create_project(&self, input: ProjectInput, now_ms: i64) -> Result<Project, String> {
        self.call(|reply| DbCmd::CreateProject {
            input,
            now_ms,
            reply,
        })
    }

    pub fn update_project(&self, id: String, patch: ProjectPatch) -> Result<Project, String> {
        self.call(|reply| DbCmd::UpdateProject { id, patch, reply })
    }

    pub fn delete_project(&self, id: String) -> Result<(), String> {
        self.call(|reply| DbCmd::DeleteProject { id, reply })
    }

    pub fn create_entry(&self, input: EntryInput) -> Result<TimeEntry, String> {
        self.call(|reply| DbCmd::CreateEntry { input, reply })
    }

    pub fn update_entry(&self, id: String, patch: EntryPatch) -> Result<TimeEntry, String> {
        self.call(|reply| DbCmd::UpdateEntry { id, patch, reply })
    }

    pub fn delete_entry(&self, id: String) -> Result<(), String> {
        self.call(|reply| DbCmd::DeleteEntry { id, reply })
    }

    pub fn start_timer(&self, input: TimerStartInput, now_ms: i64) -> Result<TimeEntry, String> {
        self.call(|reply| DbCmd::StartTimer {
            input,
            now_ms,
            reply,
        })
    }

    pub fn stop_timers(&self, now_ms: i64) -> Result<Option<TimeEntry>, String> {
        self.call(|reply| DbCmd::StopTimers { now_ms, reply })
    }

    pub fn clear_activity(&self) -> Result<(), String> {
        self.call(|reply| DbCmd::ClearActivity { reply })
    }

    pub fn save_settings(
        &self,
        patch: SettingsPatch,
        locale: SystemLocale,
    ) -> Result<Settings, String> {
        self.call(|reply| DbCmd::SaveSettings {
            patch,
            locale,
            reply,
        })
    }

    pub fn checkpoint(&self) -> Result<(), String> {
        self.call(|reply| DbCmd::Checkpoint { reply })
    }

    pub fn probe_write(&self) -> Result<bool, String> {
        self.call(|reply| DbCmd::ProbeWrite { reply })
    }

    pub fn run_housekeeping(
        &self,
        backup_dir: PathBuf,
        now_ms: i64,
    ) -> Result<HousekeepingReport, String> {
        self.call(|reply| DbCmd::RunHousekeeping {
            backup_dir,
            now_ms,
            reply,
        })
    }

    pub fn is_in_error(&self) -> bool {
        self.in_error.load(Ordering::SeqCst)
    }

    pub fn set_error_callback(&self, cb: impl Fn(bool) + Send + Sync + 'static) {
        if let Ok(mut g) = self.on_error_change.lock() {
            *g = Some(Box::new(cb));
        }
    }

    #[cfg(test)]
    pub fn sleep_for_test(&self, duration: Duration) {
        let _ = self.tx.send(DbCmd::SleepForTest { duration });
    }

    fn call<T>(&self, build: impl FnOnce(Reply<T>) -> DbCmd) -> Result<T, String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.tx
            .send(build(reply))
            .map_err(|_| "db thread stopped".to_string())?;
        rx.recv()
            .map_err(|_| "db thread dropped reply".to_string())?
    }
}

/// Non-blocking `ActivitySink` backed by `DbHandle`.
#[derive(Clone)]
pub struct QueuedSink {
    handle: DbHandle,
}

impl QueuedSink {
    pub fn new(handle: DbHandle) -> Self {
        Self { handle }
    }
}

impl ActivitySink for QueuedSink {
    fn insert_activity(&self, a: &Activity) -> rusqlite::Result<i64> {
        let token = self.handle.next_run_token();
        self.handle.open_run(token, a.clone());
        Ok(token)
    }

    fn set_activity_end(&self, id: i64, end: i64) -> rusqlite::Result<()> {
        self.handle.set_run_end(id, end);
        Ok(())
    }
}

pub struct DbThread {
    handle: DbHandle,
    join: Option<JoinHandle<()>>,
}

impl DbThread {
    pub fn spawn(db_path: PathBuf) -> Result<Self, String> {
        Self::spawn_with(move || {
            if let Some(dir) = db_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            Db::open(&db_path).map_err(|e| {
                log::db_err(Category::Db, "open_db", &e);
                e.to_string()
            })
        })
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn spawn_in_memory() -> Result<Self, String> {
        Self::spawn_with(|| Db::open_in_memory().map_err(|e| e.to_string()))
    }

    fn spawn_with(
        open_fn: impl FnOnce() -> Result<Db, String> + Send + 'static,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let in_error = Arc::new(AtomicBool::new(false));
        let on_error_change = Arc::new(std::sync::Mutex::new(None));
        let in_err_clone = Arc::clone(&in_error);
        let cb_clone = Arc::clone(&on_error_change);

        let join = std::thread::Builder::new()
            .name("focus-db".into())
            .spawn(move || {
                qos::set_background_qos(true);
                let db = match open_fn() {
                    Ok(db) => {
                        let _ = ready_tx.send(Ok(()));
                        db
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                run_loop(db, rx, in_err_clone, cb_clone);
            })
            .map_err(|e| format!("failed to spawn db thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                handle: DbHandle {
                    tx,
                    next_token: Arc::new(AtomicI64::new(1)),
                    in_error,
                    on_error_change,
                },
                join: Some(join),
            }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => {
                let _ = join.join();
                Err("db thread exited during startup".into())
            }
        }
    }

    pub fn handle(&self) -> DbHandle {
        self.handle.clone()
    }

    pub fn stop(&mut self) {
        if let Some(j) = self.join.take() {
            let (reply, rx) = mpsc::sync_channel(1);
            if self.handle.tx.send(DbCmd::Shutdown { reply }).is_ok() {
                let _ = rx.recv_timeout(Duration::from_secs(4));
            }
            let _ = j.join();
        }
    }
}

impl Drop for DbThread {
    fn drop(&mut self) {
        self.stop();
    }
}

fn is_transient_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(f.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn is_fatal_storage_error(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(
                f.code,
                ErrorCode::DiskFull
                    | ErrorCode::SystemIoFailure
                    | ErrorCode::CannotOpen
                    | ErrorCode::ReadOnly
                    | ErrorCode::DatabaseCorrupt
                    | ErrorCode::NotADatabase
            )
    )
}

fn retry_busy<T>(mut op: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    let mut attempts = 0u32;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if is_transient_busy(&e) && attempts < 3 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(20 * attempts as u64));
            }
            Err(e) => return Err(e),
        }
    }
}

struct TokenMap {
    map: HashMap<i64, i64>,
    order: VecDeque<i64>,
}

impl TokenMap {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn insert(&mut self, token: i64, row_id: i64) {
        if !self.map.contains_key(&token) {
            self.order.push_back(token);
        }
        self.map.insert(token, row_id);
        while self.order.len() > TOKEN_CACHE_CAP {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    fn get(&self, token: i64) -> Option<i64> {
        self.map.get(&token).copied()
    }
}

fn set_error_state(in_error: &AtomicBool, cb_holder: &ErrorCallbackHolder, errored: bool) {
    let prev = in_error.swap(errored, Ordering::SeqCst);
    if prev != errored {
        if let Ok(g) = cb_holder.lock() {
            if let Some(ref cb) = *g {
                cb(errored);
            }
        }
    }
}

fn apply_claude_batch_tx(db: &Db, batch: ClaudeBatch) -> rusqlite::Result<()> {
    db.in_transaction(|| -> rusqlite::Result<()> {
        let mut temp_to_real: HashMap<i64, i64> = HashMap::new();
        for op in batch.ops {
            match op {
                ClaudeOp::InsertActivity { temp_id, activity } => {
                    let row_id = db.insert_activity(&activity)?;
                    temp_to_real.insert(temp_id, row_id);
                }
                ClaudeOp::SetActivityEnd { id, end } => {
                    let real_id = if id < 0 {
                        *temp_to_real.get(&id).unwrap_or(&id)
                    } else {
                        id
                    };
                    if real_id > 0 {
                        db.set_activity_end(real_id, end)?;
                    }
                }
                ClaudeOp::SetActivityDetail { id, detail } => {
                    let real_id = if id < 0 {
                        *temp_to_real.get(&id).unwrap_or(&id)
                    } else {
                        id
                    };
                    if real_id > 0 {
                        db.set_activity_detail(real_id, &detail)?;
                    }
                }
            }
        }
        for (sid, mut s) in batch.sessions {
            if let Some(id) = s.activity_id {
                if id < 0 {
                    s.activity_id = temp_to_real.get(&id).copied();
                }
            }
            db.save_claude_session(&sid, &s)?;
        }
        db.save_claude_file(&batch.file_key, batch.offset, batch.mtime)?;
        Ok(())
    })
}

fn run_loop(
    db: Db,
    rx: mpsc::Receiver<DbCmd>,
    in_error: Arc<AtomicBool>,
    on_error_change: Arc<ErrorCallbackHolder>,
) {
    let mut tokens = TokenMap::new();
    let mut dirty_since_backup = false;
    let record_res = |op: &'static str, r: &rusqlite::Result<()>| match r {
        Ok(()) => set_error_state(&in_error, &on_error_change, false),
        Err(e) if !is_transient_busy(e) => {
            log::db_err(Category::Db, op, e);
            set_error_state(&in_error, &on_error_change, true);
        }
        Err(_) => {}
    };
    let record_ui = |op: &'static str, err: &rusqlite::Error| {
        if is_fatal_storage_error(err) {
            log::db_err(Category::Db, op, err);
            set_error_state(&in_error, &on_error_change, true);
        }
    };

    while let Ok(cmd) = rx.recv() {
        match cmd {
            DbCmd::OpenRun { token, activity } => {
                let res = retry_busy(|| db.insert_activity(&activity));
                match res {
                    Ok(row_id) => {
                        tokens.insert(token, row_id);
                        dirty_since_backup = true;
                        set_error_state(&in_error, &on_error_change, false);
                    }
                    Err(e) => {
                        if !is_transient_busy(&e) {
                            log::db_err(Category::Db, "open_run", &e);
                            set_error_state(&in_error, &on_error_change, true);
                        }
                    }
                }
            }
            DbCmd::SetRunEnd { token, end } => {
                if let Some(row_id) = tokens.get(token) {
                    let res = retry_busy(|| db.set_activity_end(row_id, end));
                    if res.is_ok() {
                        dirty_since_backup = true;
                    }
                    record_res("set_run_end", &res);
                }
            }
            DbCmd::SetRunUrl {
                token,
                url,
                context,
            } => {
                if let Some(row_id) = tokens.get(token) {
                    let res = retry_busy(|| db.set_activity_url(row_id, &url, &context));
                    if res.is_ok() {
                        dirty_since_backup = true;
                    }
                    record_res("set_run_url", &res);
                }
            }
            DbCmd::ApplyClaudeBatch { batch, reply } => {
                let res = retry_busy(|| apply_claude_batch_tx(&db, batch.clone()));
                match &res {
                    Ok(()) => {
                        dirty_since_backup = true;
                        set_error_state(&in_error, &on_error_change, false);
                    }
                    Err(e) if !is_transient_busy(e) => {
                        log::db_err(Category::Db, "claude_batch", e);
                        set_error_state(&in_error, &on_error_change, true);
                    }
                    Err(_) => {}
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::CreateProject {
                input,
                now_ms,
                reply,
            } => {
                let res = retry_busy(|| db.create_project(&input, now_ms));
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("create_project", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::UpdateProject { id, patch, reply } => {
                let res = retry_busy(|| db.update_project(&id, &patch));
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("update_project", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::DeleteProject { id, reply } => {
                let res = retry_busy(|| db.delete_project(&id));
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("delete_project", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::CreateEntry { input, reply } => {
                let res = retry_busy(|| db.create_entry(&input));
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("create_entry", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::UpdateEntry { id, patch, reply } => {
                let res = retry_busy(|| db.update_entry(&id, &patch));
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("update_entry", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::DeleteEntry { id, reply } => {
                let res = retry_busy(|| db.delete_entry(&id));
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("delete_entry", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::StartTimer {
                input,
                now_ms,
                reply,
            } => {
                let res = retry_busy(|| db.start_timer(&input, now_ms));
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("start_timer", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::StopTimers { now_ms, reply } => {
                let res = retry_busy(|| db.stop_all_timers(now_ms));
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("stop_timers", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::ClearActivity { reply } => {
                let res = retry_busy(|| db.clear_activity());
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("clear_activity", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::SaveSettings {
                patch,
                locale,
                reply,
            } => {
                let res = retry_busy(|| db.save_settings(&patch, &locale));
                if res.is_ok() {
                    dirty_since_backup = true;
                    set_error_state(&in_error, &on_error_change, false);
                } else if let Err(ref e) = res {
                    record_ui("save_settings", e);
                }
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::Checkpoint { reply } => {
                let res = retry_busy(|| db.checkpoint());
                record_res("checkpoint", &res);
                let _ = reply.send(res.map_err(|e| e.to_string()));
            }
            DbCmd::ProbeWrite { reply } => {
                let res = db.in_transaction(|| Ok::<(), rusqlite::Error>(()));
                match res {
                    Ok(()) => {
                        set_error_state(&in_error, &on_error_change, false);
                        let _ = reply.send(Ok(true));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e.to_string()));
                    }
                }
            }
            DbCmd::RunHousekeeping {
                backup_dir,
                now_ms,
                reply,
            } => {
                let rep =
                    backup::run_housekeeping(&db, &backup_dir, &mut dirty_since_backup, now_ms);
                let _ = reply.send(Ok(rep));
            }
            #[cfg(test)]
            DbCmd::SleepForTest { duration } => {
                std::thread::sleep(duration);
            }
            DbCmd::Shutdown { reply } => {
                let _ = db.optimize();
                let _ = db.checkpoint();
                let _ = reply.send(());
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::{Info, Recorder};
    use std::time::Instant;

    #[test]
    fn slow_writer_does_not_block_monitor_recording_or_debounce() {
        let dir = std::env::temp_dir().join(format!("focus-dbthread-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db_path = dir.join("focus.db");

        let mut db_thread = DbThread::spawn(db_path.clone()).unwrap();
        let handle = db_thread.handle();
        let sink = QueuedSink::new(handle.clone());
        let mut recorder = Recorder::default();

        // Stall the writer thread for 2 seconds.
        handle.sleep_for_test(Duration::from_secs(2));

        // Recording and extending runs on the monitor side must return immediately (< 50 ms).
        let t0 = Instant::now();
        recorder
            .on_focus(
                &sink,
                Info {
                    key: "code".into(),
                    app: "Code".into(),
                    exe: "code".into(),
                    detail: "main.rs".into(),
                    pid: 1,
                },
                10_000,
            )
            .unwrap();
        recorder.tick(&sink, 20_000).unwrap();
        recorder
            .on_focus(
                &sink,
                Info {
                    key: "msedge".into(),
                    app: "Edge".into(),
                    exe: "msedge".into(),
                    detail: "Docs".into(),
                    pid: 2,
                },
                30_000,
            )
            .unwrap();
        recorder.close(&sink, 45_000).unwrap();
        assert!(
            t0.elapsed() < Duration::from_millis(100),
            "monitor writes blocked for {:?}",
            t0.elapsed()
        );

        // Shut down cleanly (drains the 2s sleep + queued writes + checkpoint).
        db_thread.stop();

        let verify_db = Db::open(&db_path).unwrap();
        let rows = verify_db.list_activity(0, 100_000).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].app.as_str(), rows[0].start, rows[0].end),
            ("Code", 10_000, 30_000)
        );
        assert_eq!(
            (rows[1].app.as_str(), rows[1].start, rows[1].end),
            ("Edge", 30_000, 45_000)
        );
        drop(verify_db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
