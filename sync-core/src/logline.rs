//! rclone's log lines, sorted by what they mean (the window colours them).

use serde::Serialize;

/// What a log line means. The console colour it was first given in brackets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Anything else (gray)
    Info,
    /// Details that don't matter much (dark gray)
    Dim,
    /// A file was copied (green)
    Copied,
    /// A file was deleted (magenta)
    Deleted,
    /// Dry run: a file would be copied (cyan)
    WouldCopy,
    /// Dry run: a file would be deleted (cyan)
    WouldDelete,
    /// rclone NOTICE (yellow)
    Warning,
    /// rclone ERROR (red)
    Error,
    /// "Bisync successful" (green)
    Success,
}

#[derive(Debug, PartialEq)]
pub enum Parsed {
    /// One line of `--stats-one-line`: goes to the progress bar, not the log.
    Progress { text: String, percent: Option<u8> },
    /// A line to show. `time` is `HH:MM:SS` for rclone's own lines.
    Line { time: Option<String>, text: String, kind: Kind },
    Empty,
}

pub fn parse(line: &str) -> Parsed {
    let line = strip_ansi(line.trim_end_matches(['\r', '\n']));
    let line = line.as_str();
    let Some((time, level, msg)) = split_rclone_line(line) else {
        return match line {
            "" => Parsed::Empty,
            // Continuation lines (indented, or JSON from a NOTICE)
            _ if line.starts_with([' ', '\t', '{', '}']) => line_of(None, line, Kind::Dim),
            _ => line_of(None, line, Kind::Info),
        };
    };
    if let Some(percent) = stats_percent(msg) {
        return Parsed::Progress { text: msg.trim().to_string(), percent };
    }
    let kind = if msg.contains(": Skipped copy as --dry-run") {
        Kind::WouldCopy
    } else if msg.contains(": Skipped delete as --dry-run") {
        Kind::WouldDelete
    } else if msg.contains(" as --dry-run is set") {
        // The other steps a test run skips (make or remove a directory, set a
        // modification time) are NOTICE too, but no warnings
        Kind::Dim
    } else if level == "NOTICE" && (msg.starts_with("- Path1 ") || msg.starts_with("- Path2 ")) {
        // bisync's own step messages ("- Path2  Resync is copying files to") are NOTICE, but no warnings
        Kind::Info
    } else if level == "ERROR" || level == "CRITICAL" {
        Kind::Error
    } else if level == "NOTICE" {
        Kind::Warning
    } else if msg.contains(": Copied (") {
        Kind::Copied
    } else if msg.ends_with(": Deleted") {
        Kind::Deleted
    } else if msg.contains("Bisync successful") {
        Kind::Success
    } else {
        Kind::Info
    };
    line_of(Some(time), msg, kind)
}

/// Removes colour codes (`ESC [ â€¦ m`), in case rclone runs with `--color ALWAYS`.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip "[" and the parameters up to the final letter
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn line_of(time: Option<&str>, text: &str, kind: Kind) -> Parsed {
    Parsed::Line { time: time.map(str::to_string), text: text.to_string(), kind }
}

/// `2026/10/28 15:31:47 INFO  : message` â†’ (`15:31:47`, `INFO`, `message`)
fn split_rclone_line(line: &str) -> Option<(&str, &str, &str)> {
    let b = line.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    let date_ok = b.len() > 20
        && digits(0..4) && b[4] == b'/' && digits(5..7) && b[7] == b'/' && digits(8..10) && b[10] == b' '
        && digits(11..13) && b[13] == b':' && digits(14..16) && b[16] == b':' && digits(17..19) && b[19] == b' ';
    if !date_ok {
        return None;
    }
    let rest = &line[20..];
    let level_end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_')?;
    let (level, after) = rest.split_at(level_end);
    let after = after.trim_start().strip_prefix(':')?;
    let msg = after.strip_prefix(' ').unwrap_or(after);
    (!level.is_empty()).then_some((&line[11..19], level, msg))
}

/// `  1.2 KiB / 3.4 KiB, 35%, 0 B/s, ETA -` â†’ Some(Some(35)); `, -,` â†’ Some(None)
fn stats_percent(msg: &str) -> Option<Option<u8>> {
    let mut parts = msg.trim_start().splitn(3, ", ");
    let sizes = parts.next()?;
    let pct = parts.next()?;
    let (done, total) = sizes.split_once(" / ")?;
    let is_size = |s: &str| {
        let s = s.trim();
        s.ends_with('B') && s.starts_with(|c: char| c.is_ascii_digit())
    };
    if !is_size(done) || !is_size(total) {
        return None;
    }
    match pct.trim_end_matches('%') {
        "-" => Some(None),
        n => n.parse::<u8>().ok().map(Some),
    }
}

/// What the summary line counts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub copied: u32,
    pub deleted: u32,
    pub warnings: u32,
}

impl Counts {
    pub fn add(&mut self, kind: Kind) {
        match kind {
            Kind::Copied | Kind::WouldCopy => self.copied += 1,
            Kind::Deleted | Kind::WouldDelete => self.deleted += 1,
            Kind::Warning | Kind::Error => self.warnings += 1,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(line: &str) -> Kind {
        match parse(line) {
            Parsed::Line { kind, .. } => kind,
            other => panic!("not a line: {other:?}"),
        }
    }

    #[test]
    fn sorts_lines() {
        assert_eq!(kind("2026/10/28 15:31:47 INFO  : Work/a.md: Copied (new)"), Kind::Copied);
        assert_eq!(kind("2026/10/28 15:31:47 INFO  : Work/a.md: Deleted"), Kind::Deleted);
        assert_eq!(kind("2026/10/28 15:31:47 NOTICE: Work/a.md: Skipped copy as --dry-run is set (size 12)"), Kind::WouldCopy);
        assert_eq!(kind("2026/10/28 15:31:47 NOTICE: b.md: Skipped delete as --dry-run is set (size 3)"), Kind::WouldDelete);
        assert_eq!(kind("2026/10/28 15:31:47 NOTICE: - Path2    Resync is copying files to         - Path1"), Kind::Info);
        assert_eq!(kind("2026/10/28 15:31:47 NOTICE: Encrypted drive 'x' has no files"), Kind::Warning);
        assert_eq!(kind("2026/10/28 15:31:47 ERROR : Bisync critical error: oops"), Kind::Error);
        assert_eq!(kind("2026/10/29 11:02:36 CRITICAL: Failed to create file system: knownhosts: key mismatch"), Kind::Error);
        assert_eq!(kind("2026/10/28 15:31:47 INFO  : Bisync successful"), Kind::Success);
        assert_eq!(kind("2026/10/28 15:31:47 INFO  : Work: Skipped set directory modification time as --dry-run is set"), Kind::Dim);
        assert_eq!(kind("2026/10/28 15:31:47 NOTICE: Work: Skipped make directory as --dry-run is set"), Kind::Dim);
        assert_eq!(kind("2026/10/28 15:31:47 NOTICE: Old: Skipped remove directory as --dry-run is set"), Kind::Dim);
        assert_eq!(
            kind("2026/10/28 15:45:12 NOTICE: - \u{1b}[34mPath2\u{1b}[0m    \u{1b}[35mResync is copying files to\u{1b}[0m"),
            Kind::Info,
            "colour codes don't turn bisync's step messages into warnings"
        );
        assert_eq!(kind("  some continuation"), Kind::Dim);
        assert_eq!(kind("plain text"), Kind::Info);
    }

    #[test]
    fn keeps_time_and_message() {
        assert_eq!(
            parse("2026/10/28 15:31:47 INFO  : Work/a.md: Copied (new)\r\n"),
            Parsed::Line { time: Some("15:31:47".into()), text: "Work/a.md: Copied (new)".into(), kind: Kind::Copied }
        );
        assert_eq!(parse(""), Parsed::Empty);
    }

    #[test]
    fn stats_go_to_the_progress_bar() {
        assert_eq!(
            parse("2026/10/28 15:31:48 INFO  :    1.234 KiB / 2.468 KiB, 50%, 0 B/s, ETA -"),
            Parsed::Progress { text: "1.234 KiB / 2.468 KiB, 50%, 0 B/s, ETA -".into(), percent: Some(50) }
        );
        assert_eq!(
            parse("2026/10/28 15:31:48 INFO  :          0 B / 0 B, -, 0 B/s, ETA -"),
            Parsed::Progress { text: "0 B / 0 B, -, 0 B/s, ETA -".into(), percent: None }
        );
    }

    #[test]
    fn counts_like_the_summary() {
        let mut c = Counts::default();
        for k in [Kind::Copied, Kind::WouldCopy, Kind::Deleted, Kind::Warning, Kind::Error, Kind::Info] {
            c.add(k);
        }
        assert_eq!(c, Counts { copied: 2, deleted: 1, warnings: 2 });
    }
}
