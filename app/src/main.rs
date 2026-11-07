// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod host;
mod m4;
mod ui;

fn main() {
    host::run();
}
