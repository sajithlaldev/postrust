// No console window behind the GUI on Windows release builds.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use postrust::{app::App, storage::Storage};

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1200.0, 780.0])
        .with_min_inner_size([800.0, 500.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "postrust",
        options,
        Box::new(|_cc| Ok(Box::new(App::new(Storage::default_location())))),
    )
}
