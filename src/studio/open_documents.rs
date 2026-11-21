//! Files the Mac asks ZigZag to open: a double-click in the Finder, Open
//! With, a drop on the Dock icon, `open -a ZigZag file.svg`. The Finder
//! doesn't put the file on the command line: it launches ZigZag (or finds
//! it running) and sends it an "open documents" Apple Event. Those files
//! open as tabs in the window, which comes to the front, the same way as
//! files a later launch hands over (see `instance`).
//!
//! winit has the application delegate and doesn't pass this event on, so
//! ZigZag takes it from the Apple Event manager instead. AppKit puts its own
//! handler there while it finishes launching, before the event for the
//! files ZigZag was launched with arrives: ZigZag's goes in on the
//! "will finish launching" notification, which comes in between.
//!
//! Elsewhere there's no such event, and nothing arrives here.

use std::path::PathBuf;
use std::sync::Mutex;

type Wake = Box<dyn Fn() + Send>;

/// What the system asked to open and the window hasn't taken yet. (At
/// launch the event comes before there's a window to take it.)
static ARRIVED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// Wakes the window once it's there.
static WAKE: Mutex<Option<Wake>> = Mutex::new(None);

/// Take the event from launch on. Call it first thing, on the main thread,
/// before the event loop runs.
pub fn install() {
    #[cfg(target_os = "macos")]
    mac::install();
}

/// Called when files arrive (the window repaints, and takes them).
pub fn on_arrival(wake: impl Fn() + Send + 'static) {
    if let Ok(mut w) = WAKE.lock() {
        *w = Some(Box::new(wake));
    }
    // (Some may have come before there was a window.)
    if ARRIVED.lock().is_ok_and(|a| !a.is_empty()) {
        wake_window();
    }
}

/// Anything arrived since last time.
pub fn pending() -> bool {
    ARRIVED.lock().is_ok_and(|a| !a.is_empty())
}

/// The files that arrived since last time, in order.
pub fn take() -> Vec<PathBuf> {
    ARRIVED
        .lock()
        .map(|mut a| std::mem::take(&mut *a))
        .unwrap_or_default()
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn arrive(files: Vec<PathBuf>) {
    if let Ok(mut a) = ARRIVED.lock() {
        a.extend(files);
    }
    wake_window();
}

fn wake_window() {
    if let Ok(w) = WAKE.lock()
        && let Some(w) = w.as_ref()
    {
        w();
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::NSApplicationWillFinishLaunchingNotification;
    use objc2_foundation::{
        NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter,
    };
    use std::path::PathBuf;

    const fn four_char(c: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*c)
    }
    /// kCoreEventClass
    const CORE_EVENT_CLASS: u32 = four_char(b"aevt");
    /// kAEOpenDocuments
    const OPEN_DOCUMENTS: u32 = four_char(b"odoc");
    /// keyDirectObject: the list of files.
    const DIRECT_OBJECT: u32 = four_char(b"----");

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "ZigZagOpenDocuments"]
        struct Opener;

        impl Opener {
            #[unsafe(method(willFinishLaunching:))]
            fn will_finish_launching(&self, _note: &NSNotification) {
                self.take_event();
            }

            #[unsafe(method(openDocuments:withReplyEvent:))]
            fn open_documents(
                &self,
                event: &NSAppleEventDescriptor,
                _reply: &NSAppleEventDescriptor,
            ) {
                let files = files_of(event);
                if !files.is_empty() {
                    super::arrive(files);
                }
            }
        }
    );

    impl Opener {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        }

        /// Be the one the "open documents" event goes to.
        fn take_event(&self) {
            let manager = NSAppleEventManager::sharedAppleEventManager();
            // SAFETY: the handler is an object that lives as long as the
            // app, and its method takes the event and the reply.
            unsafe {
                let _: () = msg_send![
                    &*manager,
                    setEventHandler: self,
                    andSelector: sel!(openDocuments:withReplyEvent:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: OPEN_DOCUMENTS,
                ];
            }
        }
    }

    /// The files an "open documents" event names.
    fn files_of(event: &NSAppleEventDescriptor) -> Vec<PathBuf> {
        // SAFETY: a keyword is a four-character code.
        let direct: Option<Retained<NSAppleEventDescriptor>> =
            unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
        let Some(direct) = direct else {
            return Vec::new();
        };
        // A list (1-based), or now and then a single file.
        let n = direct.numberOfItems();
        let items: Vec<Retained<NSAppleEventDescriptor>> = if n > 0 {
            (1..=n)
                .filter_map(|i| direct.descriptorAtIndex(i))
                .collect()
        } else {
            vec![direct]
        };
        items
            .iter()
            .filter_map(|d| d.fileURLValue()?.to_file_path())
            .collect()
    }

    pub fn install() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let opener = Opener::new(mtm);
        // Now, and again once AppKit has put its own handler in.
        opener.take_event();
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: the observer lives as long as the app, and its method
        // takes the notification.
        unsafe {
            center.addObserver_selector_name_object(
                &opener,
                sel!(willFinishLaunching:),
                Some(NSApplicationWillFinishLaunchingNotification),
                None,
            );
        }
        // Neither the notification centre nor the Apple Event manager keeps
        // it: it stays for as long as ZigZag runs.
        std::mem::forget(opener);
    }
}
