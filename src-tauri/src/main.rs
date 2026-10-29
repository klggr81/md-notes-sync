// No console window next to the app in release builds. Don't remove.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    md_notes_sync_lib::run()
}
