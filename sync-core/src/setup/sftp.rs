//! SFTP servers, for example a Hetzner Storage Box: the server's address, and
//! its key. On the first connection the server's public key is read with
//! Windows' `ssh-keyscan` and kept in `known_hosts` in the app folder; rclone
//! then refuses a server that shows another key (someone in between).

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// The server keys rclone checks, next to rclone.conf
pub const KNOWN_HOSTS: &str = "known_hosts";
/// rclone.conf's value for it: rclone puts in the folder of rclone.conf, so the app folder can move
pub const KNOWN_HOSTS_SETTING: &str = "${RCLONE_CONFIG_DIR}/known_hosts";
/// A new server's key while the connection is tried; it becomes known_hosts once it works
pub const KNOWN_HOSTS_TRIAL: &str = "known_hosts.new";
pub const KNOWN_HOSTS_TRIAL_SETTING: &str = "${RCLONE_CONFIG_DIR}/known_hosts.new";
const DEFAULT_PORT: u16 = 22;
/// A Hetzner Storage Box has SFTP (and md5sum and sha1sum for checksums) on port 23
const STORAGE_BOX_PORT: u16 = 23;

/// `u12345.your-storagebox.de`, `sftp://host:23`, `host:2222` → host and port.
/// Without a port in the address, `port` (as typed; empty = the usual one).
pub fn parse_server(server: &str, port: &str) -> Result<(String, u16), String> {
    let s = server.trim().trim_start_matches("sftp://").trim_start_matches("ssh://").trim_end_matches('/');
    let s = s.rsplit_once('@').map_or(s, |(_, host)| host);
    let (host, from_address) = match s.rsplit_once(':') {
        Some((host, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (host, Some(p)),
        _ => (s, None),
    };
    if host.is_empty() || host.contains(['/', ' ', ':']) {
        return Err("Enter the server's address, for example u12345.your-storagebox.de".into());
    }
    let port = match from_address.or(Some(port.trim()).filter(|p| !p.is_empty())) {
        Some(p) => p.parse::<u16>().ok().filter(|&p| p > 0).ok_or("The port is a number, usually 22 (23 for a Hetzner Storage Box).")?,
        None if is_storage_box(host) => STORAGE_BOX_PORT,
        None => DEFAULT_PORT,
    };
    Ok((host.to_lowercase(), port))
}

pub fn is_storage_box(host: &str) -> bool {
    host.to_lowercase().ends_with(".your-storagebox.de")
}

/// The server's public keys in known_hosts form (`[host]:23 ssh-ed25519 AAAA…`).
pub fn scan_keys(host: &str, port: u16) -> Result<String, String> {
    let mut command = Command::new(keyscan());
    command.args(["-T", "15", "-p", &port.to_string(), host]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let out = command.output().map_err(|_| {
        "Can't read the server's key: Windows' OpenSSH client is missing. Add it in Settings > System > Optional features > OpenSSH Client.".to_string()
    })?;
    let keys: String = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| format!("{l}\n"))
        .collect();
    if keys.is_empty() {
        return Err(format!("Can't reach an SFTP server at {host}, port {port}. Check the address and the port."));
    }
    Ok(keys)
}

/// Windows' own ssh-keyscan (Settings > Optional features > OpenSSH Client), or one on the PATH.
fn keyscan() -> PathBuf {
    let windows = std::env::var("SystemRoot").map(|root| Path::new(&root).join(r"System32\OpenSSH\ssh-keyscan.exe"));
    match windows {
        Ok(path) if path.exists() => path,
        _ => PathBuf::from("ssh-keyscan"),
    }
}

/// The fingerprint people compare a server's key with: `SHA256:…` of the
/// ED25519 key if there is one, else of the first, with its type.
pub fn fingerprint(known_hosts: &str) -> Option<String> {
    let keys: Vec<(&str, &str)> = known_hosts
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            Some((parts.nth(1)?, parts.next()?))
        })
        .collect();
    let (kind, key) = keys.iter().find(|(kind, _)| *kind == "ssh-ed25519").or(keys.first())?;
    let hash = Sha256::digest(base64_decode(key)?);
    let kind = kind.trim_start_matches("ssh-").to_uppercase();
    Some(format!("SHA256:{} ({kind})", super::http::base64(&hash).trim_end_matches('=')))
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut bits, mut count) = (0u32, 0);
    for c in text.trim_end_matches('=').bytes() {
        bits = bits << 6 | u32::from(value(c)?);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_server() {
        let parsed = |server, port| parse_server(server, port).unwrap();
        assert_eq!(parsed("u12345.your-storagebox.de", ""), ("u12345.your-storagebox.de".to_string(), 23));
        assert_eq!(parsed("sftp://you@Files.Example.com:2222/", ""), ("files.example.com".to_string(), 2222));
        assert_eq!(parsed("files.example.com", ""), ("files.example.com".to_string(), 22));
        assert_eq!(parsed("files.example.com", " 2200 "), ("files.example.com".to_string(), 2200));
        assert_eq!(parsed("u12345.your-storagebox.de", "22"), ("u12345.your-storagebox.de".to_string(), 22));
        assert!(parse_server("", "").is_err());
        assert!(parse_server("files.example.com", "ssh").is_err());
        assert!(parse_server("files.example.com/notes", "").is_err());
    }

    #[test]
    fn computes_the_fingerprint() {
        // A key and the fingerprint `ssh-keygen -lf` shows for it
        let known = "# comment\n[files.example.com]:23 ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAgQC7\n\
            [files.example.com]:23 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB9kq7n01mXcRShedXZP3qlPgp3+zvajObBWfQnbvFp+\n";
        assert_eq!(fingerprint(known).as_deref(), Some("SHA256:OiYss9l5N3WGroe04ZyWLNaDR2AsnhAKCQB3Qkbv72Y (ED25519)"));
        assert_eq!(fingerprint("nothing"), None);
    }

    #[test]
    fn decodes_base64() {
        assert_eq!(base64_decode("Zm9v"), Some(b"foo".to_vec()));
        assert_eq!(base64_decode("Zm8="), Some(b"fo".to_vec()));
        assert_eq!(base64_decode("Zg=="), Some(b"f".to_vec()));
        assert_eq!(base64_decode("not base64!"), None);
    }
}
