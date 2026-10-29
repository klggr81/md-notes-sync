//! HTTPS for the setup: downloading rclone and talking to Nextcloud. Uses
//! Windows' own TLS and certificate store, so a server with a company
//! certificate that Windows trusts works too.

use std::io::Read;
use std::time::Duration;

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

pub fn agent() -> Agent {
    Agent::config_builder()
        .tls_config(TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build())
        // Nextcloud shows this as the name of the device in its security settings
        .user_agent(concat!("md-notes-sync/", env!("CARGO_PKG_VERSION"), " (Windows)"))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(600)))
        .build()
        .into()
}

/// Downloads `url` into memory; `on_progress(bytes so far, total)` follows it.
pub fn download(url: &str, host: &str, mut on_progress: impl FnMut(u64, Option<u64>)) -> Result<Vec<u8>, String> {
    let response = agent().get(url).call().map_err(|e| describe(&e, host))?;
    let total = response.body().content_length();
    let mut reader = response.into_body().into_reader();
    let mut data = Vec::with_capacity(total.unwrap_or(30_000_000) as usize);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("The download stopped: {e}"))?;
        if n == 0 {
            return Ok(data);
        }
        data.extend_from_slice(&buf[..n]);
        on_progress(data.len() as u64, total);
    }
}

/// What went wrong, in words, for a request to `host`.
pub fn describe(e: &ureq::Error, host: &str) -> String {
    match e {
        ureq::Error::StatusCode(401) => "The server didn't accept the login.".into(),
        ureq::Error::StatusCode(404) => format!("{host} doesn't know this address (404)."),
        ureq::Error::StatusCode(code) => format!("{host} answered with error {code}."),
        ureq::Error::Timeout(_) => format!("{host} didn't answer in time."),
        ureq::Error::HostNotFound => format!("{host} wasn't found. Check the address."),
        // A server without HTTPS, or a certificate Windows doesn't trust
        ureq::Error::NativeTls(_) | ureq::Error::Tls(_) => {
            format!("Can't open a secure (HTTPS) connection to {host}. The app only connects over HTTPS, with a certificate Windows trusts.")
        }
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) => format!("Can't reach {host}. Check the address and the internet connection."),
        other => format!("Can't reach {host}: {other}"),
    }
}

/// `Authorization` header for a user name and password.
pub fn basic_auth(user: &str, password: &str) -> String {
    format!("Basic {}", base64(format!("{user}:{password}").as_bytes()))
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// One part of a URL path, with everything but letters, digits and `-._~` percent-encoded.
pub fn encode_segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(basic_auth("you", "pa:ß"), "Basic eW91OnBhOsOf");
    }

    #[test]
    fn encodes_path_segments() {
        assert_eq!(encode_segment("you@example.com"), "you%40example.com");
        assert_eq!(encode_segment("Anna Maria"), "Anna%20Maria");
        assert_eq!(encode_segment("a-b_c.d~"), "a-b_c.d~");
    }
}
