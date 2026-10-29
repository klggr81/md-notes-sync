//! Downloads the current rclone from rclone.org into `rclone\rclone.exe` in the
//! app folder, after checking the download against the release's SHA256SUMS.

use std::io::{Cursor, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::http;
use super::rclone::Rclone;

const DOWNLOADS: &str = "https://downloads.rclone.org";
const HOST: &str = "downloads.rclone.org";

/// Installs rclone; `on_progress(bytes so far, total)` follows the download.
pub fn install(dir: &Path, on_progress: impl FnMut(u64, Option<u64>)) -> Result<Rclone, String> {
    let agent = http::agent();
    let get_text = |url: &str| -> Result<String, String> {
        agent.get(url).call().and_then(|mut r| r.body_mut().read_to_string()).map_err(|e| http::describe(&e, HOST))
    };

    let version = parse_version(&get_text(&format!("{DOWNLOADS}/version.txt"))?)
        .ok_or("Couldn't read the current rclone version from rclone.org.")?;
    let name = format!("rclone-{version}-windows-{}", arch());
    let sums = get_text(&format!("{DOWNLOADS}/{version}/SHA256SUMS"))?;
    let expected = parse_sum(&sums, &format!("{name}.zip")).ok_or("The checksum of the download is missing on rclone.org.")?;

    let zip = http::download(&format!("{DOWNLOADS}/{version}/{name}.zip"), HOST, on_progress)?;

    let actual: String = Sha256::digest(&zip).iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err("The download doesn't match its checksum. Nothing was installed.".into());
    }

    let exe = extract_exe(&zip)?;
    let target_dir = dir.join("rclone");
    std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    let target = target_dir.join("rclone.exe");
    let part = target_dir.join("rclone.exe.part");
    std::fs::write(&part, exe).map_err(|e| format!("Can't write {}: {e}", part.display()))?;
    std::fs::rename(&part, &target).map_err(|e| format!("Can't write {}: {e}", target.display()))?;
    Ok(Rclone::new(target, dir))
}

/// The rclone build for this app: 64-bit, or ARM64 for an ARM64 build of the app.
fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86" => "386",
        _ => "amd64",
    }
}

/// `rclone v1.71.2` → `v1.71.2`
fn parse_version(text: &str) -> Option<String> {
    let start = text.find('v')?;
    let version: String = text[start..].chars().take_while(|c| *c == 'v' || *c == '.' || c.is_ascii_digit()).collect();
    (version.matches('.').count() == 2).then_some(version)
}

/// The SHA-256 of `file` in SHA256SUMS (`<hash>  <file>`; the file is PGP-signed text).
fn parse_sum(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == file && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_lowercase())
    })
}

/// Only rclone.exe is taken from the zip.
fn extract_exe(zip: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("The download isn't a valid zip: {e}"))?;
    let name = archive
        .file_names()
        .filter_map(Result::ok)
        .find(|n| n == "rclone.exe" || n.ends_with("/rclone.exe"))
        .map(|n| n.into_owned())
        .ok_or("rclone.exe isn't in the download.")?;
    let mut file = archive.by_name(&name).map_err(|e| e.to_string())?;
    let mut exe = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut exe).map_err(|e| e.to_string())?;
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version() {
        assert_eq!(parse_version("rclone v1.71.2\n").as_deref(), Some("v1.71.2"));
        assert_eq!(parse_version("nothing"), None);
    }

    #[test]
    fn reads_the_checksum() {
        let sums = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA1\n\n\
            0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  rclone-v1.71.2-windows-arm64.zip\n\
            ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789  rclone-v1.71.2-windows-amd64.zip\n";
        assert_eq!(
            parse_sum(sums, "rclone-v1.71.2-windows-amd64.zip").as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
        );
        assert_eq!(parse_sum(sums, "rclone-v1.71.2-linux-amd64.zip"), None);
    }

    /// Downloads the real rclone (about 30 MB): `cargo test -p sync-core -- --ignored`
    #[test]
    #[ignore]
    fn installs_rclone() {
        let dir = std::env::temp_dir().join(format!("sync-core-test-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rclone = install(&dir, |_, _| {}).unwrap();
        assert!(rclone.version().unwrap().starts_with("rclone v"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
