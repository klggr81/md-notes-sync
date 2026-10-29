//! The setup wizard's commands. The work is done by `sync_core::setup`; this
//! adds the window's side: progress and sign-in links as events, and a
//! Cancel for the browser sign-in.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sync_core::setup::{self, Choice, Current, LocalFolder, Provider, Rclone, Saved, nextcloud, rclone::Remote, sftp};
use tauri::{AppHandle, Emitter, Manager};

use crate::{Run, app_dir, system};

/// The running browser sign-in, so Cancel can stop it.
#[derive(Default)]
pub struct SignIn(Mutex<Option<Arc<AtomicBool>>>);

impl SignIn {
    /// A new sign-in; one still running is stopped.
    fn start(&self) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.0.lock().unwrap().replace(flag.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        flag
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RcloneInfo {
    path: String,
    version: Option<String>,
}

/// A cloud connection that works, or one that can be used.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    provider: Provider,
    /// The remote's name in rclone's normal config (only for ones that can be copied)
    name: String,
    /// For Nextcloud: `you on cloud.example.com`; for SFTP: `you on host:23`
    label: String,
    /// SFTP: the fingerprint of the server's key the app keeps, `SHA256:… (ED25519)`
    #[serde(skip_serializing_if = "Option::is_none")]
    server_key: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    rclone: Option<RcloneInfo>,
    current: Option<Current>,
    /// `C:\Users\you\Notes`
    default_local: String,
    /// The remotes in the app's rclone.conf
    connected: Vec<Connection>,
    /// Remotes in rclone's normal config that can be copied
    existing: Vec<Connection>,
    autostart: bool,
}

fn rclone_info(rclone: &Rclone) -> RcloneInfo {
    RcloneInfo { path: rclone.exe().display().to_string(), version: rclone.version() }
}

fn connection(provider: Provider, name: &str, remote: &Remote) -> Connection {
    let label = match (provider, remote.get("url"), remote.get("user")) {
        (Provider::Nextcloud, Some(url), Some(user)) => {
            format!("{user} on {}", nextcloud::base_of(url).split("://").nth(1).unwrap_or(url))
        }
        (Provider::Sftp, _, user) => {
            let host = remote.get("host").map_or("", String::as_str);
            let port = remote.get("port").filter(|p| *p != "22").map(|p| format!(":{p}")).unwrap_or_default();
            user.map_or(format!("{host}{port}"), |user| format!("{user} on {host}{port}"))
        }
        _ => provider.name().to_string(),
    };
    // Only for the key the app keeps itself
    let server_key = (remote.get("known_hosts_file").map(String::as_str) == Some(sftp::KNOWN_HOSTS_SETTING))
        .then(|| std::fs::read_to_string(app_dir().join(sftp::KNOWN_HOSTS)).ok())
        .flatten()
        .and_then(|known| sftp::fingerprint(&known));
    Connection { provider, name: name.into(), label, server_key }
}

fn connections(remotes: &std::collections::BTreeMap<String, Remote>, by_name: bool) -> Vec<Connection> {
    let mut found = Vec::new();
    for provider in Provider::ALL {
        for (name, remote) in remotes {
            // In the app's own rclone.conf only the provider's own remote counts
            if provider.accepts(remote) && (by_name || name == provider.remote()) {
                found.push(connection(provider, name, remote));
            }
        }
    }
    found
}

fn rclone() -> Result<Rclone, String> {
    Rclone::find(&app_dir()).ok_or_else(|| "rclone isn't installed yet.".into())
}

#[tauri::command(async)]
pub fn setup_info() -> Info {
    let dir = app_dir();
    let rclone = Rclone::find(&dir);
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    Info {
        current: setup::current(&dir),
        default_local: format!("{profile}\\Notes"),
        connected: rclone.as_ref().map(|r| connections(&r.remotes(), false)).unwrap_or_default(),
        existing: rclone.as_ref().map(|r| connections(&r.default_remotes(), true)).unwrap_or_default(),
        rclone: rclone.as_ref().map(rclone_info),
        autostart: system::autostart_enabled(),
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub done: u64,
    pub total: Option<u64>,
}

/// Downloads rclone from rclone.org; the window gets `setup-download` events.
#[tauri::command(async)]
pub fn install_rclone(app: AppHandle) -> Result<RcloneInfo, String> {
    let mut last = Instant::now();
    let rclone = setup::install::install(&app_dir(), |done, total| {
        if last.elapsed() > Duration::from_millis(100) || Some(done) == total {
            last = Instant::now();
            let _ = app.emit("setup-download", Download { done, total });
        }
    })?;
    Ok(rclone_info(&rclone))
}

/// Checks the connection works, then writes it as the provider's remote in
/// the app's rclone.conf.
fn connect(rclone: &Rclone, provider: Provider, props: &Remote) -> Result<Connection, String> {
    try_remote(rclone, provider, props)?;
    rclone.write_remote(provider.remote(), props)?;
    Ok(connection(provider, provider.remote(), props))
}

/// Tries a connection under a name of its own, so a failed attempt (a typo
/// in "Sign in again") leaves the connection that works as it is.
fn try_remote(rclone: &Rclone, provider: Provider, props: &Remote) -> Result<(), String> {
    let trial = format!("{}-trial", provider.remote());
    rclone.write_remote(&trial, props)?;
    let tested = rclone.test(&trial);
    let _ = rclone.delete_remote(&trial);
    tested.map_err(|e| format!("Connected, but rclone can't list the files: {e}"))
}

/// Copies a remote from rclone's normal config.
#[tauri::command(async)]
pub fn use_existing(provider: Provider, name: String) -> Result<Connection, String> {
    let rclone = rclone()?;
    let props = rclone.default_remotes().remove(&name).ok_or("That remote isn't in rclone's config any more.")?;
    connect(&rclone, provider, &props)
}

fn nextcloud_remote(rclone: &Rclone, base: &str, login: &str, password: &str) -> Result<Remote, String> {
    // The files' address uses the user's ID, which can differ from the login name
    let id = nextcloud::user_id(base, login, password)?;
    let pass = rclone.obscure(password)?;
    Ok(Remote::from([
        ("type".into(), "webdav".into()),
        ("url".into(), nextcloud::webdav_url(base, &id)),
        ("vendor".into(), "nextcloud".into()),
        ("user".into(), login.into()),
        ("pass".into(), pass),
    ]))
}

/// Signs in to Nextcloud in the browser: Nextcloud makes an app password for
/// md-notes-sync. The window gets the page's address as `setup-link`.
#[tauri::command(async)]
pub fn nextcloud_sign_in(app: AppHandle, server: String) -> Result<Connection, String> {
    let rclone = rclone()?;
    let base = nextcloud::base_url(&server)?;
    let cancel = app.state::<SignIn>().start();
    let flow = nextcloud::start_login(&base)?;
    system::open_url(&flow.login_url);
    let _ = app.emit("setup-link", &flow.login_url);
    let started = Instant::now();
    let creds = loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Sign-in cancelled.".into());
        }
        if started.elapsed() > Duration::from_secs(20 * 60) {
            return Err("The sign-in ran out after 20 minutes. Try again.".into());
        }
        if let Some(creds) = nextcloud::poll_login(&flow)? {
            break creds;
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    let props = nextcloud_remote(&rclone, &creds.server, &creds.login_name, &creds.app_password)?;
    connect(&rclone, Provider::Nextcloud, &props)
}

/// Nextcloud with an app password made by hand (Settings > Security).
#[tauri::command(async)]
pub fn nextcloud_app_password(server: String, user: String, password: String) -> Result<Connection, String> {
    let rclone = rclone()?;
    let base = nextcloud::base_url(&server)?;
    let user = user.trim();
    if user.is_empty() || password.is_empty() {
        return Err("Enter your user name and the app password.".into());
    }
    let props = nextcloud_remote(&rclone, &base, user, &password)?;
    connect(&rclone, Provider::Nextcloud, &props)
}

/// An SFTP server (a Hetzner Storage Box, for example) with a user name and
/// password. The server's key is kept in known_hosts in the app folder, so
/// rclone refuses another server that pretends to be this one later.
#[tauri::command(async)]
pub fn sftp_connect(server: String, port: String, user: String, password: String) -> Result<Connection, String> {
    let rclone = rclone()?;
    let (host, port) = sftp::parse_server(&server, &port)?;
    let user = user.trim();
    if user.is_empty() || password.is_empty() {
        return Err("Enter the user name and the password.".into());
    }
    // The new key replaces the saved one only once the connection works
    let trial_hosts = app_dir().join(sftp::KNOWN_HOSTS_TRIAL);
    std::fs::write(&trial_hosts, sftp::scan_keys(&host, port)?).map_err(|e| format!("Can't write {}: {e}", trial_hosts.display()))?;
    let mut props = Remote::from([
        ("type".into(), "sftp".into()),
        ("host".into(), host),
        ("port".into(), port.to_string()),
        ("user".into(), user.into()),
        ("pass".into(), rclone.obscure(&password)?),
        ("known_hosts_file".into(), sftp::KNOWN_HOSTS_TRIAL_SETTING.into()),
    ]);
    if let Err(e) = try_remote(&rclone, Provider::Sftp, &props) {
        let _ = std::fs::remove_file(&trial_hosts);
        return Err(if e.contains("unable to authenticate") { "The server didn't accept the user name or the password.".into() } else { e });
    }
    let known_hosts = app_dir().join(sftp::KNOWN_HOSTS);
    std::fs::rename(&trial_hosts, &known_hosts).map_err(|e| format!("Can't write {}: {e}", known_hosts.display()))?;
    props.insert("known_hosts_file".into(), sftp::KNOWN_HOSTS_SETTING.into());
    rclone.write_remote(Provider::Sftp.remote(), &props)?;
    Ok(connection(Provider::Sftp, Provider::Sftp.remote(), &props))
}

/// Dropbox and Google Drive: rclone opens the browser to sign in and allow access.
#[tauri::command(async)]
pub fn cloud_sign_in(app: AppHandle, provider: Provider) -> Result<Connection, String> {
    let rclone = rclone()?;
    let cancel = app.state::<SignIn>().start();
    let page = setup::signin_page(provider).ok();
    let token = rclone.authorize(provider.backend(), page.as_deref(), &cancel, |link| {
        let _ = app.emit("setup-link", link);
    });
    if let Some(page) = page {
        let _ = std::fs::remove_file(page);
    }
    let mut props = Remote::from([("type".into(), provider.backend().into())]);
    // Google Drive: all files, so notes made by other apps can be seen too
    if provider == Provider::GoogleDrive {
        props.insert("scope".into(), "drive".into());
    }
    props.insert("token".into(), token?);
    connect(&rclone, provider, &props)
}

#[tauri::command]
pub fn cancel_sign_in(sign_in: tauri::State<SignIn>) {
    if let Some(flag) = sign_in.0.lock().unwrap().take() {
        flag.store(true, Ordering::Relaxed);
    }
}

fn cloud_path(provider: Provider, path: &str) -> String {
    format!("{}:{}", provider.remote(), path.trim().replace('\\', "/").trim_matches('/'))
}

/// The folders in a folder in the cloud ("" = the top).
#[tauri::command(async)]
pub fn cloud_folders(provider: Provider, path: String) -> Result<Vec<String>, String> {
    rclone()?.dirs(&cloud_path(provider, &path))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesFolder {
    path: String,
    /// The Notes app said so (otherwise it's a folder with the usual name)
    from_app: bool,
}

/// Nextcloud: the folder the Notes app keeps its notes in.
#[tauri::command(async)]
pub fn notes_folder() -> Option<NotesFolder> {
    let rclone = rclone().ok()?;
    let remote = rclone.remotes().remove(Provider::Nextcloud.remote())?;
    let (url, user) = (remote.get("url")?, remote.get("user")?);
    let password = rclone.reveal(remote.get("pass")?).ok()?;
    if let Some(path) = nextcloud::notes_folder(nextcloud::base_of(url), user, &password) {
        return Some(NotesFolder { path, from_app: true });
    }
    let top = rclone.dirs(&cloud_path(Provider::Nextcloud, "")).ok()?;
    let guess = top.into_iter().find(|d| nextcloud::KNOWN_NOTES_FOLDERS.contains(&d.as_str()))?;
    Some(NotesFolder { path: guess, from_app: false })
}

#[tauri::command(async)]
pub fn check_cloud_folder(provider: Provider, path: String) -> Result<bool, String> {
    Ok(rclone()?.exists(&cloud_path(provider, &path)))
}

#[tauri::command(async)]
pub fn create_cloud_folder(provider: Provider, path: String) -> Result<(), String> {
    rclone()?.mkdir(&cloud_path(provider, &path))
}

#[tauri::command(async)]
pub fn check_local_folder(path: String) -> Result<LocalFolder, String> {
    setup::check_local(&app_dir(), &path)
}

/// Writes sync.conf and the marker files. Not while a sync runs.
#[tauri::command(async)]
pub fn save_setup(app: AppHandle, provider: Provider, remote_path: String, local: String) -> Result<Saved, String> {
    if app.state::<Mutex<Run>>().lock().unwrap().running {
        return Err("A sync is running. Save again when it's done.".into());
    }
    let dir = app_dir();
    let local = setup::check_local(&dir, &local)?;
    let remote_path = remote_path.trim().replace('\\', "/").trim_matches('/').to_string();
    if remote_path.is_empty() {
        return Err("Choose a folder in the cloud.".into());
    }
    let saved = setup::save(&dir, &rclone()?, &Choice { provider, remote_path: &remote_path, local: &local.path })?;
    crate::update_tray(&app, false);
    Ok(saved)
}

/// The setup's "Start with Windows"; the tray menu's tick follows.
#[tauri::command]
pub fn set_autostart(app: AppHandle, on: bool) -> Result<(), String> {
    let result = system::set_autostart(on);
    let _ = app.state::<crate::AutostartItem>().0.set_checked(system::autostart_enabled());
    result
}
