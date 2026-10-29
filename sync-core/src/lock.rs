//! One sync at a time: the named mutex `Local\md-notes-sync`. It also keeps
//! two copies of the app (in different folders) from syncing at once.
//!
//! A Windows mutex belongs to the thread that took it: keep the [`SyncLock`]
//! on that thread and drop it there.
//!
//! bisync also has a lock of its own: a `.lck` file in bisync-state, which an
//! interrupted run leaves behind. [`clear_bisync_locks`] removes it safely.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::history;
use crate::runner::WORK_DIR;

/// Removes bisync's lock files left by an interrupted sync. Refuses while
/// another sync runs or while the rclone named in the
/// lock file is still running. Returns the files it removed.
pub fn clear_bisync_locks(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let _lock = SyncLock::acquire(Duration::ZERO)
        .map_err(|e| e.to_string())?
        .ok_or("Another sync is still running.")?;
    let files: Vec<PathBuf> = std::fs::read_dir(dir.join(WORK_DIR))
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "lck"))
        .collect();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        if let Some(pid) = lock_pid(&text).filter(|&pid| process_running(pid)) {
            return Err(format!("rclone is still running (process {pid}). Wait for it to finish."));
        }
    }
    for file in &files {
        std::fs::remove_file(file).map_err(|e| e.to_string())?;
        let _ = history::log(dir, &format!("removed bisync lock file of an interrupted sync: {}", file.display()));
    }
    Ok(files)
}

/// `"PID": "33560"` in the lock file's JSON
fn lock_pid(text: &str) -> Option<u32> {
    let rest = text.split("\"PID\"").nth(1)?;
    let digits: String = rest.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(windows)]
fn process_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: plain Win32 calls on a handle we own
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        ok && code == STILL_ACTIVE as u32
    }
}

#[cfg(not(windows))]
fn process_running(_pid: u32) -> bool {
    false
}

pub const NAME: &str = r"Local\md-notes-sync";

pub struct SyncLock {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

impl SyncLock {
    /// Takes the lock, waiting up to `wait`. `Ok(None)`: another sync holds it.
    #[cfg(windows)]
    pub fn acquire(wait: Duration) -> io::Result<Option<SyncLock>> {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

        let name: Vec<u16> = NAME.encode_utf16().chain([0]).collect();
        // SAFETY: plain Win32 calls; the name is NUL-terminated and outlives the call
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: handle is a valid mutex handle
        match unsafe { WaitForSingleObject(handle, ms) } {
            // An abandoned mutex (a sync that crashed) is ours now, as in .NET
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(SyncLock { handle })),
            result => {
                let err = io::Error::last_os_error();
                // SAFETY: we own this handle
                unsafe { CloseHandle(handle) };
                if result == WAIT_TIMEOUT { Ok(None) } else { Err(err) }
            }
        }
    }

    /// Without Windows there is no named mutex: no lock.
    #[cfg(not(windows))]
    pub fn acquire(_wait: Duration) -> io::Result<Option<SyncLock>> {
        Ok(Some(SyncLock {}))
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: the handle is ours and the mutex is held by this thread
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.handle);
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_pid_and_checks_it() {
        let text = "{\n\t\"Session\": \"C:\\\\x\",\n\t\"PID\": \"33560\",\n\t\"TimeRenewed\": \"2026\"\n}";
        assert_eq!(lock_pid(text), Some(33560));
        assert_eq!(lock_pid("{}"), None);
        #[cfg(windows)]
        assert!(process_running(std::process::id()));
    }

    #[test]
    #[cfg(windows)]
    fn a_second_holder_has_to_wait() {
        let first = SyncLock::acquire(Duration::ZERO).unwrap();
        assert!(first.is_some());
        // Another thread can't take it while the first one holds it
        let other = std::thread::spawn(|| SyncLock::acquire(Duration::from_millis(50)).unwrap().is_some());
        assert!(!other.join().unwrap());
        drop(first);
        let again = std::thread::spawn(|| SyncLock::acquire(Duration::ZERO).unwrap().is_some());
        assert!(again.join().unwrap());
    }
}
