//! A document's last stitch plan, kept beside its recovery file
//! (`recovery-N.plan` beside `recovery-N.svg`), so a tab that comes back
//! with unsaved changes shows its stitches straight away. Planning it again
//! takes a while (the first time a lettering's letters are split in a
//! launch, seconds): meanwhile what's shown is the plan as it was, and the
//! worker's plan takes its place when it's done, as after any edit.
//!
//! The file is the whole [`Design`] and its check results, exactly, under
//! the hash of the recovery file's text it was planned from: one that
//! doesn't match (the document changed and its plan wasn't kept again, or
//! a newer version wrote it) is ignored. It's only ever shown, never sewn
//! or exported: those plan the document again first (`plan_now`).
//!
//! Its stitches are where the document planned them, which is its
//! artwork less its `offset`; and that's worked out again from the artwork
//! when the recovery file is read (after an edit, somewhere else). So the
//! offset is kept too, and the stitches moved by the difference.
//!
//! Little-endian, every length checked against what's left (see
//! `zigzag::formats::plan`).

use zigzag::checks::{Issue, Level};
use zigzag::design::{Block, Design, Run, Travel, TravelHow};
use zigzag::geom::P;

const MAGIC: &[u8; 6] = b"ZZKEEP";
const VERSION: u8 = 1;

/// A stable hash of a recovery file's text (FNV-1a; the same in every
/// build, unlike std's).
pub(crate) fn key_of(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in text.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ (text.len() as u64).rotate_left(32)
}

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: usize) {
        self.0.extend_from_slice(&(v as u32).to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn p(&mut self, p: P) {
        self.f64(p.x);
        self.f64(p.y);
    }
    fn pts(&mut self, pts: &[P]) {
        self.u32(pts.len());
        for &p in pts {
            self.p(p);
        }
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len());
        self.0.extend_from_slice(s.as_bytes());
    }
}

fn how_code(h: TravelHow) -> u8 {
    match h {
        TravelHow::Across => 0,
        TravelHow::Edge => 1,
        TravelHow::Under => 2,
        TravelHow::Open => 3,
        TravelHow::Blend => 4,
        TravelHow::Routed => 5,
    }
}

fn how_of(c: u8) -> Option<TravelHow> {
    Some(match c {
        0 => TravelHow::Across,
        1 => TravelHow::Edge,
        2 => TravelHow::Under,
        3 => TravelHow::Open,
        4 => TravelHow::Blend,
        5 => TravelHow::Routed,
        _ => return None,
    })
}

/// The file for `d` and its `issues`, planned from the recovery file whose
/// [`key_of`] is `key`, for a document whose offset is `offset`.
pub(crate) fn encode(key: u64, offset: P, d: &Design, issues: &[Issue]) -> Vec<u8> {
    let mut w = W(Vec::with_capacity(64 + 16 * d.stitch_count()));
    w.0.extend_from_slice(MAGIC);
    w.u8(VERSION);
    w.0.extend_from_slice(&key.to_le_bytes());
    w.p(offset);
    w.str(&d.name);
    w.u32(d.blocks.len());
    for b in &d.blocks {
        w.0.extend_from_slice(&b.color);
        w.u32(b.runs.len());
        for r in &b.runs {
            w.u8(r.trim_before as u8);
            w.pts(&r.pts);
        }
    }
    let list = |w: &mut W, v: &[usize]| {
        w.u32(v.len());
        for &x in v {
            w.u32(x);
        }
    };
    list(&mut w, &d.per_shape);
    list(&mut w, &d.sew_order);
    w.u32(d.notes.len());
    for (i, m) in &d.notes {
        w.u32(*i);
        w.str(m);
    }
    w.u32(d.block_shapes.len());
    for v in &d.block_shapes {
        list(&mut w, v);
    }
    w.u32(d.covered.len());
    for &c in &d.covered {
        w.u8(c as u8);
    }
    w.u32(d.bridges.len());
    for &(a, b) in &d.bridges {
        w.p(a);
        w.p(b);
    }
    w.u32(d.blends.len());
    for v in &d.blends {
        w.pts(v);
    }
    w.u32(d.routes.len());
    for (i, v) in &d.routes {
        w.u32(*i);
        w.pts(v);
    }
    w.u32(d.stitch_shapes.len());
    for b in &d.stitch_shapes {
        w.u32(b.len());
        for r in b {
            w.u32(r.len());
            for &(at, s) in r {
                w.u32(at);
                w.u32(s);
            }
        }
    }
    w.u32(d.travels.len());
    for t in &d.travels {
        w.u8(how_code(t.how));
        for x in [t.shape, t.block, t.run, t.from, t.to] {
            w.u32(x);
        }
    }
    w.u32(d.satin_columns.len());
    for &(s, c, back) in &d.satin_columns {
        w.u32(s);
        w.u32(c);
        w.u8(back as u8);
    }
    w.u32(issues.len());
    for i in issues {
        match i.shape {
            Some(s) => {
                w.u8(1);
                w.u32(s);
            }
            None => w.u8(0),
        }
        w.u8(match i.level {
            Level::Info => 0,
            Level::Warning => 1,
            Level::Error => 2,
        });
        w.str(&i.message);
    }
    w.0
}

struct R<'a> {
    b: &'a [u8],
    at: usize,
}

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn u32(&mut self) -> Option<usize> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?) as usize)
    }
    fn f64(&mut self) -> Option<f64> {
        let v = f64::from_le_bytes(self.take(8)?.try_into().ok()?);
        v.is_finite().then_some(v)
    }
    /// A count of things at least `each` bytes long: no more than what's
    /// left could hold.
    fn count(&mut self, each: usize) -> Option<usize> {
        let n = self.u32()?;
        (n.checked_mul(each)? <= self.b.len() - self.at).then_some(n)
    }
    fn p(&mut self) -> Option<P> {
        Some(P::new(self.f64()?, self.f64()?))
    }
    fn pts(&mut self) -> Option<Vec<P>> {
        (0..self.count(16)?).map(|_| self.p()).collect()
    }
    fn str(&mut self) -> Option<String> {
        let n = self.count(1)?;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn list(&mut self) -> Option<Vec<usize>> {
        (0..self.count(4)?).map(|_| self.u32()).collect()
    }
}

/// The plan in a kept file, if it was planned from the recovery file whose
/// [`key_of`] is `key`, for the document read from it: `shapes` objects,
/// its offset `offset`.
pub(crate) fn decode(
    bytes: &[u8],
    key: u64,
    shapes: usize,
    offset: P,
) -> Option<(Design, Vec<Issue>)> {
    let mut r = R { b: bytes, at: 0 };
    if r.take(6)? != MAGIC || r.u8()? != VERSION {
        return None;
    }
    if u64::from_le_bytes(r.take(8)?.try_into().ok()?) != key {
        return None;
    }
    // (Where its stitches were planned from, and where they are now.)
    let by = r.p()? - offset;
    let at = |p: P| p + by;
    let name = r.str()?;
    let blocks = (0..r.count(7)?)
        .map(|_| {
            let c = r.take(3)?;
            let color = [c[0], c[1], c[2]];
            let runs = (0..r.count(5)?)
                .map(|_| {
                    let trim_before = r.bool()?;
                    Some(Run {
                        pts: r.pts()?.into_iter().map(at).collect(),
                        trim_before,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Block { color, runs })
        })
        .collect::<Option<Vec<_>>>()?;
    let per_shape = r.list()?;
    let sew_order = r.list()?;
    let notes = (0..r.count(8)?)
        .map(|_| Some((r.u32()?, r.str()?)))
        .collect::<Option<Vec<_>>>()?;
    let block_shapes = (0..r.count(4)?)
        .map(|_| r.list())
        .collect::<Option<Vec<_>>>()?;
    let covered = (0..r.count(1)?)
        .map(|_| r.bool())
        .collect::<Option<Vec<_>>>()?;
    let bridges = (0..r.count(32)?)
        .map(|_| Some((at(r.p()?), at(r.p()?))))
        .collect::<Option<Vec<_>>>()?;
    let moved = |v: Vec<P>| v.into_iter().map(at).collect::<Vec<_>>();
    let blends = (0..r.count(4)?)
        .map(|_| r.pts().map(moved))
        .collect::<Option<Vec<_>>>()?;
    let routes = (0..r.count(8)?)
        .map(|_| Some((r.u32()?, moved(r.pts()?))))
        .collect::<Option<Vec<_>>>()?;
    let stitch_shapes = (0..r.count(4)?)
        .map(|_| {
            (0..r.count(4)?)
                .map(|_| {
                    (0..r.count(8)?)
                        .map(|_| Some((r.u32()?, r.u32()?)))
                        .collect::<Option<Vec<_>>>()
                })
                .collect::<Option<Vec<_>>>()
        })
        .collect::<Option<Vec<_>>>()?;
    let travels = (0..r.count(21)?)
        .map(|_| {
            Some(Travel {
                how: how_of(r.u8()?)?,
                shape: r.u32()?,
                block: r.u32()?,
                run: r.u32()?,
                from: r.u32()?,
                to: r.u32()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let satin_columns = (0..r.count(9)?)
        .map(|_| Some((r.u32()?, r.u32()?, r.bool()?)))
        .collect::<Option<Vec<_>>>()?;
    let issues = (0..r.count(6)?)
        .map(|_| {
            let shape = if r.bool()? { Some(r.u32()?) } else { None };
            let level = match r.u8()? {
                0 => Level::Info,
                1 => Level::Warning,
                2 => Level::Error,
                _ => return None,
            };
            Some(Issue {
                shape,
                level,
                message: r.str()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    if r.at != bytes.len() {
        return None;
    }
    // Everything that points somewhere points at something there: what
    // reads the plan indexes with these.
    let ok_shape = |i: usize| i < shapes;
    let run_len = |b: usize, k: usize| Some(blocks.get(b)?.runs.get(k)?.pts.len());
    let fits = per_shape.len() <= shapes
        && sew_order.iter().all(|&i| ok_shape(i))
        && notes.iter().all(|(i, _)| ok_shape(*i))
        && block_shapes.len() == blocks.len()
        && block_shapes.iter().flatten().all(|&i| ok_shape(i))
        && covered.len() <= shapes
        && routes.iter().all(|(i, _)| ok_shape(*i))
        && (stitch_shapes.is_empty()
            || (stitch_shapes.len() == blocks.len()
                && stitch_shapes.iter().enumerate().all(|(b, runs)| {
                    runs.len() == blocks[b].runs.len()
                        && runs.iter().enumerate().all(|(k, v)| {
                            v.iter()
                                .all(|&(at, s)| ok_shape(s) && at < blocks[b].runs[k].pts.len())
                        })
                })))
        && travels.iter().all(|t| {
            ok_shape(t.shape) && run_len(t.block, t.run).is_some_and(|n| t.from <= t.to && t.to < n)
        })
        && satin_columns.iter().all(|&(s, _, _)| ok_shape(s))
        && issues.iter().all(|i| i.shape.is_none_or(ok_shape));
    if !fits {
        return None;
    }
    Some((
        Design {
            name,
            blocks,
            per_shape,
            sew_order,
            notes,
            block_shapes,
            covered,
            bridges,
            blends,
            routes,
            stitch_shapes,
            travels,
            satin_columns,
        },
        issues,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_plan_reads_back_exactly() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg");
        let text = std::fs::read_to_string(path).unwrap();
        let doc = zigzag::document::Document::from_svg(&text, "badge", None).unwrap();
        let d = doc.plan_cached(&mut Default::default());
        let issues = zigzag::checks::check(&doc.shapes, &d, &doc.plan_options(), None);
        let key = key_of(&text);
        let o = doc.offset;
        let bytes = encode(key, o, &d, &issues);
        let n = doc.shapes.len();
        let (back, back_issues) = decode(&bytes, key, n, o).expect("reads back");
        assert_eq!(format!("{back:?}"), format!("{d:?}"));
        assert_eq!(back_issues, issues);
        // For a document read with its artwork somewhere else: moved there.
        let (moved, _) = decode(&bytes, key, n, o - P::new(10.0, -5.0)).unwrap();
        let (a, b) = (d.blocks[0].runs[0].pts[0], moved.blocks[0].runs[0].pts[0]);
        assert!((b - a - P::new(10.0, -5.0)).len() < 1e-9);
        // Not for another text, nor for a document with fewer objects.
        assert!(decode(&bytes, key_of("<svg/>"), n, o).is_none());
        let used = d.sew_order.iter().max().copied().unwrap_or(0);
        assert!(decode(&bytes, key, used, o).is_none());
        // Cut short anywhere: nothing.
        for k in [0, 7, 15, 25, bytes.len() / 2, bytes.len() - 1] {
            assert!(decode(&bytes[..k], key, n, o).is_none());
        }
    }
}
