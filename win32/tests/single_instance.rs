use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

use rusqlite::Connection;

#[test]
fn background_instance_records_and_quits_cleanly_on_quit_flag() {
    let exe = env!("CARGO_BIN_EXE_focus");
    let tag = format!("it-{}", std::process::id());
    let dir = std::env::temp_dir().join(format!("focus-it-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("focus.db");

    // 1. `--version` prints version and exits cleanly without acquiring the instance mutex.
    let ver = Command::new(exe)
        .arg("--version")
        .output()
        .expect("run --version");
    assert!(ver.status.success(), "--version failed: {ver:?}");
    let ver_str = String::from_utf8_lossy(&ver.stdout);
    assert!(
        ver_str.trim().starts_with("focus "),
        "unexpected --version output: {ver_str:?}"
    );

    // 2. Start primary instance with temp `--db` and `--background`.
    let mut primary = Command::new(exe)
        .arg("--db")
        .arg(&db_path)
        .arg("--background")
        .env("FOCUS_TEST_INSTANCE", &tag)
        .spawn()
        .expect("spawn primary --background");

    // Wait until the database file has been created and migrated.
    let wait_start = Instant::now();
    while !db_path.exists() && wait_start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(db_path.exists(), "primary did not initialize database");
    std::thread::sleep(Duration::from_millis(150));

    // 3. A second `--background` invocation exits silently with status 0.
    let second_bg = Command::new(exe)
        .arg("--background")
        .env("FOCUS_TEST_INSTANCE", &tag)
        .status()
        .expect("run second --background");
    assert!(second_bg.success());
    assert!(
        primary.try_wait().unwrap().is_none(),
        "second --background should leave primary running"
    );

    // 4. Send `--quit` and verify the primary exits cleanly.
    let quit_status = Command::new(exe)
        .arg("--quit")
        .env("FOCUS_TEST_INSTANCE", &tag)
        .status()
        .expect("run --quit");
    assert!(quit_status.success(), "--quit command failed");

    let exit_deadline = Instant::now() + Duration::from_secs(5);
    let primary_status = loop {
        if let Some(st) = primary.try_wait().expect("poll primary") {
            break st;
        }
        if Instant::now() >= exit_deadline {
            let _ = primary.kill();
            panic!("primary process did not exit within 5 s after --quit");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        primary_status.success(),
        "primary exited with {primary_status}"
    );

    // 5. Database is valid, migrated, and every recorded activity row is closed (`end >= start`).
    let conn = Connection::open(&db_path).expect("open temp db");
    let uv: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(uv, 3);
    let qc: String = conn
        .pragma_query_value(None, "quick_check", |r| r.get(0))
        .unwrap();
    assert_eq!(qc, "ok");
    let bad_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM activity WHERE end < start", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(bad_rows, 0);
    drop(conn);

    let _ = fs::remove_dir_all(&dir);
}
