// No console window behind the GUI on Windows release builds.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    use postrust::{app::App, storage::Storage};

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

/// In the browser, Trunk runs this as the wasm start function.
#[cfg(target_arch = "wasm32")]
fn main() {
    wasm_bindgen_futures::spawn_local(postrust::web_entry::start("postrust_canvas"));
}
