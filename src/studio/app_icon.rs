#[cfg(target_os = "macos")]
pub fn sync(ctx: &eframe::egui::Context) {
    use eframe::egui::Theme;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    use std::cell::Cell;

    thread_local! {
        static APPLIED_THEME: Cell<Option<Theme>> = const { Cell::new(None) };
    }

    let theme = ctx.theme();
    APPLIED_THEME.with(|applied| {
        if applied.get() == Some(theme) {
            return;
        }
        let Some(main_thread) = MainThreadMarker::new() else {
            return;
        };
        let bytes: &[u8] = match theme {
            Theme::Light => include_bytes!("../../packaging/macos/AppIcon.icns"),
            Theme::Dark => include_bytes!("../../packaging/macos/AppIcon-dark.icns"),
        };
        let data = NSData::with_bytes(bytes);
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
            unsafe {
                NSApplication::sharedApplication(main_thread).setApplicationIconImage(Some(&image));
            }
            applied.set(Some(theme));
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn sync(_ctx: &eframe::egui::Context) {}
