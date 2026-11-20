//! ZZZ: the ZigZag studio in the browser.
//!
//! The page (index.html) and its start. The studio is the `zigzag-studio`
//! library, the same one the desktop app is built on; what it asks of the
//! browser is in its `platform` module.
//!
//! Everything here is for the browser: on any other target the crate is
//! empty (the tests in `tests/` still run there, to compare with it).
#![cfg(target_arch = "wasm32")]

use wasm_bindgen::prelude::*;
use zigzag_studio::{Studio, platform};

#[wasm_bindgen]
extern "C" {
    /// The page's (index.html): the studio is on screen.
    #[wasm_bindgen(js_namespace = window, js_name = zzzStarted)]
    fn page_started();
    /// The page's: the studio went down, and why.
    #[wasm_bindgen(js_namespace = window, js_name = zzzFailed)]
    fn page_failed(what: &str);
}

#[wasm_bindgen(start)]
pub async fn start() -> Result<(), JsValue> {
    // A failure ends the app here (nothing can catch it in the browser):
    // the page says so, and offers to reload.
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        page_failed(&info.to_string());
        before(info);
    }));

    // What was kept from the last visit: settings, the session and its
    // documents, the fonts that were added.
    platform::restore().await;

    // There are no system fonts to find. ZZZ comes with Inter (regular and
    // bold; assets/fonts, SIL Open Font License), which the studio also
    // takes for its own text, as on the desktop where it's installed; the
    // two that come with egui are there too; and the person can add more
    // (Type › Add Fonts…).
    let fonts = eframe::egui::FontDefinitions::default();
    zigzag::svg::add_fonts(
        [
            include_bytes!("../assets/fonts/Inter-Regular.ttf").to_vec(),
            include_bytes!("../assets/fonts/Inter-Bold.ttf").to_vec(),
        ]
        .into_iter()
        .chain(
            ["Ubuntu-Light", "Hack"]
                .iter()
                .filter_map(|name| fonts.font_data.get(*name))
                .map(|data| data.font.as_ref().as_ref().to_vec()),
        ),
    );

    let window = web_sys::window().ok_or("no window")?;
    // Leaving with unsaved changes: the browser asks first.
    let leaving = Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(
        |e: web_sys::BeforeUnloadEvent| {
            if platform::unsaved() {
                e.prevent_default();
                e.set_return_value("There are unsaved changes.");
            }
        },
    );
    window.add_event_listener_with_callback("beforeunload", leaving.as_ref().unchecked_ref())?;
    leaving.forget();
    // What the studio writes goes out each frame (`platform::flush`), but
    // the last thing a frame writes (the session, say) would wait for the
    // next one, which may not come while nothing moves: so also on a
    // timer, and when the page is put away.
    let keep = Closure::<dyn FnMut()>::new(platform::flush);
    window.set_interval_with_callback_and_timeout_and_arguments_0(
        keep.as_ref().unchecked_ref(),
        1000,
    )?;
    for event in ["pagehide", "visibilitychange"] {
        window.add_event_listener_with_callback(event, keep.as_ref().unchecked_ref())?;
    }
    keep.forget();

    let canvas = window
        .document()
        .and_then(|d| d.get_element_by_id("zzz"))
        .ok_or("no #zzz canvas")?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            // No file to open, nothing selected, no agent server here:
            // agents come through the relay, if the page was opened with
            // its link.
            Box::new(|cc| {
                platform::relay_begin(&cc.egui_ctx);
                platform::load_font_library(&cc.egui_ctx);
                Ok(Box::new(Studio::new(cc, None, None, None)))
            }),
        )
        .await?;
    page_started();
    Ok(())
}
