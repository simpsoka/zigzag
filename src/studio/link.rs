//! `zigzag://` links: the "Open" button on a zigzag.diy share page.
//!
//! The page links to `zigzag://open?url=<file>&name=<design name>`, where
//! `<file>` is an `https` address of the design (an SVG, or a PES when its
//! designer keeps the SVG). ZigZag downloads it into a ZigZag folder under
//! Downloads and opens it as a new tab, as if the file had been opened.
//!
//! The link arrives three ways: as an Apple Event on macOS (`GURL`, the
//! same whether ZigZag was running or not), and as a command-line argument
//! on Windows and Linux, which a later launch hands to the running window
//! like a file (see `instance`). Each one is queued here and downloaded on
//! a thread of its own; the window takes what's finished each frame.
//!
//! Anything can make a link, so a link is taken at its word for nothing:
//! only `https` addresses are fetched (and `http://localhost` in debug
//! builds), redirects must stay on `https`, at most [`MAX_BYTES`] are
//! read, the file name the server gives is cut down to a plain name in the
//! ZigZag folder (never a path, never over another file), and the file
//! has to be a kind ZigZag opens and look like one inside.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// The largest file fetched: the server's own limit on what's shared.
pub const MAX_BYTES: u64 = 25_000_000;
/// A link longer than this isn't one the share page made.
const LONGEST_LINK: usize = 8 * 1024;

/// A parsed `zigzag://open` link.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// The file to fetch: `https://…` (or `http://localhost…` in debug builds).
    pub url: String,
    /// The design's name, for a file name when the server gives none.
    pub name: Option<String>,
}

/// Whether a command-line argument is a link rather than a file.
pub fn is_link(arg: &str) -> bool {
    arg.get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("zigzag:"))
}

/// Read a `zigzag://open?url=…&name=…` link.
pub fn parse(link: &str) -> Result<Link, String> {
    let bad = |why: &str| Err(format!("That ZigZag link can't be opened: {why}."));
    if link.len() > LONGEST_LINK {
        return bad("it's too long");
    }
    if link.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return bad("it has spaces or control characters in it");
    }
    let Some(rest) = link
        .get(..9)
        .filter(|s| s.eq_ignore_ascii_case("zigzag://"))
        .map(|_| &link[9..])
    else {
        return bad("it isn't a zigzag:// link");
    };
    let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
    let query = query.split('#').next().unwrap_or("");
    if !action.trim_end_matches('/').eq_ignore_ascii_case("open") {
        return bad("ZigZag only knows zigzag://open");
    }
    let (mut url, mut name) = (None, None);
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let Some(v) = percent_decode(v) else {
            return bad("it isn't encoded properly");
        };
        match k {
            "url" if url.is_none() => url = Some(v),
            "url" => return bad("it names more than one file"),
            "name" => name = Some(v).filter(|n| !n.trim().is_empty()),
            _ => {}
        }
    }
    let Some(url) = url.filter(|u| !u.is_empty()) else {
        return bad("it doesn't say which file to open");
    };
    if let Err(why) = check_url(&url, cfg!(debug_assertions)) {
        return bad(why);
    }
    Ok(Link { url, name })
}

/// An address ZigZag may fetch: `https` with a host and no password in it
/// (and `http://localhost` when `local` is let, for a server under test).
fn check_url(url: &str, local: bool) -> Result<(), &'static str> {
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("the file's address has spaces in it");
    }
    let lower = url.to_ascii_lowercase();
    let (rest, http) = if let Some(r) = lower.strip_prefix("https://") {
        (r, false)
    } else if let Some(r) = lower.strip_prefix("http://") {
        (r, true)
    } else {
        return Err("the file isn't on an https:// address");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("the file's address has a user name in it");
    }
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    if host.is_empty() {
        return Err("the file's address has no host");
    }
    if http && !(local && matches!(host, "localhost" | "127.0.0.1" | "::1")) {
        return Err("the file isn't on an https:// address");
    }
    Ok(())
}

/// `%xx` and `+` decoded; None when that isn't UTF-8 or a `%` is cut short.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The file name a `Content-Disposition` header gives, if any
/// (`filename*=UTF-8''…` before `filename="…"`), not yet made safe.
fn disposition_name(header: &str) -> Option<String> {
    let mut plain = None;
    for part in header.split(';').map(str::trim) {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        match k.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                let v = v.trim().trim_matches('"');
                // charset'language'value (one that isn't leaves the
                // plain `filename`, if there is one)
                let mut it = v.splitn(3, '\'');
                if let (Some(charset), Some(_), Some(value)) = (it.next(), it.next(), it.next())
                    && charset.eq_ignore_ascii_case("utf-8")
                    && let Some(name) = percent_decode(&value.replace('+', "%2B"))
                {
                    return Some(name);
                }
            }
            "filename" => plain = Some(v.trim().trim_matches('"').to_string()),
            _ => {}
        }
    }
    plain
}

/// A name that's only a name: no folders, no control characters, nothing
/// Windows refuses, not hidden, not too long. Empty when nothing's left.
fn safe_stem(name: &str) -> String {
    // The last part of anything that looks like a path.
    let name = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' ']);
    let mut stem: String = trimmed.chars().take(80).collect();
    // Names Windows keeps for devices.
    let upper = stem.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    if matches!(base, "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit())
    {
        stem.insert(0, '_');
    }
    stem
}

/// What kind of file `bytes` is, going by what's inside, given the
/// extension it was named with: `svg`, or a machine format ZigZag reads.
fn kind_of(bytes: &[u8], named: &str) -> Result<&'static str, String> {
    let named = named.to_ascii_lowercase();
    let head = &bytes[..bytes.len().min(4096)];
    if head.starts_with(b"#PES") {
        return Ok("pes");
    }
    if let Some(f) = zigzag::formats::READABLE_FORMATS
        .iter()
        .find(|f| **f == named && named != "pes")
    {
        // (Formats without a signature: the reader checks them.)
        return Ok(f);
    }
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    if text.starts_with('<') && text.contains("<svg") {
        return Ok("svg");
    }
    Err("the file isn't an SVG or a stitch file ZigZag reads".into())
}

/// Where downloaded designs go: a ZigZag folder in Downloads, else in the
/// temporary folder.
fn download_dir() -> PathBuf {
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    match home.map(|h| h.join("Downloads")).filter(|d| d.is_dir()) {
        Some(d) => d.join("ZigZag"),
        None => std::env::temp_dir().join("ZigZag downloads"),
    }
}

/// Write `bytes` to a new file `stem.ext` in `dir` (`stem 2.ext`, … when
/// that's taken). Never over a file that's there.
fn save_new(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    for n in 1..1000 {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} {n}.{ext}")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(bytes)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many files with that name"))
}

/// Fetch the link's file into `dir`, and say where it is.
pub fn download(link: &Link, dir: &Path) -> Result<PathBuf, String> {
    let (bytes, disposition) = fetch(&link.url)?;
    let from_server = disposition.as_deref().and_then(disposition_name);
    let from_url = link
        .url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('/').next())
        .and_then(percent_decode);
    let named = from_server.or(from_url).unwrap_or_default();
    let (stem, ext) = match safe_stem(&named).rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), e.to_string()),
        _ => (safe_stem(&named), String::new()),
    };
    let ext =
        kind_of(&bytes, &ext).map_err(|why| format!("Couldn't open the shared design: {why}."))?;
    // A stitch format with no signature to go by (DST, EXP, JEF, PEC):
    // kept only if it reads as one, so nothing else is saved under its name.
    if !matches!(ext, "svg" | "pes") && zigzag::formats::read::read_stitch_file(&bytes, ext).is_err()
    {
        return Err(format!(
            "Couldn't open the shared design: it isn't a {} file ZigZag can read.",
            ext.to_ascii_uppercase()
        ));
    }
    // The design's own name is nicer than the server's when there's none.
    let stem = Some(stem)
        .filter(|s| !s.is_empty() && s != "design")
        .or_else(|| link.name.as_deref().map(safe_stem))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Shared design".into());
    save_new(dir, &stem, ext, &bytes)
        .map_err(|e| format!("Couldn't save the shared design in {}: {e}", dir.display()))
}

/// The body of `url` (at most [`MAX_BYTES`]) and its `Content-Disposition`,
/// with the system's `curl` (as updates are fetched).
fn fetch(url: &str) -> Result<(Vec<u8>, Option<String>), String> {
    let headers = tempfile_path("headers");
    let protocols = if cfg!(debug_assertions) && url.to_ascii_lowercase().starts_with("http://") {
        "=http,https"
    } else {
        "=https"
    };
    let mut cmd = Command::new("curl");
    cmd.args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", protocols, "--proto-redir", protocols])
        .args([
            "--max-redirs",
            "5",
            "--connect-timeout",
            "20",
            "--max-time",
            "120",
        ])
        .args(["--max-filesize", &MAX_BYTES.to_string()])
        .args([
            "--user-agent",
            concat!("ZigZag/", env!("CARGO_PKG_VERSION")),
        ])
        .arg("--dump-header")
        .arg(&headers)
        .args(["--output", "-", "--"])
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Couldn't download the shared design (curl: {e})."))?;
    let mut body = Vec::new();
    let read = child
        .stdout
        .take()
        .map(|out| out.take(MAX_BYTES + 1).read_to_end(&mut body));
    if body.len() as u64 > MAX_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&headers);
        return Err(too_big());
    }
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let status = child.wait().map_err(|e| e.to_string());
    let head = std::fs::read_to_string(&headers).unwrap_or_default();
    let _ = std::fs::remove_file(&headers);
    // (63: the server said beforehand that it's over --max-filesize.)
    if matches!(&status, Ok(s) if s.code() == Some(63)) {
        return Err(too_big());
    }
    if !matches!(status, Ok(s) if s.success()) || !matches!(read, Some(Ok(_))) {
        let why = err.trim().trim_start_matches("curl: ").to_string();
        return Err(if why.is_empty() {
            "Couldn't download the shared design.".into()
        } else {
            format!("Couldn't download the shared design ({why}).")
        });
    }
    if body.is_empty() {
        return Err("The shared design came back empty.".into());
    }
    // After redirects, the last response's headers are the file's.
    let last = head
        .rsplit("\r\n\r\n")
        .find(|b| !b.trim().is_empty())
        .unwrap_or("");
    let disposition = last.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("content-disposition")
            .then(|| v.trim().to_string())
    });
    Ok((body, disposition))
}

fn too_big() -> String {
    format!(
        "The shared design is too big to open (over {} MB).",
        MAX_BYTES / 1_000_000
    )
}

/// A new, empty temporary file of this user's own for curl to write to:
/// under a name nobody could have guessed, and made here (never one
/// that's already there, a link to somewhere else say).
fn tempfile_path(what: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for _ in 0..8 {
        // (The standard library's hasher is keyed at random each time.)
        use std::hash::{BuildHasher, Hasher};
        let any = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        path = std::env::temp_dir().join(format!("zigzag-link-{any:016x}-{what}"));
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        if open.open(&path).is_ok() {
            break;
        }
    }
    path
}

/// What happened to a link, for the window to show.
#[derive(Debug)]
pub enum Event {
    /// Downloading the design called this.
    Started(String),
    /// Downloaded here: open it.
    Ready(PathBuf),
    /// Didn't work, and why.
    Failed(String),
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static WAKE: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

fn post(e: Event) {
    if let Ok(mut q) = EVENTS.lock() {
        q.push(e);
    }
    if let Ok(w) = WAKE.lock()
        && let Some(w) = w.as_ref()
    {
        w();
    }
}

/// Called when there's news (the window repaints and takes it).
pub fn on_news(wake: impl Fn() + Send + 'static) {
    if let Ok(mut w) = WAKE.lock() {
        *w = Some(Box::new(wake));
    }
}

/// What happened since last time.
pub fn take() -> Vec<Event> {
    EVENTS
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// The links being downloaded now (their addresses).
static OPENING: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// No more than this many at once (a page can fire as many as it likes).
const OPENING_AT_MOST: usize = 4;

/// Open a `zigzag://` link: download its file in the background. (One
/// already on its way isn't fetched twice.)
pub fn open(link: &str) {
    let link = match parse(link) {
        Ok(l) => l,
        Err(why) => return post(Event::Failed(why)),
    };
    {
        let mut opening = OPENING.lock().unwrap_or_else(|e| e.into_inner());
        if opening.contains(&link.url) {
            return;
        }
        if opening.len() >= OPENING_AT_MOST {
            return post(Event::Failed(
                "Too many shared designs are opening at once. Try that one again in a moment."
                    .into(),
            ));
        }
        opening.push(link.url.clone());
    }
    post(Event::Started(
        link.name
            .clone()
            .unwrap_or_else(|| "the shared design".into()),
    ));
    let _ = std::thread::Builder::new()
        .name("zigzag-link".into())
        .spawn(move || {
            let got = download(&link, &download_dir());
            OPENING
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|u| *u != link.url);
            post(match got {
                Ok(path) => Event::Ready(path),
                Err(why) => Event::Failed(why),
            })
        });
}

/// macOS hands a clicked link to the app as an Apple Event (`GURL`), not
/// on the command line, whether the app was running or is being started
/// for it. Set up before the app runs, so a link that started it isn't
/// missed.
#[cfg(target_os = "macos")]
pub fn listen_for_apple_events() {
    mac::listen();
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{AnyThread, class, define_class, msg_send, sel};

    /// `kInternetEventClass` and `kAEGetURL`: both 'GURL'.
    const GURL: u32 = u32::from_be_bytes(*b"GURL");
    /// `keyDirectObject`: '----'.
    const DIRECT: u32 = u32::from_be_bytes(*b"----");

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and this class
        // has no instance variables and doesn't implement Drop.
        #[unsafe(super(NSObject))]
        #[name = "ZigZagLinkHandler"]
        struct Handler;

        impl Handler {
            #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
            fn get_url(&self, event: &AnyObject, _reply: &AnyObject) {
                // SAFETY: a GURL event's direct object is the URL, as text.
                let text = unsafe {
                    let d: *mut AnyObject = msg_send![event, paramDescriptorForKeyword: DIRECT];
                    let Some(d) = d.as_ref() else { return };
                    let s: *mut AnyObject = msg_send![d, stringValue];
                    let Some(s) = s.as_ref() else { return };
                    let utf8: *const std::ffi::c_char = msg_send![s, UTF8String];
                    if utf8.is_null() {
                        return;
                    }
                    std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
                };
                super::open(&text);
            }
        }
    );

    pub fn listen() {
        // SAFETY: plain AppKit calls on the main thread, before the app runs;
        // the handler is kept for as long as the app runs (leaked).
        unsafe {
            let handler: Retained<Handler> = msg_send![Handler::alloc(), init];
            let manager: *mut AnyObject =
                msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
            let _: () = msg_send![
                manager,
                setEventHandler: &*handler,
                andSelector: sel!(handleGetURLEvent:withReplyEvent:),
                forEventClass: GURL,
                andEventID: GURL
            ];
            std::mem::forget(handler);
        }
    }
}

/// Windows and Linux have no Apple Events: the system starts ZigZag with
/// the link as its argument, once it's told to. The downloads are a zip
/// and a tarball with no installer, so ZigZag tells it itself, for this
/// user, each time a release build starts (then a moved copy is found
/// again). macOS reads `CFBundleURLTypes` in the app's Info.plist instead.
pub fn register_scheme() {
    if cfg!(debug_assertions) || cfg!(target_os = "macos") {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("zigzag-scheme".into())
        .spawn(move || {
            #[cfg(windows)]
            register_windows(&exe);
            #[cfg(all(unix, not(target_os = "macos")))]
            register_linux(&exe);
            let _ = exe;
        });
}

/// `HKCU\Software\Classes\zigzag`: a URL protocol opened with this exe.
#[cfg(windows)]
fn register_windows(exe: &Path) {
    use std::os::windows::process::CommandExt;
    let key = r"HKCU\Software\Classes\zigzag";
    let command = format!("\"{}\" \"%1\"", exe.display());
    let icon = format!("\"{}\",0", exe.display());
    for args in [
        vec!["add", key, "/ve", "/d", "URL:ZigZag", "/f"],
        vec!["add", key, "/v", "URL Protocol", "/d", "", "/f"],
        vec![
            "add",
            &format!(r"{key}\DefaultIcon"),
            "/ve",
            "/d",
            &icon,
            "/f",
        ],
        vec![
            "add",
            &format!(r"{key}\shell\open\command"),
            "/ve",
            "/d",
            &command,
            "/f",
        ],
    ] {
        let _ = Command::new("reg")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }
}

/// A `.desktop` entry for `x-scheme-handler/zigzag`, in the user's
/// applications folder, made the default for it.
#[cfg(all(unix, not(target_os = "macos")))]
fn register_linux(exe: &Path) {
    let Some(dir) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|d| d.join("applications"))
    else {
        return;
    };
    let entry = desktop_entry(exe);
    let file = dir.join("zigzag-studio.desktop");
    if std::fs::read_to_string(&file).ok().as_deref() == Some(entry.as_str()) {
        return;
    }
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&file, entry).is_err() {
        return;
    }
    let quiet = |c: &mut Command| {
        let _ = c
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    };
    quiet(Command::new("update-desktop-database").arg(&dir));
    quiet(Command::new("xdg-mime").args([
        "default",
        "zigzag-studio.desktop",
        "x-scheme-handler/zigzag",
    ]));
}

/// The `.desktop` entry that opens links with `exe`.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn desktop_entry(exe: &Path) -> String {
    // Exec quoting: inside double quotes, `"`, `` ` ``, `$` and `\` are escaped.
    let mut quoted = String::from("\"");
    for c in exe.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    // Then the string rule (a backslash is written twice), and `%` for Exec.
    let quoted = quoted.replace('\\', "\\\\").replace('%', "%%");
    format!(
        "[Desktop Entry]\nType=Application\nName=ZigZag\nComment=Embroidery design studio\n\
         Exec={} %U\nTerminal=false\nCategories=Graphics;VectorGraphics;\n\
         MimeType=x-scheme-handler/zigzag;\nStartupWMClass=zigzag-studio\n",
        quoted
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESIGN: &str = "https://api.zigzag.diy/v1/share/abc123/design";

    fn link(url: &str) -> String {
        format!(
            "zigzag://open?url={}&name=Bee%20badge",
            url.replace('%', "%25")
                .replace(':', "%3A")
                .replace('/', "%2F")
                .replace('?', "%3F")
                .replace('&', "%26")
                .replace('=', "%3D")
        )
    }

    #[test]
    fn reads_a_share_page_link() {
        let l = parse(&link(DESIGN)).unwrap();
        assert_eq!(l.url, DESIGN);
        assert_eq!(l.name.as_deref(), Some("Bee badge"));
        // A signed link keeps its query.
        let signed = format!("{DESIGN}?sig=a%2Bb&exp=17");
        assert_eq!(parse(&link(&signed)).unwrap().url, signed);
        // The name is optional, and so is the slash; the scheme's case isn't fixed.
        let l = parse(&format!(
            "ZigZag://open/?url={}",
            "https%3A%2F%2Fx.diy%2Fa.svg"
        ))
        .unwrap();
        assert_eq!((l.url.as_str(), l.name), ("https://x.diy/a.svg", None));
    }

    #[test]
    fn refuses_other_schemes_and_actions() {
        for bad in [
            format!("https://zigzag.diy/open?url={DESIGN}"),
            format!("zigzag:open?url={DESIGN}"),
            format!("zigzag://save?url={DESIGN}"),
            format!("zigzag://open-it?url={DESIGN}"),
            "zigzag://".to_string(),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn refuses_addresses_that_arent_https() {
        for url in [
            "http://api.zigzag.diy/design",
            "file:///etc/passwd",
            "ftp://x.diy/a.svg",
            "javascript:alert(1)",
            "/v1/share/abc/design",
            "https://",
            "https:///a.svg",
            "https://user:pw@x.diy/a.svg",
            "https://x.diy/a b.svg",
        ] {
            assert!(parse(&link(url)).is_err(), "{url}");
        }
    }

    #[test]
    fn localhost_only_when_let() {
        assert!(check_url("http://localhost:8787/v1/share/a/design", true).is_ok());
        assert!(check_url("http://127.0.0.1:8787/x", true).is_ok());
        assert!(check_url("http://localhost:8787/x", false).is_err());
        assert!(check_url("http://localhost.evil.com/x", true).is_err());
        assert!(check_url("http://evil.com/x", true).is_err());
    }

    #[test]
    fn refuses_links_without_a_file() {
        for bad in [
            "zigzag://open",
            "zigzag://open?",
            "zigzag://open?name=Bee",
            "zigzag://open?url=&name=Bee",
            "zigzag://open?url=https%3A%2F%2Fa.diy%2Fx&url=https%3A%2F%2Fb.diy%2Fy",
            "zigzag://open?url=https%3A%2F%2Fa.diy%2Fx%2",
            "zigzag://open?url=https%3A%2F%2Fa.diy%2F%FF",
            "zigzag://open?url=https%3A%2F%2Fa.diy%2Fx%0A",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse(&"a".repeat(LONGEST_LINK + 1)).is_err());
    }

    #[test]
    fn tells_links_from_files() {
        assert!(is_link("zigzag://open?url=x"));
        assert!(is_link("ZIGZAG://open"));
        assert!(!is_link("zigzag.svg"));
        assert!(!is_link("/Users/me/zigzag:/a.svg"));
    }

    #[test]
    fn file_names_from_the_server() {
        let h = "attachment; filename*=UTF-8''Bee%20badge%20%E2%9C%BF.svg";
        assert_eq!(disposition_name(h).unwrap(), "Bee badge ✿.svg");
        assert_eq!(
            disposition_name("attachment; filename=\"a.pes\"").unwrap(),
            "a.pes"
        );
        assert_eq!(disposition_name("attachment").as_deref(), None);
        // A starred name that isn't one leaves the plain name.
        assert_eq!(
            disposition_name("attachment; filename*=nonsense; filename=\"b.svg\"").unwrap(),
            "b.svg"
        );
    }

    #[test]
    fn file_names_are_only_names() {
        assert_eq!(safe_stem("../../.ssh/authorized_keys"), "authorized_keys");
        assert_eq!(safe_stem(r"..\..\Startup\evil.svg"), "evil.svg");
        assert_eq!(safe_stem(".hidden.svg"), "hidden.svg");
        assert_eq!(safe_stem("a\u{0}b<c>.svg"), "a_b_c_.svg");
        assert_eq!(safe_stem("CON.svg"), "_CON.svg");
        assert_eq!(safe_stem("..."), "");
        assert_eq!(safe_stem(&"x".repeat(500)).len(), 80);
    }

    #[test]
    fn files_must_look_like_what_they_are() {
        assert_eq!(
            kind_of(b"<?xml version='1.0'?><svg/>", "svg").unwrap(),
            "svg"
        );
        assert_eq!(kind_of(b"\xef\xbb\xbf <svg></svg>", "").unwrap(), "svg");
        assert_eq!(kind_of(b"#PES0001....", "svg").unwrap(), "pes");
        assert_eq!(kind_of(b"LA:design", "dst").unwrap(), "dst");
        assert!(kind_of(b"MZ\x90\x00", "exe").is_err());
        assert!(kind_of(b"MZ\x90\x00", "svg").is_err());
        assert!(kind_of(b"not a pes", "pes").is_err());
        assert!(kind_of(b"<html><script>", "svg").is_err());
    }

    #[test]
    fn saving_never_writes_over_a_file() {
        let dir = std::env::temp_dir().join(format!("zigzag-link-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = save_new(&dir, "Bee", "svg", b"one").unwrap();
        let b = save_new(&dir, "Bee", "svg", b"two").unwrap();
        assert_eq!(a.file_name().unwrap(), "Bee.svg");
        assert_eq!(b.file_name().unwrap(), "Bee 2.svg");
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn desktop_entry_quotes_the_path() {
        let e = desktop_entry(Path::new("/opt/Zig Zag $1/zigzag-studio"));
        assert!(
            e.contains("Exec=\"/opt/Zig Zag \\\\$1/zigzag-studio\" %U\n"),
            "{e}"
        );
        assert!(e.contains("MimeType=x-scheme-handler/zigzag;"));
    }

    /// A server on this computer answering every request with `body`.
    fn serve(body: Vec<u8>, disposition: Option<&'static str>) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for r in server.incoming_requests() {
                let mut res = tiny_http::Response::from_data(body.clone());
                if let Some(d) = disposition {
                    res.add_header(
                        tiny_http::Header::from_bytes("Content-Disposition", d).unwrap(),
                    );
                }
                let _ = r.respond(res);
            }
        });
        format!("http://127.0.0.1:{port}/v1/share/abc/design")
    }

    fn temp(what: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zigzag-link-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn downloads_into_the_folder_under_the_servers_name() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="5" height="5"/></svg>"#;
        let url = serve(
            svg.to_vec(),
            Some("attachment; filename*=UTF-8''..%2F..%2FBee%20badge.svg"),
        );
        let dir = temp("download");
        let l = parse(&link(&url)).unwrap();
        let path = download(&l, &dir).unwrap();
        assert_eq!(path, dir.join("Bee badge.svg"));
        assert_eq!(std::fs::read(&path).unwrap(), svg);
        // Again: a new file beside it.
        assert_eq!(download(&l, &dir).unwrap(), dir.join("Bee badge 2.svg"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_name_from_the_server_takes_the_designs() {
        let url = serve(b"#PES0001 stitches".to_vec(), None);
        let dir = temp("unnamed");
        let path = download(&parse(&link(&url)).unwrap(), &dir).unwrap();
        assert_eq!(path, dir.join("Bee badge.pes"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_files_too_big_or_not_designs() {
        let dir = temp("refused");
        let big = serve(vec![b'<'; MAX_BYTES as usize + 10], None);
        assert!(
            download(&parse(&link(&big)).unwrap(), &dir)
                .unwrap_err()
                .contains("too big")
        );
        // (And when it doesn't say how big beforehand.)
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for r in server.incoming_requests() {
                let body = std::io::repeat(b'<').take(MAX_BYTES + 1000);
                let res = tiny_http::Response::new(200.into(), vec![], body, None, None);
                let _ = r.respond(res);
            }
        });
        let chunked = format!("http://localhost:{port}/x");
        assert!(
            download(&parse(&link(&chunked)).unwrap(), &dir)
                .unwrap_err()
                .contains("too big")
        );
        let exe = serve(
            b"MZ\x90\x00 program".to_vec(),
            Some("attachment; filename=\"a.svg\""),
        );
        assert!(download(&parse(&link(&exe)).unwrap(), &dir).is_err());
        assert!(!dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none());
    }
}
