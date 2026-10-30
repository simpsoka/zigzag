//! Capture mode, for the README's screenshots and videos: off unless the
//! studio is started with `ZIGZAG_CAPTURE=<folder>`. Then
//!
//! - ⌃⌥⌘S saves the window, at full (Retina) resolution, as a PNG;
//! - ⌃⌥⌘R starts recording (the window, at half resolution, up to 30 frames a
//!   second) and stops it: a folder of numbered frames and `frames.txt`, the
//!   time each one stayed on screen, which ffmpeg turns into a video
//!   (`ffmpeg -f concat -i frames.txt …`).
//!
//! Nothing is drawn on screen for it (it would be in the pictures); what it
//! did is printed on stderr. PNGs are encoded on worker threads; if they fall
//! behind, a frame is skipped and the one before stays on screen longer.

use eframe::egui::{self, Key, Modifiers};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// At most this many frames a second while recording.
const FPS: f64 = 30.0;
/// Frames waiting to be encoded before new ones are skipped.
const QUEUE: usize = 24;
const WORKERS: usize = 4;

struct Job {
    path: PathBuf,
    image: Arc<egui::ColorImage>,
    /// Halve its size (video frames; a still keeps full resolution).
    half: bool,
}

/// The frames a recording asked for: (file name, when it was taken).
type FrameLog = Arc<Mutex<Vec<(String, Duration)>>>;

struct Recording {
    dir: PathBuf,
    started: Instant,
    last: Option<Instant>,
    frames: FrameLog,
    next: usize,
}

pub struct Capture {
    dir: PathBuf,
    jobs: SyncSender<Job>,
    pending: Arc<AtomicUsize>,
    recording: Option<Recording>,
    /// A recording stopped (when, and the latest to wait for its frames):
    /// its `frames.txt` is written once they're all on disk.
    stopping: Option<(Recording, Duration, Instant)>,
    stills: usize,
    takes: usize,
}

impl Capture {
    /// Capture mode, when `ZIGZAG_CAPTURE` names a folder.
    pub fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("ZIGZAG_CAPTURE")?);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("capture: can't use {}: {e}", dir.display());
            return None;
        }
        let (jobs, rx) = mpsc::sync_channel::<Job>(QUEUE);
        let rx = Arc::new(Mutex::new(rx));
        let pending = Arc::new(AtomicUsize::new(0));
        for k in 0..WORKERS {
            let rx = rx.clone();
            let pending = pending.clone();
            let _ = std::thread::Builder::new()
                .name(format!("capture-{k}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock().map(|r| r.recv()) {
                            Ok(Ok(j)) => j,
                            _ => return,
                        };
                        if let Err(e) = write_png(&job) {
                            eprintln!("capture: {}: {e}", job.path.display());
                        }
                        pending.fetch_sub(1, Ordering::SeqCst);
                    }
                });
        }
        eprintln!(
            "capture: on, into {} (⌃⌥⌘S: screenshot, ⌃⌥⌘R: start/stop recording)",
            dir.display()
        );
        // Continue the numbering of what's there already.
        let count = |prefix: &str| {
            std::fs::read_dir(&dir)
                .map(|d| {
                    d.filter_map(Result::ok)
                        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
                        .count()
                })
                .unwrap_or(0)
        };
        Some(Capture {
            stills: count("still-"),
            takes: count("take-"),
            dir,
            jobs,
            pending,
            recording: None,
            stopping: None,
        })
    }

    /// Call once a frame: the shortcuts, and the next frame of a recording.
    pub fn frame(&mut self, ctx: &egui::Context) {
        let chord = Modifiers::COMMAND | Modifiers::CTRL | Modifiers::ALT;
        let (still, record) =
            ctx.input_mut(|i| (i.consume_key(chord, Key::S), i.consume_key(chord, Key::R)));
        if still {
            self.stills += 1;
            let path = self.dir.join(format!("still-{:03}.png", self.stills));
            eprintln!("capture: {}", path.display());
            self.ask(ctx, path, false, None);
        }
        if record {
            match self.recording.take() {
                Some(r) => {
                    eprintln!("capture: stopping…");
                    let end = r.started.elapsed();
                    self.stopping = Some((r, end, Instant::now() + Duration::from_secs(30)));
                }
                None if self.stopping.is_none() => self.start(),
                None => eprintln!("capture: still writing the last recording"),
            }
        }
        // (Not waited for here: the frames still to come need this thread
        // to paint.)
        if let Some((_, _, deadline)) = &self.stopping {
            if self.pending.load(Ordering::SeqCst) == 0 || Instant::now() > *deadline {
                if let Some((r, end, _)) = self.stopping.take() {
                    finish(r, end);
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(20));
            }
        }
        let Some(r) = &mut self.recording else {
            return;
        };
        // Keep frames coming while recording, even when nothing moves.
        ctx.request_repaint_after(Duration::from_secs_f64(1.0 / FPS));
        let now = Instant::now();
        if r.last
            .is_some_and(|t| now - t < Duration::from_secs_f64(1.0 / FPS * 0.9))
        {
            return;
        }
        if self.pending.load(Ordering::SeqCst) >= QUEUE {
            return; // Behind: the last frame stays on screen a little longer.
        }
        r.last = Some(now);
        r.next += 1;
        let name = format!("frame-{:05}.png", r.next);
        let at = now - r.started;
        let frames = r.frames.clone();
        let path = r.dir.join(&name);
        self.ask(ctx, path, true, Some((frames, name, at)));
    }

    /// Ask egui for this frame's pixels; they're written on a worker.
    fn ask(
        &self,
        ctx: &egui::Context,
        path: PathBuf,
        half: bool,
        note: Option<(FrameLog, String, Duration)>,
    ) {
        let jobs = self.jobs.clone();
        let pending = self.pending.clone();
        pending.fetch_add(1, Ordering::SeqCst);
        ctx.request_screenshot(move |image| {
            if let Some((frames, name, at)) = note
                && let Ok(mut f) = frames.lock()
            {
                f.push((name, at));
            }
            if jobs.send(Job { path, image, half }).is_err() {
                pending.fetch_sub(1, Ordering::SeqCst);
            }
        });
    }

    fn start(&mut self) {
        self.takes += 1;
        let dir = self.dir.join(format!("take-{:03}", self.takes));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("capture: can't record into {}: {e}", dir.display());
            return;
        }
        eprintln!("capture: recording into {}", dir.display());
        self.recording = Some(Recording {
            dir,
            started: Instant::now(),
            last: None,
            frames: Default::default(),
            next: 0,
        });
    }
}

/// A recording's frames are all written: its `frames.txt`.
fn finish(r: Recording, end: Duration) {
    {
        let mut frames = r.frames.lock().map(|f| f.clone()).unwrap_or_default();
        frames.sort_by_key(|(_, at)| *at);
        let mut list = String::from("ffconcat version 1.0\n");
        for (k, (name, at)) in frames.iter().enumerate() {
            let until = frames.get(k + 1).map_or(end, |(_, t)| *t);
            let shown = until.saturating_sub(*at).as_secs_f64().max(0.001);
            list.push_str(&format!("file '{name}'\nduration {shown:.4}\n"));
        }
        // (ffmpeg's concat takes the last file's duration only if it's listed twice.)
        if let Some((name, _)) = frames.last() {
            list.push_str(&format!("file '{name}'\n"));
        }
        let txt = r.dir.join("frames.txt");
        match std::fs::write(&txt, list) {
            Ok(()) => eprintln!(
                "capture: stopped: {} frames, {:.1} s, in {}",
                frames.len(),
                end.as_secs_f64(),
                r.dir.display()
            ),
            Err(e) => eprintln!("capture: {}: {e}", txt.display()),
        }
    }
}

/// Write `job`'s image as a PNG (halved for a video frame).
fn write_png(job: &Job) -> anyhow::Result<()> {
    use resvg::tiny_skia::Pixmap;
    let img = &job.image;
    let [w, h] = img.size;
    let (w2, h2) = if job.half { (w / 2, h / 2) } else { (w, h) };
    let mut pm = Pixmap::new(w2 as u32, h2 as u32).ok_or_else(|| anyhow::anyhow!("empty image"))?;
    let out = pm.data_mut();
    if job.half {
        // Each pixel the average of the 2×2 it stands for (the window is
        // opaque, so averaging the colors as they are is fine).
        for y in 0..h2 {
            for x in 0..w2 {
                let mut acc = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let c = img.pixels[(2 * y + dy) * w + 2 * x + dx].to_array();
                    for k in 0..4 {
                        acc[k] += c[k] as u32;
                    }
                }
                let o = (y * w2 + x) * 4;
                for k in 0..4 {
                    out[o + k] = (acc[k] / 4) as u8;
                }
            }
        }
    } else {
        for (o, c) in out.chunks_exact_mut(4).zip(&img.pixels) {
            o.copy_from_slice(&c.to_array());
        }
    }
    // Opaque: premultiplied and straight alpha are the same.
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    std::fs::write(&job.path, pm.encode_png()?)?;
    Ok(())
}
