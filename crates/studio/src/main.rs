//! zigzag-studio: the ZigZag studio on the desktop. (The studio itself is
//! the `zigzag_studio` library; the browser's start is the `zigzag-web`
//! crate.)

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    zigzag_studio::run()
}

// (There's no desktop binary in the browser.)
#[cfg(target_arch = "wasm32")]
fn main() {}
