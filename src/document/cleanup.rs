//! Path clean-up across the document: VectorCraft's Smooth and Path Eraser
//! applied to objects, Join, and Object › Path › Clean Up.

use super::Document;
use crate::bezier::{self, BezPath};
use crate::pathtools::{self, End};
use anyhow::{Result, anyhow, bail};
use std::collections::HashSet;

/// What Clean Up takes out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CleanUp {
    /// Objects with nothing to them: a single point, or no length at all.
    pub stray: bool,
    /// Objects smaller than `speck` (design units) across: too small to sew.
    pub specks: bool,
    pub speck: f64,
    /// Points on top of each other, and points in the middle of straight lines.
    pub points: bool,
}

impl Default for CleanUp {
    fn default() -> Self {
        CleanUp {
            stray: true,
            specks: true,
            speck: 10.0,
            points: true,
        }
    }
}

/// What Clean Up found (or took out).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanUpReport {
    pub stray: usize,
    pub specks: usize,
    pub points: usize,
}

impl CleanUpReport {
    pub fn total(&self) -> usize {
        self.stray + self.specks + self.points
    }
}

/// Points closer than this (design units, 0.005 mm) are one point.
const SAME_POINT: f64 = 0.05;

impl Document {
    /// The objects at `indices` (one per SVG element, lettering left out),
    /// by uid, so the list survives edits that renumber them.
    fn editable_uids(&self, indices: &[usize]) -> Vec<usize> {
        let mut seen = HashSet::new();
        indices
            .iter()
            .filter_map(|&i| self.shapes.get(i).map(|s| (i, s)))
            .filter(|(_, s)| seen.insert(s.id.clone()))
            .map(|(_, s)| s.uid)
            .collect()
    }

    /// Remove every part of the element behind object `uid`.
    fn delete_element(&mut self, uid: usize) -> Result<()> {
        let id = self
            .shapes
            .iter()
            .find(|s| s.uid == uid)
            .map(|s| s.id.clone())
            .ok_or_else(|| anyhow!("the object went away"))?;
        while let Some(i) = self.shapes.iter().position(|s| s.id == id) {
            self.delete(i)?;
        }
        Ok(())
    }

    /// Rewrite the outlines of the objects at `indices` with `f` (`None`:
    /// leave it; an empty path: the object goes). One read of the SVG and
    /// one write. Returns (objects changed, objects removed).
    pub fn edit_paths_with(
        &mut self,
        indices: &[usize],
        mut f: impl FnMut(&BezPath) -> Option<BezPath>,
    ) -> Result<(usize, usize)> {
        let uids = self.editable_uids(indices);
        let paths = self.all_edit_paths();
        let mut edits = Vec::new();
        let mut gone = Vec::new();
        for uid in uids {
            let Some(i) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            let Some(path) = &paths[i] else { continue };
            match f(path) {
                Some(p) if p.iter().all(|sp| sp.anchors.len() < 2) => gone.push(uid),
                Some(p) => edits.push((i, p)),
                None => {}
            }
        }
        let changed = edits.len();
        self.set_paths(&edits)?;
        for &uid in &gone {
            self.delete_element(uid)?;
        }
        Ok((changed, gone.len()))
    }

    /// Clean Up: what `opt` finds across the document, taken out unless
    /// `dry_run`. Lettering is left alone.
    pub fn clean_up(&mut self, opt: &CleanUp, dry_run: bool) -> Result<CleanUpReport> {
        let mut report = CleanUpReport::default();
        let all: Vec<usize> = (0..self.shapes.len()).collect();
        let uids = self.editable_uids(&all);
        let paths = self.all_edit_paths();
        let mut gone = Vec::new();
        let mut edits = Vec::new();
        for uid in uids {
            let Some(i) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            let Some(path) = &paths[i] else { continue };
            let points: usize = path.iter().map(|sp| sp.anchors.len()).sum();
            let b = bezier::bounds(path);
            let size = if b.is_empty() {
                0.0
            } else {
                b.width().max(b.height())
            };
            if points <= 1 || size <= SAME_POINT {
                if opt.stray {
                    report.stray += 1;
                    gone.push(uid);
                }
                continue;
            }
            if opt.specks && size < opt.speck {
                report.specks += 1;
                gone.push(uid);
                continue;
            }
            if opt.points
                && let Some((clean, n)) = pathtools::clean_path(path, SAME_POINT)
            {
                report.points += n;
                if clean.is_empty() {
                    gone.push(uid);
                } else {
                    edits.push((i, clean));
                }
            }
        }
        if dry_run {
            return Ok(report);
        }
        self.set_paths(&edits)?;
        for uid in gone {
            self.delete_element(uid)?;
        }
        Ok(report)
    }

    /// Join two open ends: end `a_end` of object `a` and `b_end` of object
    /// `b` (the same object, or two: then `b`'s outline joins `a`'s and `b`
    /// goes). Ends within `merge` (design units) become one point.
    pub fn join_ends(
        &mut self,
        a: usize,
        a_end: End,
        b: usize,
        b_end: End,
        merge: f64,
    ) -> Result<usize> {
        // Text joined point by point becomes plain paths first.
        let both = self.outlines_first(&[a, b])?;
        let (a, b) = match both.as_slice() {
            [x, y] => (*x, *y),
            [x] => (*x, *x),
            _ => bail!("nothing to join"),
        };
        let pa = self.edit_path(a)?;
        let same = self.shapes.get(a).map(|s| &s.id) == self.shapes.get(b).map(|s| &s.id);
        let (path, b_end) = if same {
            (pa, b_end)
        } else {
            let pb = self.edit_path(b)?;
            let shift = pa.len();
            let mut both = pa;
            both.extend(pb);
            (both, (b_end.0 + shift, b_end.1))
        };
        let joined = pathtools::join_ends(&path, a_end, b_end, merge)
            .ok_or_else(|| anyhow!("those ends can't be joined"))?;
        let (ua, ub) = (self.shapes[a].uid, self.shapes[b].uid);
        self.set_paths(&[(a, joined)])?;
        if !same {
            self.delete_element(ub)?;
        }
        self.shapes
            .iter()
            .position(|s| s.uid == ua)
            .ok_or_else(|| anyhow!("the object went away"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ART: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40">
      <path id="box" d="M0 0 L10 0 L20 0 L20 20 L0 20 Z" fill="#c00"/>
      <rect id="speck" x="40" y="10" width="0.4" height="0.4" fill="#00c"/>
      <path id="a" d="M30 30 L40 30" stroke="#000" fill="none"/>
      <path id="b" d="M45 30 L60 30" stroke="#000" fill="none"/>
    </svg>"##;

    fn find(d: &Document, id: &str) -> Option<usize> {
        let tagged = format!("{id}_");
        d.shapes
            .iter()
            .position(|s| s.id == id || s.id.starts_with(&tagged))
    }

    #[test]
    fn clean_up_counts_then_takes_out() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let found = d.clean_up(&CleanUp::default(), true).unwrap();
        assert_eq!(
            found,
            CleanUpReport {
                stray: 0,
                specks: 1,
                points: 1
            }
        );
        // A dry run changes nothing.
        assert!(find(&d, "speck").is_some());
        let done = d.clean_up(&CleanUp::default(), false).unwrap();
        assert_eq!(done, found);
        assert!(find(&d, "speck").is_none());
        let i = find(&d, "box").unwrap();
        assert_eq!(d.edit_path(i).unwrap()[0].anchors.len(), 4);
        assert_eq!(d.clean_up(&CleanUp::default(), true).unwrap().total(), 0);
    }

    #[test]
    fn joining_two_lines_makes_one_object() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let (a, b) = (find(&d, "a").unwrap(), find(&d, "b").unwrap());
        let n = d.shapes.len();
        let i = d.join_ends(a, (0, false), b, (0, true), 0.1).unwrap();
        assert_eq!(d.shapes.len(), n - 1);
        assert_eq!(find(&d, "a"), Some(i));
        let p = d.edit_path(i).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].anchors.len(), 4);
        assert!((p[0].anchors[3].p - p[0].anchors[0].p).len() > 250.0);
    }

    #[test]
    fn erasing_all_of_an_object_removes_it() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let a = find(&d, "a").unwrap();
        let (changed, gone) = d
            .edit_paths_with(&[a], |p| pathtools::erase_near(p, &|_| true, 1.0))
            .unwrap();
        assert_eq!((changed, gone), (0, 1));
        assert!(find(&d, "a").is_none());
    }
}
