//! The connection to the Skitch, kept by a worker thread so the UI never
//! waits on Bluetooth. (In the browser, which has no threads, the same
//! worker is an async task on the page's own; see
//! `zigzag::machine::session`.) The worker connects, runs what the Machine
//! panel asks (send, trace, sew, move the needle), and between times asks
//! the machine how it's doing: every half second while it sews, less often
//! while it waits. The panel reads the latest [`Snapshot`].

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use web_time::Instant;
use zigzag::design::Design;
use zigzag::formats::pen::PenData;
use zigzag::machine::protocol::{self, MachineInfo, PatternInfo, Progress, Status};
use zigzag::machine::session::{NotFound, NotPaired, Session, Transport};
use zigzag::machine::sim::Sim;

/// Where the connection is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Link {
    #[default]
    Off,
    Searching,
    /// Nothing answered the scan.
    NotFound,
    /// It connected but said nothing: not paired with this computer.
    NotPaired,
    Failed(String),
    Connected,
    /// Was connected, isn't any more.
    Lost(String),
}

/// The machine as last heard from.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub link: Link,
    /// Talking to the simulated machine.
    pub sim: bool,
    pub info: Option<MachineInfo>,
    /// Stitches since the last service, and ever.
    pub counts: Option<(u32, u32)>,
    pub status: Option<Status>,
    pub error: u8,
    pub progress: Option<Progress>,
    pub pattern: Option<PatternInfo>,
    /// The id of the design on the machine.
    pub uuid: Option<[u8; 16]>,
    /// Sending a design: how far.
    pub upload: Option<f32>,
    /// A command in flight ("Sending", "Starting"…).
    pub doing: Option<&'static str>,
    /// The last command that didn't work.
    pub problem: Option<String>,
    /// After a thread error the needle went back this many stitches.
    pub rolled_back: Option<(u8, u16)>,
    /// The machine's settings bytes, each time they were read, with what
    /// was changed before (working out tension and speed).
    pub settings: Vec<(String, Vec<u8>)>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            link: Link::Off,
            sim: false,
            info: None,
            counts: None,
            status: None,
            error: protocol::NO_ERROR,
            progress: None,
            pattern: None,
            uuid: None,
            upload: None,
            doing: None,
            problem: None,
            rolled_back: None,
            settings: Vec::new(),
        }
    }
}

pub enum Cmd {
    /// Connect: to the simulated machine when given one, else over Bluetooth.
    Connect(Option<Sim>),
    Disconnect,
    /// Send a design, placed by the layout, under an id; then do the trial
    /// run if asked.
    Upload(Arc<PenData>, protocol::Layout, [u8; 16], bool),
    Trace,
    /// Take the design off the machine (ending the job).
    Delete,
    Sew,
    SetStitch(u16),
    /// Read the machine's settings bytes, noting what was changed before.
    ReadSettings(String),
}

/// What was last sent: kept to show the machine's progress on the design
/// as it was sent (even if it's edited since).
#[derive(Clone)]
pub struct Sent {
    pub pen: Arc<PenData>,
    pub design: Arc<Design>,
    pub uuid: [u8; 16],
    /// Sent to sew in the middle of the hoop (else where it sat in it).
    pub centered: bool,
    /// The design's bounds (units), worked out once rather than per frame.
    pub bounds: zigzag::geom::Bounds,
}

pub struct Machine {
    tx: Option<Sender<Cmd>>,
    pub shared: Arc<Mutex<Snapshot>>,
    pub sent: Option<Sent>,
    /// The simulated machine, when that's what's connected.
    pub sim: Option<Sim>,
    /// The person's "before you sew" checklist, ticked off for this job.
    pub checked: [bool; 5],
    /// Draw the machine's progress over the canvas.
    pub follow: bool,
    /// The Skitch window is showing.
    pub open: bool,
    /// Do the trial run as soon as the design is on the machine.
    pub trace_first: bool,
    /// Sew it in the middle of the hoop, rather than where it sits in the
    /// studio's hoop.
    pub centered: bool,
    /// A stitch picked on the window's timeline, not yet sent (dragging).
    pub picking: Option<usize>,
    /// Bring the window back when the machine needs you (a thread
    /// change, an error, done).
    pub auto_show: bool,
    /// The status last frame (to notice it changing).
    pub last_status: Option<protocol::Status>,
    /// What was changed before the next settings read ("tension +1").
    pub settings_label: String,
}

impl Default for Machine {
    fn default() -> Self {
        Machine {
            tx: None,
            shared: Default::default(),
            sent: None,
            sim: None,
            checked: [false; 5],
            follow: true,
            open: false,
            trace_first: true,
            centered: false,
            picking: None,
            auto_show: true,
            last_status: None,
            settings_label: String::new(),
        }
    }
}

impl Machine {
    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().unwrap().clone()
    }

    /// Hand the worker a command (starting it the first time).
    pub fn ask(&mut self, cmd: Cmd, ctx: &eframe::egui::Context) {
        if self.tx.is_none() {
            let (tx, rx) = mpsc::channel();
            let shared = self.shared.clone();
            let ctx = ctx.clone();
            #[cfg(target_arch = "wasm32")]
            wasm_bindgen_futures::spawn_local(worker(rx, shared, ctx));
            #[cfg(not(target_arch = "wasm32"))]
            {
                let started = std::thread::Builder::new()
                    .name("skitch".into())
                    .spawn(move || worker(rx, shared, ctx));
                // No thread to keep the connection on: say so, rather than
                // search for ever.
                if started.is_err() {
                    self.shared.lock().unwrap().link =
                        Link::Failed("Couldn't start talking to the machine.".into());
                    return;
                }
            }
            self.tx = Some(tx);
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    /// Connect (to the simulated machine when `sim`, else over Bluetooth).
    pub fn connect(&mut self, sim: bool, ctx: &eframe::egui::Context) {
        let sim = sim.then(|| {
            self.sim
                .get_or_insert_with(|| Sim::new(Default::default()))
                .clone()
        });
        if sim.is_none() {
            self.sim = None;
        }
        self.shared.lock().unwrap().link = Link::Searching;
        self.ask(Cmd::Connect(sim), ctx);
    }

    /// Send `design` (already encoded as `pen`) to the machine.
    pub fn send(&mut self, design: &Design, pen: PenData, ctx: &eframe::egui::Context) {
        let pen = Arc::new(pen);
        let uuid = new_uuid();
        self.sent = Some(Sent {
            pen: pen.clone(),
            design: Arc::new(design.clone()),
            uuid,
            centered: self.centered,
            bounds: design.bounds(),
        });
        self.checked = [false; 5];
        self.follow = true;
        {
            let mut s = self.shared.lock().unwrap();
            s.upload = Some(0.0);
            s.problem = None;
        }
        let trace = self.trace_first;
        let layout = if self.centered {
            protocol::Layout::centered(pen.bounds)
        } else {
            protocol::Layout::as_placed(pen.bounds)
        };
        self.ask(Cmd::Upload(pen, layout, uuid, trace), ctx);
    }

    /// What was sent is what's on the machine.
    pub fn ours<'a>(&'a self, s: &Snapshot) -> Option<&'a Sent> {
        self.sent.as_ref().filter(|x| s.uuid == Some(x.uuid))
    }
}

/// A random id for a sent design (no crate needed: the clock, hashed).
fn new_uuid() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; 16];
    for half in out.chunks_mut(8) {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        half.copy_from_slice(&h.finish().to_le_bytes());
    }
    out
}

/// Where settings reads are kept: one line each, the label then the bytes.
pub fn settings_log_path() -> Option<std::path::PathBuf> {
    crate::dock::config_file("skitch-settings.log")
}

fn settings_log(label: &str, bytes: &[u8]) {
    use std::io::Write;
    let Some(path) = settings_log_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = zigzag::vfs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            f,
            "{}\t{}",
            label.replace('\t', " "),
            zigzag::machine::capture::hex(bytes, &[])
        );
    }
}

/// How long to wait between questions, by what the machine is doing.
fn interval(status: Option<Status>) -> Duration {
    match status {
        Some(Status::Sewing | Status::MaskTracing | Status::ReceivingData) => {
            Duration::from_millis(500)
        }
        Some(
            Status::WaitingForColorChange
            | Status::WaitingForMaskTrace
            | Status::ReadyToSew
            | Status::Initial,
        ) => Duration::from_millis(1000),
        _ => Duration::from_millis(2000),
    }
}

/// Statuses in which the machine has a place in the design worth showing.
pub fn has_progress(st: Status) -> bool {
    matches!(
        st,
        Status::Sewing
            | Status::WaitingForColorChange
            | Status::Paused
            | Status::Stopped
            | Status::Interrupted
            | Status::ReadyToSew
            | Status::Complete
    )
}

#[maybe_async::maybe_async(?Send)]
async fn open(sim: Option<Sim>) -> anyhow::Result<Session> {
    let link: Box<dyn Transport> = match sim {
        Some(s) => Box::new(s),
        None => ble_link().await?,
    };
    Session::open(link).await
}

/// In the browser: its own Bluetooth (the page asks which device).
#[cfg(target_arch = "wasm32")]
async fn ble_link() -> anyhow::Result<Box<dyn Transport>> {
    crate::platform::ble_link().await
}

/// The next command, or none within `wait`. (In the browser nothing can
/// block: it looks again every so often.)
#[cfg(not(target_arch = "wasm32"))]
fn next_cmd(rx: &Receiver<Cmd>, wait: Duration) -> Result<Cmd, RecvTimeoutError> {
    rx.recv_timeout(wait)
}
#[cfg(target_arch = "wasm32")]
async fn next_cmd(rx: &Receiver<Cmd>, wait: Duration) -> Result<Cmd, RecvTimeoutError> {
    const LOOK: Duration = Duration::from_millis(30);
    let until = Instant::now() + wait;
    loop {
        match rx.try_recv() {
            Ok(c) => return Ok(c),
            Err(mpsc::TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if Instant::now() >= until {
            return Err(RecvTimeoutError::Timeout);
        }
        zigzag::machine::session::pause(LOOK).await;
    }
}

#[cfg(all(feature = "ble", not(target_arch = "wasm32")))]
fn ble_link() -> anyhow::Result<Box<dyn Transport>> {
    Ok(Box::new(zigzag::machine::skitch::BleLink::connect(
        Duration::from_secs(12),
    )?))
}

#[cfg(all(not(feature = "ble"), not(target_arch = "wasm32")))]
fn ble_link() -> anyhow::Result<Box<dyn Transport>> {
    anyhow::bail!("This build has no Bluetooth. Rebuild with --features zigzag-studio/ble.")
}

#[maybe_async::maybe_async(?Send)]
async fn worker(rx: Receiver<Cmd>, shared: Arc<Mutex<Snapshot>>, ctx: eframe::egui::Context) {
    let mut session: Option<Session> = None;
    let mut last_counts = Instant::now();
    // The thread error the needle was already moved back for.
    let mut rolled_for: Option<u8> = None;
    let update = |f: &mut dyn FnMut(&mut Snapshot)| {
        f(&mut shared.lock().unwrap());
        ctx.request_repaint();
    };
    loop {
        let wait = interval(shared.lock().unwrap().status);
        let cmd = match next_cmd(&rx, wait).await {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match cmd {
            Some(Cmd::Connect(sim)) => {
                if let Some(mut s) = session.take() {
                    s.close();
                }
                let is_sim = sim.is_some();
                update(&mut |s| {
                    *s = Snapshot {
                        link: Link::Searching,
                        sim: is_sim,
                        ..Snapshot::default()
                    }
                });
                match open(sim).await {
                    Ok(mut s) => {
                        let info = s.info().await.ok();
                        let counts = s.service_count().await.ok();
                        update(&mut |snap| {
                            snap.link = Link::Connected;
                            snap.info = info.clone();
                            snap.counts = counts;
                        });
                        last_counts = Instant::now();
                        session = Some(s);
                    }
                    Err(e) => {
                        let link = if e.downcast_ref::<NotPaired>().is_some() {
                            Link::NotPaired
                        } else if e.downcast_ref::<NotFound>().is_some() {
                            Link::NotFound
                        } else {
                            Link::Failed(format!("{e:#}"))
                        };
                        update(&mut |s| s.link = link.clone());
                    }
                }
            }
            Some(Cmd::Disconnect) => {
                if let Some(mut s) = session.take() {
                    s.close();
                }
                update(&mut |s| *s = Snapshot::default());
                continue;
            }
            Some(cmd) => {
                let Some(s) = session.as_mut() else {
                    update(&mut |snap| {
                        snap.problem = Some("Not connected to the machine.".into());
                        snap.upload = None;
                    });
                    continue;
                };
                let (doing, r) = match cmd {
                    Cmd::Upload(pen, layout, uuid, trace) => {
                        update(&mut |snap| {
                            snap.doing = Some("Sending");
                            snap.upload = Some(0.0);
                            snap.problem = None;
                        });
                        let mut last = 0.0;
                        let r = s.upload(&pen, layout, uuid, |f| {
                            if f - last >= 0.02 || f >= 1.0 {
                                last = f;
                                update(&mut |snap| snap.upload = Some(f));
                            }
                        })
                        .await;
                        if r.is_ok() {
                            let info = s.pattern_info().await.ok();
                            update(&mut |snap| {
                                snap.uuid = Some(uuid);
                                snap.pattern = info;
                                snap.progress = None;
                                snap.rolled_back = None;
                                snap.upload = None;
                            });
                        }
                        match r {
                            Ok(()) if trace => ("Starting the trial run", s.mask_trace().await),
                            r => ("Sending", r),
                        }
                    }
                    Cmd::Trace => ("Starting the trial run", s.mask_trace().await),
                    Cmd::Sew => {
                        rolled_for = None;
                        ("Starting", s.start_sewing().await)
                    }
                    Cmd::SetStitch(k) => ("Moving the needle", s.set_stitch(k).await),
                    Cmd::ReadSettings(label) => {
                        let r = s.settings().await;
                        if let Ok(bytes) = &r {
                            settings_log(&label, bytes);
                            let entry = (label, bytes.clone());
                            update(&mut |snap| snap.settings.push(entry.clone()));
                        }
                        ("Reading the settings", r.map(|_| ()))
                    }
                    Cmd::Delete => {
                        let r = s.delete_pattern().await;
                        if r.is_ok() {
                            update(&mut |snap| {
                                snap.uuid = None;
                                snap.pattern = None;
                                snap.progress = None;
                                snap.rolled_back = None;
                            });
                        }
                        ("Ending the job", r)
                    }
                    Cmd::Connect(_) | Cmd::Disconnect => unreachable!(),
                };
                update(&mut |snap| {
                    snap.doing = None;
                    snap.upload = None;
                    snap.problem = r
                        .as_ref()
                        .err()
                        .map(|e| format!("{doing} didn't work: {e:#}"));
                });
            }
            None => {}
        }
        // Then (and on every quiet interval) see how it's doing.
        let Some(s) = session.as_mut() else { continue };
        match s.state().await {
            Ok((st, err)) => {
                let progress = if has_progress(st) {
                    s.progress().await.ok()
                } else {
                    None
                };
                let have = shared.lock().unwrap().clone();
                let uuid = if have.uuid.is_none() || st == Status::Idle {
                    s.pattern_uuid().await.ok().flatten()
                } else {
                    have.uuid
                };
                let pattern =
                    if uuid.is_some() && have.pattern.is_none_or(|p| p.total_stitches == 0) {
                        s.pattern_info().await.ok()
                    } else {
                        have.pattern
                    };
                let counts = if last_counts.elapsed() > Duration::from_secs(10) {
                    last_counts = Instant::now();
                    s.service_count().await.ok().or(have.counts)
                } else {
                    have.counts
                };
                // A thread broke: go back over the gap it left (Artspira's
                // rule), once per break.
                let mut rolled = have.rolled_back;
                if matches!(st, Status::Interrupted | Status::Paused)
                    && rolled_for != Some(err)
                    && let Some(n) = protocol::error_help(err).and_then(|h| h.rollback)
                    && let Some(p) = progress
                {
                    rolled_for = Some(err);
                    let to = p.current_stitch.saturating_sub(n);
                    if s.set_stitch(to).await.is_ok() {
                        rolled = Some((err, n));
                    }
                }
                if st == Status::Sewing {
                    rolled_for = None;
                    rolled = None;
                }
                let progress = if rolled != have.rolled_back {
                    s.progress().await.ok().or(progress)
                } else {
                    progress
                };
                update(&mut |snap| {
                    snap.status = Some(st);
                    snap.error = err;
                    snap.progress = progress.or(if has_progress(st) {
                        snap.progress
                    } else {
                        None
                    });
                    snap.uuid = uuid;
                    snap.pattern = pattern;
                    snap.counts = counts;
                    snap.rolled_back = rolled;
                });
            }
            Err(e) => {
                // (Asked twice: one failed answer isn't a lost machine.)
                let lost = !s.connected() || s.state().await.is_err();
                if lost {
                    if let Some(mut s) = session.take() {
                        s.close();
                    }
                    let msg = format!("{e:#}");
                    update(&mut |snap| {
                        snap.link = Link::Lost(msg.clone());
                        snap.doing = None;
                        snap.upload = None;
                    });
                }
            }
        }
    }
}
