//! One ZigZag at a time. A file opened with ZigZag while it's already
//! running (Open With, a drop on the app's icon, the command line) opens
//! as a tab in the window that's there, which comes to the front, and the
//! new launch ends. Two windows would bring back the same session and then
//! write over each other's `session.json` and recovery files (see
//! `session.rs`), so whichever wrote last would win and the other's unsaved
//! changes could be gone at the next launch.
//!
//! The first launch holds a lock on `instance.lock` in the settings folder
//! for as long as it runs (the system lets go of it when the process ends,
//! crashed or not), listens on a local port, and writes the port with a
//! token to `instance.json`. A later launch that can't take the lock sends
//! its files there and quits once they're taken. A window that's on its
//! way out says so, and the later launch waits for the lock instead (then
//! it's the one window). If no one answers for a while, it opens a window
//! of its own, as it did before there was this.
//!
//! The address is only on this computer (127.0.0.1), and the token is in
//! the user's own settings folder (on Unix in a file only they can read),
//! so another user's program can't ask this ZigZag to open files.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Held by the ZigZag that's running.
const LOCK: &str = "instance.lock";
/// Where it listens, and the token it wants: `{"port", "token", "pid"}`.
const ADDRESS: &str = "instance.json";
/// How long a later launch keeps trying before it opens a window anyway.
const PATIENCE: Duration = Duration::from_secs(10);
/// A message larger than this isn't a list of files.
const LONGEST_MESSAGE: u64 = 1 << 20;

/// What a later launch asks for: its files (absolute paths), and the
/// object to select (`--select`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Request {
    pub files: Vec<PathBuf>,
    pub select: Option<usize>,
    /// `zigzag://` links to open (see `link`).
    pub links: Vec<String>,
}

pub enum Start {
    /// This launch is the one: open the window, and take what later
    /// launches send.
    First(Inbox),
    /// The ZigZag that's running took the files: nothing left to do.
    HandedOver,
    /// Neither (no settings folder, no lock, or the one running never
    /// answered): open a window regardless.
    Alone,
}

/// Where the requests of later launches arrive.
pub struct Inbox {
    /// Held for as long as this ZigZag runs.
    _lock: std::fs::File,
    rx: mpsc::Receiver<Request>,
    wake: Arc<Mutex<Option<Box<dyn Fn() + Send>>>>,
    closing: Arc<AtomicBool>,
}

impl Inbox {
    /// Called when a request arrives (the window repaints, and takes it).
    pub fn on_arrival(&self, wake: impl Fn() + Send + 'static) {
        if let Ok(mut w) = self.wake.lock() {
            *w = Some(Box::new(wake));
        }
    }

    /// The requests that arrived since last time.
    pub fn take(&self) -> Vec<Request> {
        self.rx.try_iter().collect()
    }

    /// The window is going: a later launch waits for it to be gone and
    /// opens a window itself, rather than handing its files to this one.
    pub fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        self.close();
    }
}

/// At launch, in ZigZag's settings folder.
pub fn claim_in_config(request: &Request) -> Start {
    match crate::dock::config_file(LOCK).and_then(|f| Some(f.parent()?.to_path_buf())) {
        Some(dir) => claim(&dir, request),
        None => Start::Alone,
    }
}

/// Be the one ZigZag kept in `dir`, or hand `request` to the one that is.
pub fn claim(dir: &Path, request: &Request) -> Start {
    let _ = std::fs::create_dir_all(dir);
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK))
    else {
        return Start::Alone;
    };
    let deadline = Instant::now() + PATIENCE;
    loop {
        match lock.try_lock() {
            Ok(()) => {
                return match listen(dir, lock) {
                    Ok(inbox) => Start::First(inbox),
                    Err(_) => Start::Alone,
                };
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            // (A folder where files can't be locked.)
            Err(std::fs::TryLockError::Error(_)) => return Start::Alone,
        }
        // The one running may have only just taken the lock and not yet
        // said where it listens, or be on its way out: try again shortly.
        if hand_over(dir, request).is_ok() {
            return Start::HandedOver;
        }
        if Instant::now() >= deadline {
            return Start::Alone;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Listen for later launches, and say where.
fn listen(dir: &Path, lock: std::fs::File) -> std::io::Result<Inbox> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let token = new_token();
    let address = serde_json::json!({
        "port": port,
        "token": token,
        "pid": std::process::id(),
    });
    // Only this user may read the token (`~/.config` is often readable
    // by everyone on Linux).
    crate::session::write_private(&dir.join(ADDRESS), &address.to_string())?;
    let (tx, rx) = mpsc::channel();
    let wake: Arc<Mutex<Option<Box<dyn Fn() + Send>>>> = Arc::default();
    let closing = Arc::new(AtomicBool::new(false));
    let (w, c) = (wake.clone(), closing.clone());
    std::thread::Builder::new()
        .name("zigzag-instance".into())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let _ = answer(conn, &token, &tx, &w, &c);
            }
        })?;
    Ok(Inbox {
        _lock: lock,
        rx,
        wake,
        closing,
    })
}

/// One later launch: its request goes to the window, and it hears `ok`
/// (or `closing`, if the window is going).
fn answer(
    conn: TcpStream,
    token: &str,
    tx: &mpsc::Sender<Request>,
    wake: &Mutex<Option<Box<dyn Fn() + Send>>>,
    closing: &AtomicBool,
) -> std::io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut line = String::new();
    BufReader::new((&conn).take(LONGEST_MESSAGE)).read_line(&mut line)?;
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
        return Ok(());
    };
    if msg["token"].as_str() != Some(token) {
        return Ok(());
    }
    if closing.load(Ordering::SeqCst) {
        return (&conn).write_all(b"closing\n");
    }
    let request = Request {
        files: msg["files"]
            .as_array()
            .map(|a| a.iter().filter_map(|f| f.as_str()).map(PathBuf::from))
            .into_iter()
            .flatten()
            .collect(),
        select: msg["select"].as_u64().map(|n| n as usize),
        links: msg["links"]
            .as_array()
            .map(|a| a.iter().filter_map(|l| l.as_str()).map(String::from))
            .into_iter()
            .flatten()
            .collect(),
    };
    if tx.send(request).is_err() {
        return (&conn).write_all(b"closing\n");
    }
    if let Ok(w) = wake.lock()
        && let Some(w) = w.as_ref()
    {
        w();
    }
    (&conn).write_all(b"ok\n")
}

/// Send `request` to the ZigZag listening in `dir`. Ok once it's taken.
fn hand_over(dir: &Path, request: &Request) -> std::io::Result<()> {
    let refused = || std::io::Error::other("not taken");
    let address: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(ADDRESS))?)?;
    let port = address["port"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
        .ok_or_else(refused)?;
    let token = address["token"].as_str().ok_or_else(refused)?;
    let to = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut conn = TcpStream::connect_timeout(&to, Duration::from_millis(500))?;
    conn.set_read_timeout(Some(Duration::from_secs(2)))?;
    // This launch was started by the user, so it may bring a window to the
    // front; the running one may not by itself (Windows), unless let.
    #[cfg(windows)]
    if let Some(pid) = address["pid"].as_u64().and_then(|p| u32::try_from(p).ok()) {
        let_come_forward(pid);
    }
    let files: Vec<String> = request
        .files
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    let msg = serde_json::json!({
        "token": token,
        "files": files,
        "select": request.select,
        "links": request.links,
    });
    writeln!(conn, "{msg}")?;
    let mut reply = String::new();
    BufReader::new(conn).read_line(&mut reply)?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        Err(refused())
    }
}

/// Let process `pid` bring its window to the front.
#[cfg(windows)]
fn let_come_forward(pid: u32) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn AllowSetForegroundWindow(pid: u32) -> i32;
    }
    // SAFETY: takes a process id and touches no memory of ours.
    unsafe {
        AllowSetForegroundWindow(pid);
    }
}

/// A token no one else can guess: the standard library's hasher keys come
/// from the system's randomness.
fn new_token() -> String {
    use std::hash::BuildHasher;
    let a = std::collections::hash_map::RandomState::new()
        .hash_one((std::process::id(), std::time::SystemTime::now()));
    let b = std::collections::hash_map::RandomState::new().hash_one(a);
    format!("{a:016x}{b:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tl-instance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn first(dir: &Path) -> Inbox {
        match claim(dir, &Request::default()) {
            Start::First(i) => i,
            Start::HandedOver => panic!("handed over, with no one running"),
            Start::Alone => panic!("alone"),
        }
    }

    /// Waits for what arrives (it's taken on another thread).
    fn arrived(inbox: &Inbox) -> Vec<Request> {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let got = inbox.take();
            if !got.is_empty() || Instant::now() > until {
                return got;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_second_launch_hands_its_files_to_the_first() {
        let dir = temp_dir("handover");
        let inbox = first(&dir);
        let woken = Arc::new(AtomicBool::new(false));
        let w = woken.clone();
        inbox.on_arrival(move || w.store(true, Ordering::SeqCst));

        let request = Request {
            files: vec![dir.join("a.svg"), dir.join("b ü.svg")],
            select: Some(2),
            links: vec!["zigzag://open?url=https%3A%2F%2Fzigzag.diy%2Fa.svg".into()],
        };
        assert!(matches!(claim(&dir, &request), Start::HandedOver));
        assert_eq!(arrived(&inbox), vec![request]);
        assert!(woken.load(Ordering::SeqCst));

        // And again, with nothing to open (just bring the window forward).
        assert!(matches!(
            claim(&dir, &Request::default()),
            Start::HandedOver
        ));
        assert_eq!(arrived(&inbox), vec![Request::default()]);
    }

    #[test]
    fn launches_at_once_end_in_one_window_with_all_their_files() {
        // (Open With on several files starts a launch for each, together.)
        let dir = temp_dir("together");
        let launches: Vec<_> = (0..4)
            .map(|k| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let r = Request {
                        files: vec![dir.join(format!("{k}.svg"))],
                        ..Default::default()
                    };
                    (r.clone(), claim(&dir, &r))
                })
            })
            .collect();
        let mut firsts = Vec::new();
        let mut handed = Vec::new();
        for l in launches {
            match l.join().unwrap() {
                (r, Start::First(i)) => firsts.push((r, i)),
                (r, Start::HandedOver) => handed.push(r),
                (_, Start::Alone) => panic!("a launch opened a window of its own"),
            }
        }
        assert_eq!(firsts.len(), 1, "one window");
        let (_, inbox) = &firsts[0];
        let mut got = Vec::new();
        while got.len() < handed.len() {
            let more = arrived(inbox);
            assert!(!more.is_empty(), "a handed-over file never arrived");
            got.extend(more);
        }
        let key = |r: &Request| r.files.clone();
        got.sort_by_key(key);
        handed.sort_by_key(key);
        assert_eq!(got, handed);
    }

    #[test]
    fn two_launches_in_a_row_give_one_window_with_both_files_as_tabs() {
        let dir = temp_dir("tabs");
        let svg = |fill: &str| {
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 40 40"><rect x="5" y="5" width="30" height="30" fill="{fill}"/></svg>"#
            )
        };
        let (a, b) = (dir.join("first.svg"), dir.join("second.svg"));
        std::fs::write(&a, svg("#c0392b")).unwrap();
        std::fs::write(&b, svg("#2c3e50")).unwrap();

        // The first launch opens its window, with its file.
        let mut window = crate::app::Studio::with_file(None, None);
        window.start_with(std::slice::from_ref(&a), None, false);
        window.inbox = Some(first(&dir));

        // The second hands its file over, and opens no window.
        let second = Request {
            files: vec![b.clone()],
            ..Default::default()
        };
        assert!(matches!(claim(&dir, &second), Start::HandedOver));

        // The window opens it in a tab of its own, in front.
        let ctx = eframe::egui::Context::default();
        let until = Instant::now() + Duration::from_secs(5);
        while window.tab_count() < 2 && Instant::now() < until {
            window.take_handed_over(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        let names: Vec<String> = window.tab_infos().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["first", "second"]);
        assert_eq!(window.doc.name, "second");

        // The first file again: its tab comes forward, no third one.
        let again = Request {
            files: vec![a.clone()],
            ..Default::default()
        };
        assert!(matches!(claim(&dir, &again), Start::HandedOver));
        let until = Instant::now() + Duration::from_secs(5);
        while window.doc.name != "first" && Instant::now() < until {
            window.take_handed_over(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(window.tab_count(), 2);
        assert_eq!(window.doc.name, "first");
    }

    #[test]
    fn a_window_on_its_way_out_leaves_the_next_launch_to_open_its_own() {
        let dir = temp_dir("closing");
        let inbox = first(&dir);
        inbox.close();
        let request = Request {
            files: vec![dir.join("late.svg")],
            ..Default::default()
        };
        let later = {
            let (dir, request) = (dir.clone(), request.clone());
            std::thread::spawn(move || claim(&dir, &request))
        };
        // The window is gone a little later; the late launch then is the one.
        std::thread::sleep(Duration::from_millis(300));
        assert!(inbox.take().is_empty());
        drop(inbox);
        assert!(matches!(later.join().unwrap(), Start::First(_)));
    }

    #[test]
    fn a_wrong_token_is_ignored() {
        let dir = temp_dir("token");
        let inbox = first(&dir);
        let address: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(ADDRESS)).unwrap()).unwrap();
        let port = address["port"].as_u64().unwrap() as u16;
        let mut conn = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        writeln!(conn, r#"{{"token":"guess","files":["x.svg"]}}"#).unwrap();
        let mut reply = String::new();
        let _ = BufReader::new(conn).read_line(&mut reply);
        assert_eq!(reply, "");
        std::thread::sleep(Duration::from_millis(100));
        assert!(inbox.take().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn only_this_user_can_read_the_token() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("private");
        let _inbox = first(&dir);
        let mode = std::fs::metadata(dir.join(ADDRESS))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
