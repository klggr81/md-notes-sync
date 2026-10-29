//! What the app needs from Windows: the accent colour and whether the Mica
//! backdrop is available (to look native), starting with Windows, opening a
//! web page in the browser, and waiting for the old version to quit after an update.

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager and Settings > Apps > Startup note that the user turned an entry off
const RUN_APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const RUN_NAME: &str = "md-notes-sync";
/// Passed by the Run entry: start in the tray, without the window
pub const AUTOSTART_ARG: &str = "--autostart";

/// The seven shades of the Windows accent colour, as `#rrggbb`: light 3, 2, 1,
/// the accent, dark 1, 2, 3. Windows computes them when the user picks a colour.
pub fn accent_palette() -> Option<Vec<String>> {
    let data = registry::read(registry::User, r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent", "AccentPalette")?;
    // 8 colours of 4 bytes (r, g, b, unused); the 8th isn't a shade of the accent
    if data.len() < 28 {
        return None;
    }
    Some(data.chunks(4).take(7).map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect())
}

/// Mica needs Windows 11 (build 22000 or newer).
pub fn has_mica() -> bool {
    registry::read(registry::Machine, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber")
        .and_then(|data| registry::to_string(&data).parse::<u32>().ok())
        .is_some_and(|build| build >= 22000)
}

/// Opens an `https://` page in the default browser, or a `mailto:` link in the mail app.
#[cfg(windows)]
pub fn open_url(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !url.starts_with("https://") && !url.starts_with("mailto:") {
        return;
    }
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, url) = (wide("open"), wide(url));
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), url.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

#[cfg(not(windows))]
pub fn open_url(_url: &str) {}

/// Waits until the process `pid` has quit, or `timeout` is over.
#[cfg(windows)]
pub fn wait_for_exit(pid: u32, timeout: std::time::Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        // Already gone
        if process.is_null() {
            return;
        }
        WaitForSingleObject(process, timeout.as_millis() as u32);
        CloseHandle(process);
    }
}

#[cfg(not(windows))]
pub fn wait_for_exit(_pid: u32, _timeout: std::time::Duration) {}

/// The command the Run entry starts: this exe, in the tray.
fn autostart_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" {AUTOSTART_ARG}", exe.display()))
}

/// Starts with Windows: the Run entry points to this exe (not to a copy that
/// was moved or unpacked elsewhere), and the user hasn't turned it off in
/// Task Manager or Settings.
pub fn autostart_enabled() -> bool {
    let Some(command) = autostart_command() else { return false };
    let registered = registry::read(registry::User, RUN, RUN_NAME)
        .is_some_and(|data| registry::to_string(&data).eq_ignore_ascii_case(&command));
    // First byte odd (03, 07, …) = turned off there
    let turned_off = registry::read(registry::User, RUN_APPROVED, RUN_NAME)
        .is_some_and(|data| data.first().is_some_and(|b| b & 1 == 1));
    registered && !turned_off
}

/// Adds or removes the Run entry. Only the current user, no admin rights.
pub fn set_autostart(on: bool) -> Result<(), String> {
    if on {
        let command = autostart_command().ok_or("Can't find the app's own exe")?;
        registry::write_string(RUN, RUN_NAME, &command)?;
        // Undo an earlier "Disabled" from Task Manager, or Windows would ignore the entry
        registry::delete(RUN_APPROVED, RUN_NAME)
    } else {
        registry::delete(RUN, RUN_NAME)?;
        registry::delete(RUN_APPROVED, RUN_NAME)
    }
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW,
        RegGetValueW, RegSetKeyValueW,
    };

    pub use Root::*;
    pub enum Root {
        User,
        Machine,
    }

    fn hkey(root: Root) -> HKEY {
        match root {
            User => HKEY_CURRENT_USER,
            Machine => HKEY_LOCAL_MACHINE,
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// A string or binary value, as raw bytes.
    pub fn read(root: Root, key: &str, name: &str) -> Option<Vec<u8>> {
        let (key, name) = (wide(key), wide(name));
        let mut data = vec![0u8; 1024];
        let mut size = data.len() as u32;
        let result = unsafe {
            RegGetValueW(
                hkey(root),
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_BINARY,
                std::ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if result != ERROR_SUCCESS {
            return None;
        }
        data.truncate(size as usize);
        Some(data)
    }

    /// A string value read with `read`.
    pub fn to_string(data: &[u8]) -> String {
        let wide: Vec<u16> = data.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
        String::from_utf16_lossy(&wide).trim_end_matches('\0').to_string()
    }

    /// Writes a string value under HKCU, creating the key if needed.
    pub fn write_string(key: &str, name: &str, value: &str) -> Result<(), String> {
        let (key, name, value) = (wide(key), wide(name), wide(value));
        let result = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        if result == ERROR_SUCCESS { Ok(()) } else { Err(format!("Can't write to the registry (error {result})")) }
    }

    /// Deletes a value under HKCU; a value that isn't there is fine.
    pub fn delete(key: &str, name: &str) -> Result<(), String> {
        let (key, name) = (wide(key), wide(name));
        let result = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        match result {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            _ => Err(format!("Can't change the registry (error {result})")),
        }
    }
}

#[cfg(not(windows))]
mod registry {
    pub use Root::*;
    pub enum Root {
        User,
        Machine,
    }
    pub fn read(_root: Root, _key: &str, _name: &str) -> Option<Vec<u8>> {
        None
    }
    pub fn to_string(_data: &[u8]) -> String {
        String::new()
    }
    pub fn write_string(_key: &str, _name: &str, _value: &str) -> Result<(), String> {
        Err("Only on Windows".into())
    }
    pub fn delete(_key: &str, _name: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Changes the real Run key of this user (and puts it back): `cargo test -p md-notes-sync -- --ignored`
    #[test]
    #[ignore]
    fn autostart_on_and_off() {
        let before = autostart_enabled();
        set_autostart(true).unwrap();
        assert!(autostart_enabled());
        let command = registry::to_string(&registry::read(registry::User, RUN, RUN_NAME).unwrap());
        assert!(command.ends_with(&format!("\" {AUTOSTART_ARG}")), "{command}");
        set_autostart(false).unwrap();
        assert!(!autostart_enabled());
        assert!(registry::read(registry::User, RUN, RUN_NAME).is_none());
        set_autostart(before).unwrap();
    }
}
