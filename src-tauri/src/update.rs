//! Check for updates: asks GitHub for the newest release and, on request, puts
//! the exe from its zip in place of this one. The zip is checked against the
//! SHA-256 that GitHub lists for it. The restart is up to the caller.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sync_core::setup::http;

const HOST: &str = "github.com";
const EXE: &str = "md-notes-sync.exe";
/// The files of the zip that are replaced along with the exe (and the screenshots in docs\)
const DOCS: [&str; 5] = ["README.md", "SETUP.md", "CHANGELOG.md", "LICENSE", "THIRD-PARTY-NOTICES.html"];
/// Passed to the new version: `--updated <process id of the old one>`
pub const UPDATED_ARG: &str = "--updated";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>`, computed by GitHub
    digest: Option<String>,
}

/// The newest release, for the About window.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Latest {
    /// `0.2.0`
    version: String,
    /// Newer than this app
    newer: bool,
    /// The release page, with what's new
    page: String,
    /// The release has the zip and its checksum, so the app can install it
    installable: bool,
}

/// The latest release of the repository in Cargo.toml. A debug build can be
/// pointed elsewhere with `MD_NOTES_SYNC_UPDATE_URL`, for testing.
fn api_url() -> String {
    if cfg!(debug_assertions)
        && let Ok(url) = std::env::var("MD_NOTES_SYNC_UPDATE_URL")
    {
        return url;
    }
    let repo = env!("CARGO_PKG_REPOSITORY").trim_start_matches("https://github.com/");
    format!("https://api.github.com/repos/{repo}/releases/latest")
}

fn latest_release() -> Result<Release, String> {
    http::agent()
        .get(&api_url())
        .header("Accept", "application/vnd.github+json")
        .call()
        .and_then(|mut r| r.body_mut().read_json::<Release>())
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "No version has been published on GitHub yet.".into(),
            ureq::Error::StatusCode(403 | 429) => "GitHub limits how often it can be asked. Try again in an hour.".into(),
            e => http::describe(&e, HOST),
        })
}

/// The release's zip and its SHA-256, if GitHub lists one.
fn zip_asset<'a>(release: &'a Release, version: &str) -> Option<(&'a Asset, String)> {
    let asset = release.assets.iter().find(|a| a.name == format!("md-notes-sync-{version}.zip"))?;
    let sum = asset.digest.as_deref()?.strip_prefix("sha256:")?;
    (sum.len() == 64 && sum.chars().all(|c| c.is_ascii_hexdigit())).then(|| (asset, sum.to_lowercase()))
}

/// `v0.2.0` → `0.2.0`
fn version_of(release: &Release) -> String {
    release.tag_name.trim_start_matches('v').to_string()
}

/// `1.2.3` as numbers; a pre-release (`1.2.3-beta`) counts as `1.2.3`.
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse().ok());
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(parsed)
}

fn is_newer(version: &str, than: &str) -> bool {
    matches!((parse_version(version), parse_version(than)), (Some(a), Some(b)) if a > b)
}

pub fn check() -> Result<Latest, String> {
    let release = latest_release()?;
    let version = version_of(&release);
    Ok(Latest {
        newer: is_newer(&version, env!("CARGO_PKG_VERSION")),
        installable: zip_asset(&release, &version).is_some(),
        page: release.html_url.clone(),
        version,
    })
}

/// Downloads the newest release and puts its exe in place of this one, which
/// becomes `md-notes-sync.exe.old` (removed by `clean_up` at the next start).
/// `on_progress(bytes so far, total)` follows the download. Returns the new version.
pub fn install(on_progress: impl FnMut(u64, Option<u64>)) -> Result<String, String> {
    let release = latest_release()?;
    let version = version_of(&release);
    if !is_newer(&version, env!("CARGO_PKG_VERSION")) {
        return Err("This is already the newest version.".into());
    }
    let (asset, expected) =
        zip_asset(&release, &version).ok_or("The app can't install this release by itself. Download it from the release page.")?;
    let zip = http::download(&asset.browser_download_url, HOST, on_progress)?;
    let actual: String = Sha256::digest(&zip).iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err("The download doesn't match its checksum. Nothing was installed.".into());
    }

    let files = unpack(&zip)?;
    let exe = std::env::current_exe().map_err(|e| format!("Can't find the app's own exe: {e}"))?;
    let dir = exe.parent().ok_or("Can't find the app's folder")?;
    let new_exe = files.iter().find(|(path, _)| path == Path::new(EXE)).map(|(_, data)| data).ok_or("md-notes-sync.exe isn't in the download.")?;
    replace_exe(&exe, new_exe)?;
    // The exe is what counts: an old README is no reason to fail
    for (path, data) in files.iter().filter(|(path, _)| path != Path::new(EXE)) {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(target, data);
    }
    Ok(version)
}

/// The exe and the docs from the zip, which has them in one folder (`md-notes-sync-<version>\`).
fn unpack(zip: &[u8]) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("The download isn't a valid zip: {e}"))?;
    let mut files = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(path) = file.enclosed_name().filter(|_| file.is_file()) else { continue };
        let path: PathBuf = path.components().skip(1).collect();
        if !wanted(&path) {
            continue;
        }
        let mut data = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        files.push((path, data));
    }
    Ok(files)
}

/// The exe, the docs and the screenshots, nothing else.
fn wanted(path: &Path) -> bool {
    let parts: Vec<&str> = path.iter().filter_map(|p| p.to_str()).collect();
    match parts.as_slice() {
        [name] => *name == EXE || DOCS.contains(name),
        ["docs", name] => name.ends_with(".png"),
        _ => false,
    }
}

/// Windows lets a running exe be renamed, not overwritten: this one moves
/// aside, the new one takes its name.
fn replace_exe(exe: &Path, data: &[u8]) -> Result<(), String> {
    let new = exe.with_extension("exe.new");
    let old = exe.with_extension("exe.old");
    std::fs::write(&new, data).map_err(|e| format!("Can't write {}: {e}", new.display()))?;
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("Can't replace {}: {e}", exe.display()));
    }
    if let Err(e) = std::fs::rename(&new, exe) {
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err(format!("Can't replace {}: {e}", exe.display()));
    }
    Ok(())
}

/// Removes what an update left behind: the old exe.
pub fn clean_up() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
        let _ = std::fs::remove_file(exe.with_extension("exe.new"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.3"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-beta", "0.2.0"));
        assert!(!is_newer("nonsense", "0.1.0"));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn takes_only_the_exe_and_docs() {
        assert!(wanted(Path::new("md-notes-sync.exe")));
        assert!(wanted(Path::new("README.md")));
        assert!(wanted(Path::new("SETUP.md")));
        assert!(wanted(Path::new("docs/home.png")));
        assert!(!wanted(Path::new("sync.conf")));
        assert!(!wanted(Path::new("docs/sub/home.png")));
        assert!(!wanted(Path::new("rclone/rclone.exe")));
    }

    #[test]
    fn finds_the_zip_and_its_checksum() {
        let sum = "AB".repeat(32);
        let release = Release {
            tag_name: "v0.2.0".into(),
            html_url: String::new(),
            assets: vec![
                Asset { name: "md-notes-sync-0.2.0.zip".into(), browser_download_url: "x".into(), digest: Some(format!("sha256:{sum}")) },
                Asset { name: "other.zip".into(), browser_download_url: "y".into(), digest: None },
            ],
        };
        let (asset, found) = zip_asset(&release, &version_of(&release)).unwrap();
        assert_eq!(asset.browser_download_url, "x");
        assert_eq!(found, sum.to_lowercase());
        assert!(zip_asset(&release, "0.3.0").is_none());
    }
}
