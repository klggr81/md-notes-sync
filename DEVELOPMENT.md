# Development

md-notes-sync is a [Tauri v2](https://tauri.app) app: a Rust backend and a window in plain TypeScript and CSS (Vite, no framework).

## Layout

| Path | What |
|---|---|
| `sync-core/` | Rust library with the sync and the setup. No Tauri dependency |
| `sync-core/src/conf.rs` | Reads `sync.conf` |
| `sync-core/src/runner.rs` | Runs one sync with rclone bisync and reports each line as an event |
| `sync-core/src/logline.rs` | Sorts rclone's log lines (copied, deleted, warning, …) |
| `sync-core/src/lock.rs` | The named mutex `Local\md-notes-sync`: one sync at a time |
| `sync-core/src/backup.rs` | Weekly backup zip, keeping each file's modified date |
| `sync-core/src/history.rs` | `sync.log`: writing, rotating, and reading when the last sync was |
| `sync-core/src/setup/` | The setup: rclone download (`install.rs`), remotes, folders and browser sign-in through rclone (`rclone.rs`), Nextcloud's sign-in and Notes folder (`nextcloud.rs`), SFTP's server address and key (`sftp.rs`), HTTPS (`http.rs`), saving (`mod.rs`) |
| `sync-core/assets/` | `filters.txt`, the `sync.conf` template and the sign-in page `signin.html`, built into the exe |
| `src-tauri/` | The app: tray, window, commands |
| `src-tauri/src/setup.rs` | The setup wizard's commands |
| `src-tauri/src/system.rs` | Windows: accent colour, Mica, Start with Windows, opening a web page |
| `src-tauri/src/update.rs` | Check for updates: the newest GitHub release, and installing its exe |
| `src/`, `index.html` | The window. `main.ts`: home and sync, `wizard.ts`: the setup, `look.ts`: accent colour, Mica and icons for both windows |
| `about.html`, `src/about.ts` | The About window (tray menu), with Check for updates |
| `src/glyphs.ts` | The icons: code points of Segoe Fluent Icons |
| `icons/icons.ts` | Draws the app and tray icons and renders them (see below) |
| `package.ps1` | Builds the exe and the release zip |
| `about.toml`, `about.hbs` | cargo-about's settings and template for `THIRD-PARTY-NOTICES.html` in the zip: the licenses of the Rust crates in the exe. A crate with a license that isn't accepted in `about.toml` stops the build |

## Building

Needs, once:
- [Rust](https://rustup.rs) (`winget install Rustlang.Rustup`)
- Visual Studio Build Tools with the C++ workload (`winget install Microsoft.VisualStudio.BuildTools --override "--passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"`)
- [Node.js](https://nodejs.org) 22.18 or newer (it runs `icons.ts` directly)
- WebView2, which is built into Windows 10 and 11
- [cargo-about](https://github.com/EmbarkStudios/cargo-about) for the third-party licenses in the zip (`cargo install cargo-about --locked --features cli`)

Build from PowerShell, not Git Bash: Git's `link.exe` can shadow the one from Visual Studio.

```
npm install
npm run tauri dev      # run with live reload
cargo test -p sync-core
cargo test -p sync-core -- --ignored        # also downloads the real rclone (about 30 MB)
cargo test -p md-notes-sync -- --ignored    # turns "Start with Windows" on and off once (puts it back as it was)
.\package.ps1          # release exe + release\md-notes-sync-<version>.zip
```

A development build keeps its settings, rclone and logs in `.dev\` in the repository. The released exe uses the folder it sits in.

**Trying it without risk:** unpack the release zip to a test folder and start the exe. For a sync without any cloud account, write a `sync.conf` there by hand whose `LOCAL` and `REMOTE` are two folders on the PC (for example `REMOTE=C:\Temp\test\cloud`): rclone accepts a plain folder as the other side. To try the setup without an account, `rclone serve webdav <folder> --addr 127.0.0.1:8090 --baseurl /remote.php/dav/files/test --user test --pass test` stands in for a Nextcloud; add it as a `webdav` remote with `vendor = nextcloud` to an rclone config and start the app with `APPDATA` pointing to that config's parent folder, so the setup offers it as an existing remote. For SFTP, `rclone serve sftp <folder> --addr 127.0.0.1:2022 --user test --pass test` is a server to connect to (in Git Bash, set `MSYS_NO_PATHCONV=1` for `rclone serve` commands with paths like `/remote.php/...`).

## Icons

```
node icons/icons.ts
```

It writes `app-icon.svg` and `tray-idle.svg`, `tray-syncing.svg`, `tray-error.svg` in `icons/`, then renders them with `tauri icon`:
- the app icon to `src-tauri/icons/` (`icon.ico`, `32x32.png`, …),
- each tray icon to `src-tauri/icons/tray/<state>-<size>.png` in 16, 20, 24, 32, 40 and 48 px (taskbar scale 100 % to 300 %). The app picks the size that fits the main display's scale, so Windows never has to resize it.

## Design

Fluent for Windows 11, like PowerToys and the Settings app, in plain CSS: the colours and sizes of WinUI 3, the Mica backdrop (a solid background on Windows 10), the Windows accent colour (read from the registry) for buttons and progress, setting cards, info bars, and the Segoe Fluent Icons font. `signin.html` uses the same values with Windows' default blue.

## Releases

GitHub Actions (`.github/workflows/build.yml`) tests and builds every push and keeps the zip as a build artifact. Pushing a tag `vX.Y.Z` that matches the version in `Cargo.toml` publishes the zip as a GitHub release, with the notes of that version from `CHANGELOG.md`.

The version is set in `Cargo.toml` (`[workspace.package]`) and `package.json`.

**Check for updates** reads `releases/latest` of the repository in `Cargo.toml` and installs `md-notes-sync-<version>.zip` from it, checked against the asset's SHA-256 `digest` that GitHub computes. So a release needs that zip, named exactly so, and the tag `vX.Y.Z`. To try the update without a release, a debug build reads the release JSON from `MD_NOTES_SYNC_UPDATE_URL` instead (for example a local `python -m http.server` with a copy of GitHub's answer).
