//! macOS: the window's close, minimise and zoom buttons ("traffic lights"),
//! moved down onto the toolbar's centre line.
//!
//! The toolbar is drawn under a transparent title bar (see `main`), and is
//! taller than a title bar, so left alone the buttons sit above the row of
//! toolbar buttons beside them. AppKit has no setting for where they go, and
//! puts them back whenever it lays the title bar out again (a resize, coming
//! out of full screen, the window gaining or losing focus), so they're placed
//! again each time that happens, and checked every frame besides.

/// The space the toolbar leaves at its left edge for the buttons when their
/// place isn't known (before the first frame, and in tests).
const ROOM: f32 = 70.0;

/// How far the first button is from the window's left edge: about its
/// distance from the top, so the three sit squarely in the corner.
#[cfg(target_os = "macos")]
const LEFT: f64 = 18.0;

/// The gap between the last button and whatever the toolbar draws next.
#[cfg(target_os = "macos")]
const GAP: f32 = 16.0;

#[cfg(target_os = "macos")]
mod imp {
    use super::{GAP, LEFT};
    use objc2::encode::{Encode, Encoding};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{AnyThread, MainThreadMarker, class, define_class, msg_send, sel};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Size {
        w: f64,
        h: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Rect {
        at: Point,
        size: Size,
    }
    // SAFETY: laid out as, and named for, the CoreGraphics types AppKit
    // passes (two doubles; two doubles; a point and a size).
    unsafe impl Encode for Point {
        const ENCODING: Encoding =
            Encoding::Struct("CGPoint", &[Encoding::Double, Encoding::Double]);
    }
    unsafe impl Encode for Size {
        const ENCODING: Encoding =
            Encoding::Struct("CGSize", &[Encoding::Double, Encoding::Double]);
    }
    unsafe impl Encode for Rect {
        const ENCODING: Encoding = Encoding::Struct("CGRect", &[Point::ENCODING, Size::ENCODING]);
    }

    /// `NSWindowStyleMaskTitled`, `NSWindowStyleMaskFullScreen`.
    const TITLED: usize = 1;
    const FULL_SCREEN: usize = 1 << 14;

    /// Where the toolbar's own things start, in points from the window's
    /// left edge, as f32 bits (0: not known yet).
    static AFTER: AtomicU32 = AtomicU32::new(0);
    /// The toolbar's height, as f32 bits: what the buttons are centred in.
    static STRIP: AtomicU32 = AtomicU32::new(0);

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and this class
        // has no instance variables and doesn't implement Drop.
        #[unsafe(super(NSObject))]
        #[name = "ZigZagTrafficLights"]
        struct Watcher;

        impl Watcher {
            /// AppKit has just laid the title bar out again.
            #[unsafe(method(windowChanged:))]
            fn window_changed(&self, note: &AnyObject) {
                // SAFETY: a window notification's object is its window.
                unsafe {
                    let window: *mut AnyObject = msg_send![note, object];
                    if let Some(window) = window.as_ref() {
                        place(window);
                    }
                }
            }
        }
    );

    struct Placed {
        window: Retained<AnyObject>,
        _watcher: Retained<Watcher>,
    }

    thread_local! {
        static PLACED: RefCell<Option<Placed>> = const { RefCell::new(None) };
    }

    unsafe fn string(s: &std::ffi::CStr) -> *mut AnyObject {
        unsafe { msg_send![class!(NSString), stringWithUTF8String: s.as_ptr()] }
    }

    /// The app's document window: the one with a title bar's buttons.
    unsafe fn main_window() -> Option<Retained<AnyObject>> {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let windows: *mut AnyObject = msg_send![app, windows];
            let n: usize = msg_send![windows, count];
            for i in 0..n {
                let w: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                let panel: bool = msg_send![w, isKindOfClass: class!(NSPanel)];
                let mask: usize = msg_send![w, styleMask];
                let close: *mut AnyObject = msg_send![w, standardWindowButton: 0usize];
                if !panel && mask & TITLED != 0 && !close.is_null() {
                    return Retained::retain(w);
                }
            }
            None
        }
    }

    /// Puts the three buttons on the toolbar's centre line. Does nothing
    /// when they're there already, so it's cheap to call every frame.
    unsafe fn place(window: &AnyObject) {
        let strip = f32::from_bits(STRIP.load(Ordering::Relaxed)) as f64;
        if strip <= 0.0 {
            return;
        }
        unsafe {
            // In full screen the title bar is the system's (it slides down
            // with the menu bar), and moving its buttons does nothing good.
            let mask: usize = msg_send![window, styleMask];
            if mask & FULL_SCREEN != 0 {
                return;
            }
            let mut buttons = [std::ptr::null_mut::<AnyObject>(); 3];
            for (kind, b) in buttons.iter_mut().enumerate() {
                *b = msg_send![window, standardWindowButton: kind];
                if b.is_null() {
                    return;
                }
            }
            let bar: *mut AnyObject = msg_send![buttons[0], superview];
            if bar.is_null() {
                return;
            }
            let container: *mut AnyObject = msg_send![bar, superview];
            if container.is_null() {
                return;
            }
            let frame: Rect = msg_send![window, frame];
            // The title bar's container, made as tall as the toolbar: the
            // buttons are only drawn, and only take clicks, inside it.
            let old: Rect = msg_send![container, frame];
            let tall = Rect {
                at: Point {
                    x: old.at.x,
                    y: frame.size.h - strip,
                },
                size: Size {
                    w: old.size.w,
                    h: strip,
                },
            };
            if !near(old.at.y, tall.at.y) || !near(old.size.h, tall.size.h) {
                let _: () = msg_send![container, setFrame: tall];
            }
            // The centre line, in the coordinates the buttons are placed in.
            let centre = Point {
                x: LEFT,
                y: frame.size.h - strip / 2.0,
            };
            let centre: Point =
                msg_send![bar, convertPoint: centre, fromView: std::ptr::null::<AnyObject>()];
            let first: Rect = msg_send![buttons[0], frame];
            let second: Rect = msg_send![buttons[1], frame];
            let step = second.at.x - first.at.x;
            let mut right = LEFT;
            for (i, &b) in buttons.iter().enumerate() {
                let at: Rect = msg_send![b, frame];
                let want = Point {
                    x: centre.x + step * i as f64,
                    y: centre.y - at.size.h / 2.0,
                };
                if !near(at.at.x, want.x) || !near(at.at.y, want.y) {
                    let _: () = msg_send![b, setFrameOrigin: want];
                }
                right = LEFT + step * i as f64 + at.size.w;
            }
            AFTER.store((right as f32 + GAP).to_bits(), Ordering::Relaxed);
            if std::env::var_os("ZIGZAG_LIGHTS_DEBUG").is_some() {
                report(window, &buttons, strip);
            }
        }
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    /// Where the buttons ended up, measured from the window's top left
    /// (for checking the placement without looking at the screen).
    unsafe fn report(window: &AnyObject, buttons: &[*mut AnyObject; 3], strip: f64) {
        thread_local! {
            static LAST: RefCell<String> = const { RefCell::new(String::new()) };
        }
        unsafe {
            let frame: Rect = msg_send![window, frame];
            let mut line = format!("lights: toolbar centre {:.1}", strip / 2.0);
            for &b in buttons {
                let bounds: Rect = msg_send![b, bounds];
                let r: Rect =
                    msg_send![b, convertRect: bounds, toView: std::ptr::null::<AnyObject>()];
                line += &format!(
                    " | x {:.1}..{:.1} centre y {:.1}",
                    r.at.x,
                    r.at.x + r.size.w,
                    frame.size.h - (r.at.y + r.size.h / 2.0)
                );
            }
            LAST.with(|last| {
                if *last.borrow() != line {
                    eprintln!("{line}");
                    *last.borrow_mut() = line;
                }
            });
        }
    }

    pub fn sync(strip: f32) {
        if MainThreadMarker::new().is_none() {
            return;
        }
        STRIP.store(strip.to_bits(), Ordering::Relaxed);
        PLACED.with(|placed| {
            let mut placed = placed.borrow_mut();
            if placed.is_none() {
                // SAFETY: on the main thread, with AppKit's own classes.
                *placed = unsafe { main_window() }.map(|window| {
                    let watcher: Retained<Watcher> = unsafe { msg_send![Watcher::alloc(), init] };
                    for name in [
                        c"NSWindowDidResizeNotification",
                        c"NSWindowDidExitFullScreenNotification",
                        c"NSWindowDidBecomeKeyNotification",
                        c"NSWindowDidResignKeyNotification",
                        c"NSWindowDidBecomeMainNotification",
                        c"NSWindowDidResignMainNotification",
                        c"NSWindowDidChangeScreenNotification",
                    ] {
                        // SAFETY: the watcher lives as long as the app does
                        // (it's kept beside the window, and never dropped).
                        unsafe {
                            let centre: *mut AnyObject =
                                msg_send![class!(NSNotificationCenter), defaultCenter];
                            let _: () = msg_send![
                                centre,
                                addObserver: &*watcher,
                                selector: sel!(windowChanged:),
                                name: string(name),
                                object: &*window,
                            ];
                        }
                    }
                    Placed {
                        window,
                        _watcher: watcher,
                    }
                });
            }
            if let Some(p) = placed.as_ref() {
                // SAFETY: the window is kept alive by `Placed`.
                unsafe { place(&p.window) };
            }
        });
    }

    pub fn room() -> Option<f32> {
        let after = f32::from_bits(AFTER.load(Ordering::Relaxed));
        (after > 0.0).then_some(after)
    }
}

/// Keeps the window's buttons centred in a toolbar `strip` points tall.
/// Called every frame; a no-op off the Mac and off the main thread.
pub fn sync(strip: f32) {
    #[cfg(target_os = "macos")]
    imp::sync(strip);
    #[cfg(not(target_os = "macos"))]
    let _ = strip;
}

/// How far from the window's left edge the toolbar's own things start.
pub fn room() -> f32 {
    #[cfg(target_os = "macos")]
    if let Some(r) = imp::room() {
        return r;
    }
    ROOM + 10.0
}
