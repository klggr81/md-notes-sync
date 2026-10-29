//! Runs one sync: checks sync.conf, takes the lock, makes the weekly backup,
//! runs rclone bisync and reads its exit code. rclone's output arrives line by line as
//! [`Event`]s while it runs, and is also written to sync.log.

use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;

use crate::backup;
use crate::conf::{Conf, ConfError};
use crate::history;
use crate::lock::SyncLock;
use crate::logline::{self, Counts, Kind, Parsed};

pub const WORK_DIR: &str = "bisync-state";
pub const FILTERS: &str = "filters.txt";
pub const RCLONE_CONF: &str = "rclone.conf";
/// How long a sync waits for another one to finish
const LOCK_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Show what would happen, change nothing
    pub dry_run: bool,
    /// Rebuild bisync's state: merge both sides, newer file wins, nothing is deleted
    pub resync: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    /// Another sync holds the lock
    Waiting,
    #[serde(rename_all = "camelCase")]
    Started { local: String, remote: String, dry_run: bool, resync: bool },
    Backup { path: String },
    BackupFailed { message: String },
    Line { time: Option<String>, text: String, kind: Kind },
    Progress { text: String, percent: Option<u8> },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub dry_run: bool,
    pub resync: bool,
    pub copied: u32,
    pub deleted: u32,
    pub warnings: u32,
}

#[derive(Debug)]
pub enum SyncError {
    Conf(ConfError),
    RcloneNotFound(String),
    LocalNotFound(String),
    Busy,
    Io(io::Error),
    /// Exit code 7: bisync stopped and needs a resync
    Critical,
    /// bisync's own lock file from an interrupted run; with `--max-lock` it
    /// runs out at the given time (`HH:MM:SS`)
    BisyncLocked(Option<String>),
    /// The SFTP server's key isn't the one kept in known_hosts at the setup
    ServerKeyChanged,
    Exit(i32),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Conf(e) => write!(f, "{e}"),
            SyncError::RcloneNotFound(cmd) => write!(f, "rclone not found ({cmd})"),
            SyncError::LocalNotFound(dir) => write!(f, "Local folder not found: {dir}"),
            SyncError::Busy => write!(f, "Another sync is still running."),
            SyncError::Io(e) => write!(f, "Can't run rclone: {e}"),
            SyncError::Critical => write!(f, "Critical bisync error (exit 7). Check the log, then run a resync."),
            SyncError::BisyncLocked(Some(until)) => write!(
                f,
                "An earlier sync was interrupted and rclone's lock file is still there. It runs out at {until}: sync again after that."
            ),
            SyncError::BisyncLocked(None) => write!(
                f,
                "An earlier sync was interrupted and rclone's lock file is still there. If no other sync runs, delete the .lck file in bisync-state, then sync again."
            ),
            SyncError::ServerKeyChanged => write!(
                f,
                "The SFTP server's key isn't the one saved at the setup. Either the server was replaced, or someone else is answering in its place, so nothing was synced. If you expect the change (ask your provider), open the setup and connect again: it saves the new key."
            ),
            SyncError::Exit(code) => write!(f, "rclone bisync exited with code {code}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl SyncError {
    /// Fixed by a resync (exit code 7)
    pub fn needs_resync(&self) -> bool {
        matches!(self, SyncError::Critical)
    }

    /// Fixed by removing bisync's lock file ([`crate::lock::clear_bisync_locks`])
    pub fn left_lock(&self) -> bool {
        matches!(self, SyncError::BisyncLocked(_))
    }
}

/// Syncs the folder `dir` (the one with sync.conf). Blocks until rclone is
/// done; call it on its own thread. Failures are logged to sync.log as
/// `ERROR: …`.
pub fn run(dir: &Path, opts: Options, mut on_event: impl FnMut(Event)) -> Result<Summary, SyncError> {
    let result = run_logged(dir, opts, &mut on_event);
    if let Err(e) = &result {
        let _ = history::log(dir, &format!("ERROR: {e}"));
    }
    result
}

fn run_logged(dir: &Path, opts: Options, on_event: &mut impl FnMut(Event)) -> Result<Summary, SyncError> {
    let conf = Conf::load(dir).map_err(SyncError::Conf)?;
    conf.check().map_err(SyncError::Conf)?;
    let local = conf.local().unwrap_or_default();
    let remote = conf.remote().unwrap_or_default().to_string();
    let rclone_cmd = conf.rclone().unwrap_or_default();
    let rclone = find_rclone(dir, &rclone_cmd).ok_or(SyncError::RcloneNotFound(rclone_cmd))?;
    if !Path::new(&local).exists() {
        return Err(SyncError::LocalNotFound(local));
    }
    // Also for a sync.conf written by hand
    crate::setup::ensure_filters(dir).map_err(SyncError::Io)?;

    let _lock = match SyncLock::acquire(Duration::ZERO).map_err(SyncError::Io)? {
        Some(lock) => lock,
        None => {
            on_event(Event::Waiting);
            SyncLock::acquire(LOCK_WAIT).map_err(SyncError::Io)?.ok_or(SyncError::Busy)?
        }
    };
    history::rotate(dir).map_err(SyncError::Io)?;

    let work_dir = dir.join(WORK_DIR);
    let resync = opts.resync || is_first_run(&work_dir);
    let args = rclone_args(&local, &remote, dir, conf.get("EXTRA_FLAGS"), opts.dry_run, resync);

    if !opts.dry_run {
        match backup::weekly(Path::new(&local), &dir.join(backup::DIR), backup::keep(&conf), backup::today()) {
            Ok(Some(zip)) => {
                let _ = history::log(dir, &format!("backup {}", zip.display()));
                on_event(Event::Backup { path: zip.display().to_string() });
            }
            Ok(None) => {}
            Err(e) => {
                let _ = history::log(dir, &format!("WARNING: backup failed: {e}"));
                on_event(Event::BackupFailed { message: e.to_string() });
            }
        }
    }

    let flags = format!("{}{}", if opts.dry_run { " (dry run)" } else { "" }, if resync { " (resync)" } else { "" });
    let _ = history::log(dir, &format!("---- sync start{flags}"));
    on_event(Event::Started { local, remote, dry_run: opts.dry_run, resync });

    let mut command = Command::new(&rclone);
    command.args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let rclone_conf = dir.join(RCLONE_CONF);
    if rclone_conf.exists() {
        command.env("RCLONE_CONFIG", rclone_conf);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(SyncError::Io)?;

    // rclone logs to stderr; each line goes to sync.log (as --log-file would) and to the window
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(history::LOG)).ok();
    let mut counts = Counts::default();
    let mut lock = LockNotice::default();
    let mut key_changed = false;
    let mut reader = BufReader::new(child.stderr.take().expect("stderr is piped"));
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf).map_err(SyncError::Io)? == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(log) = log.as_mut() {
            let _ = writeln!(log, "{line}");
        }
        match logline::parse(line) {
            Parsed::Progress { text, percent } => on_event(Event::Progress { text, percent }),
            Parsed::Line { time, text, kind } => {
                counts.add(kind);
                lock.read(&text);
                key_changed |= text.contains("knownhosts: ");
                on_event(Event::Line { time, text, kind });
            }
            Parsed::Empty => {}
        }
    }
    drop(log);

    let status = child.wait().map_err(SyncError::Io)?;
    match status.code() {
        Some(0) => {}
        Some(7) => return Err(SyncError::Critical),
        Some(_) if lock.found => return Err(SyncError::BisyncLocked(lock.expires)),
        Some(_) if key_changed => return Err(SyncError::ServerKeyChanged),
        code => return Err(SyncError::Exit(code.unwrap_or(-1))),
    }
    let _ = history::log(dir, "---- sync ok");
    Ok(Summary { dry_run: opts.dry_run, resync, copied: counts.copied, deleted: counts.deleted, warnings: counts.warnings })
}

/// bisync's messages about its own lock file (in bisync-state), left by an
/// interrupted run:
/// `….lck: Valid lock file found. Expires at 2026-10-28 16:06:53.6657264 +0100 CET. (1m26s from now)`
/// `Failed to bisync: prior lock file found: ….lck`
#[derive(Default)]
struct LockNotice {
    found: bool,
    expires: Option<String>,
}

impl LockNotice {
    fn read(&mut self, text: &str) {
        if text.contains("prior lock file found") {
            self.found = true;
        }
        if let Some(rest) = text.split("Valid lock file found. Expires at ").nth(1) {
            // "2026-10-28 16:06:53.6657264 +0100 CET…" → "16:06:53"
            self.expires = rest.split(' ').nth(1).and_then(|t| t.get(..8)).map(str::to_string);
        }
    }
}

/// rclone's flags. No `--log-file`: the output is read from stderr and
/// written to sync.log here.
fn rclone_args(local: &str, remote: &str, dir: &Path, extra: Option<&str>, dry_run: bool, resync: bool) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "bisync".into(),
        local.into(),
        remote.into(),
        "--workdir".into(),
        dir.join(WORK_DIR).display().to_string(),
        "--filters-file".into(),
        dir.join(FILTERS).display().to_string(),
        "--log-level".into(),
        "INFO".into(),
        "--stats".into(),
        "1s".into(),
        "--stats-one-line".into(),
        // rclone colours bisync's messages when it writes to a pipe; the log file never has colours
        "--color".into(),
        "NEVER".into(),
    ];
    args.extend(extra.unwrap_or_default().split_whitespace().map(str::to_string));
    if resync {
        args.extend(["--resync", "--resync-mode", "newer"].map(str::to_string));
    }
    if dry_run {
        args.push("--dry-run".into());
    }
    args
}

/// No bisync state yet: the first sync has to be a resync.
fn is_first_run(work_dir: &Path) -> bool {
    !std::fs::read_dir(work_dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|e| e.file_name().to_string_lossy().ends_with(".path1.lst"))
    })
}

/// RCLONE from sync.conf: relative to the app folder if it exists there
/// (`rclone\rclone.exe`), a full path, or a program on the PATH (`rclone`).
pub fn find_rclone(dir: &Path, cmd: &str) -> Option<PathBuf> {
    let path = Path::new(cmd);
    let with_exe = |p: PathBuf| {
        if p.is_file() {
            Some(p)
        } else {
            let exe = p.with_extension("exe");
            (p.extension().is_none() && exe.is_file()).then_some(exe)
        }
    };
    if !path.is_absolute()
        && let Some(found) = with_exe(dir.join(path))
    {
        return Some(found);
    }
    if path.is_absolute() || path.components().count() > 1 {
        return with_exe(path.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|p| with_exe(p.join(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_flags() {
        let dir = Path::new(r"C:\Tools\md-notes-sync");
        let args = rclone_args(r"C:\Notes", "Nextcloud:Notizen", dir, Some("--conflict-resolve newer  --max-delete 25"), true, true);
        assert_eq!(
            args.join(" "),
            r"bisync C:\Notes Nextcloud:Notizen --workdir C:\Tools\md-notes-sync\bisync-state --filters-file C:\Tools\md-notes-sync\filters.txt --log-level INFO --stats 1s --stats-one-line --color NEVER --conflict-resolve newer --max-delete 25 --resync --resync-mode newer --dry-run"
        );
    }

    #[test]
    fn recognises_a_left_over_bisync_lock() {
        let mut lock = LockNotice::default();
        lock.read(r"C:\x\bisync-state\C__Notes..Nextcloud_Notizen.lck: Valid lock file found. Expires at 2026-10-28 16:06:53.6657264 +0100 CET. (1m26s from now)");
        assert!(!lock.found);
        lock.read(r"Failed to bisync: prior lock file found: C:\x\bisync-state\C__Notes..Nextcloud_Notizen.lck ");
        assert!(lock.found);
        assert_eq!(
            SyncError::BisyncLocked(lock.expires).to_string(),
            "An earlier sync was interrupted and rclone's lock file is still there. It runs out at 16:06:53: sync again after that."
        );
    }

    #[test]
    fn finds_rclone_next_to_the_app() {
        let dir = std::env::temp_dir().join(format!("sync-core-test-rclone-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("rclone")).unwrap();
        std::fs::write(dir.join("rclone").join("rclone.exe"), "").unwrap();
        assert_eq!(find_rclone(&dir, r"rclone\rclone.exe"), Some(dir.join(r"rclone\rclone.exe")));
        assert_eq!(find_rclone(&dir, r"rclone\rclone"), Some(dir.join(r"rclone\rclone.exe")));
        assert_eq!(find_rclone(&dir, r"nope\rclone.exe"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_run_without_state() {
        let dir = std::env::temp_dir().join(format!("sync-core-test-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(is_first_run(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(is_first_run(&dir));
        std::fs::write(dir.join("C__Notes..Nextcloud_Notizen.path1.lst"), "").unwrap();
        assert!(!is_first_run(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
