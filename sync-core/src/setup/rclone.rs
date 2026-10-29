//! rclone as the setup uses it: its version, the remotes in rclone.conf, the
//! folders in the cloud, and the browser sign-in of Dropbox and Google Drive.
//! The app's remotes live in rclone.conf in the app folder, not in rclone's
//! normal config; remotes from the normal config can be copied over.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::runner::{RCLONE_CONF, find_rclone};

/// The settings of one remote, as `rclone config dump` shows them.
pub type Remote = BTreeMap<String, String>;

pub struct Rclone {
    exe: PathBuf,
    config: PathBuf,
}

impl Rclone {
    /// rclone for the app in `dir`, with its rclone.conf.
    pub fn new(exe: PathBuf, dir: &Path) -> Self {
        Rclone { exe, config: dir.join(RCLONE_CONF) }
    }

    /// rclone next to the app (`rclone\rclone.exe`) or on the PATH.
    pub fn find(dir: &Path) -> Option<Self> {
        find_rclone(dir, r"rclone\rclone.exe").or_else(|| find_rclone(dir, "rclone")).map(|exe| Rclone::new(exe, dir))
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// With the app's rclone.conf, without a console window.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.exe);
        command.env("RCLONE_CONFIG", &self.config).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
    }

    /// Runs rclone; on failure, its last lines of output say why.
    fn run(&self, mut command: Command) -> Result<String, String> {
        let out = command.output().map_err(|e| format!("Can't run rclone: {e}"))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        Err(last_lines(&String::from_utf8_lossy(&out.stderr), 3))
    }

    fn rclone(&self, args: &[&str]) -> Result<String, String> {
        let mut command = self.command();
        command.args(args);
        self.run(command)
    }

    /// `rclone v1.71.2`
    pub fn version(&self) -> Option<String> {
        self.rclone(&["version"]).ok()?.lines().next().map(|l| l.trim().to_string())
    }

    /// The remotes in the app's rclone.conf.
    pub fn remotes(&self) -> BTreeMap<String, Remote> {
        self.dump(self.command())
    }

    /// The remotes in rclone's normal config (`%APPDATA%\rclone\rclone.conf`),
    /// which the user may have set up before.
    pub fn default_remotes(&self) -> BTreeMap<String, Remote> {
        let mut command = self.command();
        command.env_remove("RCLONE_CONFIG");
        self.dump(command)
    }

    fn dump(&self, mut command: Command) -> BTreeMap<String, Remote> {
        command.args(["config", "dump"]);
        let Ok(json) = self.run(command) else { return BTreeMap::new() };
        // Every value is a string, also a token (JSON in a string)
        let parsed: BTreeMap<String, BTreeMap<String, serde_json::Value>> = serde_json::from_str(&json).unwrap_or_default();
        parsed
            .into_iter()
            .map(|(name, props)| {
                let props = props.into_iter().map(|(k, v)| (k, v.as_str().map_or_else(|| v.to_string(), str::to_string)));
                (name, props.collect())
            })
            .collect()
    }

    /// Writes a remote into the app's rclone.conf, replacing one of the same
    /// name. Written as text, so a token's JSON never passes through a command line.
    pub fn write_remote(&self, name: &str, props: &Remote) -> Result<(), String> {
        let _ = self.rclone(&["config", "delete", name]);
        let mut text = format!("\r\n[{name}]\r\n");
        for (key, value) in props {
            text.push_str(&format!("{key} = {value}\r\n"));
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.config)
            .map_err(|e| format!("Can't write {}: {e}", self.config.display()))?;
        file.write_all(text.as_bytes()).map_err(|e| format!("Can't write {}: {e}", self.config.display()))
    }

    pub fn delete_remote(&self, name: &str) -> Result<(), String> {
        self.rclone(&["config", "delete", name]).map(|_| ())
    }

    /// Can rclone reach the remote? Lists its top folders.
    pub fn test(&self, remote: &str) -> Result<(), String> {
        self.rclone(&["lsd", &format!("{remote}:"), "--max-depth", "1"]).map(|_| ())
    }

    /// The folders in a folder (`Nextcloud:` for the top), sorted.
    pub fn dirs(&self, path: &str) -> Result<Vec<String>, String> {
        let out = self.rclone(&["lsf", path, "--dirs-only"])?;
        let mut dirs: Vec<String> = out.lines().map(|l| l.trim_end_matches('/').to_string()).filter(|l| !l.is_empty()).collect();
        dirs.sort_by_key(|d| d.to_lowercase());
        Ok(dirs)
    }

    /// A folder or file exists.
    pub fn exists(&self, path: &str) -> bool {
        self.rclone(&["lsf", path, "--max-depth", "1"]).is_ok()
    }

    pub fn mkdir(&self, path: &str) -> Result<(), String> {
        self.rclone(&["mkdir", path]).map(|_| ())
    }

    pub fn copyto(&self, from: &str, to: &str) -> Result<(), String> {
        self.rclone(&["copyto", from, to]).map(|_| ())
    }

    /// The form rclone.conf keeps a password in. Through stdin, so no special
    /// character ever touches a command line.
    pub fn obscure(&self, secret: &str) -> Result<String, String> {
        let mut command = self.command();
        command.args(["obscure", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("Can't run rclone: {e}"))?;
        child.stdin.take().expect("stdin is piped").write_all(secret.as_bytes()).map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let obscured = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || obscured.is_empty() {
            return Err("rclone couldn't store the password.".into());
        }
        // Check it comes back unchanged before anything is saved
        if self.reveal(&obscured)? != secret {
            return Err("The password couldn't be stored unchanged. Nothing was saved.".into());
        }
        Ok(obscured)
    }

    pub fn reveal(&self, obscured: &str) -> Result<String, String> {
        let out = self.rclone(&["reveal", obscured])?;
        Ok(out.trim_end_matches(['\r', '\n']).to_string())
    }

    /// Signs in through the browser (`rclone authorize dropbox|drive`): rclone
    /// opens the sign-in page and waits for the browser to come back. Returns
    /// the token. `page` is the page the browser shows afterwards; `on_link`
    /// gets the sign-in link, for when no browser opens. Setting `cancel` stops it.
    pub fn authorize(
        &self,
        backend: &str,
        page: Option<&Path>,
        cancel: &AtomicBool,
        mut on_link: impl FnMut(&str),
    ) -> Result<String, String> {
        let mut command = self.command();
        command.args(["authorize", backend]);
        if let Some(page) = page {
            command.arg("--template").arg(page);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("Can't run rclone: {e}"))?;

        // Both streams line by line; the token comes on stdout, the link on stderr
        let (tx, rx) = mpsc::channel::<String>();
        let readers: Vec<_> = [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .map(|stream| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let _ = tx.send(line);
                }
            })
        })
        .collect();
        drop(tx);

        let mut output = String::new();
        let mut link_sent = false;
        let status = loop {
            while let Ok(line) = rx.try_recv() {
                if !link_sent && let Some(link) = sign_in_link(&line) {
                    on_link(link);
                    link_sent = true;
                }
                output.push_str(&line);
                output.push('\n');
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Sign-in cancelled.".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(200)),
                Err(e) => return Err(e.to_string()),
            }
        };
        for reader in readers {
            let _ = reader.join();
        }
        output.extend(rx.try_iter().map(|line| line + "\n"));
        match parse_token(&output) {
            Some(token) if status.success() => Ok(token),
            _ => Err(format!("The sign-in didn't work. {}", last_lines(&output, 2)).trim().to_string()),
        }
    }
}

/// The local link rclone opens for the sign-in:
/// `… go to the following link: http://127.0.0.1:53682/auth?state=…`
fn sign_in_link(line: &str) -> Option<&str> {
    let start = line.find("http://127.0.0.1:")?;
    Some(line[start..].split_whitespace().next().unwrap_or_default())
}

/// The token rclone prints between `Paste the following into your remote
/// machine --->` and `<---End paste`.
fn parse_token(output: &str) -> Option<String> {
    let mut inside = false;
    let mut token = None;
    for line in output.lines().map(str::trim) {
        if line.starts_with("Paste the following") {
            inside = true;
        } else if line.starts_with("<---End paste") {
            inside = false;
        } else if inside && !line.is_empty() {
            token = Some(line.to_string());
        }
    }
    token
}

/// The last lines of rclone's output, without the date and level in front.
fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| strip_prefix(l))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `2026/10/28 16:06:53 ERROR : Nextcloud: …` → `Nextcloud: …`
fn strip_prefix(line: &str) -> &str {
    let mut parts = line.splitn(4, ' ');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(date), Some(time), Some(_level), Some(rest)) if date.contains('/') && time.contains(':') => {
            rest.trim_start_matches([':', ' ']).trim_end()
        }
        _ => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_token_between_the_markers() {
        let out = "2026/10/28 16:00:00 NOTICE: Got code\nPaste the following into your remote machine --->\n{\"access_token\":\"x\",\"expiry\":\"2026\"}\n<---End paste\n";
        assert_eq!(parse_token(out).as_deref(), Some("{\"access_token\":\"x\",\"expiry\":\"2026\"}"));
        assert_eq!(parse_token("no token here"), None);
    }

    #[test]
    fn finds_the_sign_in_link() {
        let line = "2026/10/28 16:00:00 NOTICE: If your browser doesn't open automatically go to the following link: http://127.0.0.1:53682/auth?state=abc";
        assert_eq!(sign_in_link(line), Some("http://127.0.0.1:53682/auth?state=abc"));
        assert_eq!(sign_in_link("NOTICE: Log in and authorize rclone for access"), None);
    }

    #[test]
    fn shortens_rclone_errors() {
        let err = "2026/10/28 16:06:53 ERROR : : error listing: couldn't list files: 401 Unauthorized\n2026/10/28 16:06:53 NOTICE: Failed to lsd with 2 errors: last error was: 401 Unauthorized\n";
        assert_eq!(last_lines(err, 1), "Failed to lsd with 2 errors: last error was: 401 Unauthorized");
    }
}
