mod setup;
mod system;
mod update;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use sync_core::conf::Conf;
use sync_core::{backup, history, Options, Summary};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::window::{Effect, EffectsBuilder};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

const TRAY_ID: &str = "main";

/// The tray icon in each taskbar size (16 px at 100 % scale … 48 px at 300 %),
/// so Windows never has to resize it. Written by icons.ts.
const TRAY_SIZES: [u32; 6] = [16, 20, 24, 32, 40, 48];
macro_rules! tray_icons {
    ($state:literal) => {
        [
            include_bytes!(concat!("../icons/tray/", $state, "-16.png")) as &[u8],
            include_bytes!(concat!("../icons/tray/", $state, "-20.png")) as &[u8],
            include_bytes!(concat!("../icons/tray/", $state, "-24.png")) as &[u8],
            include_bytes!(concat!("../icons/tray/", $state, "-32.png")) as &[u8],
            include_bytes!(concat!("../icons/tray/", $state, "-40.png")) as &[u8],
            include_bytes!(concat!("../icons/tray/", $state, "-48.png")) as &[u8],
        ]
    };
}
const TRAY_IDLE: [&[u8]; 6] = tray_icons!("idle");
const TRAY_SYNCING: [&[u8]; 6] = tray_icons!("syncing");
const TRAY_ERROR: [&[u8]; 6] = tray_icons!("error");

/// The icon for the taskbar's scale: the smallest one that is big enough.
fn tray_image(app: &AppHandle, icons: [&'static [u8]; 6]) -> tauri::Result<Image<'static>> {
    let scale = app.primary_monitor().ok().flatten().map_or(1.0, |m| m.scale_factor());
    let want = (16.0 * scale).round() as u32;
    let i = TRAY_SIZES.iter().position(|&size| size >= want).unwrap_or(TRAY_SIZES.len() - 1);
    Image::from_bytes(icons[i])
}

/// The app folder: sync.conf, rclone.conf, rclone\, the sync state, the log
/// and the backups live next to the exe, so the app is portable. A debug
/// build uses `.dev\` in the repository instead of target\debug.
fn app_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        // CARGO_MANIFEST_DIR = <repo>/src-tauri
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".dev");
        let _ = std::fs::create_dir_all(&dev);
        return std::path::absolute(dev).unwrap_or_default();
    }
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)).unwrap_or_default()
}

/// sync.conf is there and complete.
fn is_set_up() -> bool {
    Conf::load(&app_dir()).is_ok_and(|conf| conf.check().is_ok())
}

/// Shows the window with the setup.
fn open_setup(app: &AppHandle) {
    show_window(app);
    let _ = app.emit("open-setup", ());
}

/// Whether a sync runs, and what to do when it ends.
struct Run {
    running: bool,
    quit_after: bool,
    /// An update is being installed: no sync may start, the app restarts soon
    updating: bool,
    /// "Back up now" runs: no sync may start until the zip is written
    backing_up: bool,
    quit_item: MenuItem<Wry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    version: &'static str,
    folder: String,
    local: Option<String>,
    remote: Option<String>,
    /// Why the sync can't run yet (no sync.conf, a missing key), if so
    problem: Option<String>,
    /// "5 min ago", from sync.log
    last_sync: Option<String>,
    /// The error of the most recent run, if it failed
    failed: Option<String>,
    running: bool,
    /// The Windows accent colour in its seven shades, for the buttons and links
    accent: Option<Vec<String>>,
    /// The window has the Mica backdrop (Windows 11), so the page leaves its background clear
    mica: bool,
}

/// Sent to the window when a sync ends.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Finished {
    summary: Option<Summary>,
    error: Option<String>,
    needs_resync: bool,
    /// An interrupted sync left bisync's lock file: offer to remove it
    left_lock: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Begin {
    dry_run: bool,
    resync: bool,
}

#[tauri::command]
fn status(run: tauri::State<Mutex<Run>>) -> Status {
    let dir = app_dir();
    let conf = Conf::load(&dir).and_then(|conf| conf.check().map(|_| conf));
    let history = history::read(&dir);
    Status {
        version: env!("CARGO_PKG_VERSION"),
        folder: dir.display().to_string(),
        local: conf.as_ref().ok().and_then(Conf::local),
        remote: conf.as_ref().ok().and_then(|c| c.remote().map(str::to_string)),
        problem: conf.err().map(|e| e.to_string()),
        last_sync: history.last_sync.map(history::ago_now),
        failed: history.failed,
        running: {
            let run = run.lock().unwrap();
            run.running || run.backing_up
        },
        accent: system::accent_palette(),
        mica: system::has_mica(),
    }
}

#[tauri::command]
fn sync(app: AppHandle, dry_run: bool, resync: bool) -> Result<(), String> {
    start_sync(&app, Options { dry_run, resync })
}

/// Removes the lock file an interrupted sync left behind, then syncs.
/// Refuses while a sync or that sync's rclone still runs.
#[tauri::command]
fn unlock_and_sync(app: AppHandle) -> Result<(), String> {
    if app.state::<Mutex<Run>>().lock().unwrap().running {
        return Err("A sync is already running.".into());
    }
    sync_core::lock::clear_bisync_locks(&app_dir())?;
    start_sync(&app, Options::default())
}

#[tauri::command]
fn open_notes() -> Result<(), String> {
    let conf = Conf::load(&app_dir()).map_err(|e| e.to_string())?;
    open(Path::new(&conf.local().ok_or("sync.conf has no LOCAL")?))
}

#[tauri::command]
fn open_log() -> Result<(), String> {
    open(&app_dir().join(history::LOG))
}

#[tauri::command]
fn open_backups() -> Result<(), String> {
    let dir = app_dir().join(backup::DIR);
    if !dir.exists() {
        return Err("There's no backup yet. The app makes one before the next sync, then once a week.".into());
    }
    open(&dir)
}

/// Esc: back to the tray
#[tauri::command]
fn hide_window(window: tauri::WebviewWindow) {
    let _ = window.hide();
}

/// The links in the About window: a web page, or the mail app.
#[tauri::command]
fn open_link(url: String) {
    system::open_url(&url);
}

/// What the About window shows when it opens.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AboutInfo {
    version: &'static str,
    /// `https://github.com/<owner>/md-notes-sync`
    repository: &'static str,
    accent: Option<Vec<String>>,
    mica: bool,
    /// The app was just updated to this version
    updated: bool,
}

/// Started by an update: the About window says so, once.
struct JustUpdated(AtomicBool);

#[tauri::command]
fn about_info(updated: tauri::State<JustUpdated>) -> AboutInfo {
    AboutInfo {
        version: env!("CARGO_PKG_VERSION"),
        repository: env!("CARGO_PKG_REPOSITORY"),
        accent: system::accent_palette(),
        mica: system::has_mica(),
        updated: updated.0.swap(false, Ordering::Relaxed),
    }
}

#[tauri::command(async)]
fn check_update() -> Result<update::Latest, String> {
    update::check()
}

/// Downloads and installs the newest version, then starts it and quits. The
/// About window gets `update-download` events. Not while a sync runs.
#[tauri::command(async)]
fn install_update(app: AppHandle) -> Result<(), String> {
    {
        let run = app.state::<Mutex<Run>>();
        let mut run = run.lock().unwrap();
        if run.running {
            return Err("A sync is running. Install the update when it's done.".into());
        }
        run.updating = true;
    }
    let mut last = Instant::now();
    let installed = update::install(|done, total| {
        if last.elapsed() > Duration::from_millis(100) || Some(done) == total {
            last = Instant::now();
            let _ = app.emit_to("about", "update-download", setup::Download { done, total });
        }
    });
    // The new version waits until this one has quit: it would find this one
    // running and hand over to it
    let restarted = installed.and_then(|_| {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        std::process::Command::new(exe)
            .args([update::UPDATED_ARG, &std::process::id().to_string()])
            .spawn()
            .map_err(|e| format!("Updated, but the new version didn't start ({e}). Start md-notes-sync again."))
    });
    match restarted {
        Ok(_) => {
            app.exit(0);
            Ok(())
        }
        Err(e) => {
            app.state::<Mutex<Run>>().lock().unwrap().updating = false;
            Err(e)
        }
    }
}

/// Esc in the About window
#[tauri::command]
fn close_window(window: tauri::WebviewWindow) {
    let _ = window.close();
}

/// Opens a folder in Explorer, or a file in its program (sync.log: the text editor).
fn open(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("Not found: {}", path.display()));
    }
    std::process::Command::new("explorer").arg(path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Starts a sync on its own thread. The window gets every line while it runs.
fn start_sync(app: &AppHandle, opts: Options) -> Result<(), String> {
    {
        let run = app.state::<Mutex<Run>>();
        let mut run = run.lock().unwrap();
        if run.running {
            return Err("A sync is already running.".into());
        }
        if run.updating {
            return Err("An update is being installed. The app restarts in a moment.".into());
        }
        if run.backing_up {
            return Err("A backup is being made. Sync again in a moment.".into());
        }
        run.running = true;
    }
    let _ = app.emit("sync-begin", Begin { dry_run: opts.dry_run, resync: opts.resync });
    update_tray(app, true);

    let app = app.clone();
    std::thread::spawn(move || {
        let result = sync_core::run(&app_dir(), opts, |event| {
            let _ = app.emit("sync-event", event);
        });
        let finished = match &result {
            Ok(summary) => Finished { summary: Some(*summary), error: None, needs_resync: false, left_lock: false },
            Err(e) => Finished {
                summary: None,
                error: Some(e.to_string()),
                needs_resync: e.needs_resync(),
                left_lock: e.left_lock(),
            },
        };
        let _ = app.emit("sync-finished", finished);

        let quit = {
            let run = app.state::<Mutex<Run>>();
            let mut run = run.lock().unwrap();
            run.running = false;
            run.quit_after
        };
        update_tray(&app, false);
        if quit {
            app.exit(0);
        } else if result.is_err() {
            show_window(&app);
        }
    });
    Ok(())
}

/// "Back up now" in the tray menu: zips the notes folder on its own thread,
/// whatever the age of the newest backup. Not while a sync runs. The window
/// shows how it went.
fn backup_now(app: &AppHandle) -> Result<(), String> {
    let dir = app_dir();
    let conf = Conf::load(&dir).and_then(|conf| conf.check().map(|_| conf)).map_err(|e| e.to_string())?;
    let local = conf.local().ok_or("sync.conf has no LOCAL")?;
    {
        let run = app.state::<Mutex<Run>>();
        let mut run = run.lock().unwrap();
        if run.running {
            return Err("A sync is running. Back up when it's done.".into());
        }
        if run.updating {
            return Err("An update is being installed. The app restarts in a moment.".into());
        }
        if run.backing_up {
            return Err("A backup is already being made.".into());
        }
        run.backing_up = true;
    }
    show_window(app);
    let _ = app.emit("backup-begin", &local);
    let app = app.clone();
    std::thread::spawn(move || {
        let mut last = Instant::now();
        let made = backup::now(Path::new(&local), &dir.join(backup::DIR), backup::keep(&conf), backup::today(), |progress| {
            // Often enough to move the bar, not for every small note
            if last.elapsed() > Duration::from_millis(100) || progress.files == progress.total {
                last = Instant::now();
                let _ = app.emit("backup-progress", progress);
            }
        });
        app.state::<Mutex<Run>>().lock().unwrap().backing_up = false;
        let finished = match made {
            Ok(zip) => {
                let _ = history::log(&dir, &format!("backup {} (by hand)", zip.display()));
                let size = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
                BackupFinished { path: Some(zip.display().to_string()), size, error: None }
            }
            Err(e) => {
                let _ = history::log(&dir, &format!("WARNING: backup failed: {e}"));
                BackupFinished { path: None, size: 0, error: Some(e.to_string()) }
            }
        };
        let _ = app.emit("backup-finished", finished);
    });
    Ok(())
}

/// Sent to the window when "Back up now" is done: the zip and its size, or why it failed.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupFinished {
    path: Option<String>,
    size: u64,
    error: Option<String>,
}

/// Tray icon and tooltip: syncing, the last error, or when the last sync was.
fn update_tray(app: &AppHandle, syncing: bool) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let history = history::read(&app_dir());
    let (icon, tooltip) = if syncing {
        (TRAY_SYNCING, "Syncing notes…".to_string())
    } else if let Some(err) = &history.failed {
        (TRAY_ERROR, format!("Sync failed: {err}"))
    } else if let Some(t) = history.last_sync {
        (TRAY_IDLE, format!("Notes synced {}", history::ago_now(t)))
    } else {
        (TRAY_IDLE, "Sync notes".to_string())
    };
    if let Ok(image) = tray_image(app, icon) {
        let _ = tray.set_icon(Some(image));
    }
    // Windows cuts tooltips at 127 characters
    let tooltip: String = tooltip.chars().take(120).collect();
    let _ = tray.set_tooltip(Some(tooltip));
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// A window of the app. Portable: WebView2 keeps its cache next to the exe,
/// not in %LOCALAPPDATA%. Windows 11: the Mica backdrop, as in PowerToys and
/// the Settings app; the page leaves its background clear so it shows through.
fn window_builder<'a>(app: &'a AppHandle, label: &str, page: &str) -> WebviewWindowBuilder<'a, Wry, AppHandle> {
    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into())).data_directory(app_dir().join("webview"));
    if system::has_mica() {
        builder.transparent(true).effects(EffectsBuilder::new().effect(Effect::Mica).build())
    } else {
        builder
    }
}

/// The About window, which also checks for updates.
fn open_about(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("about") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    window_builder(app, "about", "about.html")
        .title("About md-notes-sync")
        .inner_size(440.0, 700.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .center()
        .build()?;
    Ok(())
}

fn quit(app: &AppHandle) {
    let run = app.state::<Mutex<Run>>();
    let mut run = run.lock().unwrap();
    if run.running {
        // Stopping rclone halfway could leave bisync needing a resync: quit when it's done
        run.quit_after = true;
        let _ = run.quit_item.set_text("Quits after this sync");
        let _ = run.quit_item.set_enabled(false);
    } else {
        app.exit(0);
    }
}

/// The "Start with Windows" check in the tray menu.
struct AutostartItem(CheckMenuItem<Wry>);

/// Turns starting with Windows on or off, then shows in the menu what the registry says.
fn toggle_autostart(app: &AppHandle) -> Result<(), String> {
    let result = system::set_autostart(!system::autostart_enabled());
    let _ = app.state::<AutostartItem>().0.set_checked(system::autostart_enabled());
    result
}

fn build_tray(app: &AppHandle) -> tauri::Result<(MenuItem<Wry>, CheckMenuItem<Wry>)> {
    let sync_now = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
    let test_run = MenuItem::with_id(app, "test", "Test run (changes nothing)", true, None::<&str>)?;
    let backup = MenuItem::with_id(app, "backup", "Back up now", true, None::<&str>)?;
    let notes = MenuItem::with_id(app, "notes", "Open notes folder", true, None::<&str>)?;
    let log = MenuItem::with_id(app, "log", "Open sync log", true, None::<&str>)?;
    let setup = MenuItem::with_id(app, "setup", "Set up…", true, None::<&str>)?;
    let autostart =
        CheckMenuItem::with_id(app, "autostart", "Start with Windows", true, system::autostart_enabled(), None::<&str>)?;
    let about = MenuItem::with_id(app, "about", "About", true, None::<&str>)?;
    let window = MenuItem::with_id(app, "window", "Show window", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(
        app,
        &[
            &sync_now,
            &test_run,
            &backup,
            &separator()?,
            &notes,
            &log,
            &separator()?,
            &setup,
            &autostart,
            &separator()?,
            &about,
            &separator()?,
            &window,
            &quit_item,
        ],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_image(app, TRAY_IDLE)?)
        .tooltip("Sync notes")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let result = match event.id().as_ref() {
                "sync" | "test" => {
                    show_window(app);
                    start_sync(app, Options { dry_run: event.id() == "test", resync: false })
                }
                "backup" => backup_now(app),
                "notes" => open_notes(),
                "log" => open_log(),
                "setup" => {
                    open_setup(app);
                    Ok(())
                }
                "autostart" => toggle_autostart(app),
                "about" => open_about(app).map_err(|e| e.to_string()),
                "window" => {
                    show_window(app);
                    Ok(())
                }
                "quit" => {
                    quit(app);
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                let _ = app.emit("app-error", e);
                show_window(app);
            }
        })
        .on_tray_icon_event(|tray, event| match event {
            // Left click: sync now, and show what happens. Before the setup: the setup
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
                let app = tray.app_handle();
                if !is_set_up() {
                    open_setup(app);
                    return;
                }
                show_window(app);
                let _ = start_sync(app, Options::default());
            }
            // Keeps "synced 5 min ago" up to date, and the autostart check (it can
            // also be turned off in Task Manager or Settings)
            TrayIconEvent::Enter { .. } => {
                let app = tray.app_handle();
                let running = app.state::<Mutex<Run>>().lock().unwrap().running;
                update_tray(app, running);
                let _ = app.state::<AutostartItem>().0.set_checked(system::autostart_enabled());
            }
            _ => {}
        })
        .build(app)?;
    Ok((quit_item, autostart))
}

/// Started by an update (`--updated <process id>`): waits until the old
/// version has quit. Every start removes what an update left behind.
fn after_update() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let updated = args.iter().position(|arg| arg == update::UPDATED_ARG);
    if let Some(pid) = updated.and_then(|i| args.get(i + 1)).and_then(|pid| pid.parse().ok()) {
        system::wait_for_exit(pid, Duration::from_secs(15));
    }
    update::clean_up();
    updated.is_some()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let updated = after_update();
    tauri::Builder::default()
        // Starting the exe again shows the running app instead of a second one
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_window(app)))
        // The folder picker of the setup
        .plugin(tauri_plugin_dialog::init())
        .manage(setup::SignIn::default())
        .manage(JustUpdated(AtomicBool::new(updated)))
        .setup(move |app| {
            let (quit_item, autostart_item) = build_tray(app.handle())?;
            app.manage(Mutex::new(Run { running: false, quit_after: false, updating: false, backing_up: false, quit_item }));
            app.manage(AutostartItem(autostart_item));
            update_tray(app.handle(), false);

            // Started with Windows: only the tray icon, the window opens on a click.
            // After an update: the About window says so
            let autostarted = std::env::args().any(|arg| arg == system::AUTOSTART_ARG);
            let window = window_builder(app.handle(), "main", "index.html")
                .title("Sync notes")
                .inner_size(600.0, 680.0)
                .min_inner_size(420.0, 480.0)
                .visible(!autostarted && !updated)
                .build()?;
            if updated {
                open_about(app.handle())?;
            }
            // Closing the window keeps the app in the tray; Quit is in the tray menu
            let hide = window.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = hide.hide();
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            sync,
            unlock_and_sync,
            open_notes,
            open_log,
            open_backups,
            hide_window,
            open_link,
            about_info,
            check_update,
            install_update,
            close_window,
            setup::setup_info,
            setup::install_rclone,
            setup::use_existing,
            setup::nextcloud_sign_in,
            setup::nextcloud_app_password,
            setup::cloud_sign_in,
            setup::sftp_connect,
            setup::cancel_sign_in,
            setup::cloud_folders,
            setup::notes_folder,
            setup::check_cloud_folder,
            setup::create_cloud_folder,
            setup::check_local_folder,
            setup::save_setup,
            setup::set_autostart,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
