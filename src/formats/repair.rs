//! Checking a machine file someone brings (PES, PEC, DST, EXP, JEF), and
//! fixing what can be fixed safely in its stitches alone: what the web's
//! file checker runs, in the browser.
//!
//! A machine file holds only stitches, so the fixes are the ones that don't
//! need the shapes they came from: same-thread blocks sewn one after another
//! joined (fewer thread changes), stitches too short to sew taken out,
//! stitches too long to sew split, thread carried across the design cut,
//! and tie stitches added where pieces start and end without them. What
//! needs the design itself (density, underlay, pull, lettering) is only
//! reported.

use super::read::{StitchFile, read_stitch_file};
use super::{ExportOptions, Lock, LockStyle, plan, write_format_with};
use crate::design::{Block, Design, Run};
use crate::geom::{P, Polyline};
use anyhow::Result;

/// Stitches shorter than this (units, tenths of a mm) are taken out: the
/// needle going down twice in one hole, which piles thread and knots. (ZigZag
/// itself sews stitches down to 0.1 mm, as on a satin's tight curves:
/// those stay.)
pub const SHORT: f64 = 1.0;
/// Stitches longer than this are split (the length ZigZag's checks call too
/// long for a machine to sew as one stitch).
pub const LONG: f64 = 121.0;
/// The longest part of a split stitch.
const SPLIT_TO: f64 = 70.0;
/// Thread carried at least this far without a trim is cut (so it isn't
/// left across the design to cut by hand).
pub const TRIM_OVER: f64 = 30.0;

/// A design's numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub stitches: usize,
    /// Different thread colours.
    pub colors: usize,
    pub color_changes: usize,
    pub trims: usize,
    /// Moves to the next piece without cutting the thread.
    pub jumps: usize,
    pub width_mm: f64,
    pub height_mm: f64,
}

/// One kind of fix, and how many times it was made.
#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub key: &'static str,
    pub count: usize,
    /// What was done, in a sentence.
    pub said: String,
}

/// A file read, checked and fixed.
#[derive(Clone, Debug)]
pub struct Checked {
    pub name: String,
    /// The format it came in (lower case, no dot).
    pub format: String,
    /// What's wrong with the file itself.
    pub notes: Vec<String>,
    pub original: Design,
    pub fixed: Design,
    /// Tie stitches for the fixed file (none when it has its own).
    pub lock: Lock,
    pub fixes: Vec<Fix>,
    pub before: Stats,
    pub after: Stats,
    /// What's still worth fixing, which needs the design rather than its
    /// stitches.
    pub remaining: Vec<String>,
    /// The smallest common hoop it fits, mm (width, height).
    pub hoop: Option<(f64, f64)>,
    /// Machines ZigZag knows with a hoop it fits.
    pub machines: Vec<String>,
}

/// Read `bytes` (a `.ext` file), check it and fix what's safe to fix.
pub fn check(bytes: &[u8], ext: &str) -> Result<Checked> {
    let file = read_stitch_file(bytes, ext)?;
    Ok(check_file(file, ext))
}

/// [`check`], on a file already read.
pub fn check_file(file: StitchFile, ext: &str) -> Checked {
    let mut notes = file.notes.clone();
    let original = design_of(&file);
    let before = stats(&original);
    if before.stitches == 0 {
        notes.push("It has no stitches in it.".into());
    }
    let (fixed, fixes) = fix_design(&original, writes_trims(ext));
    let after = stats(&fixed);
    let remaining = remaining(&fixed);
    let (hoop, machines) = fits(&after);
    Checked {
        name: file.name.clone(),
        format: ext.to_ascii_lowercase(),
        notes,
        original,
        fixed,
        lock: no_lock(),
        fixes,
        before,
        after,
        remaining,
        hoop,
        machines,
    }
}

/// Whether ZigZag writes trims into a `.ext` file (not a JEF: its machine
/// cuts long jumps itself).
fn writes_trims(ext: &str) -> bool {
    !ext.eq_ignore_ascii_case("jef")
}

/// No tie stitches of the writer's: the fixed design has its own.
fn no_lock() -> Lock {
    Lock {
        style: LockStyle::None,
        ..Lock::default()
    }
}

/// `original` with the safe fixes made, and what they were. `trims`: the
/// file it's for can hold trims, so thread carried across the design is cut.
fn fix_design(original: &Design, trims: bool) -> (Design, Vec<Fix>) {
    let mut fixed = original.clone();
    let mut fixes = Vec::new();
    let mut fix = |key: &'static str, count: usize, said: String| {
        if count > 0 {
            fixes.push(Fix { key, count, said });
        }
    };

    let joined = join_same_threads(&mut fixed);
    fix(
        "join",
        joined,
        format!(
            "Joined {} that sewed the same thread one after another: {} fewer thread change{}.",
            plural(joined, "block", "blocks"),
            joined,
            if joined == 1 { "" } else { "s" }
        ),
    );
    let short = drop_short(&mut fixed);
    fix(
        "short",
        short,
        format!(
            "Took out {} that went nowhere (the needle down twice in one hole), which piles thread and can knot.",
            plural(short, "stitch", "stitches"),
        ),
    );
    let long = split_long(&mut fixed);
    fix(
        "long",
        long,
        format!(
            "Split {} longer than {:.1} mm, too long to sew as one stitch (they snag and sag).",
            plural(long, "stitch", "stitches"),
            LONG / 10.0
        ),
    );
    // (Not for a JEF: ZigZag writes no trims into one, so they'd be claimed
    // and not there. Its machine cuts long jumps itself.)
    let cut = if trims { trim_crossings(&mut fixed) } else { 0 };
    fix(
        "trim",
        cut,
        format!(
            "Added {} where the thread was carried {:.0} mm or more across the design, which you'd otherwise cut by hand.",
            plural(cut, "trim", "trims"),
            TRIM_OVER / 10.0
        ),
    );
    // Ties, when most pieces have none: on the pieces without them only (a
    // piece with its own keeps them: two sets would pile up). They go into
    // the stitches themselves, so the file is written with no lock of the
    // writer's on top.
    let untied = untied_pieces(&fixed);
    let pieces = piece_count(&fixed);
    let tied = if pieces > 0 && untied * 2 > pieces { add_ties(&mut fixed) } else { 0 };
    fix(
        "ties",
        tied,
        format!(
            "Added tie stitches where {} started or ended without them, so the thread doesn't pull out after a trim.",
            plural(tied, "piece", "pieces")
        ),
    );
    (fixed, fixes)
}

impl Checked {
    /// The fixed design as a `.ext` machine file. (Fixed for that format:
    /// see [`Checked::for_format`].)
    pub fn write(&self, ext: &str) -> Result<Vec<u8>> {
        if writes_trims(ext) == writes_trims(&self.format) {
            write_format_with(&self.fixed, ext, &self.export_options())
        } else {
            let c = self.for_format(ext);
            write_format_with(&c.fixed, ext, &c.export_options())
        }
    }

    /// This, with the fixes that hold in a `.ext` file: what to show for a
    /// file saved in another format than it came in. (A JEF takes no trims,
    /// so none are added for one, or said to be; a JEF saved as something
    /// else gets them.)
    pub fn for_format(&self, ext: &str) -> Checked {
        let mut c = self.clone();
        if writes_trims(ext) != writes_trims(&self.format) {
            let (fixed, fixes) = fix_design(&self.original, writes_trims(ext));
            c.after = stats(&fixed);
            c.remaining = remaining(&fixed);
            (c.hoop, c.machines) = fits(&c.after);
            c.fixed = fixed;
            c.fixes = fixes;
        }
        c
    }

    /// The fixed design (or the original) as a stitch plan, for the stitch
    /// player.
    pub fn plan(&self, original: bool) -> Vec<u8> {
        // (Neither is given ties here: the original is shown as it came,
        // and the fixed design has its own.)
        let d = if original { &self.original } else { &self.fixed };
        plan::write(d, &self.export_options())
    }

    fn export_options(&self) -> ExportOptions {
        ExportOptions {
            lock: self.lock,
            min_stitch: 0.0,
        }
    }
}

/// The file as a design: a block per thread, a run per piece.
fn design_of(f: &StitchFile) -> Design {
    let blocks = f
        .blocks
        .iter()
        .map(|b| Block {
            color: b.color,
            runs: b
                .runs
                .iter()
                .enumerate()
                .map(|(i, pts)| Run {
                    pts: pts.clone(),
                    // (A block's first run follows a thread change, which cuts.)
                    trim_before: i > 0 && b.trims.get(i).copied().unwrap_or(false),
                })
                .collect(),
        })
        .collect();
    Design {
        name: f.name.clone(),
        blocks,
        ..Default::default()
    }
}

pub fn stats(d: &Design) -> Stats {
    let runs = || d.blocks.iter().flat_map(|b| b.runs.iter().skip(1));
    let mut colors: Vec<[u8; 3]> = Vec::new();
    for b in &d.blocks {
        if !colors.contains(&b.color) {
            colors.push(b.color);
        }
    }
    let b = d.bounds();
    let (w, h) = if b.is_empty() { (0.0, 0.0) } else { (b.width() / 10.0, b.height() / 10.0) };
    Stats {
        stitches: d.blocks.iter().flat_map(|b| &b.runs).map(|r| r.pts.len()).sum(),
        colors: colors.len(),
        color_changes: d.blocks.len().saturating_sub(1),
        trims: runs().filter(|r| r.trim_before).count(),
        jumps: runs().filter(|r| !r.trim_before).count(),
        width_mm: w,
        height_mm: h,
    }
}

/// Blocks of one thread sewn straight after a block of the same thread
/// become one (the thread change between them is a cut). How many joined.
fn join_same_threads(d: &mut Design) -> usize {
    let mut out: Vec<Block> = Vec::with_capacity(d.blocks.len());
    let mut joined = 0;
    for mut b in d.blocks.drain(..) {
        match out.last_mut() {
            Some(prev) if prev.color == b.color => {
                if let Some(first) = b.runs.first_mut() {
                    first.trim_before = true;
                }
                prev.runs.append(&mut b.runs);
                joined += 1;
            }
            _ => out.push(b),
        }
    }
    d.blocks = out;
    joined
}

/// Points of a run's ends left as they are: where tie stitches are, and a
/// file's own short or doubled stitches there hold the thread.
const ENDS: usize = 7;

/// Points closer than [`SHORT`] to the last one kept are taken out, away
/// from a run's ends (see [`ENDS`]). How many.
fn drop_short(d: &mut Design) -> usize {
    let mut dropped = 0;
    for r in d.blocks.iter_mut().flat_map(|b| b.runs.iter_mut()) {
        let n = r.pts.len();
        if n <= 2 * ENDS + 1 {
            continue;
        }
        let mut kept: Polyline = Vec::with_capacity(n);
        kept.extend_from_slice(&r.pts[..ENDS]);
        for &p in &r.pts[ENDS..n - ENDS] {
            match kept.last() {
                Some(&q) if q.dist(p) < SHORT => dropped += 1,
                _ => kept.push(p),
            }
        }
        // (The tail goes back as it was: a point dropped just before it
        // only shortens the step into it.)
        kept.extend_from_slice(&r.pts[n - ENDS..]);
        r.pts = kept;
    }
    dropped
}

/// Stitches longer than [`LONG`] become equal stitches of up to
/// [`SPLIT_TO`]. How many were split.
fn split_long(d: &mut Design) -> usize {
    let mut split = 0;
    for r in d.blocks.iter_mut().flat_map(|b| b.runs.iter_mut()) {
        if !r.pts.windows(2).any(|w| w[0].dist(w[1]) > LONG) {
            continue;
        }
        let mut out: Polyline = vec![r.pts[0]];
        for w in r.pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = a.dist(b);
            if len > LONG {
                split += 1;
                let n = (len / SPLIT_TO).ceil() as usize;
                for k in 1..n {
                    let t = k as f64 / n as f64;
                    out.push(P::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
                }
            }
            out.push(b);
        }
        r.pts = out;
    }
    split
}

/// Moves to a next piece of [`TRIM_OVER`] or more without a cut get one.
/// How many.
fn trim_crossings(d: &mut Design) -> usize {
    let mut cut = 0;
    for b in &mut d.blocks {
        for i in 1..b.runs.len() {
            let from = *b.runs[i - 1].pts.last().unwrap();
            let r = &mut b.runs[i];
            if !r.trim_before && from.dist(r.pts[0]) >= TRIM_OVER {
                r.trim_before = true;
                cut += 1;
            }
        }
    }
    cut
}

/// The pieces: runs from one cut (or thread change) to the next. Their
/// first and last runs, by (block, run) index.
fn pieces(d: &Design) -> Vec<((usize, usize), (usize, usize))> {
    let mut out = Vec::new();
    for (bi, b) in d.blocks.iter().enumerate() {
        let mut start = 0;
        for ri in 1..=b.runs.len() {
            if ri == b.runs.len() || b.runs[ri].trim_before {
                out.push(((bi, start), (bi, ri - 1)));
                start = ri;
            }
        }
    }
    out
}

fn piece_count(d: &Design) -> usize {
    pieces(d).len()
}

/// Is there a tie in `pts` (its first few stitches, within a short way of
/// where it starts, going back on themselves)?
fn tied(pts: &[P]) -> bool {
    let near: Vec<P> = pts
        .iter()
        .take(7)
        .take_while(|p| p.dist(pts[0]) <= 15.0)
        .copied()
        .collect();
    near.len() >= 3
        && near.windows(3).any(|w| {
            let (u, v) = (w[1] - w[0], w[2] - w[1]);
            u.x * v.x + u.y * v.y < 0.0
        })
}

/// How long a tie stitch is (units): 0.4 mm, as ZigZag's own.
const TIE: f64 = 4.0;

/// `pts` with a tie at its start: two stitches out along the way it sets
/// off and back again, so four in all, as ZigZag's own writer ties.
fn tie_start(pts: &mut Polyline) {
    let Some(&at) = pts.first() else { return };
    let dir = pts
        .iter()
        .map(|&p| p - at)
        .find(|v| v.x.hypot(v.y) > 1e-6)
        .map(|v| {
            let l = v.x.hypot(v.y);
            P::new(v.x / l, v.y / l)
        })
        .unwrap_or(P::new(1.0, 0.0));
    let out = P::new((at.x + dir.x * TIE).round(), (at.y + dir.y * TIE).round());
    pts.splice(1..1, [out, at, out, at]);
}

/// Tie stitches on the pieces that start or end without them (at the end
/// that has none). How many pieces.
fn add_ties(d: &mut Design) -> usize {
    let mut tied_now = 0;
    for ((sb, sr), (eb, er)) in pieces(d) {
        let start = !tied(&d.blocks[sb].runs[sr].pts);
        let end: Polyline = d.blocks[eb].runs[er].pts.iter().rev().take(ENDS).copied().collect();
        let end_untied = !tied(&end);
        if start {
            tie_start(&mut d.blocks[sb].runs[sr].pts);
        }
        if end_untied {
            let pts = &mut d.blocks[eb].runs[er].pts;
            pts.reverse();
            tie_start(pts);
            pts.reverse();
        }
        tied_now += usize::from(start || end_untied);
    }
    tied_now
}

/// Pieces that start or end with no tie.
fn untied_pieces(d: &Design) -> usize {
    pieces(d)
        .into_iter()
        .filter(|&((sb, sr), (eb, er))| {
            let start = &d.blocks[sb].runs[sr].pts;
            let end: Polyline = d.blocks[eb].runs[er].pts.iter().rev().take(ENDS).copied().collect();
            !tied(start) || !tied(&end)
        })
        .count()
}

/// What's still worth fixing, which needs the design: said for a person
/// with only the file, from ZigZag's own checks.
fn remaining(d: &Design) -> Vec<String> {
    use crate::checks::machine_issues;
    use crate::design::PlanOptions;
    let mut out = Vec::new();
    let issues = machine_issues(d, &PlanOptions::default());
    let has = |s: &str| issues.iter().any(|i| i.message.contains(s));
    if has("Thread piles up") {
        out.push("Some areas are sewn very thick: stiff, and hard on needles. The fills there need fewer rows, or the pieces under them taken out.".into());
    }
    if has("needle points fall within") {
        out.push("In places many needle holes fall close together, which can cut the fabric. Tight curves of satin are the usual cause.".into());
    }
    if has("between cuts with under") {
        out.push("Some pieces between trims are only a few stitches long, which trimmers can miss. Joining them to what's beside them takes the design.".into());
    }
    out
}

/// The smallest common hoop the design fits (either way round), and the
/// machines ZigZag knows with a hoop it fits.
fn fits(s: &Stats) -> (Option<(f64, f64)>, Vec<String>) {
    if s.stitches == 0 {
        return (None, Vec::new());
    }
    let fit = |h: (f64, f64)| {
        (s.width_mm <= h.0 && s.height_mm <= h.1) || (s.width_mm <= h.1 && s.height_mm <= h.0)
    };
    let profiles = crate::machine::profile::builtin();
    let mut hoops: Vec<(f64, f64)> = profiles.iter().flat_map(|p| p.hoops.iter().copied()).collect();
    hoops.sort_by(|a, b| (a.0 * a.1).total_cmp(&(b.0 * b.1)));
    let hoop = hoops.into_iter().find(|&h| fit(h));
    let machines = profiles
        .iter()
        .filter(|p| p.hoops.iter().any(|&h| fit(h)))
        .map(|p| p.name.clone())
        .collect();
    (hoop, machines)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::write_format;

    fn run(pts: &[(f64, f64)], trim: bool) -> Run {
        Run {
            pts: pts.iter().map(|&(x, y)| P::new(x, y)).collect(),
            trim_before: trim,
        }
    }

    /// A file with each problem, as read (not written by ZigZag, whose
    /// writers tie, split and trim already): two same-coloured blocks in a
    /// row, doubled points mid-run, a 20 mm stitch, 8 mm of thread carried
    /// uncut, and no ties.
    fn messy() -> StitchFile {
        use super::super::read::StitchBlock;
        let red = [200, 30, 30];
        let line = |y: f64, from: f64, n: usize| -> Polyline { (0..n).map(|k| P::new(from + 20.0 * k as f64, y)).collect() };
        let mut first = line(0.0, 0.0, 20);
        // Doubled points in the middle, and a 20 mm stitch.
        first.insert(10, first[9]);
        first.insert(11, first[10] + P::new(0.4, 0.0));
        let end = *first.last().unwrap();
        first.push(end + P::new(200.0, 0.0));
        let second = line(0.0, end.x + 280.0, 20);
        StitchFile {
            name: "messy".into(),
            blocks: vec![
                StitchBlock { color: red, runs: vec![first, second], trims: vec![false, false] },
                StitchBlock { color: red, runs: vec![line(50.0, 0.0, 20)], trims: vec![false] },
                StitchBlock { color: [20, 20, 120], runs: vec![line(100.0, 0.0, 20)], trims: vec![false] },
            ],
            notes: Vec::new(),
        }
    }

    #[test]
    fn the_safe_fixes_are_made_and_said() {
        let c = check_file(messy(), "pes");
        let key = |k: &str| c.fixes.iter().find(|f| f.key == k).map(|f| f.count);
        assert_eq!(key("join"), Some(1), "{:?}", c.fixes);
        assert_eq!(c.after.color_changes, c.before.color_changes - 1);
        assert_eq!(key("short"), Some(2), "{:?}", c.fixes);
        assert_eq!(key("long"), Some(1), "{:?}", c.fixes);
        assert_eq!(key("trim"), Some(1), "{:?}", c.fixes);
        assert!(key("ties").is_some(), "{:?}", c.fixes);
        assert!(c.after.trims > c.before.trims);
        // Nothing longer than the limit is left.
        let longest = c
            .fixed
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .flat_map(|r| r.pts.windows(2).map(|w| w[0].dist(w[1])))
            .fold(0.0, f64::max);
        assert!(longest <= LONG, "{longest}");
        assert!(c.notes.is_empty(), "a whole file: {:?}", c.notes);
        // It writes, and reads back, in every format.
        for ext in ["pes", "dst", "jef", "exp", "pec"] {
            let out = c.write(ext).unwrap();
            assert!(read_stitch_file(&out, ext).is_ok(), "{ext}");
        }
        assert!(plan::read(&c.plan(false)).is_ok());
        // The ties are in the stitches, on the pieces that had none: every
        // piece is tied now, and the writer adds none of its own.
        assert_eq!(c.lock.style, LockStyle::None);
        assert_eq!(untied_pieces(&c.fixed), 0);
        // The original is shown as it came: no ties of ZigZag's on it.
        let shown = plan::read(&c.plan(true)).unwrap();
        assert_eq!(shown.1.lock.style, LockStyle::None, "the original, as it is");
        // A JEF gets no trims it can't keep.
        let j = check_file(messy(), "jef");
        assert!(j.fixes.iter().all(|f| f.key != "trim"), "{:?}", j.fixes);
        // Saved as another format, the fixes are that format's: a PES saved
        // as a JEF claims no trims, and a JEF saved as a PES gets them.
        assert!(c.for_format("jef").fixes.iter().all(|f| f.key != "trim"));
        assert_eq!(c.for_format("jef").after.trims, j.after.trims);
        assert!(j.for_format("pes").fixes.iter().any(|f| f.key == "trim"));
        assert_eq!(j.for_format("pes").after, c.after);
        assert!(j.write("pes").is_ok() && c.write("jef").is_ok());
    }

    #[test]
    fn a_piece_with_its_own_ties_gets_no_more() {
        // Three pieces, two with no ties and one tied at both ends.
        let plain = |x: f64| run(&[(x, 0.0), (x + 20.0, 0.0), (x + 40.0, 0.0), (x + 60.0, 0.0)], true);
        let tied_run = run(
            &[(0.0, 50.0), (4.0, 50.0), (0.0, 50.0), (20.0, 50.0), (40.0, 50.0), (36.0, 50.0), (40.0, 50.0)],
            true,
        );
        let mut d = Design {
            name: "mixed".into(),
            blocks: vec![Block { color: [10, 10, 10], runs: vec![plain(0.0), tied_run.clone(), plain(200.0)] }],
            ..Default::default()
        };
        assert_eq!(untied_pieces(&d), 2);
        assert_eq!(add_ties(&mut d), 2);
        assert_eq!(untied_pieces(&d), 0);
        assert_eq!(d.blocks[0].runs[1].pts, tied_run.pts, "left as it was");
        // Four stitches at each end, out along the piece and back.
        let first = &d.blocks[0].runs[0].pts;
        assert_eq!(first.len(), 4 + 8);
        assert_eq!(&first[..5], &[P::new(0.0, 0.0), P::new(4.0, 0.0), P::new(0.0, 0.0), P::new(4.0, 0.0), P::new(0.0, 0.0)]);
        assert_eq!(first[first.len() - 1], P::new(60.0, 0.0));
        assert_eq!(first[first.len() - 2], P::new(56.0, 0.0));
    }

    #[test]
    fn zigzags_own_files_need_nothing() {
        // A satin round a tight curve: short inner stitches are meant.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 40 40"><circle cx="20" cy="20" r="6" fill="none" stroke="#c03" stroke-width="3"/><rect x="4" y="4" width="10" height="10" fill="#036"/></svg>"##;
        let d = crate::document::Document::from_svg(svg, "curve", None).unwrap().plan();
        for ext in ["pes", "dst", "exp"] {
            let c = check(&write_format(&d, ext).unwrap(), ext).unwrap();
            assert!(c.fixes.is_empty(), "{ext}: {:?}", c.fixes);
            assert!(c.notes.is_empty(), "{ext}: {:?}", c.notes);
        }
    }

    #[test]
    fn a_tidy_file_is_left_as_it_is() {
        let tidy = Design {
            name: "tidy".into(),
            blocks: vec![Block {
                color: [10, 10, 10],
                runs: vec![run(&[(0.0, 0.0), (20.0, 0.0), (40.0, 0.0), (60.0, 0.0)], false)],
            }],
            ..Default::default()
        };
        let c = check(&write_format(&tidy, "dst").unwrap(), "dst").unwrap();
        // (The writer's own ties are in it: it's left with them, not given more.)
        assert!(c.fixes.iter().all(|f| f.key != "join" && f.key != "long" && f.key != "trim"), "{:?}", c.fixes);
        assert_eq!(c.lock.style, LockStyle::None, "it has its ties");
        assert_eq!(c.before.stitches, c.after.stitches, "its ties kept");
    }

    #[test]
    fn trims_survive_reading_each_format() {
        let d = Design {
            name: "two".into(),
            blocks: vec![Block {
                color: [10, 10, 10],
                runs: vec![
                    run(&[(0.0, 0.0), (20.0, 0.0), (40.0, 0.0)], false),
                    run(&[(120.0, 0.0), (140.0, 0.0), (160.0, 0.0)], true),
                ],
            }],
            ..Default::default()
        };
        // (Not JEF: ZigZag's JEF writer leaves cutting to the machine.)
        for ext in ["pes", "dst", "exp"] {
            let back = check(&write_format(&d, ext).unwrap(), ext).unwrap();
            // Read back, the cut is a trim, not a carried thread.
            assert_eq!((back.before.trims, back.before.jumps), (1, 0), "{ext}: {:?}", back.before);
        }
    }

    #[test]
    fn a_cut_short_file_is_noted() {
        let bytes = check_file(messy(), "dst").write("dst").unwrap();
        let cut = &bytes[..bytes.len() - 6];
        let c = check(cut, "dst").unwrap();
        assert!(c.notes.iter().any(|n| n.contains("cut short")), "{:?}", c.notes);
        assert!(check(b"#PES0001", "pes").is_err(), "too short to read");
    }

    #[test]
    fn the_smallest_hoop_and_machines_that_fit() {
        let c = check_file(messy(), "pes");
        let (w, h) = c.hoop.expect("fits a hoop");
        let (dw, dh) = (c.after.width_mm, c.after.height_mm);
        assert!((dw <= w && dh <= h) || (dw <= h && dh <= w), "{dw}×{dh} in {w}×{h}");
        assert!(!c.machines.is_empty());
    }
}
