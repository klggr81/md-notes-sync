//! Weekly backup: before a real sync, if the newest backup is a week old or
//! there is none, the notes folder is zipped into
//! `backups\notes-YYYY-MM-DD.zip`, and only the newest zips are kept. A
//! backup can also be made by hand (`now`).

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Local, NaiveDate, Timelike};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use serde::Serialize;

use crate::conf::Conf;

pub const DIR: &str = "backups";
pub const DEFAULT_KEEP: usize = 3;
/// Days between two backups
const EVERY_DAYS: i64 = 7;

/// How far a backup is: files zipped so far of all, and their size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub files: usize,
    pub total: usize,
    pub bytes: u64,
}

/// Zips `notes` into `backup_dir` unless the newest zip there is less than a
/// week old. Returns the new zip, or `None` if there was nothing to do.
pub fn weekly(notes: &Path, backup_dir: &Path, keep: usize, today: NaiveDate) -> io::Result<Option<PathBuf>> {
    if keep == 0 {
        return Ok(None);
    }
    if fs::exists(backup_dir)?
        && let Some(newest) = zips(backup_dir)?.last().and_then(|zip| date_of(zip))
        // A date in the future (the clock was wrong) doesn't count
        && (0..EVERY_DAYS).contains(&(today - newest).num_days())
    {
        return Ok(None);
    }
    let zip = backup_dir.join(format!("notes-{}.zip", today.format("%Y-%m-%d")));
    if zip.exists() {
        return Ok(None);
    }
    make(notes, &zip, keep, &mut |_| {}).map(Some)
}

/// A backup now, whatever the age of the newest one; it replaces one made
/// earlier today. Kept also when backups are turned off, until the next one.
/// `on_progress` follows it file by file.
pub fn now(notes: &Path, backup_dir: &Path, keep: usize, today: NaiveDate, mut on_progress: impl FnMut(Progress)) -> io::Result<PathBuf> {
    make(notes, &backup_dir.join(format!("notes-{}.zip", today.format("%Y-%m-%d"))), keep.max(1), &mut on_progress)
}

/// Today on this PC, the date in a backup's name.
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// How many backups to keep: `BACKUP_KEEP` in sync.conf.
pub fn keep(conf: &Conf) -> usize {
    conf.get("BACKUP_KEEP").and_then(|k| k.parse().ok()).unwrap_or(DEFAULT_KEEP)
}

/// Writes `zip` (through a temporary file, so there's never half a zip), then
/// keeps the newest `keep` zips in its folder.
fn make(notes: &Path, zip: &Path, keep: usize, on_progress: &mut dyn FnMut(Progress)) -> io::Result<PathBuf> {
    let backup_dir = zip.parent().expect("the zip is in the backup folder");
    fs::create_dir_all(backup_dir)?;
    let tmp = zip.with_extension("zip.tmp");
    // rename replaces an older zip of the same name
    let written = write_zip(notes, &tmp, on_progress).and_then(|()| fs::rename(&tmp, zip));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    prune(backup_dir, keep)?;
    Ok(zip.to_path_buf())
}

/// The backup zips, oldest first (the names sort by date).
fn zips(backup_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut zips: Vec<PathBuf> = fs::read_dir(backup_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| date_of(p).is_some())
        .collect();
    zips.sort();
    Ok(zips)
}

/// `notes-2026-10-29.zip` → 2026-10-29
fn date_of(zip: &Path) -> Option<NaiveDate> {
    let name = zip.file_name()?.to_str()?;
    let date = name.strip_prefix("notes-")?.strip_suffix(".zip")?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

/// Deletes all but the newest `keep` zips.
fn prune(backup_dir: &Path, keep: usize) -> io::Result<()> {
    let zips = zips(backup_dir)?;
    for old in zips.iter().take(zips.len().saturating_sub(keep)) {
        fs::remove_file(old)?;
    }
    Ok(())
}

/// The whole folder, with each file's modified time (the date a note shows in
/// the Notes app).
fn write_zip(notes: &Path, zip_path: &Path, on_progress: &mut dyn FnMut(Progress)) -> io::Result<()> {
    let mut zip = ZipWriter::new(File::create(zip_path)?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut tracker = Tracker { progress: Progress { total: count_files(notes)?, ..Progress::default() }, on_progress };
    report(&mut tracker);
    add_dir(&mut zip, notes, "", options, &mut tracker)?;
    zip.finish().map_err(io::Error::other)?;
    Ok(())
}

struct Tracker<'a> {
    progress: Progress,
    on_progress: &'a mut dyn FnMut(Progress),
}

/// Tells `on_progress` how far it is.
fn report(tracker: &mut Tracker) {
    (tracker.on_progress)(tracker.progress);
}

fn count_files(dir: &Path) -> io::Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        count += if entry.metadata()?.is_dir() { count_files(&entry.path())? } else { 1 };
    }
    Ok(count)
}

fn add_dir(zip: &mut ZipWriter<File>, dir: &Path, prefix: &str, options: SimpleFileOptions, tracker: &mut Tracker) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    if entries.is_empty() && !prefix.is_empty() {
        zip.add_directory(prefix, options).map_err(io::Error::other)?;
    }
    for entry in entries {
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        let meta = entry.metadata()?;
        let options = match meta.modified().ok().and_then(zip_time) {
            Some(t) => options.last_modified_time(t),
            None => options,
        };
        if meta.is_dir() {
            add_dir(zip, &entry.path(), &format!("{name}/"), options, tracker)?;
        } else {
            let data = fs::read(entry.path())?;
            zip.start_file(name, options).map_err(io::Error::other)?;
            zip.write_all(&data)?;
            tracker.progress.files += 1;
            tracker.progress.bytes += data.len() as u64;
            report(tracker);
        }
    }
    Ok(())
}

/// Zip times are local time without a time zone, like .NET writes them.
fn zip_time(t: std::time::SystemTime) -> Option<zip::DateTime> {
    let t: DateTime<Local> = t.into();
    zip::DateTime::from_date_and_time(
        u16::try_from(t.year()).ok()?,
        t.month() as u8,
        t.day() as u8,
        t.hour() as u8,
        t.minute() as u8,
        t.second() as u8,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sync-core-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        names
    }

    #[test]
    fn zips_once_a_week_and_keeps_the_newest() {
        let root = temp("backup");
        let notes = root.join("notes");
        fs::create_dir_all(notes.join("Work/empty")).unwrap();
        fs::write(notes.join("Work/a.md"), "# A").unwrap();
        fs::write(notes.join("b.md"), "B").unwrap();
        let backups = root.join("backups");
        let day = |d| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();

        let first = weekly(&notes, &backups, 3, day(1)).unwrap().unwrap();
        assert!(first.ends_with("notes-2026-10-01.zip"));
        let mut archive = zip::ZipArchive::new(File::open(&first).unwrap()).unwrap();
        let mut entries: Vec<_> = archive.file_names().map(|n| n.unwrap().to_string()).collect();
        entries.sort();
        assert_eq!(entries, ["Work/a.md", "Work/empty/", "b.md"]);
        assert_eq!(archive.by_name("Work/a.md").unwrap().size(), 3);

        // Not again within the week
        assert_eq!(weekly(&notes, &backups, 3, day(1)).unwrap(), None);
        assert_eq!(weekly(&notes, &backups, 3, day(7)).unwrap(), None);
        // A week later, and then whenever the newest is a week old
        assert!(weekly(&notes, &backups, 3, day(8)).unwrap().is_some());
        assert!(weekly(&notes, &backups, 3, day(20)).unwrap().is_some());
        assert_eq!(weekly(&notes, &backups, 3, day(26)).unwrap(), None);
        assert!(weekly(&notes, &backups, 3, day(27)).unwrap().is_some());
        assert_eq!(names(&backups), ["notes-2026-10-08.zip", "notes-2026-10-20.zip", "notes-2026-10-27.zip"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zip_from_the_future_doesnt_stop_backups() {
        let root = temp("backup-future");
        let (notes, backups) = (root.join("notes"), root.join("backups"));
        fs::create_dir_all(&notes).unwrap();
        fs::create_dir_all(&backups).unwrap();
        fs::write(backups.join("notes-2030-01-01.zip"), "").unwrap();
        let made = weekly(&notes, &backups, 3, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()).unwrap();
        assert!(made.is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn by_hand_replaces_todays_zip_and_ignores_the_week() {
        let root = temp("backup-now");
        let (notes, backups) = (root.join("notes"), root.join("backups"));
        fs::create_dir_all(&notes).unwrap();
        fs::write(notes.join("a.md"), "A").unwrap();
        let day = |d| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        weekly(&notes, &backups, 3, day(1)).unwrap();
        fs::write(notes.join("b.md"), "B").unwrap();
        let mut last = Progress::default();
        let zip = now(&notes, &backups, 3, day(1), |p| last = p).unwrap();
        assert_eq!(last, Progress { files: 2, total: 2, bytes: 2 }, "follows it to the end");
        let mut archive = zip::ZipArchive::new(File::open(&zip).unwrap()).unwrap();
        assert!(archive.by_name("b.md").is_ok(), "today's zip was replaced");
        now(&notes, &backups, 3, day(2), |_| {}).unwrap();
        assert_eq!(names(&backups), ["notes-2026-10-01.zip", "notes-2026-10-02.zip"]);
        // Turned off: the one made by hand stays, until the next
        now(&notes, &backups, 0, day(3), |_| {}).unwrap();
        assert_eq!(names(&backups), ["notes-2026-10-03.zip"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn keep_zero_turns_it_off() {
        let root = temp("backup-off");
        assert_eq!(weekly(&root, &root.join("b"), 0, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()).unwrap(), None);
        assert!(!root.join("b").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
