//! The setup, without any window: gets rclone, connects it to the cloud,
//! checks the two folders and writes the settings. Everything goes into the
//! app folder (`dir`), next to the exe: sync.conf, rclone.conf, filters.txt,
//! rclone\rclone.exe. It never deletes or changes notes.

pub mod http;
pub mod install;
pub mod nextcloud;
pub mod rclone;
pub mod sftp;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::conf::{self, Conf};
use crate::runner::{FILTERS, WORK_DIR};
pub use rclone::Rclone;

/// What is synced: hidden folders are skipped except Nextcloud Notes' images
/// (`.attachments.*`), and so are editors' databases and system files.
const FILTERS_TXT: &[u8] = include_bytes!("../../assets/filters.txt");
/// sync.conf with comments; the setup fills in LOCAL, REMOTE and RCLONE
const CONF_TEMPLATE: &str = include_str!("../../assets/sync.conf");
/// The page the browser shows after signing in to Dropbox or Google Drive
const SIGNIN_HTML: &str = include_str!("../../assets/signin.html");
/// `--check-access` refuses to sync unless this file is on both sides
const MARKER: &str = "RCLONE_TEST";
const MARKER_TEXT: &str = "Marker file for rclone bisync --check-access (md-notes-sync). Sync refuses to run if this file is missing on either side. Do not delete.\r\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    Nextcloud,
    Dropbox,
    GoogleDrive,
    /// An SFTP server, for example a Hetzner Storage Box
    Sftp,
}

impl Provider {
    pub const ALL: [Provider; 4] = [Provider::Nextcloud, Provider::Dropbox, Provider::GoogleDrive, Provider::Sftp];

    /// Its remote's name in rclone.conf
    pub fn remote(self) -> &'static str {
        match self {
            Provider::Nextcloud => "Nextcloud",
            Provider::Dropbox => "Dropbox",
            Provider::GoogleDrive => "GoogleDrive",
            Provider::Sftp => "SFTP",
        }
    }

    /// rclone's backend
    pub fn backend(self) -> &'static str {
        match self {
            Provider::Nextcloud => "webdav",
            Provider::Dropbox => "dropbox",
            Provider::GoogleDrive => "drive",
            Provider::Sftp => "sftp",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Nextcloud => "Nextcloud",
            Provider::Dropbox => "Dropbox",
            Provider::GoogleDrive => "Google Drive",
            Provider::Sftp => "SFTP server",
        }
    }

    pub fn from_remote(remote: &str) -> Option<Self> {
        Provider::ALL.into_iter().find(|p| p.remote() == remote)
    }

    /// A remote of this kind that can be used: Nextcloud with a password,
    /// Dropbox and Google Drive with a token, SFTP with a password or a key.
    pub fn accepts(self, remote: &rclone::Remote) -> bool {
        let has = |key: &str| remote.get(key).is_some_and(|v| !v.is_empty());
        remote.get("type").map(String::as_str) == Some(self.backend())
            && match self {
                Provider::Nextcloud => remote.get("vendor").map(String::as_str) == Some("nextcloud") && has("url") && has("pass"),
                Provider::Sftp => has("host") && (has("pass") || has("key_file") || has("key_pem")),
                _ => has("token"),
            }
    }
}

/// filters.txt in the app folder; written if it's missing. One that is there
/// is kept, also if it was changed by hand.
pub fn ensure_filters(dir: &Path) -> std::io::Result<()> {
    let path = dir.join(FILTERS);
    if path.exists() { Ok(()) } else { std::fs::write(path, FILTERS_TXT) }
}

/// The page for `rclone authorize --template`, as a temporary file. Delete it afterwards.
pub fn signin_page(provider: Provider) -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("md-notes-sync-signin-{}.html", std::process::id()));
    std::fs::write(&path, SIGNIN_HTML.replace("__SERVICE__", provider.name()))?;
    Ok(path)
}

/// What sync.conf says now.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Current {
    pub provider: Option<Provider>,
    /// The folder in the cloud, without the remote's name
    pub remote_path: String,
    /// The folder on this PC, with variables expanded
    pub local: String,
}

pub fn current(dir: &Path) -> Option<Current> {
    let conf = Conf::load(dir).ok()?;
    let (remote, path) = conf.remote()?.split_once(':')?;
    Some(Current { provider: Provider::from_remote(remote), remote_path: path.to_string(), local: conf.local()? })
}

/// A folder on this PC that was checked.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFolder {
    /// Full path
    pub path: String,
    pub exists: bool,
    /// Files in it, also in subfolders (up to 100 000)
    pub files: usize,
}

/// Checks a folder for the notes on this PC: a full path, not a file, and
/// apart from the app folder (neither inside it nor around it).
pub fn check_local(dir: &Path, input: &str) -> Result<LocalFolder, String> {
    let typed = conf::expand_env(input.trim().trim_matches('"'));
    if typed.is_empty() {
        return Err("Enter a folder, for example C:\\Users\\you\\Notes".into());
    }
    let path = Path::new(&typed);
    if !path.is_absolute() {
        return Err("Enter a full path, for example C:\\Users\\you\\Notes".into());
    }
    let full = std::path::absolute(path).map_err(|e| e.to_string())?;
    let full = full.to_string_lossy().trim_end_matches('\\').to_string();
    if full.len() <= 2 {
        return Err("Choose a folder, not a whole drive.".into());
    }
    let inside = |a: &str, b: &str| {
        let (a, b) = (a.to_lowercase() + "\\", b.to_lowercase() + "\\");
        a.starts_with(&b)
    };
    let app = dir.to_string_lossy().trim_end_matches('\\').to_string();
    if inside(&full, &app) || inside(&app, &full) {
        return Err("Choose a folder apart from md-notes-sync's own folder (not inside it and not around it).".into());
    }
    let path = Path::new(&full);
    if path.is_file() {
        return Err("That is a file, not a folder.".into());
    }
    Ok(LocalFolder { exists: path.is_dir(), files: count_files(path, 100_000), path: full })
}

fn count_files(dir: &Path, max: usize) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(entry.path()),
                Ok(_) => count += 1,
                Err(_) => {}
            }
            if count >= max {
                return count;
            }
        }
    }
    count
}

/// Paths in the user folder are written as `%USERPROFILE%\…`, so sync.conf
/// also works after the user folder moves.
fn conf_path(full: &str) -> String {
    let Ok(profile) = std::env::var("USERPROFILE") else { return full.to_string() };
    let profile = profile.trim_end_matches('\\');
    if full.eq_ignore_ascii_case(profile) {
        return "%USERPROFILE%".into();
    }
    match full.get(..profile.len() + 1) {
        Some(head) if head.eq_ignore_ascii_case(&format!("{profile}\\")) => format!("%USERPROFILE%{}", &full[profile.len()..]),
        _ => full.to_string(),
    }
}

/// Sets `KEY=value` in sync.conf's text, keeping the comments; appends it if missing.
fn set_value(text: &str, key: &str, value: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            let is_key = line.trim_start().split_once('=').is_some_and(|(k, _)| k.trim() == key) && !line.trim_start().starts_with('#');
            if is_key && !found {
                found = true;
                format!("{key}={value}")
            } else {
                line.to_string()
            }
        })
        .collect();
    if !found {
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        lines.extend([String::new(), format!("{key}={value}")]);
    }
    lines.join(nl) + nl
}

/// The setup's choices.
pub struct Choice<'a> {
    pub provider: Provider,
    /// The folder in the cloud, without the remote's name
    pub remote_path: &'a str,
    /// The folder on this PC, full path (it is created if it doesn't exist)
    pub local: &'a str,
}

/// What saving did besides writing sync.conf.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    /// The folders changed: the next sync starts with a merge of both sides
    pub fresh_start: bool,
    /// Couldn't put RCLONE_TEST on both sides: the sync refuses to run until it's there
    pub marker_problem: Option<String>,
}

/// Writes sync.conf and filters.txt, creates the folder on this PC and puts
/// the RCLONE_TEST marker on both sides.
pub fn save(dir: &Path, rclone: &Rclone, choice: &Choice) -> Result<Saved, String> {
    let conf_file = dir.join(conf::FILE_NAME);
    let old = std::fs::read_to_string(&conf_file).ok();
    let old_conf = old.as_deref().map(Conf::parse);
    let mut text = old.clone().unwrap_or_else(|| CONF_TEMPLATE.to_string());

    let local = conf_path(choice.local);
    let remote = format!("{}:{}", choice.provider.remote(), choice.remote_path.trim_matches('/'));
    // The copy next to the app is written relative to it, so the app folder can be moved
    let rclone_value = match rclone.exe().strip_prefix(dir) {
        Ok(rel) => rel.display().to_string(),
        Err(_) if crate::runner::find_rclone(dir, "rclone").as_deref() == Some(rclone.exe()) => "rclone".to_string(),
        Err(_) => rclone.exe().display().to_string(),
    };
    text = set_value(&text, "LOCAL", &local);
    text = set_value(&text, "REMOTE", &remote);
    text = set_value(&text, "RCLONE", &rclone_value);
    // Google Docs and Sheets have no real file size, which bisync can't handle: leave them out
    if choice.provider == Provider::GoogleDrive {
        let flags = Conf::parse(&text).get("EXTRA_FLAGS").unwrap_or_default().to_string();
        if !flags.contains("--drive-skip-gdocs") {
            text = set_value(&text, "EXTRA_FLAGS", format!("{flags} --drive-skip-gdocs").trim());
        }
    }

    std::fs::create_dir_all(choice.local).map_err(|e| format!("Can't create {}: {e}", choice.local))?;
    ensure_filters(dir).map_err(|e| format!("Can't write filters.txt: {e}"))?;
    std::fs::write(&conf_file, text).map_err(|e| format!("Can't write sync.conf: {e}"))?;

    // A new pair of folders starts fresh: the old bisync state belongs to the old pair
    let changed = old_conf.is_some_and(|c| c.get("LOCAL") != Some(local.as_str()) || c.get("REMOTE") != Some(remote.as_str()));
    let state = dir.join(WORK_DIR);
    let fresh_start = changed && state.exists();
    if fresh_start {
        let backup = dir.join(format!("{WORK_DIR}.old"));
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(&state, &backup).map_err(|e| format!("Can't move the old sync state aside: {e}"))?;
    }

    let marker_problem = set_markers(rclone, Path::new(choice.local), &remote).err();
    Ok(Saved { fresh_start, marker_problem })
}

/// RCLONE_TEST on both sides: copied from the side that has it, or new.
fn set_markers(rclone: &Rclone, local_dir: &Path, remote: &str) -> Result<(), String> {
    let local = local_dir.join(MARKER);
    let local_str = local.display().to_string();
    let remote_file = format!("{remote}/{MARKER}");
    let remote_has = rclone.exists(&remote_file);
    if !remote_has {
        if !local.exists() {
            std::fs::write(&local, MARKER_TEXT).map_err(|e| e.to_string())?;
        }
        rclone.copyto(&local_str, &remote_file)
    } else if !local.exists() {
        rclone.copyto(&remote_file, &local_str)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_values_and_keeps_comments() {
        let text = "# Folder\r\nLOCAL=C:\\Old\r\n# REMOTE=commented\r\nREMOTE=Nextcloud:Notes\r\n";
        let text = set_value(text, "LOCAL", "%USERPROFILE%\\Notes");
        let text = set_value(&text, "RCLONE", "rclone\\rclone.exe");
        assert_eq!(
            text,
            "# Folder\r\nLOCAL=%USERPROFILE%\\Notes\r\n# REMOTE=commented\r\nREMOTE=Nextcloud:Notes\r\n\r\nRCLONE=rclone\\rclone.exe\r\n"
        );
    }

    #[test]
    fn the_template_has_every_required_key() {
        assert!(Conf::parse(CONF_TEMPLATE).check().is_ok());
    }

    #[test]
    fn checks_the_folder_on_this_pc() {
        let app = std::env::temp_dir().join(format!("sync-core-test-app-{}", std::process::id()));
        let app_str = app.display().to_string();
        assert!(check_local(&app, &format!("{app_str}\\notes")).is_err(), "inside the app folder");
        assert!(check_local(&app, &std::env::temp_dir().display().to_string()).is_err(), "around the app folder");
        assert!(check_local(&app, "Notes").is_err(), "not a full path");
        assert!(check_local(&app, "C:\\").is_err(), "a whole drive");
        let other = format!("{app_str}-notes\\");
        let checked = check_local(&app, &other).unwrap();
        assert_eq!(checked.path, format!("{app_str}-notes"));
        assert!(!checked.exists);
    }

    #[test]
    fn knows_the_remotes() {
        let remote = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<rclone::Remote>();
        let nc = remote(&[("type", "webdav"), ("vendor", "nextcloud"), ("url", "https://c.example.com/remote.php/dav/files/you"), ("pass", "x")]);
        assert!(Provider::Nextcloud.accepts(&nc));
        assert!(!Provider::Nextcloud.accepts(&remote(&[("type", "webdav"), ("vendor", "owncloud"), ("url", "u"), ("pass", "x")])));
        assert!(Provider::Dropbox.accepts(&remote(&[("type", "dropbox"), ("token", "{}")])));
        assert!(!Provider::GoogleDrive.accepts(&remote(&[("type", "drive")])));
        assert!(Provider::Sftp.accepts(&remote(&[("type", "sftp"), ("host", "u1.your-storagebox.de"), ("pass", "x")])));
        assert!(!Provider::Sftp.accepts(&remote(&[("type", "sftp"), ("host", "u1.your-storagebox.de")])), "no way to sign in");
    }
}
