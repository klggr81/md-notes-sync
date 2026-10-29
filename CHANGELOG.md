# Changelog

All notable changes are listed here. Versions follow [Semantic Versioning](https://semver.org) (`MAJOR.MINOR.PATCH`).

## [0.1.0] – 2026-10-29

First version.

### Added
- **Tray app** for the two-way sync of Markdown notes between Nextcloud Notes, Dropbox, Google Drive or an SFTP server and a folder on Windows, with rclone bisync. Portable: one folder, no installer, no admin rights.
- **Setup in the app** (opens by itself on the first start):
  - Downloads rclone from rclone.org and checks it against the release's checksum, or uses an rclone that is already installed.
  - Nextcloud: sign-in in the browser (Nextcloud makes an app password for the app), or an app password made by hand. Dropbox and Google Drive: sign-in in the browser through rclone. SFTP server (for example a Hetzner Storage Box): address, port, user name and password; the server's key is saved and checked on every sync. Existing rclone remotes can be reused.
  - Finds the Nextcloud Notes app's folder by asking the Notes app; otherwise a folder can be picked from a list, or created.
  - Checks the folder on this PC, writes the settings and puts the `RCLONE_TEST` marker on both sides.
  - Ends with a test run: it lists what the first sync would copy and delete, and changes nothing. Then **Sync now**, or **Close** to sync later.
  - Can be opened again from the window (**Change**) or the tray menu (**Set up…**).
- **Tray icon:** left-click syncs and shows the window. The menu has Sync now, Test run, Back up now, Open notes folder, Open sync log, Set up…, Start with Windows, About, Show window and Quit. The icon is green, blue while syncing and red after an error; the tooltip says when the last sync was.
- **Window:** the two folders, Sync now, buttons that open the log and the backups folder, rclone's output while it syncs, a progress bar and a summary. It offers a resync after the critical bisync error (exit 7) and explains a lock file left by an interrupted sync, with **Remove lock and sync**.
- **Start with Windows** (off by default): the app then starts in the tray. It never syncs on its own.
- **About** window (tray menu): version, links to the source code, the developer, contact, Buy me a coffee and the license.
- **Check for updates** (in About): asks GitHub for the newest release; **Install and restart** downloads the zip, checks it against the SHA-256 GitHub lists for it, replaces the exe and restarts. Never checks by itself.
- **Third-party licenses:** `THIRD-PARTY-NOTICES.html` in the zip lists the licenses of the libraries built into the app.
- **Safety:** weekly backup zip (the newest 3 are kept), access check with `RCLONE_TEST`, deletion limit, conflict copies, one sync at a time.
- **Design:** Fluent for Windows 11: Mica background, the Windows accent colour, Segoe Fluent Icons, light and dark mode as set in Windows. The page after a Dropbox or Google Drive sign-in uses the same style.
