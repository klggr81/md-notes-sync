//! An interrupted sync leaves bisync's lock file behind: the next sync reports
//! it, and clear_bisync_locks removes it unless its rclone still runs.
//! Needs rclone on the PATH, so it only runs when asked:
//!   cargo test -p sync-core -- --ignored

use std::fs;
use std::path::{Path, PathBuf};

use sync_core::lock::clear_bisync_locks;
use sync_core::{run, Options, SyncError};

fn setup() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sync-core-lock-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("pc")).unwrap();
    fs::create_dir_all(dir.join("cloud")).unwrap();
    fs::write(dir.join("pc/a.md"), "A").unwrap();
    fs::write(dir.join("cloud/b.md"), "B").unwrap();
    fs::write(dir.join("filters.txt"), "- *.tmp\n").unwrap();
    fs::write(
        dir.join("sync.conf"),
        format!(
            "LOCAL={}\nREMOTE={}\nRCLONE=rclone\nEXTRA_FLAGS=--resilient --recover --max-lock 2m\nBACKUP_KEEP=0\n",
            dir.join("pc").display(),
            dir.join("cloud").display()
        ),
    )
    .unwrap();
    dir
}

/// Writes a lock file like bisync's, next to its listings, held by `pid`
fn fake_lock(dir: &Path, pid: u32) -> PathBuf {
    let state = dir.join("bisync-state");
    let listing = fs::read_dir(&state)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".path1.lst"))
        .expect("a listing after the first sync");
    let lck = state.join(listing.replace(".path1.lst", ".lck"));
    let expires = chrono::Local::now() + chrono::Duration::minutes(2);
    fs::write(
        &lck,
        format!(
            "{{\n\t\"Session\": \"x\",\n\t\"PID\": \"{pid}\",\n\t\"TimeRenewed\": \"{now}\",\n\t\"TimeExpires\": \"{exp}\"\n}}",
            now = chrono::Local::now().to_rfc3339(),
            exp = expires.to_rfc3339()
        ),
    )
    .unwrap();
    lck
}

#[test]
#[ignore = "needs rclone on the PATH"]
fn a_left_over_lock_is_reported_and_can_be_removed() {
    let dir = setup();
    run(&dir, Options::default(), |_| {}).expect("first sync");

    // A process number that no running process has
    let dead_pid = 999_999;
    let lck = fake_lock(&dir, dead_pid);
    let err = run(&dir, Options::default(), |_| {}).unwrap_err();
    assert!(matches!(err, SyncError::BisyncLocked(Some(_))), "got: {err}");
    assert!(err.to_string().contains("runs out at"), "{err}");

    // While its rclone still runs, the lock stays
    fake_lock(&dir, std::process::id());
    assert!(clear_bisync_locks(&dir).unwrap_err().contains("still running"));
    assert!(lck.exists());

    fake_lock(&dir, dead_pid);
    assert_eq!(clear_bisync_locks(&dir).unwrap(), vec![lck.clone()]);
    assert!(!lck.exists());
    let summary = run(&dir, Options::default(), |_| {}).expect("sync after removing the lock");
    assert_eq!(summary.warnings, 0);
    let _ = fs::remove_dir_all(&dir);
}
