#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use env_logger;
use log;

fn main() {
    if app_lib::formal_whisper_baseline::requested() {
        std::process::exit(app_lib::formal_whisper_baseline::run_cli());
    }
    std::env::set_var("RUST_LOG", "info");
    env_logger::init();

    // Async logger will be initialized lazily when first needed (after Tauri runtime starts)
    log::info!("Starting application...");
    app_lib::run();
}
