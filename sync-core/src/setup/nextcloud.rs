//! Nextcloud: signing in through the browser (Login Flow v2: Nextcloud makes an
//! app password for md-notes-sync), the WebDAV address of the user's files, and
//! the folder the Notes app keeps its notes in.

use serde_json::Value;

use super::http;

/// `https://` and the server address without anything after it, from what was
/// typed or pasted: `cloud.example.com`, `https://example.com/nextcloud/`,
/// `https://cloud.example.com/index.php/apps/files` … The setup always uses HTTPS.
pub fn base_url(input: &str) -> Result<String, String> {
    let mut s = input.trim();
    if let Some(i) = s.find("://") {
        s = &s[i + 3..];
    }
    for cut in ["/index.php", "/remote.php", "/ocs/", "/apps/", "/login"] {
        if let Some(i) = s.find(cut) {
            s = &s[..i];
        }
    }
    let s = s.trim_end_matches('/');
    if s.is_empty() {
        return Err("Enter the server address, for example cloud.example.com".into());
    }
    if s.contains(char::is_whitespace) {
        return Err("The server address can't contain spaces.".into());
    }
    Ok(format!("https://{s}"))
}

/// The server's address from a WebDAV URL in rclone.conf.
pub fn base_of(webdav_url: &str) -> &str {
    webdav_url.split("/remote.php/").next().unwrap_or(webdav_url).trim_end_matches('/')
}

pub fn webdav_url(base: &str, user_id: &str) -> String {
    format!("{base}/remote.php/dav/files/{}", http::encode_segment(user_id))
}

fn host(base: &str) -> &str {
    base.split("://").nth(1).unwrap_or(base)
}

/// A started browser sign-in.
pub struct LoginFlow {
    /// Opens in the browser: the user signs in there and allows access
    pub login_url: String,
    endpoint: String,
    token: String,
}

/// What the browser sign-in gives: a new app password for md-notes-sync.
pub struct Credentials {
    pub server: String,
    pub login_name: String,
    pub app_password: String,
}

pub fn start_login(base: &str) -> Result<LoginFlow, String> {
    let json: Value = http::agent()
        .post(&format!("{base}/index.php/login/v2"))
        .send_empty()
        .and_then(|mut r| r.body_mut().read_json())
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) | ureq::Error::Json(_) => format!("There's no Nextcloud at {}.", host(base)),
            e => http::describe(&e, host(base)),
        })?;
    let text = |v: &Value| v.as_str().map(str::to_string);
    match (text(&json["login"]), text(&json["poll"]["endpoint"]), text(&json["poll"]["token"])) {
        (Some(login_url), Some(endpoint), Some(token)) => Ok(LoginFlow { login_url, endpoint, token }),
        _ => Err(format!("There's no Nextcloud at {}.", host(base))),
    }
}

/// `None` while the user hasn't finished signing in (ask again in a moment).
/// The sign-in runs out after 20 minutes.
pub fn poll_login(flow: &LoginFlow) -> Result<Option<Credentials>, String> {
    let response = http::agent().post(&flow.endpoint).send_form([("token", flow.token.as_str())]);
    let json: Value = match response {
        Ok(mut r) => r.body_mut().read_json().map_err(|e| e.to_string())?,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(http::describe(&e, host(&flow.endpoint))),
    };
    let text = |key: &str| json[key].as_str().map(str::to_string);
    match (text("server"), text("loginName"), text("appPassword")) {
        (Some(server), Some(login_name), Some(app_password)) => {
            Ok(Some(Credentials { server: server.trim_end_matches('/').to_string(), login_name, app_password }))
        }
        _ => Err("Nextcloud's answer to the sign-in wasn't complete.".into()),
    }
}

/// GET with the login, as JSON.
fn get_json(base: &str, path: &str, login: &str, password: &str) -> Result<Value, String> {
    http::agent()
        .get(&format!("{base}{path}"))
        .header("Authorization", http::basic_auth(login, password))
        .header("OCS-APIRequest", "true")
        .header("Accept", "application/json")
        .call()
        .and_then(|mut r| r.body_mut().read_json())
        .map_err(|e| http::describe(&e, host(base)))
}

/// The user's ID, which the WebDAV address uses. It can differ from the name
/// used to sign in (for example an e-mail address).
pub fn user_id(base: &str, login: &str, password: &str) -> Result<String, String> {
    let json = get_json(base, "/ocs/v2.php/cloud/user?format=json", login, password)?;
    json["ocs"]["data"]["id"].as_str().map(str::to_string).ok_or_else(|| "Nextcloud didn't say the user's ID.".into())
}

/// The folder of the Notes app (`Notes`, `Notizen`, …): it depends on the
/// account's language and can be changed in the app. `None` if the Notes app
/// isn't installed or doesn't answer.
pub fn notes_folder(base: &str, login: &str, password: &str) -> Option<String> {
    let from_settings = get_json(base, "/index.php/apps/notes/api/v1/settings", login, password)
        .ok()
        .and_then(|s| s["notesPath"].as_str().map(str::to_string));
    let path = from_settings.or_else(|| {
        let caps = get_json(base, "/ocs/v2.php/cloud/capabilities?format=json", login, password).ok()?;
        caps["ocs"]["data"]["capabilities"]["notes"]["notes_path"].as_str().map(str::to_string)
    })?;
    let path = path.trim_matches('/').to_string();
    (!path.is_empty()).then_some(path)
}

/// Default names of the Notes folder in Nextcloud's languages: a guess when
/// the Notes app doesn't answer.
pub const KNOWN_NOTES_FOLDERS: [&str; 20] = [
    "Notes", "Notizen", "Notas", "Note", "Notities", "Notatki", "Заметки", "Poznámky", "Anteckningar", "Noter",
    "Muistiinpanot", "Notater", "Notlar", "Jegyzetek", "ノート", "笔记", "Нотатки", "Beležka", "Σημειώσεις", "Notițe",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_the_server_address() {
        assert_eq!(base_url(" cloud.example.com ").unwrap(), "https://cloud.example.com");
        assert_eq!(base_url("http://example.com/nextcloud/").unwrap(), "https://example.com/nextcloud");
        assert_eq!(base_url("https://cloud.example.com/index.php/apps/files/").unwrap(), "https://cloud.example.com");
        assert_eq!(base_url("https://cloud.example.com/remote.php/dav/files/you").unwrap(), "https://cloud.example.com");
        assert!(base_url("  ").is_err());
        assert!(base_url("cloud example").is_err());
    }

    #[test]
    fn webdav_addresses() {
        assert_eq!(webdav_url("https://c.example.com", "you@example.com"), "https://c.example.com/remote.php/dav/files/you%40example.com");
        assert_eq!(base_of("https://example.com/nc/remote.php/dav/files/you"), "https://example.com/nc");
    }
}
