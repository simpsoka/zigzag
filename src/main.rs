use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::time::Instant;
use zigzag::checks::{self, Level};
use zigzag::design::Design;
use zigzag::document::Document;
use zigzag::formats::preview;
use zigzag::params::FABRICS;

#[derive(Parser)]
#[command(
    name = "zigzag",
    version,
    about = "SVG → embroidery (PES, DST, Brother Skitch)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert an SVG (or a PNG, JPEG, WebP, GIF, BMP or TIFF logo, traced first). Output format comes from each file extension:
    /// .pes .dst .exp .jef .vp3 .pec .pen (Skitch stream), .svg or .png (stitch preview), .html (worksheet).
    Convert {
        input: PathBuf,
        #[arg(short, long = "out", required = true)]
        out: Vec<PathBuf>,
        /// The hoop for previews and checks, in mm: WIDTHxHEIGHT (130x180)
        /// or one number for a square. Default: the hoop saved in the file
        /// (100 × 100 if none).
        #[arg(long, value_parser = parse_hoop)]
        hoop: Option<(f64, f64)>,
        /// Beside each machine file, write FILE.objects.json: which object
        /// each stitch, jump and trim in it is for.
        #[arg(long)]
        objects: bool,
        /// Place another file in the design, as File › Place does: a
        /// machine file (PES, PEC, DST, EXP, JEF or VP3) at the size it was
        /// digitized, or an SVG or image. Centered in the hoop, or FILE@X,Y
        /// for a center X mm right of and Y mm below the hoop's. Repeat for
        /// more; each is sewn after the ones before.
        #[arg(long, value_name = "FILE[@X,Y]", value_parser = parse_place)]
        place: Vec<(PathBuf, Option<(f64, f64)>)>,
        #[command(flatten)]
        settings: SettingsArgs,
    },
    /// Print stitch statistics and design checks for an SVG.
    Info {
        input: PathBuf,
        #[command(flatten)]
        settings: SettingsArgs,
    },
    /// List fabric presets.
    Fabrics,
    /// List the machines a design can be set up for (`--machine`).
    Machines,
    /// List the Skitch conversation in a Bluetooth recording of Artspira
    /// (iPhone PacketLogger .pklg, or Android btsnoop_hci.log), to work out
    /// undocumented commands such as the machine settings.
    Capture {
        file: PathBuf,
        /// Also the upload's data chunks and the status polling.
        #[arg(long)]
        all: bool,
        /// Every ATT write, read and notification, not just the Skitch's.
        #[arg(long)]
        raw: bool,
    },
    /// Talk to a Brother Skitch (PP1) over Bluetooth.
    Skitch {
        #[command(subcommand)]
        cmd: SkitchCmd,
        /// Only connect to a machine whose Bluetooth name contains this.
        #[arg(long, global = true)]
        name: Option<String>,
        /// Seconds to scan for the machine.
        #[arg(long, global = true, default_value_t = 10)]
        scan: u64,
    },
}

/// Override settings saved in the file.
#[derive(clap::Args)]
struct SettingsArgs {
    /// Fabric preset (see `zigzag fabrics`).
    #[arg(long)]
    fabric: Option<String>,
    /// The machine the design is for (see `zigzag machines`): the design
    /// goes in the machine's default hoop unless it's already in one of
    /// its hoops.
    #[arg(long)]
    machine: Option<String>,
    /// No stitch shorter than this, mm (0.1 to 1): for machines that drop
    /// short stitches. Default: what the file or the machine says.
    #[arg(long, value_name = "MM")]
    min_stitch: Option<f64>,
    /// Lock stitches: none, or STYLE:MM:COUNT with style back or triangle
    /// (back:0.4:4 is the usual). Default: what the file or the machine says.
    #[arg(long, value_name = "LOCK")]
    lock: Option<String>,
    /// Warn about satins narrower than this on average, mm (0 to 3; 0
    /// never warns). Default: what the file says, or 1.
    #[arg(long, value_name = "MM")]
    narrow_satin: Option<f64>,
    /// Cap sewing order: center-out, bottom-up.
    #[arg(long)]
    hat: bool,
    /// Where the thread can't get from one part of an object to the next
    /// out of sight, sew it over the fill already there instead of
    /// cutting it: fewer cuts, and running stitch on top of the fill.
    #[arg(long)]
    over_sewn: bool,
    /// Keep fill stitches underneath overlapping objects.
    #[arg(long)]
    keep_overlaps: bool,
    /// Stitch settings for every object that doesn't set its own, written
    /// as in layer names: pc=0.3 (pull compensation, mm per side),
    /// ul=center (satin underlay), s=0.4 (spacing), l=2.5 (stitch length),
    /// lean=8, even=0, split=6, u=0 and the rest. Several in one quoted
    /// value, or the option again: --set "pc=0.3 ul=center".
    #[arg(long = "set", value_name = "SETTINGS")]
    set: Vec<String>,
}

impl SettingsArgs {
    fn open(&self, input: &Path) -> Result<Document> {
        let mut doc = Document::open(input)?;
        if let Some(f) = &self.fabric {
            if zigzag::params::Fabric::by_id(f).is_none() {
                bail!("unknown fabric '{f}' (see `zigzag fabrics`)");
            }
            doc.settings.fabric = f.clone();
        }
        if let Some(m) = &self.machine {
            let Some(p) = zigzag::machine::profile::builtin_named(m) else {
                bail!("unknown machine '{m}' (see `zigzag machines`)");
            };
            p.apply(&mut doc.settings, true);
        }
        if let Some(m) = self.min_stitch {
            if !zigzag::document::MIN_STITCH_MM.contains(&m) {
                bail!("--min-stitch must be from 0.1 to 1 (mm)");
            }
            doc.settings.min_stitch_mm = m;
        }
        if let Some(l) = &self.lock {
            let Some(lock) = zigzag::formats::Lock::parse(l) else {
                bail!(
                    "--lock must be none, back, triangle, or STYLE:MM:COUNT (e.g. triangle:0.5:3)"
                );
            };
            doc.settings.lock = lock;
        }
        if let Some(m) = self.narrow_satin {
            if !zigzag::document::NARROW_SATIN_MM.contains(&m) {
                bail!("--narrow-satin must be from 0 to 3 (mm)");
            }
            doc.settings.narrow_satin_mm = m;
        }
        if self.hat {
            doc.settings.hat_order = true;
        }
        if self.over_sewn {
            doc.settings.over_sewn = true;
        }
        if self.keep_overlaps {
            doc.settings.remove_overlaps = false;
        }
        if !self.set.is_empty() {
            let all = self.set.join(" ");
            for tok in all.split([' ', ',', ';']).filter(|t| !t.is_empty()) {
                if zigzag::params::Params::parse(tok) == zigzag::params::Params::default() {
                    bail!(
                        "--set: '{tok}' isn't a stitch setting (e.g. pc=0.3, ul=center, s=0.4, lean=8)"
                    );
                }
            }
            // Under every object's own settings, like a layer they're all in.
            let base = zigzag::params::Params::parse(&all);
            for shape in &mut doc.shapes {
                shape.params = base.merged(&shape.params);
            }
        }
        Ok(doc)
    }
}

#[derive(Subcommand)]
enum SkitchCmd {
    /// Show machine model, firmware, hoop size and status.
    Info,
    /// Show status and sewing progress.
    Status,
    /// Read the machine's settings bytes; with --log, add them to a file and
    /// show which bytes changed since the last line in it.
    Settings {
        /// What you changed in Artspira since the last read.
        #[arg(long, default_value = "")]
        label: String,
        #[arg(long)]
        log: Option<PathBuf>,
    },
    /// Convert an SVG and upload it.
    Send {
        input: PathBuf,
        /// Trace the design outline after upload.
        #[arg(long)]
        trace: bool,
        /// Start sewing after upload (and trace).
        #[arg(long)]
        sew: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Convert {
            input,
            out,
            hoop,
            objects,
            place,
            settings,
        } => {
            let t0 = Instant::now();
            let mut doc = settings.open(&input)?;
            for (file, at) in &place {
                let at = zigzag::document::Placement {
                    center_mm: *at,
                    ..Default::default()
                };
                let added = doc
                    .place_file(file, at)
                    .with_context(|| format!("placing {}", file.display()))?;
                eprintln!("placed {} ({} objects)", file.display(), added.len());
            }
            let hoop = hoop.unwrap_or(doc.settings.hoop);
            let t_load = t0.elapsed();
            let d = doc.plan();
            let t_plan = t0.elapsed() - t_load;
            for path in &out {
                write_output(&doc, &d, path, hoop)?;
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let file = path.file_name().unwrap_or_default().to_string_lossy();
                if objects && let Some(map) = doc.object_map(&d, ext, &file) {
                    let to = Document::object_map_path(path);
                    std::fs::write(&to, serde_json::to_string_pretty(&map)?)?;
                    eprintln!("wrote {}", to.display());
                }
            }
            summary(&d);
            print_checks(&doc, &d, hoop);
            eprintln!(
                "loaded in {:.1} ms, planned in {:.1} ms, total {:.1} ms",
                ms(t_load),
                ms(t_plan),
                ms(t0.elapsed())
            );
        }
        Cmd::Info { input, settings } => {
            let doc = settings.open(&input)?;
            let d = doc.plan();
            eprintln!(
                "fabric: {}{}{}",
                doc.settings.fabric().name,
                if doc.settings.hat_order {
                    ", cap order"
                } else {
                    ""
                },
                if doc.settings.remove_overlaps {
                    ""
                } else {
                    ", overlaps kept"
                }
            );
            summary(&d);
            print_checks(&doc, &d, doc.settings.hoop);
        }
        Cmd::Fabrics => {
            for f in FABRICS {
                println!("{:<15} {}\n{:<15} {}", f.id, f.name, "", f.about);
            }
        }
        Cmd::Machines => {
            for p in zigzag::machine::profile::builtin() {
                println!("{}", machine_lines(p));
            }
        }
        Cmd::Skitch { cmd, name, scan } => skitch(cmd, name, scan)?,
        Cmd::Capture { file, all, raw } => {
            use zigzag::machine::capture;
            let bytes = std::fs::read(&file)?;
            let atts = capture::read_att(&bytes)?;
            if raw {
                for a in &atts {
                    println!(
                        "{:9.3}  {:?} op 0x{:02x} handle 0x{:04x}  {}",
                        a.t,
                        a.dir,
                        a.op,
                        a.handle,
                        capture::hex(&a.value, &[])
                    );
                }
            } else {
                let msgs = capture::skitch_messages(&atts);
                if msgs.is_empty() {
                    bail!(
                        "no Skitch traffic found in {} ({} ATT operations); try --raw",
                        file.display(),
                        atts.len()
                    );
                }
                print!("{}", capture::listing(&msgs, all));
            }
        }
    }
    Ok(())
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// `--hoop`: `130x180`, or `100` for a square.
fn parse_hoop(v: &str) -> std::result::Result<(f64, f64), String> {
    let square = || v.parse::<f64>().ok().map(|s| format!("{s}x{s}"));
    zigzag::document::parse_hoop(v)
        .or_else(|| zigzag::document::parse_hoop(&square()?))
        .ok_or_else(|| "use WIDTHxHEIGHT in mm, e.g. 130x180, each side 10 to 1000".to_string())
}

/// `--place`: `FILE`, or `FILE@X,Y` (mm from the hoop's center).
fn parse_place(v: &str) -> std::result::Result<(PathBuf, Option<(f64, f64)>), String> {
    if let Some((file, at)) = v.rsplit_once('@')
        && let Some((x, y)) = at.split_once(',')
        && let (Ok(x), Ok(y)) = (x.trim().parse::<f64>(), y.trim().parse::<f64>())
        && x.is_finite()
        && y.is_finite()
    {
        return Ok((PathBuf::from(file), Some((x, y))));
    }
    Ok((PathBuf::from(v), None))
}

fn write_output(doc: &Document, d: &Design, path: &Path, hoop: (f64, f64)) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = match ext.as_str() {
        "html" => {
            let info = zigzag::formats::worksheet::WorksheetInfo {
                title: doc.name.clone(),
                fabric: doc.settings.fabric().name.to_string(),
                hoop: Some((
                    format!(
                        "{} × {} mm",
                        zigzag::params::fmt_num(hoop.0),
                        zigzag::params::fmt_num(hoop.1)
                    ),
                    hoop.0,
                    hoop.1,
                )),
                background: [255, 255, 255],
                names: doc.object_names(),
                spm: 400.0,
            };
            zigzag::formats::worksheet::html(d, &info)?.into_bytes()
        }
        "svg" => preview::write_svg(d, &preview::PreviewOptions { hoop_mm: Some(hoop), ..Default::default() }).into_bytes(),
        "png" => preview::render_png(d, &preview::PreviewOptions { hoop_mm: Some(hoop), ..Default::default() }, 1200)?,
        other => zigzag::formats::write_format_with(d, other, &doc.settings.export_options())
            .map_err(|_| anyhow::anyhow!("unknown output format '.{other}' (use .pes .dst .exp .jef .vp3 .pec .pen .svg .png, or .html for a worksheet)"))?,
    };
    std::fs::write(path, bytes)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

fn print_checks(doc: &Document, d: &Design, hoop: (f64, f64)) {
    let names = doc.object_names();
    for issue in checks::check(&doc.shapes, d, &doc.plan_options(), Some(hoop)) {
        let level = match issue.level {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Info => "note",
        };
        match issue.shape {
            Some(i) => eprintln!("{level}: {}: {}", names[i], issue.message),
            None => eprintln!("{level}: {}", issue.message),
        }
    }
}

/// `zigzag machines`'s two lines for one profile: name and hoops, then what
/// choosing it (`--machine`) changes besides the hoop (`Profile::apply`) -
/// easy to miss otherwise, since picking a machine doesn't ask for them.
fn machine_lines(p: &zigzag::machine::profile::Profile) -> String {
    let hoops: Vec<String> = p
        .hoops
        .iter()
        .map(|&h| zigzag::document::format_hoop(h))
        .collect();
    let min_stitch = p
        .min_stitch_mm
        .map(|m| format!("; min stitch {} mm", zigzag::params::fmt_num(m)))
        .unwrap_or_default();
    let lock = p
        .lock
        .map(|l| format!("; lock {}", l.format()))
        .unwrap_or_default();
    let link = p
        .link
        .and_then(|id| zigzag::machine::profile::LINKS.iter().find(|l| l.0 == id))
        .map(|l| format!("; sends over {}", l.1))
        .unwrap_or_default();
    format!(
        "{:<15} {}\n{:<15} hoops {} mm{}{}{}{}{}",
        p.id,
        p.name,
        "",
        hoops.join(", "),
        p.format
            .map(|f| format!("; reads .{f}"))
            .unwrap_or_default(),
        min_stitch,
        lock,
        link,
        if p.tested { "; sewn from ZigZag" } else { "" }
    )
}

fn summary(d: &Design) {
    let b = d.bounds();
    eprintln!(
        "{:.1} × {:.1} mm, {} stitches, {} colors, {} trims",
        b.width() / 10.0,
        b.height() / 10.0,
        d.stitch_count(),
        d.blocks.len(),
        d.trim_count()
    );
    for (i, blk) in d.blocks.iter().enumerate() {
        let n: usize = blk.runs.iter().map(|r| r.pts.len()).sum();
        eprintln!(
            "  color {}: #{:02x}{:02x}{:02x}, {} stitches",
            i + 1,
            blk.color[0],
            blk.color[1],
            blk.color[2],
            n
        );
    }
}

#[cfg(not(feature = "ble"))]
fn skitch(_cmd: SkitchCmd, _name: Option<String>, _scan: u64) -> Result<()> {
    bail!(
        "this build has no Bluetooth support; rebuild with `cargo build --release --features ble`"
    )
}

#[cfg(feature = "ble")]
fn skitch(cmd: SkitchCmd, name: Option<String>, scan: u64) -> Result<()> {
    use zigzag::machine::skitch::Skitch;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        // Convert before connecting so design errors show up immediately.
        let design = match &cmd {
            SkitchCmd::Send { input, .. } => Some(zigzag::design_from_svg(input)?),
            _ => None,
        };
        eprintln!("scanning for Skitch…");
        let m = Skitch::connect(std::time::Duration::from_secs(scan), name.as_deref()).await?;
        let result = async {
            match cmd {
                SkitchCmd::Info => {
                    let i = m.info().await?;
                    let (status, err) = m.state().await?;
                    println!("model:    {}", i.model);
                    println!("serial:   {}", i.serial);
                    println!("firmware: {}", i.software_version);
                    println!(
                        "area:     {} × {} mm",
                        i.max_width as f64 / 10.0,
                        i.max_height as f64 / 10.0
                    );
                    println!("status:   {status:?} (error 0x{err:02x})");
                }
                SkitchCmd::Settings { label, log } => {
                    use zigzag::machine::capture;
                    let now = m.settings().await?;
                    let last = log
                        .as_ref()
                        .and_then(|f| std::fs::read_to_string(f).ok())
                        .and_then(|t| t.lines().last().map(str::to_string))
                        .and_then(|l| l.rsplit('\t').next().map(str::to_string))
                        .map(|h| {
                            h.split_whitespace()
                                .filter_map(|b| {
                                    u8::from_str_radix(b.trim_matches(['[', ']']), 16).ok()
                                })
                                .collect::<Vec<u8>>()
                        });
                    let marks = last
                        .as_ref()
                        .map(|l| capture::changed(l, &now))
                        .unwrap_or_default();
                    println!("{}", capture::hex(&now, &marks));
                    if !marks.is_empty() {
                        println!("changed: bytes {marks:?}");
                    }
                    if let Some(f) = log {
                        use std::io::Write;
                        let mut file = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&f)?;
                        writeln!(file, "{label}\t{}", capture::hex(&now, &[]))?;
                    }
                }
                SkitchCmd::Status => {
                    let (status, err) = m.state().await?;
                    let p = m.progress().await?;
                    println!(
                        "status: {status:?} (error 0x{err:02x}), stitch {}, {}s",
                        p.current_stitch, p.current_time_s
                    );
                }
                SkitchCmd::Send { trace, sew, .. } => {
                    let d = design.unwrap();
                    summary(&d);
                    let info = m.info().await?;
                    let b = d.bounds();
                    if b.width() > info.max_width as f64 || b.height() > info.max_height as f64 {
                        bail!(
                            "design is {:.1} × {:.1} mm but the machine's area is {:.1} × {:.1} mm",
                            b.width() / 10.0,
                            b.height() / 10.0,
                            info.max_width as f64 / 10.0,
                            info.max_height as f64 / 10.0
                        );
                    }
                    let pen = zigzag::formats::pen::encode(&d);
                    eprintln!("uploading {} bytes…", pen.bytes.len());
                    m.upload(&pen, |f| eprint!("\r  {:3.0}%", f * 100.0))
                        .await?;
                    eprintln!("\nuploaded");
                    if trace {
                        m.mask_trace().await?;
                        eprintln!("tracing outline");
                    }
                    if sew {
                        m.start_sewing().await?;
                        eprintln!("sewing started");
                    }
                }
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let _ = m.disconnect().await;
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_lines_says_nothing_extra_for_a_plain_profile() {
        let p = zigzag::machine::profile::builtin_named("brother-4x4").unwrap();
        let s = machine_lines(p);
        assert!(s.contains("brother-4x4"));
        assert!(s.contains("hoops 100x100 mm"));
        assert!(s.contains("; reads .pes"));
        assert!(!s.contains("min stitch"));
        assert!(!s.contains("lock"));
        assert!(!s.contains("sends over"));
        assert!(!s.contains("sewn from ZigZag"));
    }

    #[test]
    fn machine_lines_says_what_the_machine_changes() {
        // Drops stitches under 0.4 mm (see profiles.json): worth knowing
        // before `--machine melco-summit` quietly merges short stitches.
        let p = zigzag::machine::profile::builtin_named("melco-summit").unwrap();
        let s = machine_lines(p);
        assert!(s.contains("; min stitch 0.4 mm"), "{s}");
        assert!(!s.contains("; lock"), "{s}");
        assert!(!s.contains("sends over"), "{s}");
    }

    #[test]
    fn machine_lines_says_how_a_linked_machine_is_reached() {
        let p = zigzag::machine::profile::builtin_named("brother-skitch").unwrap();
        let s = machine_lines(p);
        assert!(s.contains("; sends over Bluetooth (Brother Skitch)"), "{s}");
        assert!(s.contains("; sewn from ZigZag"), "{s}");
    }
}
