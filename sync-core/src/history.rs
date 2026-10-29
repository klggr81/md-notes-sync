//! sync.log: the lines the app writes around each run, and what
//! they say about the last sync.
//!
//! ```text
//! 2026-10-28T15:31:47 ---- sync start (dry run) (resync)
//! 2026-10-28T15:31:49 ---- sync ok
//! 2026-10-28T15:40:02 ERROR: rclone bisync exited with code 1
//! ```
//! rclone's own lines in between start with `2026/10/28`, so they never match.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use chrono::{Local, NaiveDateTime};

pub const LOG: &str = "sync.log";
pub const OLD_LOG: &str = "sync.old.log";
/// sync.log moves to sync.old.log above this size
pub const MAX_LOG: u64 = 1024 * 1024;

const TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// Appends `<time> <msg>` to sync.log.
pub fn log(dir: &Path, msg: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(dir.join(LOG))?;
    writeln!(file, "{} {msg}", Local::now().format(TIME_FORMAT))
}

/// Keeps sync.log small: above 1 MB it becomes sync.old.log.
pub fn rotate(dir: &Path) -> io::Result<()> {
    let log = dir.join(LOG);
    if fs::metadata(&log).is_ok_and(|m| m.len() > MAX_LOG) {
        fs::rename(log, dir.join(OLD_LOG))?;
    }
    Ok(())
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct History {
    /// End of the last successful sync (dry runs don't count)
    pub last_sync: Option<NaiveDateTime>,
    /// The error, if the most recent run failed
    pub failed: Option<String>,
}

/// Reads sync.log (and sync.old.log, if sync.log has no finished sync yet).
pub fn read(dir: &Path) -> History {
    let mut history = History::default();
    for name in [OLD_LOG, LOG] {
        if let Ok(text) = fs::read_to_string(dir.join(name)) {
            history = parse(&text, history);
        }
    }
    history
}

/// Goes through the lines in order, starting from what an older log said.
pub fn parse(text: &str, mut history: History) -> History {
    let mut dry_run = false;
    for line in text.lines() {
        let Some((time, msg)) = line.split_once(' ') else { continue };
        let Ok(time) = NaiveDateTime::parse_from_str(time, TIME_FORMAT) else { continue };
        if let Some(flags) = msg.strip_prefix("---- sync start") {
            dry_run = flags.contains("(dry run)");
        } else if msg == "---- sync ok" {
            if !dry_run {
                history.last_sync = Some(time);
            }
            history.failed = None;
        } else if let Some(err) = msg.strip_prefix("ERROR: ") {
            history.failed = Some(err.to_string());
        }
    }
    history
}

/// [`ago`] from now
pub fn ago_now(then: NaiveDateTime) -> String {
    ago(then, Local::now().naive_local())
}

/// "just now", "5 min ago", "3 h ago", "yesterday 14:05", "26 Oct 14:05"
pub fn ago(then: NaiveDateTime, now: NaiveDateTime) -> String {
    let minutes = (now - then).num_minutes();
    if minutes < 1 {
        "just now".into()
    } else if minutes < 60 {
        format!("{minutes} min ago")
    } else if then.date() == now.date() {
        format!("{} h ago", minutes / 60)
    } else if now.date().pred_opt() == Some(then.date()) {
        format!("yesterday {}", then.format("%H:%M"))
    } else {
        then.format("%-d %b %H:%M").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, TIME_FORMAT).unwrap()
    }

    const LOG_TEXT: &str = "\
2026-10-28T10:00:00 ---- sync start
2026/10/28 10:00:01 INFO  : a.md: Copied (new)
2026/10/28 10:00:01 ERROR : not ours: our ERROR lines have a T in the date
2026-10-28T10:00:02 ---- sync ok
2026-10-28T11:00:00 ---- sync start (dry run)
2026-10-28T11:00:02 ---- sync ok
";

    #[test]
    fn dry_runs_dont_count_as_a_sync() {
        let h = parse(LOG_TEXT, History::default());
        assert_eq!(h, History { last_sync: Some(t("2026-10-28T10:00:02")), failed: None });
    }

    #[test]
    fn the_latest_run_decides_about_the_error() {
        let failed = format!("{LOG_TEXT}2026-10-28T12:00:00 ---- sync start\n2026-10-28T12:00:05 ERROR: rclone bisync exited with code 1\n");
        let h = parse(&failed, History::default());
        assert_eq!(h.failed.as_deref(), Some("rclone bisync exited with code 1"));
        assert_eq!(h.last_sync, Some(t("2026-10-28T10:00:02")));
        let fixed = format!("{failed}2026-10-28T12:10:00 ---- sync start\n2026-10-28T12:10:02 ---- sync ok\n");
        assert_eq!(parse(&fixed, History::default()).failed, None);
    }

    #[test]
    fn says_how_long_ago() {
        let now = t("2026-10-28T15:00:00");
        assert_eq!(ago(t("2026-10-28T14:59:30"), now), "just now");
        assert_eq!(ago(t("2026-10-28T14:55:00"), now), "5 min ago");
        assert_eq!(ago(t("2026-10-28T11:30:00"), now), "3 h ago");
        assert_eq!(ago(t("2026-10-27T14:05:00"), now), "yesterday 14:05");
        assert_eq!(ago(t("2026-10-26T14:05:00"), now), "26 Oct 14:05");
    }
}
