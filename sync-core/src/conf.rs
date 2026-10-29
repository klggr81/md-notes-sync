//! sync.conf: one `KEY=VALUE` per line, lines starting with `#` are comments.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

pub const FILE_NAME: &str = "sync.conf";

/// Keys a sync can't run without.
pub const REQUIRED: [&str; 3] = ["LOCAL", "REMOTE", "RCLONE"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Conf {
    values: BTreeMap<String, String>,
}

#[derive(Debug)]
pub enum ConfError {
    NotFound,
    Unreadable(std::io::Error),
    Missing(&'static str),
}

impl fmt::Display for ConfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfError::NotFound => write!(f, "Not set up yet: sync.conf not found."),
            ConfError::Unreadable(e) => write!(f, "sync.conf can't be read: {e}"),
            ConfError::Missing(key) => write!(f, "sync.conf is missing {key}"),
        }
    }
}

impl std::error::Error for ConfError {}

impl Conf {
    /// Reads `sync.conf` from the given folder (the app folder).
    pub fn load(dir: &Path) -> Result<Self, ConfError> {
        match std::fs::read_to_string(dir.join(FILE_NAME)) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ConfError::NotFound),
            Err(e) => Err(ConfError::Unreadable(e)),
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut values = BTreeMap::new();
        // The setup writes UTF-8 without BOM, but an editor may add one
        for line in text.trim_start_matches('\u{feff}').lines() {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                if !key.is_empty() {
                    values.insert(key.to_string(), value.trim().to_string());
                }
            }
        }
        Conf { values }
    }

    /// The value as written, or `None` if the key is missing or empty.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }

    /// Fails if LOCAL, REMOTE or RCLONE is missing.
    pub fn check(&self) -> Result<(), ConfError> {
        match REQUIRED.into_iter().find(|key| self.get(key).is_none()) {
            Some(key) => Err(ConfError::Missing(key)),
            None => Ok(()),
        }
    }

    /// Local notes folder, with `%USERPROFILE%` and other variables expanded.
    pub fn local(&self) -> Option<String> {
        self.get("LOCAL").map(expand_env)
    }

    /// `<rclone remote>:<Notes folder on the server>`
    pub fn remote(&self) -> Option<&str> {
        self.get("REMOTE")
    }

    /// rclone program as written (`rclone`, `rclone\rclone.exe`, …), variables expanded.
    pub fn rclone(&self) -> Option<String> {
        self.get("RCLONE").map(expand_env)
    }
}

/// Expands `%NAME%` like Windows' ExpandEnvironmentVariables: unknown names
/// are left as they are.
pub fn expand_env(s: &str) -> String {
    expand_with(s, |name| std::env::var(name).ok())
}

fn expand_with(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match Some(name).filter(|n| !n.is_empty()).and_then(&lookup) {
            Some(value) => {
                out.push_str(&value);
                rest = &after[end + 1..];
            }
            // Keep the "%" and look again from the next one, as Windows does
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "\u{feff}# md-notes-sync settings\n\
        LOCAL=%USERPROFILE%\\Notes\r\n\
        \n\
        # Sync target\n\
        REMOTE = Nextcloud:Notizen \n\
        RCLONE=rclone\\rclone.exe\n\
        EXTRA_FLAGS=--conflict-resolve newer --max-delete 25\n\
        BACKUP_KEEP=\n\
        no equals sign here\n";

    #[test]
    fn parses_values_comments_and_bom() {
        let conf = Conf::parse(EXAMPLE);
        assert_eq!(conf.remote(), Some("Nextcloud:Notizen"));
        assert_eq!(conf.get("EXTRA_FLAGS"), Some("--conflict-resolve newer --max-delete 25"));
        assert_eq!(conf.get("BACKUP_KEEP"), None, "empty counts as missing");
        assert_eq!(conf.get("# Sync target"), None);
        assert!(conf.check().is_ok());
    }

    #[test]
    fn value_may_contain_equals_signs() {
        let conf = Conf::parse("EXTRA_FLAGS=--backup-dir=x\n");
        assert_eq!(conf.get("EXTRA_FLAGS"), Some("--backup-dir=x"));
    }

    #[test]
    fn reports_the_first_missing_key() {
        let err = Conf::parse("LOCAL=C:\\Notes\nRCLONE=rclone\n").check().unwrap_err();
        assert_eq!(err.to_string(), "sync.conf is missing REMOTE");
    }

    #[test]
    fn expands_known_variables_and_keeps_unknown_ones() {
        let lookup = |name: &str| (name == "USERPROFILE").then(|| "C:\\Users\\you".to_string());
        assert_eq!(expand_with("%USERPROFILE%\\Notes", lookup), "C:\\Users\\you\\Notes");
        assert_eq!(expand_with("%NOPE%\\x", lookup), "%NOPE%\\x");
        assert_eq!(expand_with("50%%USERPROFILE%", lookup), "50%C:\\Users\\you");
        assert_eq!(expand_with("100% done", lookup), "100% done");
        assert_eq!(expand_with("%%", lookup), "%%");
    }

    #[test]
    fn load_reports_a_missing_file() {
        let dir = std::env::temp_dir().join("sync-core-test-no-conf");
        let _ = std::fs::create_dir_all(&dir);
        assert!(matches!(Conf::load(&dir), Err(ConfError::NotFound)));
    }
}
