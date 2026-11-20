//! What the studio asks of the browser (the desktop has no such module:
//! the calls are all under `cfg(target_arch = "wasm32")`).
//!
//! The studio works on files by path, as on the desktop: in the browser
//! they are in memory (`zigzag::vfs`). This module is what connects those
//! to the world: a file picked or dropped comes in ([`bring_in`]), and each
//! frame what the studio wrote goes out ([`flush`]): kept in the browser
//! for the next visit, and, for a design saved or exported, written to the
//! file on the computer (`files.js`).

use std::path::{Path, PathBuf};
use wasm_bindgen::prelude::*;

/// Where picked, saved and exported files are, among the files in memory:
/// no folder, so a file is known by its name alone, as the person knows it.
pub const FILES: &str = "";
/// The studio's own folder (settings, the session, fonts): kept between
/// visits, never handed to the person. (`dock::config_dir` names it too.)
const OWN: &str = "/zigzag";
/// The fonts the person added, in the studio's folder.
const FONTS: &str = "/zigzag/fonts";

#[wasm_bindgen(module = "/src/platform/files.js")]
extern "C" {
    async fn kept_load() -> JsValue;
    fn kept_put(path: &str, bytes: &[u8]);
    fn kept_remove(path: &str);
    fn keep_for_good();
    fn can_pick() -> bool;
    async fn pick_open(description: &str, exts: Vec<String>, many: bool) -> JsValue;
    async fn pick_save(name: &str, description: &str, exts: Vec<String>) -> JsValue;
    fn deliver(name: &str, bytes: &[u8]);
    async fn claim_name(name: &str, handle: JsValue) -> JsValue;
    async fn font_library() -> JsValue;
}

/// Fetch the fonts that come with ZZZ to embroider in (`assets/fonts/
/// library`), in the background: once they're in, the font lists show them.
pub fn load_font_library(ctx: &eframe::egui::Context) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let fonts = named_files(font_library().await);
        let added = zigzag::svg::add_fonts(fonts.into_iter().map(|(_, bytes)| bytes));
        if added > 0 {
            ctx.request_repaint();
        }
    });
}

/// Whose a file in memory is, which says where what's written to it goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Whose {
    /// The studio's own (settings, the session, fonts): kept in the browser.
    Studio,
    /// The person's: picked, dropped, saved or exported here, known by its
    /// name alone. Kept, and written to the file on the computer.
    Person,
    /// An agent's: a file on the agent's computer, by its full path (it
    /// came through the relay). What's written goes back the same way.
    Agent,
}

fn whose(path: &Path) -> Whose {
    if path.starts_with(OWN) {
        Whose::Studio
    } else if path.has_root() {
        Whose::Agent
    } else {
        Whose::Person
    }
}

/// A document (kept in memory and between visits), not an export (handed
/// over and done with).
fn is_document(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

/// What happens to a file just written (`now`; `None`: it was removed).
/// An agent's file goes into `for_agent`, to go back with the answer to its
/// call; with none (the person saved an agent's document by hand), it's
/// handed to the person under its name, as a new file of theirs.
fn send_on(path: &Path, now: Option<Vec<u8>>, for_agent: Option<&mut Vec<(String, Vec<u8>)>>) {
    let name = key(path);
    let Some(bytes) = now else {
        kept_remove(&name);
        return;
    };
    match (whose(path), for_agent) {
        (Whose::Studio, _) => kept_put(&name, &bytes),
        (Whose::Agent, Some(out)) => {
            out.push((name, bytes));
            // (A saved document stays in memory: it's what's open.)
            if !is_document(path) {
                zigzag::vfs::mem::forget(path);
            }
        }
        (Whose::Agent, None) => {
            let short = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "design.svg".into());
            deliver(&short, &bytes);
            if !is_document(path) {
                zigzag::vfs::mem::forget(path);
            }
        }
        (Whose::Person, _) => {
            deliver(&name, &bytes);
            // A document stays, to open again next time; an export was for
            // the machine, and is done with once it's handed over (and so
            // is any copy of it kept from before).
            if is_document(path) {
                kept_put(&name, &bytes);
            } else {
                zigzag::vfs::mem::forget(path);
                kept_remove(&name);
            }
        }
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// `[[name, bytes], …]` from `files.js`.
fn named_files(list: JsValue) -> Vec<(String, Vec<u8>)> {
    js_sys::Array::from(&list)
        .iter()
        .filter_map(|pair| {
            let pair = js_sys::Array::from(&pair);
            let name = pair.get(0).as_string()?;
            Some((name, js_sys::Uint8Array::new(&pair.get(1)).to_vec()))
        })
        .collect()
}

/// At the start, before the studio: bring back what was kept from the
/// last visit (settings, the session, its documents, added fonts).
pub async fn restore() {
    let mut fonts = Vec::new();
    for (path, bytes) in named_files(kept_load().await) {
        if Path::new(&path).starts_with(FONTS) {
            fonts.push(bytes.clone());
        }
        zigzag::vfs::mem::restore(Path::new(&path), bytes);
    }
    zigzag::svg::add_fonts(fonts);
    keep_for_good();
}

/// A file from outside (picked, dropped): among the files in memory, and
/// kept for the next visit. Its path there.
fn bring_in(name: &str, bytes: Vec<u8>) -> PathBuf {
    let path = Path::new(FILES).join(name);
    kept_put(&key(&path), &bytes);
    zigzag::vfs::mem::restore(&path, bytes);
    path
}

/// Each frame: what the studio wrote since the last one goes where it
/// belongs (see [`send_on`]).
pub fn flush() {
    for (path, now) in zigzag::vfs::mem::take_changes() {
        send_on(&path, now, None);
    }
}

/// The Open panel: the path, among the files in memory, of what was chosen.
pub async fn open_panel(filters: Vec<(String, Vec<String>)>) -> Option<PathBuf> {
    if can_pick() {
        // (One list: the browser's panel shows the first type only, and
        // the first filter is the one with every kind.)
        let (description, exts) = filters.into_iter().next().unwrap_or_default();
        let (name, bytes) = named_files(pick_open(&description, exts, false).await)
            .into_iter()
            .next()?;
        return Some(bring_in(&name, bytes));
    }
    let mut d = rfd::AsyncFileDialog::new();
    for (name, exts) in &filters {
        d = d.add_filter(name, exts);
    }
    let h = d.pick_file().await?;
    Some(bring_in(&h.file_name(), h.read().await))
}

/// The Save panel: the path to write to, among the files in memory. What's
/// written there reaches the file chosen at the next [`flush`]. (In a
/// browser without such a panel: the name suggested, and a download.)
pub async fn save_panel(
    suggested: String,
    filters: Vec<(String, Vec<String>)>,
) -> Option<PathBuf> {
    if !can_pick() {
        return Some(Path::new(FILES).join(suggested));
    }
    let (description, exts) = filters.into_iter().next().unwrap_or_default();
    let name = pick_save(&suggested, &description, exts)
        .await
        .as_string()?;
    Some(Path::new(FILES).join(name))
}

thread_local! {
    /// Files dropped on the window and read since, waiting to be opened.
    static DROPPED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Whether leaving the page now would lose unsaved changes.
    static UNSAVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The tab's title as last set.
    static TITLE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// A file was dropped on the window: read it (the browser only does that
/// later) into the files in memory; [`take_dropped`] then has its path.
pub fn read_dropped(file: eframe::egui::DroppedFileHandle, ctx: &eframe::egui::Context) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let name = file.path().to_string_lossy().into_owned();
        match file.bytes_async().await {
            Ok(bytes) => {
                // (No handle to tell it's the same file: a name that's
                // taken gets another.)
                let name = claim_name(&name, JsValue::NULL)
                    .await
                    .as_string()
                    .unwrap_or(name);
                let path = bring_in(&name, bytes);
                DROPPED.with(|d| d.borrow_mut().push(path));
                ctx.request_repaint();
            }
            Err(e) => web_sys::console::error_1(
                &format!("ZZZ: couldn't read the dropped file: {e}").into(),
            ),
        }
    });
}

/// The dropped files read since the last call.
pub fn take_dropped() -> Vec<PathBuf> {
    DROPPED.with(|d| std::mem::take(&mut *d.borrow_mut()))
}

/// Type › Add Fonts…: font files from the computer, to set text in. They
/// are kept, so they're there on the next visit too.
pub fn add_fonts(ctx: &eframe::egui::Context) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let exts = ["ttf", "otf", "ttc"].map(String::from).to_vec();
        let picked = if can_pick() {
            named_files(pick_open("Fonts", exts, true).await)
        } else {
            let mut out = Vec::new();
            let d = rfd::AsyncFileDialog::new().add_filter("Fonts", &exts);
            for h in d.pick_files().await.unwrap_or_default() {
                out.push((h.file_name(), h.read().await));
            }
            out
        };
        let mut fonts = Vec::new();
        for (name, bytes) in picked {
            // (Written as a change: `flush` keeps it.)
            let _ = zigzag::vfs::write(Path::new(FONTS).join(name), &bytes);
            fonts.push(bytes);
        }
        if zigzag::svg::add_fonts(fonts) > 0 {
            ctx.request_repaint();
        }
    });
}

/// Each frame: what the tab is called, and whether there are unsaved changes.
pub fn page_state(title: &str, unsaved: bool) {
    UNSAVED.with(|u| u.set(unsaved));
    TITLE.with(|t| {
        if *t.borrow() != title {
            if let Some(d) = web_sys::window().and_then(|w| w.document()) {
                d.set_title(title);
            }
            *t.borrow_mut() = title.to_string();
        }
    });
}

/// Whether leaving the page now would lose unsaved changes.
pub fn unsaved() -> bool {
    UNSAVED.with(|u| u.get())
}

/// Open a link, in another tab.
pub fn open_url(url: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.open_with_url_and_target(url, "_blank");
    }
}

#[wasm_bindgen(module = "/src/platform/ble.js")]
extern "C" {
    fn ble_there() -> bool;
    #[wasm_bindgen(catch)]
    async fn ble_connect(service: &str, write: &str, read: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    async fn ble_request(frame: &[u8]) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn ble_send(frame: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    async fn ble_read() -> Result<JsValue, JsValue>;
    fn ble_connected() -> bool;
    fn ble_close();
}

/// What a failed promise said.
fn js_error(e: JsValue) -> anyhow::Error {
    let said = e
        .dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| e.as_string())
        .unwrap_or_else(|| "the browser's Bluetooth failed".into());
    anyhow::anyhow!(said)
}

/// The Skitch over the browser's Bluetooth (`ble.js`).
struct WebBle;

#[async_trait::async_trait(?Send)]
impl zigzag::machine::session::Transport for WebBle {
    async fn request(&mut self, command: u16, payload: &[u8]) -> anyhow::Result<Vec<u8>> {
        let frame = zigzag::machine::protocol::frame(command, payload);
        let reply = ble_request(&frame).await.map_err(js_error)?;
        Ok(js_sys::Uint8Array::new(&reply).to_vec())
    }

    async fn send(&mut self, command: u16, payload: &[u8]) -> anyhow::Result<()> {
        let frame = zigzag::machine::protocol::frame(command, payload);
        ble_send(&frame).await.map_err(js_error)
    }

    async fn read(&mut self) -> anyhow::Result<Vec<u8>> {
        let reply = ble_read().await.map_err(js_error)?;
        Ok(js_sys::Uint8Array::new(&reply).to_vec())
    }

    fn connected(&mut self) -> bool {
        ble_connected()
    }

    fn close(&mut self) {
        ble_close();
    }
}

/// Connect to a Skitch: the browser shows its chooser, with the machines
/// in range. (Closing it, or none being there, is "not found".)
pub async fn ble_link() -> anyhow::Result<Box<dyn zigzag::machine::session::Transport>> {
    use zigzag::machine::protocol::{READ_CHAR_UUID, SERVICE_UUID, WRITE_CHAR_UUID};
    if !ble_there() {
        anyhow::bail!("This browser has no Bluetooth. ZZZ sends to the machine from Chrome or Edge.");
    }
    match ble_connect(SERVICE_UUID, WRITE_CHAR_UUID, READ_CHAR_UUID).await {
        Ok(()) => Ok(Box::new(WebBle)),
        Err(e) => {
            let not_found = e
                .dyn_ref::<js_sys::Error>()
                // (The chooser was closed, or couldn't be shown: it
                // needs a click to have just happened, which there isn't
                // when the app reconnects by itself at the start.)
                .is_some_and(|e| e.name() == "NotFoundError" || e.name() == "SecurityError");
            if not_found {
                Err(anyhow::Error::new(zigzag::machine::session::NotFound))
            } else {
                Err(js_error(e))
            }
        }
    }
}

// ── Agents (MCP), through the relay ─────────────────────────────────────

#[wasm_bindgen(module = "/src/platform/relay.js")]
extern "C" {
    fn relay_start(on_news: &js_sys::Function);
    fn relay_take() -> js_sys::Array;
    fn relay_pending() -> bool;
    fn relay_send(text: &str);
    fn relay_state() -> String;
}

/// Where agents stand with this tab, for the status bar's MCP chip.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Relay {
    /// The page wasn't opened with a relay's link.
    Off,
    /// Paired, and the relay isn't answering (not started, or stopped).
    Looking,
    On,
}

pub fn relay() -> Relay {
    match relay_state().as_str() {
        "on" => Relay::On,
        "looking" => Relay::Looking,
        _ => Relay::Off,
    }
}

/// At the start: look for the relay, if the page was opened with its link.
/// A message from it wakes the studio.
pub fn relay_begin(ctx: &eframe::egui::Context) {
    let ctx = ctx.clone();
    let on_news = Closure::<dyn FnMut()>::new(move || ctx.request_repaint());
    relay_start(on_news.as_ref().unchecked_ref());
    on_news.forget();
}

/// Each frame: run what agents asked for since the last one, as the
/// desktop's MCP server does (`mcp.rs`), with the studio's own tools.
///
/// A call names files by their path on the agent's computer, which the
/// page can't reach: the relay sends a file to be read along with the call
/// (it's put among the files in memory, under that path, for the tool to
/// find), and what a call wrote goes back to it with the answer, to be
/// written there.
///
/// True when calls are waiting for the studio to be free (a dialog, a
/// drag): it should look again soon.
pub fn serve_agents(studio: &mut crate::app::Studio, busy: bool) -> bool {
    use base64::Engine;
    use serde_json::{Value, json};
    if relay() != Relay::On {
        return false;
    }
    // (Calls wait while a dialog previews into the document or something
    // is dragged, as on the desktop: they stay with `relay.js` meanwhile.)
    if busy {
        return relay_pending();
    }
    let asked = relay_take();
    if asked.length() == 0 {
        return false;
    }
    // What the person's own work wrote goes its usual way first.
    flush();
    thread_local! {
        static TOOLS: Value = crate::tools::tool_specs();
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    for text in asked.iter().filter_map(|t| t.as_string()) {
        let Ok(mut asked) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(files) = asked["files"].as_object() {
            for (path, data) in files {
                if let Ok(bytes) = b64.decode(data.as_str().unwrap_or("")) {
                    zigzag::vfs::mem::restore(Path::new(path), bytes);
                }
            }
        }
        let rpc = asked["rpc"].take();
        let mut call = |name: String, args: Value| {
            let at = web_time::Instant::now();
            let result = studio.run_mcp_call(&name, &args);
            studio.mcp_ui.note(&name, &args, at, &result);
            result
        };
        let reply = TOOLS.with(|tools| match &rpc {
            Value::Array(batch) => {
                let answers: Vec<Value> = batch
                    .iter()
                    .filter_map(|m| {
                        crate::mcp::handle_rpc(m, tools, crate::tools::INSTRUCTIONS, &mut call)
                    })
                    .collect();
                (!answers.is_empty()).then_some(Value::Array(answers))
            }
            m => crate::mcp::handle_rpc(m, tools, crate::tools::INSTRUCTIONS, &mut call),
        });
        // What the call wrote goes where it belongs: an agent's files back
        // to its computer, with the answer; the person's (the agent saved
        // the document the person opened) to the person's file, as a save
        // of theirs would.
        let mut for_agent = Vec::new();
        for (path, now) in zigzag::vfs::mem::take_changes() {
            send_on(&path, now, Some(&mut for_agent));
        }
        let written: serde_json::Map<String, Value> = for_agent
            .into_iter()
            .map(|(name, bytes)| (name, b64.encode(&bytes).into()))
            .collect();
        relay_send(&json!({ "id": asked["id"], "reply": reply, "files": written }).to_string());
    }
    false
}
