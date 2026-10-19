//! Browser entry point: starts the app on the page's `<canvas>`.

use crate::{app::App, storage::Storage};
use wasm_bindgen::JsCast as _;

/// Starts postrust on the canvas with the given id, then removes the loading message.
pub async fn start(canvas_id: &str) {
    let document = web_sys::window().and_then(|w| w.document()).expect("no document");
    let loading = document.get_element_by_id("loading");

    let canvas = document
        .get_element_by_id(canvas_id)
        .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok());
    let result = match canvas {
        Some(canvas) => {
            eframe::WebRunner::new()
                .start(
                    canvas,
                    eframe::WebOptions::default(),
                    Box::new(|_cc| Ok(Box::new(App::new(Storage::default_location())))),
                )
                .await
        }
        None => Err(wasm_bindgen::JsValue::from_str("the page has no canvas element")),
    };

    match (loading, result) {
        (Some(loading), Ok(())) => loading.remove(),
        (Some(loading), Err(e)) => {
            let detail = e.as_string().unwrap_or_else(|| format!("{e:?}"));
            loading.set_text_content(Some(&format!("postrust could not start: {detail}")));
        }
        // No message area on the page: the console is the only place left to say what happened.
        (None, Err(e)) => web_sys::console::error_2(&"postrust could not start:".into(), &e),
        (None, Ok(())) => {}
    }
}
